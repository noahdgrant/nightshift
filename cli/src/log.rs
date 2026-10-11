//! `ns log`: the run log `<git-common-dir>/ns/runs.jsonl`, one line or one JSON object per
//! event, and with `--follow` the events appended after it starts.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::git::{self, Repo};
use crate::quality::{self, issue_of, Window};

/// The only fields `ns log` prints. The rest (prompts, transcripts, errors, paths) stay in the
/// file, so no output carries what a run happened to log.
const FIELDS: [&str; 12] = [
    "ts", "event", "unit", "issue", "phase", "attempt", "status", "wall_s", "outcome", "pr",
    "pr_url", "reason",
];

/// How long `--follow` waits at the end of the file before reading again.
const POLL: Duration = Duration::from_millis(500);

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
    let path = repo.common_dir.join("ns").join("runs.jsonl");
    let base = git::run(&repo.root, &["remote", "get-url", "origin"])
        .ok()
        .and_then(|u| web_base(u.trim()));
    let filter = Filter {
        unit,
        since: Window { since, until: None },
    };
    let show: Show = if as_json { json_line } else { text };
    let mut out = io::stdout().lock();
    let printed = if follow {
        let tail = Tail::new(path.clone(), || std::thread::sleep(POLL));
        render(tail, &filter, show, base.as_deref(), &mut out)
    } else {
        match File::open(&path) {
            Ok(f) => render(
                BufReader::new(f).lines(),
                &filter,
                show,
                base.as_deref(),
                &mut out,
            ),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                eprintln!(
                    "no run log at {} yet; ns run and ns watch write one",
                    path.display()
                );
                Ok(0)
            }
            Err(e) => Err(e),
        }
    };
    match printed {
        Ok(0) => {}
        Ok(bad) => eprintln!(
            "skipped {bad} line(s) of {} that are not JSON objects",
            path.display()
        ),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
        Err(e) => return Err(e).with_context(|| format!("ns log on {}", path.display())),
    }
    Ok(ExitCode::SUCCESS)
}

/// How one event is printed: [`text`] or [`json_line`].
type Show = fn(&Map<String, Value>) -> String;

/// Print each event `lines` yields that passes `filter`, as `show` formats it, one per line.
/// Returns how many non-blank lines were not JSON objects.
pub fn render(
    lines: impl Iterator<Item = io::Result<String>>,
    filter: &Filter,
    show: Show,
    pr_base: Option<&str>,
    out: &mut impl Write,
) -> io::Result<usize> {
    let mut bad = 0;
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(ev) = event(&line, pr_base) else {
            bad += 1;
            continue;
        };
        if !keep(&ev, filter) {
            continue;
        }
        writeln!(out, "{}", show(&ev))?;
    }
    Ok(bad)
}

/// The allowlisted, non-null fields of one run-log line, with `issue` taken from the unit id
/// and `pr_url` from `pr_base` when the line has neither. `None` when it is not a JSON object.
fn event(line: &str, pr_base: Option<&str>) -> Option<Map<String, Value>> {
    let Ok(Value::Object(mut all)) = serde_json::from_str::<Value>(line) else {
        return None;
    };
    all.retain(|k, v| FIELDS.contains(&k.as_str()) && !v.is_null());
    if !all.contains_key("issue") {
        if let Some(n) = all.get("unit").and_then(Value::as_str).and_then(issue_of) {
            all.insert("issue".into(), json!(n));
        }
    }
    if let (Some(base), Some(n)) = (pr_base, all.get("pr").and_then(Value::as_u64)) {
        all.insert("pr_url".into(), json!(pr_link(base, n)));
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

/// `<ts> <event> #<issue> <phase> attempt <n> status <s> <wall_s>s outcome <o> <pr> reason: <r>`,
/// leaving out what the event lacks.
fn text(ev: &Map<String, Value>) -> String {
    let field = |k: &str| {
        ev.get(k)
            .map(|v| v.as_str().map_or_else(|| v.to_string(), String::from))
    };
    let parts = [
        field("ts"),
        field("event"),
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

/// The web address of a repo from its `origin` URL (`https://`, `ssh://` or `git@host:`), with
/// any credentials and `.git` dropped. `None` for anything else, such as a local path.
fn web_base(url: &str) -> Option<String> {
    let (host, path) = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
    {
        rest.split_once('/')?
    } else {
        let (user_host, path) = url.split_once(':')?;
        user_host.contains('@').then_some((user_host, path))?
    };
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    (!host.is_empty() && !path.is_empty()).then(|| format!("https://{host}/{path}"))
}

/// PR `n`'s page: a GitLab merge request when the host names GitLab, else a GitHub pull request.
fn pr_link(base: &str, n: u64) -> String {
    if base.contains("gitlab") {
        format!("{base}/-/merge_requests/{n}")
    } else {
        format!("{base}/pull/{n}")
    }
}

/// The lines of a file as they are appended, forever: at the end of the file, and while it
/// does not exist yet, it calls `wait` and reads again. A line still being written is held
/// back until its newline arrives.
pub struct Tail<W: FnMut()> {
    path: PathBuf,
    reader: Option<BufReader<File>>,
    partial: String,
    wait: W,
}

impl<W: FnMut()> Tail<W> {
    pub fn new(path: PathBuf, wait: W) -> Self {
        Tail {
            path,
            reader: None,
            partial: String::new(),
            wait,
        }
    }
}

impl<W: FnMut()> Iterator for Tail<W> {
    type Item = io::Result<String>;

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
            match reader.read_line(&mut self.partial) {
                Err(e) => return Some(Err(e)),
                Ok(0) => (self.wait)(),
                Ok(_) if self.partial.ends_with('\n') => {
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

    fn lines(text: &str) -> impl Iterator<Item = io::Result<String>> + '_ {
        text.lines().map(|l| Ok(l.to_string()))
    }

    fn all() -> Filter {
        Filter {
            unit: None,
            since: Window::default(),
        }
    }

    fn run(filter: &Filter, show: Show, base: Option<&str>) -> (Vec<String>, usize) {
        let mut out = Vec::new();
        let bad = render(lines(FIXTURE), filter, show, base, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        (text.lines().map(String::from).collect(), bad)
    }

    #[test]
    fn each_event_is_one_line_without_the_fields_it_lacks() {
        let (out, bad) = run(&all(), text, Some("https://github.com/o/r"));
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
        let (out, bad) = run(&all(), json_line, Some("https://gitlab.com/g/r"));
        assert_eq!((out.len(), bad), (7, 2));
        for line in &out {
            let v: Value = serde_json::from_str(line).unwrap();
            for k in v.as_object().unwrap().keys() {
                assert!(FIELDS.contains(&k.as_str()), "{k} in {line}");
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
        let failing = [Ok("{}".to_string()), Err(io::Error::other("disk"))];
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
        assert_eq!(
            pr_link("https://github.com/o/r", 3),
            "https://github.com/o/r/pull/3"
        );
        assert_eq!(
            pr_link("https://gitlab.com/g/r", 3),
            "https://gitlab.com/g/r/-/merge_requests/3"
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
    fn follow_reads_what_the_file_holds_before_waiting() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("runs.jsonl");
        fs::write(&path, "a\nb\n").unwrap();
        let mut tail = Tail::new(path, || panic!("waited with lines left"));
        assert_eq!(tail.next().unwrap().unwrap(), "a");
        assert_eq!(tail.next().unwrap().unwrap(), "b");
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
