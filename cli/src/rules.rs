//! `ns rules check` and `ns rules baseline`: run the repo's rule pack and ratchet its violations
//! against a committed baseline (docs/factory/run.md, "Rules").

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::SfError;
use crate::ratchet::Format;

pub const BASELINE: &str = ".nightshift/rules-baseline.json";
const STACK: &str = "docs/agents/stack.md";
const RULE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

/// One violation. `snippet` is the offending source line with whitespace collapsed; with
/// `tool`, `rule` and `file` it is what matches a baseline entry, so the line number may move.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// A finding as the baseline stores it: without its line, so a line shift leaves the file
/// unchanged. A `line` in an older file is ignored.
#[derive(Serialize, Deserialize, PartialEq, Eq, Hash)]
struct Entry {
    tool: String,
    rule: String,
    file: String,
    message: String,
    snippet: String,
}

impl From<&Finding> for Entry {
    fn from(f: &Finding) -> Entry {
        Entry {
            tool: f.tool.clone(),
            rule: f.rule.clone(),
            file: f.file.clone(),
            message: f.message.clone(),
            snippet: f.snippet.clone(),
        }
    }
}

impl From<Entry> for Finding {
    fn from(e: Entry) -> Finding {
        Finding {
            tool: e.tool,
            rule: e.rule,
            file: e.file,
            line: None,
            message: e.message,
            snippet: e.snippet,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Baseline {
    version: u32,
    violations: Vec<Entry>,
}

/// The commands in the `rules` row of `stack_md`, as [`crate::stack::spans`] reads them.
pub fn commands(stack_md: &str) -> Vec<String> {
    crate::stack::spans(stack_md, "rules")
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
        let ended = crate::proc::Proc::tool("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(root)
            .output_to(log.try_clone()?)
            .timeout(timeout)
            .run()
            .with_context(|| format!("cannot run rule command `{cmd}`"))?;
        let text = read_capped(log, cmd)?;
        let exit = ended.status.and_then(|s| s.code());
        let broken = if ended.timed_out() {
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

/// The rules whose `message` carries a number the finding measures, and the text around it. A rule
/// this table lacks has no measured value and matches its baseline entry as it always did, which
/// also keeps `ns` out of most tools' message prose (D50, ADR-746).
fn shape(tool: &str, rule: &str) -> Option<(&'static str, &'static str)> {
    match (tool, rule) {
        // "this function has too many lines (261/100) (quality bar, ...)": the limit is not the
        // value.
        ("clippy", "clippy::too_many_lines") => Some(("this function has too many lines (", "/")),
        // "found 2 duplicate entries for crate 'syn' (one road per job: ...)".
        ("cargo-deny", "duplicate") => Some(("found ", " duplicate entries")),
        _ => None,
    }
}

/// The number a rule's finding measures, read out of `message`. `None` when the rule has none
/// (see [`shape`]) or the message is not its registered shape, so a message the tool reworded only
/// stops the ratchet for that rule.
fn measured(f: &Finding) -> Option<u64> {
    let (prefix, suffix) = shape(&f.tool, &f.rule)?;
    let rest = f.message.strip_prefix(prefix)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !rest[digits.len()..].starts_with(suffix) {
        return None;
    }
    digits.parse().ok()
}

/// A finding's and a baseline entry's measured values, when both have one.
fn values(f: &Finding, b: &Finding) -> Option<(u64, u64)> {
    Some((measured(f)?, measured(b)?))
}

/// Whether `f` measures more than the baseline entry `b` does.
fn grew(f: &Finding, b: &Finding) -> bool {
    values(f, b).is_some_and(|(now, was)| now > was)
}

/// Whether `f` measures less than the baseline entry `b` does.
fn shrank(f: &Finding, b: &Finding) -> bool {
    values(f, b).is_some_and(|(now, was)| now < was)
}

/// A finding the baseline does not accept: no entry shares its key, or one does and its measured
/// value outgrew it. `grew` holds the entry's value and the finding's then, for the report.
struct New {
    finding: Finding,
    grew: Option<(u64, u64)>,
}

impl New {
    /// The human line for the finding, naming the value it grew past.
    fn line(&self) -> String {
        match self.grew {
            Some((was, now)) => format!("{} (grew from {was} to {now})", line(&self.finding)),
            None => line(&self.finding),
        }
    }
}

/// Matching a run's findings against the baseline.
struct Matched {
    /// Findings the baseline does not accept, in the findings' order.
    new: Vec<New>,
    /// `(finding, entry)` index pairs the baseline accepts, in the findings' order.
    accepted: Vec<(usize, usize)>,
    /// Baseline entries nothing matched, in the baseline's order.
    fixed: Vec<usize>,
}

/// Take from `left` (indices into `baseline`) the entry that best accepts `f`: the first whose
/// measured value `f` does not outgrow, else the first left, which it did. `None` when none is
/// left.
fn take(left: &mut Vec<usize>, f: &Finding, baseline: &[Finding]) -> Option<usize> {
    if left.is_empty() {
        return None;
    }
    let at = left
        .iter()
        .position(|&i| !grew(f, &baseline[i]))
        .unwrap_or(0);
    Some(left.swap_remove(at))
}

/// Split `current` into the findings the baseline accepts and the ones it does not, and return the
/// baseline entries nothing matched. A finding whose rule measures a value is accepted only when
/// its value is no more than the matched entry's, so a baselined finding that grew reads as new.
/// Matching counts: two identical findings need two entries.
fn compare(current: &[Finding], baseline: &[Finding]) -> Matched {
    let mut left: HashMap<(&str, &str, &str, &str), Vec<usize>> = HashMap::new();
    for (i, b) in baseline.iter().enumerate() {
        left.entry(b.key()).or_default().push(i);
    }
    let mut m = Matched {
        new: Vec::new(),
        accepted: Vec::new(),
        fixed: Vec::new(),
    };
    for (i, f) in current.iter().enumerate() {
        match left.get_mut(&f.key()).and_then(|l| take(l, f, baseline)) {
            Some(bi) if grew(f, &baseline[bi]) => m.new.push(New {
                finding: f.clone(),
                grew: values(f, &baseline[bi]).map(|(now, was)| (was, now)),
            }),
            Some(bi) => m.accepted.push((i, bi)),
            None => m.new.push(New {
                finding: f.clone(),
                grew: None,
            }),
        }
    }
    m.fixed = left.into_values().flatten().collect();
    m.fixed.sort_unstable();
    m
}

/// The baseline's entries the run keeps, in order: the fixed ones dropped, and each entry whose
/// measured value shrank rewritten from the finding that matched it.
fn kept(baseline: &[Finding], m: &Matched, current: &[Finding]) -> Vec<Entry> {
    let mut gone = vec![false; baseline.len()];
    for &i in &m.fixed {
        gone[i] = true;
    }
    let mut lowered: HashMap<usize, usize> = HashMap::new();
    for &(ci, bi) in &m.accepted {
        if shrank(&current[ci], &baseline[bi]) {
            lowered.insert(bi, ci);
        }
    }
    baseline
        .iter()
        .enumerate()
        .filter(|(i, _)| !gone[*i])
        .map(|(i, b)| Entry::from(lowered.get(&i).map_or(b, |&ci| &current[ci])))
        .collect()
}

/// What `ns rules check --prune` would write: the kept entries, how many fixed entries it drops
/// and how many values it lowers, or `None` when it changes nothing, or `--only` skipped commands
/// whose entries would read as fixed.
fn planned(
    baseline: &[Finding],
    m: &Matched,
    current: &[Finding],
    only: bool,
) -> Option<(Vec<Entry>, usize, usize)> {
    if only {
        return None;
    }
    let dropped = m.fixed.len();
    let lowered = m
        .accepted
        .iter()
        .filter(|&&(ci, bi)| shrank(&current[ci], &baseline[bi]))
        .count();
    if dropped == 0 && lowered == 0 {
        return None;
    }
    Some((kept(baseline, m, current), dropped, lowered))
}

/// Write the pruned baseline when `check --prune` asks for it and nothing is new, and report on
/// stderr what the run leaves: the fixed entries, and the values it lowered. Returns how many
/// entries it dropped and how many values it lowered, 0 each when it wrote nothing.
fn finish(
    root: &Path,
    prune: bool,
    only: bool,
    baseline: &[Finding],
    m: &Matched,
    current: &[Finding],
    fixed: &[Finding],
) -> Result<(usize, usize)> {
    let plan = if prune && m.new.is_empty() {
        planned(baseline, m, current, only)
    } else {
        None
    };
    let (pruned, lowered) = match plan {
        Some((entries, dropped, lowered)) => {
            write_baseline(&root.join(BASELINE), &baseline_text(entries)?)?;
            (dropped, lowered)
        }
        None => (0, 0),
    };
    match (pruned, lowered) {
        (0, 0) if !fixed.is_empty() => eprintln!(
            "ns rules: {} baselined violations are gone; drop them with:\n  ns rules baseline",
            fixed.len()
        ),
        (0, 0) => {}
        (p, 0) => eprintln!("ns rules: dropped {p} fixed entries from {BASELINE}"),
        (0, l) => eprintln!("ns rules: lowered {l} values in {BASELINE}"),
        (p, l) => {
            eprintln!("ns rules: dropped {p} fixed entries and lowered {l} values in {BASELINE}")
        }
    }
    Ok((pruned, lowered))
}

/// Whether the baseline text `head` only removes entries from `base`: each of its entries is
/// matched by its own entry in `base`, counting duplicates, on the fields a finding matches on, so
/// a reordered file is too, and so is one whose messages `ns rules baseline` refreshed. A matched
/// entry whose measured value grew is not removal-only (ADR-746), so it keeps its human-review
/// hit. Either text not a baseline is an error naming it.
pub fn removal_only(base: &str, head: &str) -> Result<bool, String> {
    let parse = |text: &str, which: &str| {
        serde_json::from_str::<Baseline>(text)
            .map(|b| {
                b.violations
                    .into_iter()
                    .map(Finding::from)
                    .collect::<Vec<_>>()
            })
            .map_err(|e| format!("the {which}'s baseline is not a rules baseline: {e}"))
    };
    let (base, head) = (parse(base, "base")?, parse(head, "PR")?);
    let mut left: HashMap<(&str, &str, &str, &str), Vec<usize>> = HashMap::new();
    for (i, e) in base.iter().enumerate() {
        left.entry(e.key()).or_default().push(i);
    }
    Ok(head.iter().all(|f| {
        left.get_mut(&f.key())
            .and_then(|l| take(l, f, &base))
            .is_some_and(|bi| !grew(f, &base[bi]))
    }))
}

/// How many of the `accepted` findings carry a message their baseline entry lacks: those left
/// once each finding an entry equals in full is paired with it.
fn refreshed(current: &[Finding], baseline: &[Finding], accepted: &[(usize, usize)]) -> usize {
    let mut left: HashMap<Entry, usize> = HashMap::new();
    for b in baseline {
        *left.entry(Entry::from(b)).or_default() += 1;
    }
    let same = accepted
        .iter()
        .filter(|&&(ci, _)| match left.get_mut(&Entry::from(&current[ci])) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        })
        .count();
    accepted.len() - same
}

/// The baseline file's text for `violations`.
fn baseline_text(violations: Vec<Entry>) -> Result<String> {
    let text = serde_json::to_string_pretty(&Baseline {
        version: 1,
        violations,
    })?;
    Ok(text + "\n")
}

/// Write `text` to the baseline at `path`, through a temporary file.
fn write_baseline(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .with_context(|| format!("cannot create the directory for {}", path.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))
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
    Ok(Some(b.violations.into_iter().map(Finding::from).collect()))
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

fn print(v: &Value, human: &[(&str, Vec<String>)], summary: &str, format: Format) -> Result<()> {
    match format {
        Format::Human => {
            for (label, lines) in human {
                for l in lines {
                    println!("{label}: {l}");
                }
            }
            println!("{summary}");
        }
        Format::Json => println!("{}", serde_json::to_string_pretty(v)?),
    }
    Ok(())
}

/// `prune` also drops the fixed entries from the baseline file, and lowers the value it stores for
/// an entry that shrank, when nothing is new and every command ran, so the file only tightens.
pub fn check(only: Option<&str>, prune: bool, format: Format) -> Result<ExitCode> {
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
            "pruned": 0,
        });
        print(&v, &[], "0 findings, 0 baselined, 0 new, 0 fixed", format)?;
        return Ok(ExitCode::SUCCESS);
    }
    let run = run_pack(&c.root, &c.commands)?;
    let baseline = load_baseline(&c.root.join(BASELINE))?.unwrap_or_default();
    let m = compare(&run.findings, &baseline);
    let mut fixed: Vec<Finding> = m.fixed.iter().map(|&i| baseline[i].clone()).collect();
    if only.is_some() {
        fixed.clear();
    }
    let (pruned, lowered) = finish(
        &c.root,
        prune,
        only.is_some(),
        &baseline,
        &m,
        &run.findings,
        &fixed,
    )?;
    let new: Vec<&Finding> = m.new.iter().map(|n| &n.finding).collect();
    let human = [
        ("new", m.new.iter().map(|n| n.line()).collect()),
        ("fixed", fixed.iter().map(line).collect()),
    ];
    let v = json!({
        "ok": m.new.is_empty(),
        "command": "rules check",
        "baseline": BASELINE,
        "commands": run.commands,
        "findings": run.findings.len(),
        "baselined": m.accepted.len(),
        "new": new,
        "fixed": fixed,
        "pruned": pruned,
        "lowered": lowered,
    });
    let summary = format!(
        "{} findings, {} baselined, {} new, {} fixed",
        run.findings.len(),
        m.accepted.len(),
        m.new.len(),
        fixed.len()
    );
    print(&v, &human, &summary, format)?;
    if !m.new.is_empty() {
        eprintln!(
            "ns rules: {} new violations; fix them, then rerun:\n  ns rules check",
            m.new.len()
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
    let old = load_baseline(&path)?.unwrap_or_default();
    let m = compare(&run.findings, &old);
    let added: Vec<Finding> = m.new.iter().map(|n| n.finding.clone()).collect();
    let dropped: Vec<Finding> = m.fixed.iter().map(|&i| old[i].clone()).collect();
    let refreshed = refreshed(&run.findings, &old, &m.accepted);
    // In `run.findings`' order (tool, rule, file, then line, then message), so a dropped finding
    // removes only its own entry from the file, and duplicates refresh the same way every run.
    let text = baseline_text(run.findings.iter().map(Entry::from).collect())?;
    let refused = !added.is_empty() && !allow_growth;
    let status = if refused {
        "refused"
    } else if fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        "already-done"
    } else if dry_run {
        "planned"
    } else {
        write_baseline(&path, &text)?;
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
        "refreshed": refreshed,
    });
    let mut summary = format!(
        "{status}: {} violations, {} added, {} dropped",
        run.findings.len(),
        added.len(),
        dropped.len()
    );
    if refreshed > 0 {
        summary += &format!(", {refreshed} refreshed");
    }
    print(
        &v,
        &[
            ("added", added.iter().map(line).collect()),
            ("dropped", dropped.iter().map(line).collect()),
        ],
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
mod tests;
