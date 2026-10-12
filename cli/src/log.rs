//! `ns log`: the run log `<git-common-dir>/ns/runs.jsonl`, one line or one JSON object per
//! event, and with `--follow` the events appended after it starts.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::config::{self, ForgeKind};
use crate::factory;
use crate::git::{self, Repo};
use crate::quality::{self, issue_of, Window};
use crate::watch::forge as watch_forge;

/// The only fields `ns log` reads from a line. The rest (prompts, transcripts, errors, paths)
/// stay in the file, so no output carries what a run happened to log. `pr_url` is built, never read.
const RETAINED: [&str; 12] = [
    "ts", "event", "sha", "unit", "issue", "phase", "attempt", "status", "wall_s", "outcome", "pr",
    "reason",
];

/// How long `--follow` waits at the end of the file before reading again.
pub(crate) const POLL: Duration = Duration::from_millis(500);

pub struct Filter {
    pub unit: Option<String>,
    pub since: Window,
}

pub fn cli(
    unit: Option<String>,
    since: Option<String>,
    follow: bool,
    as_json: bool,
) -> Result<ExitCode> {
    let since = since
        .map(|s| quality::instant("log", "since", &s, "00:00:00"))
        .transpose()?;
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let path = repo.run_log();
    let base = pr_base(&repo);
    let filter = Filter {
        unit,
        since: Window { since, until: None },
    };
    let show: Show = if as_json { json_line } else { text };
    let mut out = io::stdout().lock();
    let printed = if follow {
        let tail = Tail::new(path.clone(), || std::thread::sleep(POLL));
        render(tail, &filter, show, base.as_ref(), &mut out)
    } else {
        open_lines(&path).and_then(|lines| render(lines, &filter, show, base.as_ref(), &mut out))
    };
    match printed {
        Ok(0) => {}
        Ok(bad) => report_bad(bad, &path),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
        Err(e) => return Err(e).with_context(|| format!("ns log on {}", path.display())),
    }
    Ok(ExitCode::SUCCESS)
}

/// The lines of the run log at `path`. A log that does not exist yet reads as empty, with a
/// note on stderr.
pub fn open_lines(path: &Path) -> io::Result<Box<dyn Iterator<Item = io::Result<Vec<u8>>>>> {
    match File::open(path) {
        Ok(f) => Ok(Box::new(byte_lines(BufReader::new(f)))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            eprintln!(
                "no run log at {} yet; ns run and ns watch write one",
                path.display()
            );
            Ok(Box::new(std::iter::empty()))
        }
        Err(e) => Err(e),
    }
}

pub fn report_bad(bad: usize, path: &Path) {
    if bad > 0 {
        eprintln!(
            "skipped {bad} line(s) of {} that are not UTF-8 JSON objects",
            path.display()
        );
    }
}

/// Where the repo's PRs live, from `origin` and the forge `ns watch` would pick.
pub fn pr_base(repo: &Repo) -> Option<PrBase> {
    let origin = git::run(&repo.root, &["remote", "get-url", "origin"]).ok()?;
    let source = factory::load(&factory::root(None, &repo.root))
        .ok()
        .and_then(|f| f.queue.source);
    let cfg = config::load(&config::path()).ok().flatten();
    let gitlab_host = watch_forge::gitlab_host(cfg.as_ref());
    PrBase::new(origin.trim(), source.as_deref(), gitlab_host.as_deref())
}

/// How one event is printed: [`text`] or [`json_line`].
type Show = fn(&Map<String, Value>) -> String;

/// Print each event `lines` yields that passes `filter`, as `show` formats it, one per line.
/// Returns how many non-blank lines were not UTF-8 JSON objects.
pub fn render(
    lines: impl Iterator<Item = io::Result<Vec<u8>>>,
    filter: &Filter,
    show: Show,
    pr_base: Option<&PrBase>,
    out: &mut impl Write,
) -> io::Result<usize> {
    let bad = Cell::new(0);
    for ev in events(lines, pr_base, &bad) {
        let ev = ev?;
        if keep(&ev, filter) {
            writeln!(out, "{}", show(&ev))?;
        }
    }
    Ok(bad.get())
}

/// The events of `lines`, counting in `bad` each non-blank line that is not a UTF-8 JSON object.
pub fn events<'a>(
    lines: impl Iterator<Item = io::Result<Vec<u8>>> + 'a,
    pr_base: Option<&'a PrBase>,
    bad: &'a Cell<usize>,
) -> impl Iterator<Item = io::Result<Map<String, Value>>> + 'a {
    lines.filter_map(move |line| {
        let line = match line {
            Ok(line) => line,
            Err(e) => return Some(Err(e)),
        };
        let Ok(text) = std::str::from_utf8(&line) else {
            bad.set(bad.get() + 1);
            return None;
        };
        if text.trim().is_empty() {
            return None;
        }
        let ev = event(text, pr_base);
        if ev.is_none() {
            bad.set(bad.get() + 1);
        }
        ev.map(Ok)
    })
}

/// The allowlisted, non-null fields of one run-log line, with `issue` taken from the unit id
/// when the line has none, and `pr_url` built from `pr_base` whenever the line has a `pr`. `None` when it is not a JSON object.
pub fn event(line: &str, pr_base: Option<&PrBase>) -> Option<Map<String, Value>> {
    let Ok(Value::Object(mut all)) = serde_json::from_str::<Value>(line) else {
        return None;
    };
    all.retain(|k, v| RETAINED.contains(&k.as_str()) && !v.is_null());
    if !all.contains_key("issue") {
        if let Some(n) = all.get("unit").and_then(Value::as_str).and_then(issue_of) {
            all.insert("issue".into(), json!(n));
        }
    }
    if let (Some(base), Some(n)) = (pr_base, all.get("pr").and_then(Value::as_u64)) {
        all.insert("pr_url".into(), json!(base.link(n)));
    }
    Some(all)
}

fn keep(ev: &Map<String, Value>, filter: &Filter) -> bool {
    let unit = ev.get("unit").and_then(Value::as_str);
    let ts = ev
        .get("ts")
        .and_then(Value::as_str)
        .and_then(crate::clock::parse_iso);
    filter.unit.as_deref().is_none_or(|u| unit == Some(u)) && filter.since.contains(ts)
}

fn json_line(ev: &Map<String, Value>) -> String {
    Value::Object(ev.clone()).to_string()
}

/// `<ts> <event> <short sha> #<issue> <phase> attempt <n> status <s> <wall_s>s outcome <o> <pr> reason: <r>`,
/// leaving out what the event lacks.
pub fn text(ev: &Map<String, Value>) -> String {
    let field = |k: &str| {
        let v = ev.get(k)?;
        let raw = match v.as_str() {
            Some("") => return None,
            Some(s) => s.to_string(),
            None => v.to_string(),
        };
        Some(raw.chars().flat_map(printable).collect::<String>())
    };
    let parts = [
        field("ts"),
        field("event"),
        field("sha").map(|s| s.chars().take(7).collect()),
        field("issue").map(|n| format!("#{n}")),
        field("phase"),
        field("attempt").map(|n| format!("attempt {n}")),
        field("status").map(|s| format!("status {s}")),
        field("wall_s").map(|s| format!("{s}s")),
        field("outcome").map(|o| format!("outcome {o}")),
        field("pr_url").or_else(|| field("pr").map(|n| format!("pr #{n}"))),
        field("reason").map(|r| format!("reason: {r}")),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join(" ")
}

pub fn printable(c: char) -> impl Iterator<Item = char> {
    let hidden = c.is_control()
        || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let escaped = hidden.then(|| c.escape_default());
    escaped.into_iter().flatten().chain((!hidden).then_some(c))
}

/// Where an event's PR lives: the repo's web address and the forge that serves it.
pub struct PrBase {
    web: String,
    kind: ForgeKind,
}

impl PrBase {
    /// From `origin`'s URL, with the forge chosen as `ns watch` chooses it (`[queue] source`,
    /// else the host). `None` when `origin` is not a remote URL, such as a local path.
    pub fn new(origin: &str, source: Option<&str>, gitlab_host: Option<&str>) -> Option<PrBase> {
        Some(PrBase {
            web: web_base(origin)?,
            kind: watch_forge::kind(source, Some(origin), gitlab_host),
        })
    }

    fn link(&self, n: u64) -> String {
        match self.kind {
            ForgeKind::Gitlab => format!("{}/-/merge_requests/{n}", self.web),
            ForgeKind::Github => format!("{}/pull/{n}", self.web),
        }
    }
}

/// The web address of a repo from its `origin` URL (`https://`, `ssh://` or `git@host:`), with
/// any credentials and `.git` dropped. `None` for anything else, such as a local path.
fn web_base(url: &str) -> Option<String> {
    let host = watch_forge::host(url).filter(|h| !h.is_empty())?;
    let rest = match url.split_once("://") {
        Some(("https" | "http" | "ssh", rest)) => rest.split_once('/')?.1,
        Some(_) => return None,
        None => url.split_once(':').filter(|(h, _)| h.contains('@'))?.1,
    };
    let path = rest.trim_end_matches('/').trim_end_matches(".git");
    (!path.is_empty()).then(|| format!("https://{host}/{path}"))
}

/// The lines of `reader` as bytes, without their newline.
fn byte_lines(mut reader: impl BufRead) -> impl Iterator<Item = io::Result<Vec<u8>>> {
    std::iter::from_fn(move || {
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Err(e) => Some(Err(e)),
            Ok(0) => None,
            Ok(_) => {
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                Some(Ok(line))
            }
        }
    })
}

/// The lines of a file as they are appended, forever: at the end of the file, and while it
/// does not exist yet, it calls `wait` and reads again. A line still being written is held
/// back until its newline arrives.
pub struct Tail<W: FnMut()> {
    path: PathBuf,
    reader: Option<BufReader<File>>,
    partial: Vec<u8>,
    wait: W,
}

impl<W: FnMut()> Tail<W> {
    pub fn new(path: PathBuf, wait: W) -> Self {
        Tail {
            path,
            reader: None,
            partial: Vec::new(),
            wait,
        }
    }
}

impl<W: FnMut()> Iterator for Tail<W> {
    type Item = io::Result<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let Some(reader) = self.reader.as_mut() else {
                match File::open(&self.path) {
                    Ok(f) => self.reader = Some(BufReader::new(f)),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (self.wait)(),
                    Err(e) => return Some(Err(e)),
                }
                continue;
            };
            match reader.read_until(b'\n', &mut self.partial) {
                Err(e) => return Some(Err(e)),
                Ok(0) => (self.wait)(),
                Ok(_) if self.partial.ends_with(b"\n") => {
                    let mut line = std::mem::take(&mut self.partial);
                    line.pop();
                    return Some(Ok(line));
                }
                Ok(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs::{self, OpenOptions};
    use std::path::Path;

    const FIXTURE: &str = r#"{"event":"start","unit":"142-uart-timeout","ts":"2026-10-09T10:00:00Z","pid":7,"default_branch":"main"}
{"event":"phase","unit":"142-uart-timeout","phase":"build","attempt":1,"status":"pass","wall_s":312.4,"cost_usd":1.2,"transcript":"/tmp/t.jsonl","ts":"2026-10-09T10:05:12Z"}
{"event":"phase","unit":"142-uart-timeout","phase":"verify","attempt":2,"status":null,"written":false,"reason":"exited 1, no artifact written","ts":"2026-10-09T10:20:00Z"}

{"event":"end","unit":"142-uart-timeout","outcome":"stuck","phase":"verify","pr":17,"reason":"verify failed twice","ts":"2026-10-09T10:30:00Z"}
{"event":"worker_start","worker":1,"issue":150,"run_pid":9,"ts":"2026-10-09T11:00:00Z"}
{"event":"end","unit":"150-docs","outcome":"merged","pr":null,"ts":"2026-10-10T01:00:00Z"}
not json
[1,2]
{"event":"requeued","issue":151,"reason":"in progress with no live ns run"}
"#;

    fn lines(text: &str) -> impl Iterator<Item = io::Result<Vec<u8>>> + '_ {
        text.lines().map(|l| Ok(l.as_bytes().to_vec()))
    }

    fn all() -> Filter {
        Filter {
            unit: None,
            since: Window::default(),
        }
    }

    fn base(web: &str) -> PrBase {
        PrBase::new(&format!("{web}.git"), None, None).unwrap()
    }

    fn run(filter: &Filter, show: Show, base: Option<&PrBase>) -> (Vec<String>, usize) {
        let mut out = Vec::new();
        let bad = render(lines(FIXTURE), filter, show, base, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        (text.lines().map(String::from).collect(), bad)
    }

    #[test]
    fn each_event_is_one_line_without_the_fields_it_lacks() {
        let (out, bad) = run(&all(), text, Some(&base("https://github.com/o/r")));
        assert_eq!(
            out,
            [
                "2026-10-09T10:00:00Z start #142",
                "2026-10-09T10:05:12Z phase #142 build attempt 1 status pass 312.4s",
                "2026-10-09T10:20:00Z phase #142 verify attempt 2 reason: exited 1, no artifact written",
                "2026-10-09T10:30:00Z end #142 verify outcome stuck https://github.com/o/r/pull/17 reason: verify failed twice",
                "2026-10-09T11:00:00Z worker_start #150",
                "2026-10-10T01:00:00Z end #150 outcome merged",
                "requeued #151 reason: in progress with no live ns run",
            ]
        );
        assert_eq!(bad, 2);
    }

    #[test]
    fn control_characters_in_a_value_cannot_reshape_the_line() {
        let line = "{\"event\":\"end\",\"unit\":\"7-a\",\"reason\":\"\\u001b[31mred\\nnext\\r\",\"outcome\":\"\"}";
        let mut out = Vec::new();
        let input = [Ok(line.as_bytes().to_vec())];
        render(input.into_iter(), &all(), text, None, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out, "end #7 reason: \\u{1b}[31mred\\nnext\\r\n");
    }

    #[test]
    fn a_line_that_is_not_utf8_is_skipped_and_counted() {
        let input = [
            Ok(b"{\"event\":\"a\"}".to_vec()),
            Ok(b"{\"event\":\"\xff\xfe\"}".to_vec()),
            Ok(b"{\"event\":\"b\"}".to_vec()),
        ];
        let mut out = Vec::new();
        let bad = render(input.into_iter(), &all(), text, None, &mut out).unwrap();
        assert_eq!(
            (String::from_utf8(out).unwrap().as_str(), bad),
            ("a\nb\n", 1)
        );
    }

    #[test]
    fn a_logged_pr_url_is_not_emitted_without_an_origin() {
        let line = r#"{"event":"end","pr":3,"pr_url":"https://evil.example/x"}"#;
        let ev = event(line, None).unwrap();
        assert!(ev.get("pr_url").is_none(), "{ev:?}");
    }

    #[test]
    fn bidi_and_format_characters_are_escaped() {
        let line = "{\"event\":\"end\",\"reason\":\"a\u{202e}b\u{200b}c\u{2066}d\"}";
        let mut out = Vec::new();
        let input = [Ok(line.as_bytes().to_vec())];
        render(input.into_iter(), &all(), text, None, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "end reason: a\\u{202e}b\\u{200b}c\\u{2066}d\n"
        );
    }

    #[test]
    fn bidi_characters_inside_a_non_string_value_are_escaped() {
        let line = "{\"event\":\"phase\",\"attempt\":{\"x\":\"\u{202e}\"}}";
        let mut out = Vec::new();
        let input = [Ok(line.as_bytes().to_vec())];
        render(input.into_iter(), &all(), text, None, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "phase attempt {\"x\":\"\\u{202e}\"}\n"
        );
    }

    #[test]
    fn a_logged_issue_wins_over_the_unit_ids() {
        let ev = event(r#"{"unit":"7-a","issue":8}"#, None).unwrap();
        assert_eq!(ev["issue"], 8);
    }

    #[test]
    fn a_pr_is_a_number_without_a_known_origin() {
        let (out, _) = run(&all(), text, None);
        assert!(
            out[3].contains(" outcome stuck pr #17 reason: "),
            "{}",
            out[3]
        );
    }

    #[test]
    fn a_main_event_keeps_its_sha_and_shows_it_short() {
        let line = r#"{"event":"main_red_unfixed","sha":"abcdef0123456789","issue":100,"outcome":"stuck","ts":"2026-10-09T03:00:00Z"}"#;
        let bare = r#"{"event":"main_red_unfixed","issue":100,"outcome":"stuck","ts":"2026-10-09T03:00:00Z"}"#;
        let one = |line: &str, show: Show| {
            let mut out = Vec::new();
            render(lines(line), &all(), show, None, &mut out).unwrap();
            String::from_utf8(out).unwrap().trim_end().to_string()
        };
        let kept: Value = serde_json::from_str(&one(line, json_line)).unwrap();
        assert_eq!(kept["sha"], "abcdef0123456789");
        assert_eq!(
            one(line, text),
            "2026-10-09T03:00:00Z main_red_unfixed abcdef0 #100 outcome stuck"
        );
        assert_eq!(
            one(bare, text),
            "2026-10-09T03:00:00Z main_red_unfixed #100 outcome stuck"
        );
        let kept: Value = serde_json::from_str(&one(bare, json_line)).unwrap();
        assert!(kept.get("sha").is_none());
    }

    #[test]
    fn unit_keeps_only_that_units_events() {
        let filter = Filter {
            unit: Some("150-docs".into()),
            ..all()
        };
        let (out, _) = run(&filter, text, None);
        assert_eq!(out, ["2026-10-10T01:00:00Z end #150 outcome merged"]);
    }

    #[test]
    fn since_drops_older_and_undated_events() {
        let at = |s| crate::clock::parse_iso(s);
        let since = |s| Filter {
            unit: None,
            since: Window {
                since: at(s),
                until: None,
            },
        };
        let (out, _) = run(&since("2026-10-09T10:30:00Z"), text, None);
        assert_eq!(out.len(), 3, "{out:?}");
        assert!(out[0].starts_with("2026-10-09T10:30:00Z end"));
        let (out, _) = run(&since("2026-10-09T10:30:01Z"), text, None);
        assert_eq!(out.len(), 2, "{out:?}");
        let both = Filter {
            unit: Some("142-uart-timeout".into()),
            ..since("2026-10-09T10:05:12Z")
        };
        assert_eq!(run(&both, text, None).0.len(), 3);
    }

    #[test]
    fn json_parses_and_holds_only_allowlisted_fields() {
        let (out, bad) = run(&all(), json_line, Some(&base("https://gitlab.com/g/r")));
        assert_eq!((out.len(), bad), (7, 2));
        for line in &out {
            let v: Value = serde_json::from_str(line).unwrap();
            for k in v.as_object().unwrap().keys() {
                assert!(
                    RETAINED.contains(&k.as_str()) || k == "pr_url",
                    "{k} in {line}"
                );
            }
        }
        let build: Value = serde_json::from_str(&out[1]).unwrap();
        assert_eq!(
            build,
            json!({"ts":"2026-10-09T10:05:12Z","event":"phase","unit":"142-uart-timeout","issue":142,
                   "phase":"build","attempt":1,"status":"pass","wall_s":312.4})
        );
        let end: Value = serde_json::from_str(&out[3]).unwrap();
        assert_eq!(end["pr"], 17);
        assert_eq!(end["pr_url"], "https://gitlab.com/g/r/-/merge_requests/17");
        let merged: Value = serde_json::from_str(&out[5]).unwrap();
        assert!(merged.get("pr").is_none());
    }

    #[test]
    fn a_read_or_write_error_ends_render() {
        let failing = [Ok(b"{}".to_vec()), Err(io::Error::other("disk"))];
        let e = render(failing.into_iter(), &all(), text, None, &mut Vec::new()).unwrap_err();
        assert_eq!(e.to_string(), "disk");
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let e = render(lines(FIXTURE), &all(), text, None, &mut Closed).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn origin_urls_become_web_addresses() {
        for (url, base) in [
            ("https://github.com/o/r.git", Some("https://github.com/o/r")),
            ("https://github.com/o/r/", Some("https://github.com/o/r")),
            (
                "https://x-access-token:secret@github.com/o/r",
                Some("https://github.com/o/r"),
            ),
            (
                "http://gitlab.example.com/g/sub/r.git",
                Some("https://gitlab.example.com/g/sub/r"),
            ),
            ("git@github.com:o/r.git", Some("https://github.com/o/r")),
            (
                "ssh://git@gitlab.com:2222/g/r.git",
                Some("https://gitlab.com/g/r"),
            ),
            ("/srv/git/r.git", None),
            ("file:///srv/git/r.git", None),
            ("C:/repos/r", None),
            ("https://github.com", None),
            ("https:///o/r", None),
            ("git@github.com:", None),
        ] {
            assert_eq!(web_base(url).as_deref(), base, "{url}");
        }
    }

    #[test]
    fn the_pr_link_follows_the_forge_ns_watch_would_pick() {
        let link = |origin, source, host| PrBase::new(origin, source, host).unwrap().link(3);
        assert_eq!(
            link("git@github.com:o/r.git", None, None),
            "https://github.com/o/r/pull/3"
        );
        assert_eq!(
            link("https://GitLab.example.com/g/r", None, None),
            "https://GitLab.example.com/g/r/-/merge_requests/3"
        );
        assert_eq!(
            link(
                "https://git.corp.example/g/r",
                None,
                Some("git.corp.example")
            ),
            "https://git.corp.example/g/r/-/merge_requests/3"
        );
        assert_eq!(
            link("https://git.corp.example/g/r", None, None),
            "https://git.corp.example/g/r/pull/3"
        );
        assert_eq!(
            link("https://git.corp.example/g/r", Some("gitlab"), None),
            "https://git.corp.example/g/r/-/merge_requests/3"
        );
        assert_eq!(
            link("https://gitlab.com/g/r", Some("github"), None),
            "https://gitlab.com/g/r/pull/3"
        );
    }

    fn append(path: &Path, text: &str) {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    #[test]
    fn follow_waits_for_the_file_and_for_each_whole_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("runs.jsonl");
        // Each wait appends the next chunk, as a run would while ns log sleeps.
        let chunks = [
            "{\"event\":\"start\",\"unit\":\"7-a\"}\n",
            "{\"event\":\"phase\",\"unit\":\"7-a\",",
            "\"phase\":\"build\"}\n\n",
            "{\"event\":\"end\",\"unit\":\"8-b\"}\n",
        ];
        let waits = Cell::new(0);
        let tail = Tail::new(path.clone(), || {
            let n = waits.get();
            append(&path, chunks[n]);
            waits.set(n + 1);
        });
        let filter = Filter {
            unit: Some("7-a".into()),
            ..all()
        };
        let mut out = Vec::new();
        render(tail.take(4), &filter, text, None, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "start #7\nphase #7 build\n"
        );
        assert_eq!(waits.get(), 4);
    }

    #[test]
    fn follow_holds_back_half_a_multibyte_character_until_the_rest_arrives() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("runs.jsonl");
        let whole = "{\"event\":\"caf\u{e9}\"}\n".as_bytes();
        let cut = whole.iter().position(|b| *b == 0xc3).unwrap() + 1;
        let chunks = [&whole[..cut], &whole[cut..]];
        fs::write(&path, b"").unwrap();
        let waits = Cell::new(0);
        let tail = Tail::new(path.clone(), || {
            let n = waits.get();
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(chunks[n])
                .unwrap();
            waits.set(n + 1);
        });
        let mut out = Vec::new();
        render(tail.take(1), &all(), text, None, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "caf\u{e9}\n");
    }

    #[test]
    fn follow_reads_what_the_file_holds_before_waiting() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("runs.jsonl");
        fs::write(&path, "a\nb\n").unwrap();
        let mut tail = Tail::new(path, || panic!("waited with lines left"));
        assert_eq!(tail.next().unwrap().unwrap(), b"a");
        assert_eq!(tail.next().unwrap().unwrap(), b"b");
    }

    #[test]
    fn follow_fails_on_a_path_it_cannot_open_or_read() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("file");
        fs::write(&file, "").unwrap();
        let mut tail = Tail::new(file.join("runs.jsonl"), || panic!("waited on a bad path"));
        assert!(tail.next().unwrap().is_err());
        let mut tail = Tail::new(tmp.path().to_path_buf(), || panic!("waited on a directory"));
        assert!(tail.next().unwrap().is_err());
    }
}
