//! `ns log --phase-output`: the assistant messages and tool calls of a unit's newest phase
//! transcript, `<git-common-dir>/ns/transcripts/<unit>/<phase>-<attempt>.jsonl`, one line each,
//! long text cut short and secret-like values redacted.

use std::collections::VecDeque;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::error::SfError;
use crate::gate;
use crate::git::Repo;
use crate::log::{printable, POLL};
use crate::transcripts::{line_entries, Entry, COMPACT_LIMIT};
use crate::worktree;

mod redact;
use redact::redact;

/// How many entries the transcript so far is cut to before printing or following.
const LATEST: usize = 20;

pub fn cli(unit: Option<&str>, follow: bool, as_json: bool) -> Result<ExitCode> {
    if let Some(u) = unit {
        worktree::check_unit_id(u, "ns log --phase-output --unit 142-uart-timeout")?;
    }
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let ns = repo.common_dir.join("ns");
    let root = ns.join("transcripts");
    let runs = ns.join("runs.jsonl");
    let current = || current(&root, &runs, unit);
    let path = current()
        .with_context(|| format!("cannot list {}", root.display()))?
        .ok_or_else(|| {
            let whose = unit.map_or(String::new(), |u| format!(" for {u}"));
            SfError::general(format!(
                "no phase transcript{whose} under {} yet",
                root.display()
            ))
            .hint("ns run writes one per phase; see which units ran with:  ns log")
        })?;
    eprintln!("phase output from {}", path.display());
    let show: Show = if as_json { json_line } else { text };
    let mut out = io::stdout().lock();
    let wait = || {
        std::thread::sleep(POLL);
        true
    };
    match print(&path, follow, show, &mut out, current, wait) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(ExitCode::SUCCESS),
        r => r
            .map(|()| ExitCode::SUCCESS)
            .with_context(|| format!("ns log --phase-output on {}", path.display())),
    }
}

/// How much of the end of a transcript is read to find its latest entries.
const WINDOW: u64 = 256 * 1024;

const RUNS_TAIL: u64 = 64 * 1024;

/// Print the last [`LATEST`] entries of the transcript's whole lines. With `follow`, then the
/// entries of each line appended after them, and when `current` names another transcript, that
/// one's last entries and what is appended to it. `wait` is called between polls; `false` stops.
fn print(
    path: &Path,
    follow: bool,
    show: Show,
    out: &mut impl Write,
    mut current: impl FnMut() -> io::Result<Option<PathBuf>>,
    mut wait: impl FnMut() -> bool,
) -> io::Result<()> {
    let mut path = path.to_path_buf();
    let mut at = snapshot(&path, show, out)?;
    if !follow {
        return Ok(());
    }
    loop {
        at = drain(&path, at, show, out)?;
        if let Some(next) = current()?.filter(|n| *n != path) {
            drain(&path, at, show, out)?;
            at = snapshot(&next, show, out)?;
            path = next;
        } else if !wait() {
            return Ok(());
        }
    }
}

/// Print the last [`LATEST`] entries within the last [`WINDOW`] bytes of `path`'s whole lines.
/// Returns the offset just past its last whole line.
fn snapshot(path: &Path, show: Show, out: &mut impl Write) -> io::Result<u64> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.take(len - start).read_to_end(&mut tail)?;
    let whole = tail.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut lines = &tail[..whole];
    if start > 0 {
        lines = lines
            .iter()
            .position(|b| *b == b'\n')
            .map_or(&[][..], |i| &lines[i + 1..]);
    }
    for e in latest(lines.split(|b| *b == b'\n'), LATEST) {
        writeln!(out, "{}", show(&e))?;
    }
    Ok(start + whole as u64)
}

/// Print the entries of the whole lines of `path` from byte `at` on, and return the offset just
/// past them. Nothing, and `at` back, while the file does not exist yet.
fn drain(path: &Path, at: u64, show: Show, out: &mut impl Write) -> io::Result<u64> {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(at),
        Err(e) => return Err(e),
    };
    if file.metadata()?.len() < at {
        return snapshot(path, show, out);
    }
    file.seek(SeekFrom::Start(at))?;
    let mut added = Vec::new();
    file.read_to_end(&mut added)?;
    let whole = added.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    for line in added[..whole].split(|b| *b == b'\n') {
        for e in entries(line) {
            writeln!(out, "{}", show(&e))?;
        }
    }
    Ok(at + whole as u64)
}

/// The transcript to show: the newest of `unit`, or of the unit the run log last names when
/// there is none, or of any unit when the log names none with a transcript. The newest is the
/// running phase's, which the log only names once the phase ends.
fn current(root: &Path, runs: &Path, unit: Option<&str>) -> io::Result<Option<PathBuf>> {
    if unit.is_some() {
        return newest(root, unit);
    }
    let logged = match gate::tail(runs, RUNS_TAIL) {
        Ok(b) => last_unit(&b),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    match newest(root, logged.as_deref())? {
        Some(p) => Ok(Some(p)),
        None => newest(root, None),
    }
}

/// The unit of the last run-log line in `log` that names a valid one.
fn last_unit(log: &[u8]) -> Option<String> {
    log.split(|b| *b == b'\n').rev().find_map(|line| {
        let v = serde_json::from_slice::<Value>(line).ok()?;
        let u = v["unit"].as_str()?;
        worktree::check_unit_id(u, "").ok()?;
        Some(u.to_string())
    })
}

/// The newest `*.jsonl` by modification time in `root/<unit>`, or in any unit's directory
/// when `unit` is `None`. `None` when there is none, or no such directory.
pub fn newest(root: &Path, unit: Option<&str>) -> io::Result<Option<PathBuf>> {
    let dirs = match unit {
        Some(u) => vec![root.join(u)],
        None => match fs::read_dir(root) {
            Ok(rd) => rd.map(|e| e.map(|e| e.path())).collect::<io::Result<_>>()?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        },
    };
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        let entries = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let meta = fs::metadata(&path)?;
            let candidate = (meta.modified()?, path);
            if meta.is_file() && best.as_ref().is_none_or(|b| candidate > *b) {
                best = Some(candidate);
            }
        }
    }
    Ok(best.map(|(_, p)| p))
}

/// The assistant messages and tool calls of one transcript line, redacted and cut short.
/// Empty for a line that is not JSON.
fn entries(line: &[u8]) -> Vec<Entry> {
    let Ok(mut v) = serde_json::from_slice::<Value>(line) else {
        return Vec::new();
    };
    if v["type"] != "assistant" {
        return Vec::new();
    }
    redact(&mut v);
    line_entries(&v, COMPACT_LIMIT)
        .into_iter()
        .filter(|e| matches!(e, Entry::Assistant { .. } | Entry::Tool { .. }))
        .collect()
}

/// The last `n` entries of `lines`.
fn latest<'a>(lines: impl Iterator<Item = &'a [u8]>, n: usize) -> VecDeque<Entry> {
    let mut kept = VecDeque::with_capacity(n + 1);
    for e in lines.flat_map(entries) {
        kept.push_back(e);
        if kept.len() > n {
            kept.pop_front();
        }
    }
    kept
}

/// How one entry is printed: [`text`] or [`json_line`].
type Show = fn(&Entry) -> String;

fn json_line(e: &Entry) -> String {
    serde_json::to_string(e).expect("an entry serializes")
}

/// `assistant: <text>` or `tool: <name> <input>`, whitespace runs collapsed to one space.
fn text(e: &Entry) -> String {
    let line = match e {
        Entry::Assistant { text } => format!("assistant: {text}"),
        Entry::Tool { name, input } => format!("tool: {name} {input}"),
        _ => unreachable!("entries keeps only assistant messages and tool calls"),
    };
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .flat_map(printable)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs::{File, OpenOptions};
    use std::time::Duration;

    const FIXTURE: &str = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Reading the brief.\n\n  Then the code."}]}}
{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hm"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cat .ns/7-a/brief.md","description":"Read the brief"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"the brief"}]}}
not json
{"type":"user","isMeta":true,"message":{"content":"meta"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"LONG"}]}}
{"type":"result","subtype":"success","total_cost_usd":0.1}
"#;

    fn fixture() -> String {
        FIXTURE.replace("LONG", &"x".repeat(COMPACT_LIMIT + 50))
    }

    fn shown(entries: impl IntoIterator<Item = Entry>, show: Show) -> Vec<String> {
        entries.into_iter().map(|e| show(&e)).collect()
    }

    #[test]
    fn a_transcript_renders_as_one_line_per_message_and_tool_call() {
        let f = fixture();
        let out = shown(latest(f.lines().map(str::as_bytes), LATEST), text);
        assert_eq!(
            out,
            [
                "assistant: Reading the brief. Then the code.".to_string(),
                r#"tool: Bash {"command":"cat .ns/7-a/brief.md","description":"Read the brief"}"#
                    .to_string(),
                format!(
                    "assistant: {}…[truncated 50 chars]",
                    "x".repeat(COMPACT_LIMIT)
                ),
            ]
        );
    }

    #[test]
    fn only_the_latest_entries_are_kept() {
        let f = fixture();
        let out = shown(latest(f.lines().map(str::as_bytes), 1), text);
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("assistant: xxx"), "{out:?}");
        assert!(latest(f.lines().map(str::as_bytes), 0).is_empty());
    }

    #[test]
    fn json_prints_one_entry_object_per_line() {
        let f = fixture();
        let out = shown(latest(f.lines().map(str::as_bytes), LATEST), json_line);
        let tool: Value = serde_json::from_str(&out[1]).unwrap();
        assert_eq!(
            tool,
            serde_json::json!({"kind":"tool","name":"Bash",
                "input":"{\"command\":\"cat .ns/7-a/brief.md\",\"description\":\"Read the brief\"}"})
        );
        let said: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(said["text"], "Reading the brief.\n\n  Then the code.");
    }

    #[test]
    fn control_characters_cannot_reshape_the_line() {
        let e = Entry::Assistant {
            text: "a\u{1b}[31mb\u{202e}c".into(),
        };
        assert_eq!(text(&e), "assistant: a\\u{1b}[31mb\\u{202e}c");
    }

    const SECRETS: [&str; 13] = [
        "ghp_0123456789abcdefghijABCDEFGHIJ",
        "github_pat_11ABCDEFG0123456789_abcdefghij",
        "glpat-abcdefghij0123456789",
        "sk-ant-api03-abcdefghij0123456789",
        "xoxb-1234567890-abcdefghij",
        "AKIAABCDEFGHIJKLMNOP",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop",
        "hunter2pass",
        "s3cr3t-value",
        "abc123def456",
        "MIIEvQIBADANBgkqhkiG9w0BAQEFAASC",
        "basicpassword99",
        "opaquebearer42",
    ];

    #[test]
    fn secret_like_values_do_not_appear_in_the_output() {
        let said = format!(
            "export GITHUB_TOKEN={} and {} then {} {} {} {}\n\
             DB_PASSWORD=\"{}\" api_key: '{}' https://user:{}@git.example.com/r.git\n\
             -----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY----- after",
            SECRETS[0],
            SECRETS[1],
            SECRETS[2],
            SECRETS[3],
            SECRETS[4],
            SECRETS[5],
            SECRETS[7],
            SECRETS[8],
            SECRETS[11],
            SECRETS[10],
        );
        let line = serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"text","text": said},
            {"type":"tool_use","name":"Bash","input":{
                "command": format!("curl -H 'Authorization: Bearer {}' x {}", SECRETS[12], SECRETS[6]),
                "env": {"npmtoken": SECRETS[9], "password": 12345}}}
        ]}})
        .to_string();
        let out = shown(latest([line.as_bytes()].into_iter(), LATEST), text).join("\n");
        let json = shown(latest([line.as_bytes()].into_iter(), LATEST), json_line).join("\n");
        for s in SECRETS.iter().chain(&["12345"]) {
            assert!(!out.contains(s), "{s} in {out}");
            assert!(!json.contains(s), "{s} in {json}");
        }
        assert!(out.contains("GITHUB_TOKEN=[redacted]"), "{out}");
        assert!(
            out.contains("https://[redacted]@git.example.com/r.git"),
            "{out}"
        );
        assert!(out.contains("[redacted private key"), "{out}");
        assert!(out.contains("Bearer [redacted]"), "{out}");
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

    fn said(t: &str) -> String {
        format!(
            "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{t}\"}}]}}}}"
        )
    }

    fn view(
        path: &Path,
        follow: bool,
        current: impl FnMut() -> io::Result<Option<PathBuf>>,
        wait: impl FnMut() -> bool,
    ) -> String {
        let mut out = Vec::new();
        print(path, follow, text, &mut out, current, wait).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn follow_prints_each_appended_entry_once_even_when_a_line_arrives_in_pieces() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        let unfinished = said("half");
        append(&path, &format!("{}\n{unfinished}", said("before")));
        let waits = Cell::new(0);
        let out = view(
            &path,
            true,
            || Ok(Some(path.clone())),
            || {
                let n = waits.get();
                match n {
                    0 => append(&path, "\n"),
                    1 => append(&path, &format!("{}\n", said("after GITHUB_TOKEN=abc123"))),
                    _ => {}
                }
                waits.set(n + 1);
                n < 2
            },
        );
        assert_eq!(
            out,
            "assistant: before\nassistant: half\nassistant: after GITHUB_TOKEN=[redacted]\n"
        );
    }

    #[test]
    fn follow_moves_to_the_next_phase_after_finishing_the_old_one() {
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build-1.jsonl");
        let verify = tmp.path().join("verify-1.jsonl");
        append(&build, &format!("{}\n", said("one")));
        let waits = Cell::new(0);
        let out = view(
            &build,
            true,
            || {
                Ok(Some(if waits.get() >= 1 {
                    verify.clone()
                } else {
                    build.clone()
                }))
            },
            || {
                let n = waits.get();
                if n == 0 {
                    append(&build, &format!("{}\n", said("last of build")));
                    append(&verify, &format!("{}\n{}\n", said("two"), said("three")));
                }
                waits.set(n + 1);
                n < 2
            },
        );
        assert_eq!(
            out,
            "assistant: one\nassistant: last of build\nassistant: two\nassistant: three\n"
        );
    }

    #[test]
    fn follow_prints_the_last_lines_of_a_finished_phase_before_switching() {
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build-1.jsonl");
        let verify = tmp.path().join("verify-1.jsonl");
        append(&build, &format!("{}\n", said("one")));
        append(&verify, &format!("{}\n", said("two")));
        let polls = Cell::new(0);
        let out = view(
            &build,
            true,
            || {
                polls.set(polls.get() + 1);
                if polls.get() == 1 {
                    return Ok(Some(build.clone()));
                }
                append(&build, &format!("{}\n", said("last of build")));
                Ok(Some(verify.clone()))
            },
            || polls.get() < 3,
        );
        assert_eq!(
            out,
            "assistant: one\nassistant: last of build\nassistant: two\n"
        );
    }

    #[test]
    fn only_the_latest_entries_of_a_long_transcript_are_printed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        let lines: String = (0..LATEST + 5)
            .map(|i| format!("{}\n", said(&format!("m{i}"))))
            .collect();
        append(&path, &lines);
        let out = view(&path, false, || Ok(None), || false);
        assert_eq!(out.lines().count(), LATEST, "{out}");
        assert_eq!(out.lines().next(), Some("assistant: m5"));
    }

    #[test]
    fn print_shows_only_whole_lines_and_stops_without_follow() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        fs::write(&path, format!("{}{}", fixture(), said("unfinished"))).unwrap();
        let out = view(&path, false, || panic!("resolved"), || panic!("waited"));
        assert_eq!(out.lines().count(), 3, "{out}");
        assert!(!out.contains("unfinished"), "{out}");
        assert!(print(
            &tmp.path().join("gone.jsonl"),
            false,
            text,
            &mut Vec::new(),
            || Ok(None),
            || false
        )
        .is_err());
    }

    #[test]
    fn only_the_end_of_a_long_transcript_is_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        let old = said(&"o".repeat(WINDOW as usize));
        append(
            &path,
            &format!("{old}\n{}\n{}\n", said("recent"), said("newest")),
        );
        let out = view(&path, false, || Ok(None), || false);
        assert_eq!(out, "assistant: recent\nassistant: newest\n");
    }

    fn touch(path: &Path, secs: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        File::create(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn newest_picks_the_latest_transcript_of_the_unit_or_of_any_unit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("transcripts");
        assert_eq!(newest(&root, None).unwrap(), None);
        assert_eq!(newest(&root, Some("7-a")).unwrap(), None);
        touch(&root.join("7-a/build-1.jsonl"), 100);
        touch(&root.join("7-a/verify-1.jsonl"), 300);
        touch(&root.join("7-a/review-1.stderr"), 900);
        touch(&root.join("8-b/build-1.jsonl"), 200);
        fs::write(root.join("stray.jsonl"), "").unwrap();
        fs::create_dir_all(root.join("8-b/dir.jsonl")).unwrap();
        let at = |p: &str| Some(root.join(p));
        assert_eq!(
            newest(&root, Some("7-a")).unwrap(),
            at("7-a/verify-1.jsonl")
        );
        assert_eq!(newest(&root, Some("8-b")).unwrap(), at("8-b/build-1.jsonl"));
        assert_eq!(newest(&root, None).unwrap(), at("7-a/verify-1.jsonl"));
        assert_eq!(newest(&root, Some("9-c")).unwrap(), None);
    }

    #[test]
    fn the_current_transcript_is_the_newest_of_the_unit_the_run_log_last_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("transcripts");
        let runs = tmp.path().join("runs.jsonl");
        touch(&root.join("7-a/build-1.jsonl"), 100);
        touch(&root.join("7-a/verify-1.jsonl"), 200);
        touch(&root.join("8-b/build-1.jsonl"), 900);
        assert_eq!(
            current(&root, &runs, None).unwrap(),
            Some(root.join("8-b/build-1.jsonl"))
        );
        fs::write(
            &runs,
            "{\"event\":\"start\",\"unit\":\"8-b\"}\n{\"event\":\"phase\",\"unit\":\"7-a\",\"phase\":\"build\"}\nnot json\n{\"event\":\"lock_wait\"}\n",
        )
        .unwrap();
        assert_eq!(
            current(&root, &runs, None).unwrap(),
            Some(root.join("7-a/verify-1.jsonl"))
        );
        assert_eq!(
            current(&root, &runs, Some("8-b")).unwrap(),
            Some(root.join("8-b/build-1.jsonl"))
        );
        fs::write(
            &runs,
            "{\"event\":\"start\",\"unit\":\"9-c\"}\n{\"event\":\"start\",\"unit\":\"../x\"}\n",
        )
        .unwrap();
        assert_eq!(
            current(&root, &runs, None).unwrap(),
            Some(root.join("8-b/build-1.jsonl"))
        );
    }

    #[test]
    fn only_the_end_of_the_run_log_names_the_unit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("transcripts");
        let runs = tmp.path().join("runs.jsonl");
        touch(&root.join("7-a/build-1.jsonl"), 100);
        touch(&root.join("8-b/build-1.jsonl"), 900);
        let filler = format!(
            "{{\"event\":\"lock_wait\",\"pad\":\"{}\"}}\n",
            "p".repeat(1000)
        );
        let log = format!(
            "{{\"event\":\"start\",\"unit\":\"7-a\"}}\n{}",
            filler.repeat(RUNS_TAIL as usize / filler.len() + 2)
        );
        fs::write(&runs, log).unwrap();
        assert_eq!(
            current(&root, &runs, None).unwrap(),
            Some(root.join("8-b/build-1.jsonl"))
        );
    }

    #[test]
    fn a_run_log_that_cannot_be_read_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let runs = tmp.path().join("runs.jsonl");
        fs::create_dir(&runs).unwrap();
        assert!(current(&tmp.path().join("transcripts"), &runs, None).is_err());
    }

    #[test]
    fn follow_starts_over_when_the_transcript_is_rewritten_shorter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        append(
            &path,
            &format!("{}\n{}\n", said("first old"), said("second old")),
        );
        let waits = Cell::new(0);
        let out = view(
            &path,
            true,
            || Ok(Some(path.clone())),
            || {
                let n = waits.get();
                if n == 0 {
                    fs::write(&path, format!("{}\n", said("new"))).unwrap();
                }
                waits.set(n + 1);
                n < 1
            },
        );
        assert_eq!(
            out,
            "assistant: first old\nassistant: second old\nassistant: new\n"
        );
    }

    #[test]
    fn follow_propagates_an_error_from_resolving_the_current_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        append(&path, &format!("{}\n", said("one")));
        let err = print(
            &path,
            true,
            text,
            &mut Vec::new(),
            || Err(io::Error::other("boom")),
            || panic!("waited"),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn follow_keeps_its_place_while_the_transcript_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        append(&path, &format!("{}\n", said("one")));
        let waits = Cell::new(0);
        let out = view(
            &path,
            true,
            || Ok(Some(path.clone())),
            || {
                let n = waits.get();
                if n == 0 {
                    fs::remove_file(&path).unwrap();
                }
                waits.set(n + 1);
                n < 2
            },
        );
        assert_eq!(out, "assistant: one\n");
        assert!(drain(&tmp.path().join("gone.jsonl"), 7, text, &mut Vec::new()).unwrap() == 7);
        assert!(snapshot(&tmp.path().join("gone.jsonl"), text, &mut Vec::new()).is_err());
    }

    #[test]
    fn a_window_without_a_newline_prints_nothing_and_stays_inside_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        let body = said(&"o".repeat(WINDOW as usize + 10));
        append(&path, &format!("{}\n{body}", said("early")));
        let len = fs::metadata(&path).unwrap().len();
        let mut out = Vec::new();
        let at = snapshot(&path, text, &mut out).unwrap();
        assert!(out.is_empty());
        assert_eq!(at, len - WINDOW);
    }

    #[test]
    fn follow_after_a_long_transcript_prints_each_appended_entry_once() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        let old = said(&"o".repeat(WINDOW as usize));
        append(&path, &format!("{old}\n{}\n", said("recent")));
        let waits = Cell::new(0);
        let out = view(
            &path,
            true,
            || Ok(Some(path.clone())),
            || {
                let n = waits.get();
                if n == 0 {
                    append(&path, &format!("{}\n", said("appended")));
                }
                waits.set(n + 1);
                n < 2
            },
        );
        assert_eq!(out, "assistant: recent\nassistant: appended\n");
    }
}
