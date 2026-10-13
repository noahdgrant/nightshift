//! `ns rules check` and `ns rules baseline`: run the repo's rule pack and ratchet its violations
//! against a committed baseline (docs/factory/run.md, "Rules").

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::SfError;
use crate::eval::trial::run_process;
use crate::ratchet::Format;

pub const BASELINE: &str = ".nightshift/rules-baseline.json";
const STACK: &str = "docs/agents/stack.md";
const RULE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

/// One violation. `snippet` is the offending source line with whitespace collapsed; with
/// `tool`, `rule` and `file` it is what matches a baseline entry, so the line number may move.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub tool: String,
    pub rule: String,
    pub file: String,
    pub line: Option<u64>,
    pub message: String,
    pub snippet: String,
}

impl Finding {
    fn key(&self) -> (&str, &str, &str, &str) {
        (&self.tool, &self.rule, &self.file, &self.snippet)
    }
}

#[derive(Serialize, Deserialize)]
struct Baseline {
    version: u32,
    violations: Vec<Finding>,
}

/// The commands in a table row `| rules | `<cmd>`, `<cmd>` | ... |`: every backticked span
/// that opens the second cell, separated by commas or spaces (a span may contain `|`).
pub fn commands(stack_md: &str) -> Vec<String> {
    crate::stack::rows(stack_md, "rules")
        .map(|mut rest| {
            let mut cmds = Vec::new();
            while let Some(span) = rest
                .trim_start_matches(|c: char| c == ',' || c.is_whitespace())
                .strip_prefix('`')
            {
                let Some((cmd, after)) = span.split_once('`') else {
                    break;
                };
                if !cmd.trim().is_empty() {
                    cmds.push(cmd.trim().to_string());
                }
                rest = after;
            }
            cmds
        })
        .find(|cmds| !cmds.is_empty())
        .unwrap_or_default()
}

fn normalise(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A finding from one output line: a JSON object with `file` (and optionally `tool`, `rule`,
/// `line`, `message`, `snippet`), or `file:line[:col]: message`. Anything else is not a finding.
fn parse_line(tool: &str, line: &str) -> Option<Finding> {
    let line = line.trim();
    if line.starts_with('{') {
        let v: Value = serde_json::from_str(line).ok()?;
        let s = |k: &str| v[k].as_str().map(String::from);
        return Some(Finding {
            tool: s("tool").unwrap_or_else(|| tool.to_string()),
            rule: s("rule").unwrap_or_default(),
            file: s("file")?,
            line: v["line"].as_u64(),
            message: s("message").unwrap_or_default(),
            snippet: s("snippet").map(|s| normalise(&s)).unwrap_or_default(),
        });
    }
    let (file, rest) = line.split_once(':')?;
    let (num, rest) = rest.split_once(':')?;
    let num: u64 = num.parse().ok()?;
    let message = match rest.split_once(':') {
        Some((col, m)) if col.parse::<u64>().is_ok() => m,
        _ => rest,
    };
    if file.is_empty() || file.contains(char::is_whitespace) {
        return None;
    }
    Some(Finding {
        tool: tool.to_string(),
        rule: String::new(),
        file: file.to_string(),
        line: Some(num),
        message: message.trim().to_string(),
        snippet: String::new(),
    })
}

/// The command's first word without its directory: `./scripts/lint-x --strict` is `lint-x`.
fn tool_name(cmd: &str) -> String {
    let first = cmd.split_whitespace().next().unwrap_or(cmd);
    first.rsplit('/').next().unwrap_or(first).to_string()
}

/// Make `file` relative to `root` and fill an empty snippet from the source line, else the message.
fn settle(mut f: Finding, root: &Path, sources: &mut HashMap<String, Option<String>>) -> Finding {
    let rel = Path::new(&f.file)
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| f.file.clone());
    f.file = rel.strip_prefix("./").unwrap_or(&rel).to_string();
    if f.snippet.is_empty() {
        let text = sources
            .entry(f.file.clone())
            .or_insert_with(|| fs::read_to_string(root.join(&f.file)).ok());
        let src = match (text, f.line) {
            (Some(t), Some(n)) if n > 0 => t.lines().nth(n as usize - 1),
            _ => None,
        };
        f.snippet = normalise(src.unwrap_or(&f.message));
    }
    f
}

struct Run {
    findings: Vec<Finding>,
    commands: Vec<Value>,
}

/// Run each command with `sh -c` in `root`, stdin closed, and collect what it printed on stdout
/// and stderr. A command that outlives its timeout, is killed by a signal or exits 126 or 127
/// is an error whatever it printed first.
fn run_pack(root: &Path, cmds: &[String]) -> Result<Run> {
    let timeout = timeout();
    let mut sources = HashMap::new();
    let mut findings = Vec::new();
    let mut commands = Vec::new();
    for cmd in cmds {
        eprintln!("ns rules: {cmd}");
        let tool = tool_name(cmd);
        let log = tempfile::tempfile().context("cannot create a file for rule output")?;
        let mut c = Command::new("sh");
        crate::git::scrub(&mut c);
        c.arg("-c")
            .arg(cmd)
            .current_dir(root)
            .stdout(log.try_clone()?)
            .stderr(log.try_clone()?);
        let (status, timed_out, _) = run_process(c, None, timeout)
            .with_context(|| format!("cannot run rule command `{cmd}`"))?;
        let text = read_capped(log, cmd)?;
        let exit = status.and_then(|s| s.code());
        let broken = if timed_out {
            Some(format!("timed out after {}s", timeout.as_secs_f64()))
        } else {
            match exit {
                None => Some("was killed by a signal".to_string()),
                Some(c @ (126 | 127)) => Some(format!("exited {c}")),
                _ => None,
            }
        };
        let found: Vec<Finding> = text
            .lines()
            .filter_map(|l| parse_line(&tool, l))
            .map(|f| settle(f, root, &mut sources))
            .collect();
        let how = broken.or_else(|| {
            (found.is_empty() && exit != Some(0)).then(|| {
                format!(
                    "exited {} with no findings it could read",
                    exit.unwrap_or_default()
                )
            })
        });
        if let Some(how) = how {
            let tail: Vec<&str> = text.trim().lines().rev().take(20).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            return Err(SfError::general(format!(
                "rule command `{cmd}` {how}:\n{}",
                tail.join("\n")
            ))
            .hint(format!("run it by hand in {}:\n  {cmd}", root.display()))
            .into());
        }
        commands.push(json!({"command": cmd, "tool": tool, "exit": exit, "findings": found.len()}));
        findings.extend(found);
    }
    findings.sort();
    Ok(Run { findings, commands })
}

fn timeout() -> Duration {
    std::env::var("NS_RULES_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(RULE_TIMEOUT, Duration::from_millis)
}

fn max_output() -> u64 {
    std::env::var("NS_RULES_MAX_OUTPUT_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MAX_OUTPUT_BYTES)
}

fn read_capped(mut log: File, cmd: &str) -> Result<String> {
    let cap = max_output();
    log.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    log.take(cap.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        return Err(SfError::general(format!(
            "rule command `{cmd}` printed more than {cap} bytes"
        ))
        .hint(format!(
            "narrow the command, or raise the cap with NS_RULES_MAX_OUTPUT_BYTES; run it by hand:\n  {cmd}"
        ))
        .into());
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Split `current` into new findings and baselined ones, and return the baseline entries
/// nothing matched. Matching counts: two identical findings need two entries.
fn compare(current: &[Finding], baseline: &[Finding]) -> (Vec<Finding>, usize, Vec<Finding>) {
    let mut left: HashMap<_, Vec<&Finding>> = HashMap::new();
    for b in baseline {
        left.entry(b.key()).or_default().push(b);
    }
    let mut new = Vec::new();
    let mut matched = 0;
    for f in current {
        match left.get_mut(&f.key()).and_then(Vec::pop) {
            Some(_) => matched += 1,
            None => new.push(f.clone()),
        }
    }
    let mut fixed: Vec<Finding> = left.into_values().flatten().cloned().collect();
    fixed.sort();
    (new, matched, fixed)
}

fn load_baseline(path: &Path) -> Result<Option<Vec<Finding>>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let b: Baseline = serde_json::from_str(&text).map_err(|e| {
        SfError::general(format!("{} is not a rules baseline: {e}", path.display()))
            .hint("restore it from git:\n  git checkout -- .nightshift/rules-baseline.json\nor delete it and have a human rerun:\n  ns rules baseline --allow-growth")
    })?;
    Ok(Some(b.violations))
}

struct Ctx {
    root: PathBuf,
    commands: Vec<String>,
}

fn ctx(usage: &str) -> Result<Ctx> {
    let cwd = std::env::current_dir().context("cannot read current directory")?;
    let root = crate::git::run(&cwd, &["rev-parse", "--show-toplevel"]).map_err(|_| {
        SfError::usage(
            format!("{} is not inside a git repository", cwd.display()),
            format!("run it from the repo:\n  cd ~/src/myrepo && {usage}"),
        )
    })?;
    let root = PathBuf::from(root);
    let stack = fs::read_to_string(root.join(STACK)).unwrap_or_default();
    Ok(Ctx {
        commands: commands(&stack),
        root,
    })
}

fn line(f: &Finding) -> String {
    let at = f.line.map_or(String::new(), |n| format!(":{n}"));
    let who = if f.rule.is_empty() {
        f.tool.clone()
    } else {
        format!("{}/{}", f.tool, f.rule)
    };
    format!("{}{at}: {who}: {}", f.file, f.message)
}

fn print(v: &Value, human: &[(&str, &[Finding])], summary: &str, format: Format) -> Result<()> {
    match format {
        Format::Human => {
            for (label, list) in human {
                for f in *list {
                    println!("{label}: {}", line(f));
                }
            }
            println!("{summary}");
        }
        Format::Json => println!("{}", serde_json::to_string_pretty(v)?),
    }
    Ok(())
}

pub fn check(only: Option<&str>, format: Format) -> Result<ExitCode> {
    let mut c = ctx("ns rules check")?;
    if let Some(text) = only.filter(|_| !c.commands.is_empty()) {
        let all = std::mem::take(&mut c.commands);
        c.commands = all
            .iter()
            .filter(|cmd| cmd.contains(text))
            .cloned()
            .collect();
        if c.commands.is_empty() {
            return Err(SfError::usage(
                format!("no command in the rules row contains `{text}`"),
                format!(
                    "pick text from one of {STACK}'s rules commands:\n  {}",
                    all.join("\n  ")
                ),
            )
            .into());
        }
    }
    if c.commands.is_empty() {
        eprintln!("ns rules: no rules row in {STACK}; nothing to check");
        let v = json!({
            "ok": true,
            "command": "rules check",
            "baseline": BASELINE,
            "commands": [],
            "findings": 0,
            "baselined": 0,
            "new": [],
            "fixed": [],
        });
        print(&v, &[], "0 findings, 0 baselined, 0 new, 0 fixed", format)?;
        return Ok(ExitCode::SUCCESS);
    }
    let run = run_pack(&c.root, &c.commands)?;
    let baseline = load_baseline(&c.root.join(BASELINE))?.unwrap_or_default();
    let (new, baselined, mut fixed) = compare(&run.findings, &baseline);
    if only.is_some() {
        fixed.clear();
    }
    let ok = new.is_empty();
    let v = json!({
        "ok": ok,
        "command": "rules check",
        "baseline": BASELINE,
        "commands": run.commands,
        "findings": run.findings.len(),
        "baselined": baselined,
        "new": new,
        "fixed": fixed,
    });
    let summary = format!(
        "{} findings, {baselined} baselined, {} new, {} fixed",
        run.findings.len(),
        new.len(),
        fixed.len()
    );
    print(&v, &[("new", &new), ("fixed", &fixed)], &summary, format)?;
    if !fixed.is_empty() {
        eprintln!(
            "ns rules: {} baselined violations are gone; drop them with:\n  ns rules baseline",
            fixed.len()
        );
    }
    if !ok {
        eprintln!(
            "ns rules: {} new violations; fix them, then rerun:\n  ns rules check",
            new.len()
        );
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

pub fn baseline(allow_growth: bool, dry_run: bool, format: Format) -> Result<ExitCode> {
    let c = ctx("ns rules baseline")?;
    if c.commands.is_empty() {
        return Err(SfError::usage(
            format!("no rules row in {STACK}"),
            "add one to its Commands table, then rerun:\n  | rules | `ruff check --output-format=concise .` | 5 s |\n  ns rules baseline",
        )
        .into());
    }
    let run = run_pack(&c.root, &c.commands)?;
    let path = c.root.join(BASELINE);
    let old = load_baseline(&path)?;
    let (added, _, dropped) = compare(&run.findings, old.as_deref().unwrap_or_default());
    let refused = !added.is_empty() && !allow_growth;
    let status = if refused {
        "refused"
    } else if added.is_empty() && dropped.is_empty() && old.is_some() {
        "already-done"
    } else if dry_run {
        "planned"
    } else {
        let text = serde_json::to_string_pretty(&Baseline {
            version: 1,
            violations: run.findings.clone(),
        })?;
        fs::create_dir_all(path.parent().unwrap_or(&c.root))
            .with_context(|| format!("cannot create the directory for {}", path.display()))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text + "\n").with_context(|| format!("cannot write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))?;
        "written"
    };
    let v = json!({
        "ok": !refused,
        "command": "rules baseline",
        "status": status,
        "dry_run": dry_run,
        "baseline": BASELINE,
        "commands": run.commands,
        "violations": run.findings.len(),
        "added": added,
        "dropped": dropped,
    });
    let summary = format!(
        "{status}: {} violations, {} added, {} dropped",
        run.findings.len(),
        added.len(),
        dropped.len()
    );
    print(
        &v,
        &[("added", &added), ("dropped", &dropped)],
        &summary,
        format,
    )?;
    if refused {
        eprintln!(
            "ns rules: {} violations are not in {BASELINE}. Fix them, or have a human accept them:\n  ns rules baseline --allow-growth",
            added.len()
        );
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(file: &str, line: u64, snippet: &str) -> Finding {
        Finding {
            tool: "t".into(),
            rule: "r".into(),
            file: file.into(),
            line: Some(line),
            message: "m".into(),
            snippet: snippet.into(),
        }
    }

    #[test]
    fn commands_reads_every_backticked_span_in_the_rules_row() {
        assert_eq!(
            commands("| rules | `ruff check .`, `lint-imports` `cargo deny check` | 5 s |"),
            ["ruff check .", "lint-imports", "cargo deny check"]
        );
        assert_eq!(
            commands("| `rules` | `a | jq -c .` | see `docs` |"),
            ["a | jq -c ."]
        );
        assert_eq!(commands("| rules | `a`, then `b` |"), ["a"]);
        assert_eq!(commands("| rules | none |\n| rules | `x` |"), ["x"]);
        assert!(commands("| rules | `` |").is_empty());
        assert!(commands("| rules | `open").is_empty());
        assert!(commands("| ci-local | `make ci` |").is_empty());
        assert!(commands("rules: `x`").is_empty());
    }

    #[test]
    fn parse_line_reads_json_and_file_line_col_message() {
        let j = parse_line(
            "ast-grep",
            r#"{"tool": "sg", "rule": "no-eval", "file": "a.py", "line": 4, "message": "m", "snippet": "  x =  1 "}"#,
        )
        .unwrap();
        assert_eq!(
            (j.tool.as_str(), j.rule.as_str(), j.line, j.snippet.as_str()),
            ("sg", "no-eval", Some(4), "x = 1")
        );
        let j = parse_line("ast-grep", r#"{"file": "a.py"}"#).unwrap();
        assert_eq!(
            (j.tool.as_str(), j.line, j.message.as_str()),
            ("ast-grep", None, "")
        );
        assert_eq!(parse_line("x", r#"{"rule": "r"}"#), None);
        assert_eq!(parse_line("x", "{not json"), None);

        let p = parse_line("ruff", "a.py:3:8: F401 `os` imported but unused").unwrap();
        assert_eq!(
            (p.file.as_str(), p.line, p.message.as_str()),
            ("a.py", Some(3), "F401 `os` imported but unused")
        );
        let p = parse_line("cc", "src/x.c:12: warning: unused: 7").unwrap();
        assert_eq!(
            (p.line, p.message.as_str()),
            (Some(12), "warning: unused: 7")
        );
        assert_eq!(parse_line("x", "warning: unused variable"), None);
        assert_eq!(parse_line("x", "a.py:x:1: m"), None);
        assert_eq!(parse_line("x", "   Compiling foo v1: 2: x"), None);
        assert_eq!(parse_line("x", ":3: m"), None);
        assert_eq!(parse_line("x", "my file.py:3: m"), None);
        assert_eq!(parse_line("x", "a.py:3"), None);
    }

    #[test]
    fn tool_name_is_the_first_word_without_its_directory() {
        assert_eq!(tool_name("./scripts/lint-x --strict"), "lint-x");
        assert_eq!(tool_name("ruff check ."), "ruff");
    }

    #[test]
    fn settle_makes_paths_relative_and_snippets_from_source() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.py"), "one\n  two   2\n").unwrap();
        let mut src = HashMap::new();
        let mut raw = f(&tmp.path().join("a.py").to_string_lossy(), 2, "");
        raw.message = "msg".into();
        let s = settle(raw.clone(), tmp.path(), &mut src);
        assert_eq!((s.file.as_str(), s.snippet.as_str()), ("a.py", "two 2"));
        raw.file = "./a.py".into();
        raw.line = Some(9);
        let s = settle(raw.clone(), tmp.path(), &mut src);
        assert_eq!((s.file.as_str(), s.snippet.as_str()), ("a.py", "msg"));
        raw.line = Some(0);
        assert_eq!(settle(raw.clone(), tmp.path(), &mut src).snippet, "msg");
        raw.line = None;
        raw.file = "missing.py".into();
        assert_eq!(settle(raw.clone(), tmp.path(), &mut src).snippet, "msg");
        raw.snippet = "given".into();
        assert_eq!(settle(raw, tmp.path(), &mut src).snippet, "given");
    }

    #[test]
    fn compare_matches_on_tool_rule_file_and_snippet_and_counts_duplicates() {
        let base = [f("a", 1, "x"), f("a", 5, "y")];
        let (new, matched, fixed) = compare(&[f("a", 9, "x"), f("a", 10, "x")], &base);
        assert_eq!(new, [f("a", 10, "x")]);
        assert_eq!(matched, 1);
        assert_eq!(fixed, [f("a", 5, "y")]);
        let mut other = f("a", 1, "x");
        other.rule = "r2".into();
        let (new, _, _) = compare(std::slice::from_ref(&other), &base);
        assert_eq!(new, [other]);
        let (new, matched, fixed) = compare(&[], &[]);
        assert!(new.is_empty() && fixed.is_empty() && matched == 0);
    }

    #[test]
    fn a_malformed_baseline_is_an_error_naming_the_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("b.json");
        assert!(load_baseline(&p).unwrap().is_none());
        let e = load_baseline(tmp.path()).unwrap_err();
        assert!(format!("{e:#}").contains("cannot read"), "{e:#}");
        fs::write(&p, "{}").unwrap();
        let e = load_baseline(&p).unwrap_err();
        let e = e.downcast_ref::<SfError>().unwrap();
        assert!(e.hint.as_deref().unwrap().contains("git checkout --"));
    }

    #[test]
    fn human_lines_name_the_rule_when_there_is_one() {
        assert_eq!(line(&f("a.py", 3, "")), "a.py:3: t/r: m");
        let mut g = f("a.py", 3, "");
        g.rule.clear();
        g.line = None;
        assert_eq!(line(&g), "a.py: t: m");
    }
}
