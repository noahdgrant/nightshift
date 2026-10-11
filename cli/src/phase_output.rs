//! `ns log --phase-output`: the assistant messages and tool calls of a unit's newest phase
//! transcript, `<git-common-dir>/ns/transcripts/<unit>/<phase>-<attempt>.jsonl`, one line each,
//! long text cut short and secret-like values redacted.

use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;
use std::time::SystemTime;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

use crate::error::SfError;
use crate::git::Repo;
use crate::log::{printable, Tail, POLL};
use crate::transcripts::{line_entries, Entry, COMPACT_LIMIT};
use crate::worktree;

/// How many entries the transcript so far is cut to before printing or following.
const LATEST: usize = 20;

const REDACTED: &str = "[redacted]";

/// Keys whose value is a credential, wherever they sit in a transcript line: any key ending in
/// one of these words, so `GH_TOKEN` and `githubtoken` but not `input_tokens`.
static SECRET_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:token|secret|password|passwd|api[_-]?key|access[_-]?key|private[_-]?key|credentials?|authorization)$").unwrap()
});

/// Secret-like text and what replaces it, applied in order.
static SECRET_TEXT: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
    [
        (
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----(?s:.*?)(?:-----END [A-Z ]*PRIVATE KEY-----|$)",
            "[redacted private key]",
        ),
        (r"(://)[^/\s:@]+:[^/\s@]+@", "${1}[redacted]@"),
        (r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}", "$1 [redacted]"),
        (
            r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_-]{20,}|sk-[A-Za-z0-9_-]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})",
            REDACTED,
        ),
        (
            r#"(?i)\b([A-Za-z0-9_.-]*(?:token|secret|password|passwd|api[_-]?key|access[_-]?key|private[_-]?key|credentials?)[A-Za-z0-9_]*"?\s*[:=]\s*)("[^"\n]*"|'[^'\n]*'|[^\s"',;]+)"#,
            "${1}[redacted]",
        ),
    ]
    .into_iter()
    .map(|(re, with)| (Regex::new(re).unwrap(), with))
    .collect()
});

pub fn cli(unit: Option<&str>, follow: bool, as_json: bool) -> Result<ExitCode> {
    if let Some(u) = unit {
        worktree::check_unit_id(u, "ns log --phase-output --unit 142-uart-timeout")?;
    }
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let root = repo.common_dir.join("ns").join("transcripts");
    let path = newest(&root, unit)
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
    match print(&path, follow, show, &mut out) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(ExitCode::SUCCESS),
        r => r
            .map(|()| ExitCode::SUCCESS)
            .with_context(|| format!("ns log --phase-output on {}", path.display())),
    }
}

/// Print the last [`LATEST`] entries of the transcript's whole lines, then with `follow` the
/// entries of each line appended after them.
fn print(path: &Path, follow: bool, show: Show, out: &mut impl Write) -> io::Result<()> {
    let bytes = fs::read(path)?;
    let whole = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    for e in latest(bytes[..whole].split(|b| *b == b'\n'), LATEST) {
        writeln!(out, "{}", show(&e))?;
    }
    if follow {
        let tail = Tail::at(path.to_path_buf(), whole as u64, || {
            std::thread::sleep(POLL)
        });
        render(tail, show, out)?;
    }
    Ok(())
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

/// Print the entries of each line `lines` yields, as `show` formats them, one per line.
fn render(
    lines: impl Iterator<Item = io::Result<Vec<u8>>>,
    show: Show,
    out: &mut impl Write,
) -> io::Result<()> {
    for line in lines {
        for e in entries(&line?) {
            writeln!(out, "{}", show(&e))?;
        }
    }
    Ok(())
}

fn redact(v: &mut Value) {
    match v {
        Value::String(s) => {
            for (re, with) in SECRET_TEXT.iter() {
                if re.is_match(s) {
                    *s = re.replace_all(s, *with).into_owned();
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        Value::Object(fields) => {
            for (k, v) in fields.iter_mut() {
                if SECRET_KEY.is_match(k) && !v.is_null() {
                    *v = Value::String(REDACTED.into());
                } else {
                    redact(v);
                }
            }
        }
        _ => {}
    }
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

    #[test]
    fn an_unterminated_private_key_is_redacted_to_the_end() {
        let mut v = Value::String("key: -----BEGIN PRIVATE KEY-----\nMIIabc".into());
        redact(&mut v);
        assert_eq!(v, "key: [redacted private key]");
    }

    #[test]
    fn ordinary_text_and_token_counts_are_left_alone() {
        let mut v = serde_json::json!({"input_tokens": 12, "text": "ran cargo test, 3 passed",
            "auth": null, "token": null});
        let before = v.clone();
        redact(&mut v);
        assert_eq!(v, before);
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
    fn follow_prints_each_appended_entry_from_where_the_snapshot_ended() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        let said = |t: &str| {
            format!(
                "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{t}\"}}]}}}}\n"
            )
        };
        append(&path, &said("before"));
        let start = fs::metadata(&path).unwrap().len();
        let next = said("after GITHUB_TOKEN=abc123");
        let (head, rest) = next.split_at(20);
        let chunks = [head.to_string(), rest.to_string(), said("last")];
        // Each wait appends the next chunk, as a running phase would while ns log sleeps.
        let waits = Cell::new(0);
        let tail = Tail::at(path.clone(), start, || {
            let n = waits.get();
            append(&path, &chunks[n]);
            waits.set(n + 1);
        });
        let mut out = Vec::new();
        render(tail.take(2), text, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "assistant: after GITHUB_TOKEN=[redacted]\nassistant: last\n"
        );
        assert_eq!(waits.get(), 3);
    }

    #[test]
    fn print_shows_only_whole_lines_and_stops_without_follow() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build-1.jsonl");
        fs::write(&path, format!("{}{{\"type\":\"assist", fixture())).unwrap();
        let mut out = Vec::new();
        print(&path, false, text, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.lines().count(), 3, "{out}");
        assert!(print(&tmp.path().join("gone.jsonl"), false, text, &mut Vec::new()).is_err());
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
}
