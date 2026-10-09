//! `ns run`: drive one unit through the phases (docs/FACTORY.md).

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::billing;
use crate::clock::{self, Clock};
use crate::config::{self, Config};
use crate::error::SfError;
use crate::eval::parser::{ClaudeStreamJson, OutputParser};
use crate::eval::trial::run_process;
use crate::factory::{self, artifact_of, Factory, PhaseSettings, PHASES};
use crate::forge;
use crate::frontmatter;
use crate::git::{self, Repo};
use crate::worktree;

pub const EXIT_STUCK: u8 = 1;
pub const EXIT_BUDGET: u8 = 3;
pub const EXIT_PAUSED: u8 = 4;
pub const EXIT_LOCKED: i32 = 5;

/// The built-in claude write command for `ns run`. The phases need Bash, which `acceptEdits`
/// can't grant headless, so permissions are bypassed; cwd is always the unit's worktree.
const RUN_CLAUDE: &[&str] = &[
    "claude",
    "-p",
    "--permission-mode",
    "bypassPermissions",
    "--model",
    "{model}",
    "--output-format",
    "stream-json",
    "--verbose",
];

#[derive(Debug, Clone, Default)]
pub struct RunArgs {
    pub unit: Option<String>,
    pub issue: Option<u64>,
    pub from: Option<String>,
    pub gates: Option<String>,
    pub dry_run: bool,
    pub factory: Option<PathBuf>,
    pub base: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Merged,
    Stuck,
    Budget,
    Paused,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::Merged => "merged",
            Outcome::Stuck => "stuck",
            Outcome::Budget => "budget",
            Outcome::Paused => "paused",
        }
    }

    fn exit(self) -> ExitCode {
        match self {
            Outcome::Done | Outcome::Merged => ExitCode::SUCCESS,
            Outcome::Stuck => ExitCode::from(EXIT_STUCK),
            Outcome::Budget => ExitCode::from(EXIT_BUDGET),
            Outcome::Paused => ExitCode::from(EXIT_PAUSED),
        }
    }
}

pub struct RunResult {
    pub unit: String,
    pub outcome: Outcome,
    pub reason: String,
    /// A `done` that still needs a human (files that need human review, no CI, ...).
    pub needs_human: bool,
    pub reset_at: Option<i64>,
    pub artifact: Option<String>,
    pub cost_usd: f64,
    pub json: Value,
}

/// State shared across the units of one `ns watch` (or the single unit of `ns run`).
pub struct Shared {
    pub spent_usd: f64,
    pub clock: Clock,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            spent_usd: 0.0,
            clock: Clock::from_env(),
        }
    }
}

pub fn cli(args: RunArgs) -> Result<ExitCode> {
    let mut shared = Shared::new();
    let dry = args.dry_run;
    let r = execute(&args, &mut shared)?;
    println!("{}", serde_json::to_string_pretty(&r.json)?);
    Ok(if dry {
        ExitCode::SUCCESS
    } else {
        r.outcome.exit()
    })
}

// ---------------------------------------------------------------- artifacts and decisions

#[derive(Debug, Clone)]
pub struct Art {
    pub status: String,
    pub sha: Option<String>,
    pub pr: Option<String>,
    pub body: String,
}

fn read_art(path: &Path) -> Option<Art> {
    let text = fs::read_to_string(path).ok()?;
    let (status, sha, pr, body) = match (frontmatter::parse(&text), frontmatter::split(&text)) {
        (Ok(Some(_)), Some((yaml, body))) => (
            frontmatter::raw_field(yaml, "status").unwrap_or_default(),
            frontmatter::raw_field(yaml, "sha"),
            frontmatter::raw_field(yaml, "pr"),
            body.to_string(),
        ),
        _ => (String::new(), None, None, text.clone()),
    };
    Some(Art {
        status,
        sha,
        pr,
        body,
    })
}

/// Move `file` into `<dir>/history/<stem>-<n>.md`, n one past the highest already there.
fn archive_file(dir: &Path, file: &str, moves: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
    let src = dir.join(file);
    if !src.exists() {
        return Ok(());
    }
    let hist = dir.join("history");
    fs::create_dir_all(&hist).with_context(|| format!("cannot create {}", hist.display()))?;
    let stem = file.trim_end_matches(".md");
    let prefix = format!("{stem}-");
    let next = fs::read_dir(&hist)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix(&prefix)?
                .strip_suffix(".md")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        + 1;
    let dst = hist.join(format!("{stem}-{next}.md"));
    fs::rename(&src, &dst)
        .with_context(|| format!("cannot move {} to {}", src.display(), dst.display()))?;
    moves.push((src, dst));
    Ok(())
}

/// Undo `archive_for` after an attempt that wrote nothing, so the unit's state is as before.
fn restore(moves: &[(PathBuf, PathBuf)]) {
    for (src, dst) in moves.iter().rev() {
        if !src.exists() {
            let _ = fs::rename(dst, src);
        }
    }
}

/// Before running `phase`: archive its artifact, and every downstream artifact that is not
/// `pass` at HEAD. Whatever exists afterwards is current, so presence alone drives `decide`.
pub fn archive_for(dir: &Path, phase: &str, head: &str) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut moves = Vec::new();
    let Some(i) = PHASES.iter().position(|p| *p == phase) else {
        return Ok(moves);
    };
    archive_file(dir, artifact_of(phase), &mut moves)?;
    for p in &PHASES[i + 1..] {
        let file = artifact_of(p);
        let keep = read_art(&dir.join(file))
            .is_some_and(|a| a.status == "pass" && same_sha(a.sha.as_deref().unwrap_or(""), head));
        if !keep {
            archive_file(dir, file, &mut moves)?;
        }
    }
    Ok(moves)
}

/// The unit's PR number: from `pr.md`, else the newest archived `pr-<n>.md` that names one.
pub fn known_pr(dir: &Path) -> Option<u64> {
    if let Some(n) = read_art(&dir.join("pr.md"))
        .and_then(|a| a.pr)
        .and_then(|v| pr_number(&v))
    {
        return Some(n);
    }
    let mut archived: Vec<(u32, PathBuf)> = fs::read_dir(dir.join("history"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name
                .strip_prefix("pr-")?
                .strip_suffix(".md")?
                .parse()
                .ok()?;
            Some((n, e.path()))
        })
        .collect();
    archived.sort();
    archived
        .iter()
        .rev()
        .find_map(|(_, p)| read_art(p).and_then(|a| a.pr).and_then(|v| pr_number(&v)))
}

pub struct State {
    pub arts: BTreeMap<&'static str, Art>,
    pub head: String,
    pub has_issue: bool,
}

fn read_state(dir: &Path, head: String, has_issue: bool) -> State {
    let mut arts = BTreeMap::new();
    for p in PHASES {
        if let Some(a) = read_art(&dir.join(artifact_of(p))) {
            arts.insert(*p, a);
        }
    }
    State {
        arts,
        head,
        has_issue,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Run {
        phase: &'static str,
        feedback: String,
        why: String,
    },
    Done,
    Stuck(String),
}

fn run(phase: &'static str, feedback: &str, why: impl Into<String>) -> Decision {
    Decision::Run {
        phase,
        feedback: feedback.to_string(),
        why: why.into(),
    }
}

/// Short shas may differ in length: compare by prefix.
pub fn same_sha(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

/// The body's first non-empty line, the contract's one-sentence reason. A heading marker is
/// dropped and its text kept.
fn first_line(body: &str) -> String {
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim_start())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

/// The phase a failed ship names first: `verify` or `review`.
fn named_phase(body: &str) -> Option<&'static str> {
    let lower = body.to_lowercase();
    let v = lower.find("verify");
    let r = lower.find("review");
    match (v, r) {
        (Some(a), Some(b)) => Some(if a < b { "verify" } else { "review" }),
        (Some(_), None) => Some("verify"),
        (None, Some(_)) => Some("review"),
        (None, None) => None,
    }
}

/// The state table in docs/FACTORY.md.
pub fn decide(s: &State) -> Decision {
    for p in PHASES {
        if let Some(a) = s.arts.get(p) {
            if a.status == "blocked" {
                return Decision::Stuck(format!(
                    "{} is blocked: {}",
                    artifact_of(p),
                    first_line(&a.body)
                ));
            }
        }
    }
    let stale = |a: &Art| !same_sha(a.sha.as_deref().unwrap_or(""), &s.head);
    if let Some(pr) = s.arts.get("ship") {
        if pr.status == "pass" && !stale(pr) {
            return Decision::Done;
        }
    }
    let Some(brief) = s.arts.get("triage") else {
        return if s.has_issue {
            run("triage", "", "no brief.md")
        } else {
            Decision::Stuck("no brief: no brief.md and no issue to triage".into())
        };
    };
    if brief.status != "pass" {
        return Decision::Stuck(format!(
            "triage decided a human or define is needed (brief.md status {:?})",
            brief.status
        ));
    }
    let Some(b) = s.arts.get("build") else {
        return run("build", "", "no build.md");
    };
    match b.status.as_str() {
        "pass" => {}
        "fail" => return run("build", &b.body, "build.md failed; build retries"),
        other => return Decision::Stuck(format!("build.md has status {other:?}")),
    }
    let stale_why = |file: &str, a: &Art| {
        format!(
            "{file} sha {} is not HEAD {}",
            a.sha.as_deref().unwrap_or("(none)"),
            s.head
        )
    };
    for (phase, file) in [("verify", "evidence.md"), ("review", "review.md")] {
        let Some(a) = s.arts.get(phase) else {
            return run(phase, "", format!("no {file}"));
        };
        if stale(a) {
            return run(phase, "", stale_why(file, a));
        }
        match a.status.as_str() {
            "pass" => {}
            "fail" => return run("build", &a.body, format!("{file} failed")),
            other => return Decision::Stuck(format!("{file} has status {other:?}")),
        }
    }
    let Some(pr) = s.arts.get("ship") else {
        return run("ship", "", "no pr.md");
    };
    match pr.status.as_str() {
        "pass" => run("ship", "", stale_why("pr.md", pr)),
        "fail" => match named_phase(&pr.body) {
            Some(p) => run(
                if p == "verify" { "verify" } else { "review" },
                &pr.body,
                format!("pr.md failed and names {p}"),
            ),
            None => Decision::Stuck(format!("ship failed: {}", first_line(&pr.body))),
        },
        other => Decision::Stuck(format!("pr.md has status {other:?}")),
    }
}

// ---------------------------------------------------------------- harness

pub fn phase_command(
    cfg: Option<&Config>,
    p: &PhaseSettings,
    subscription: bool,
) -> Result<Vec<String>> {
    let owned = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let configured = cfg.and_then(|c| c.harness.get(&p.harness));
    let argv = match configured {
        Some(h) => h.command_write.clone(),
        None if p.harness == "claude" => Some(owned(RUN_CLAUDE)),
        None => config::builtin_harness(&p.harness).and_then(|h| h.command_write),
    };
    let Some(argv) = argv.filter(|a| !a.is_empty()) else {
        return Err(SfError::usage(
            format!(
                "phase {}: harness {:?} has no command_write",
                p.name, p.harness
            ),
            format!(
                "add one to {}:\n  [harness.{}]\n  command = [...]\n  command_write = [...]\nor set the phase's harness to claude or codex",
                config::path().display(),
                p.harness
            ),
        )
        .into());
    };
    let mut argv = config::expand(&argv, p.model.as_deref());
    if billing::is_claude(&argv) {
        match argv.iter().position(|a| a == "--output-format") {
            Some(i) if i + 1 < argv.len() => argv[i + 1] = "stream-json".into(),
            Some(_) => argv.push("stream-json".into()),
            None => argv.extend(["--output-format".into(), "stream-json".into()]),
        }
        if !argv.iter().any(|a| a == "--verbose") {
            argv.push("--verbose".into());
        }
        if subscription {
            argv = billing::strip_bare(argv);
        }
    }
    Ok(argv)
}

struct PhaseRun {
    exit: Option<i32>,
    timed_out: bool,
    wall_s: f64,
    stdout: String,
}

fn run_harness(
    argv: &[String],
    prompt: &str,
    cwd: &Path,
    env: &[(&str, String)],
    timeout: Duration,
    transcript: &Path,
    subscription: bool,
) -> Result<PhaseRun> {
    if let Some(d) = transcript.parent() {
        fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    let out = File::create(transcript)
        .with_context(|| format!("cannot create {}", transcript.display()))?;
    let err = File::create(transcript.with_extension("stderr"))?;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdout(out)
        .stderr(err);
    for (k, v) in env {
        cmd.env(k, v);
    }
    if subscription && billing::is_claude(argv) {
        billing::scrub(&mut cmd);
    }
    let (status, timed_out, wall_s) =
        match run_process(cmd, Some(prompt.as_bytes().to_vec()), timeout) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SfError::usage(
                    format!("harness binary `{}` is not on PATH", argv[0]),
                    "install it, or set [defaults] harness in nightshift.toml; check with:\n  ns factory validate",
                )
                .into())
            }
            Err(e) => return Err(e).with_context(|| format!("cannot start {}", argv[0])),
        };
    Ok(PhaseRun {
        exit: status.and_then(|s| s.code()),
        timed_out,
        wall_s,
        stdout: fs::read_to_string(transcript).unwrap_or_default(),
    })
}

// ---------------------------------------------------------------- lock

pub struct Lock {
    path: PathBuf,
}

fn pid_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

impl Lock {
    pub fn acquire(common: &Path, unit: &str) -> Result<Lock> {
        let path = common.join("ns-run.lock");
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    writeln!(f, "{}", json!({"pid": std::process::id(), "unit": unit}))?;
                    return Ok(Lock { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let text = fs::read_to_string(&path).unwrap_or_default();
                    let held: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
                    let pid = held.get("pid").and_then(Value::as_i64).unwrap_or(0) as i32;
                    if pid > 0 && pid_alive(pid) {
                        return Err(SfError::new(
                            EXIT_LOCKED,
                            format!(
                                "another ns run (pid {pid}, unit {}) holds {}",
                                held.get("unit").and_then(Value::as_str).unwrap_or("?"),
                                path.display()
                            ),
                        )
                        .hint("one unit at a time per repo; wait for it to finish, or stop it")
                        .into());
                    }
                    let _ = fs::remove_file(&path);
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("cannot create {}", path.display()))
                }
            }
        }
        Err(SfError::general(format!("cannot take {}", path.display())).into())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let mine = fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(t.trim()).ok())
            .and_then(|v| v.get("pid").and_then(Value::as_u64))
            == Some(u64::from(std::process::id()));
        if mine {
            let _ = fs::remove_file(&self.path);
        }
    }
}

// ---------------------------------------------------------------- gh and git helpers

pub fn gh(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("gh")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .context("cannot run gh; is it installed and on PATH?")?;
    if !out.status.success() {
        return Err(SfError::general(format!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn gh_json(cwd: &Path, args: &[&str]) -> Result<Value> {
    let text = gh(cwd, args)?;
    serde_json::from_str(text.trim())
        .with_context(|| format!("gh {} gave bad JSON", args.join(" ")))
}

/// `origin`'s default branch and its sha, or `None` without an origin remote.
pub fn remote_default(dir: &Path) -> Option<(String, String)> {
    git::run(dir, &["remote", "get-url", "origin"]).ok()?;
    let text = git::run(dir, &["ls-remote", "--symref", "origin", "HEAD"]).ok()?;
    let mut branch = None;
    let mut sha = None;
    for line in text.lines() {
        if let Some(r) = line.strip_prefix("ref: refs/heads/") {
            branch = r.split_whitespace().next().map(String::from);
        } else if line.ends_with("\tHEAD") {
            sha = line.split_whitespace().next().map(String::from);
        }
    }
    Some((branch?, sha?))
}

fn remote_sha(dir: &Path, branch: &str) -> Option<String> {
    let text = git::run(
        dir,
        &["ls-remote", "origin", &format!("refs/heads/{branch}")],
    )
    .ok()?;
    text.split_whitespace().next().map(String::from)
}

/// A PR number from `pr:` (`12`, `#12` or a URL ending in the number).
pub fn pr_number(v: &str) -> Option<u64> {
    let digits: String = v
        .trim()
        .trim_end_matches('/')
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.chars().rev().collect::<String>().parse().ok()
}

pub fn slug(title: &str) -> String {
    let mut s = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let mut s = s.trim_matches('-').to_string();
    if s.len() > 40 {
        s.truncate(40);
        if let Some(i) = s.rfind('-') {
            s.truncate(i);
        }
    }
    s
}

// ---------------------------------------------------------------- the run

struct Ctx<'a> {
    unit: String,
    worktree: PathBuf,
    artifacts: PathBuf,
    common: PathBuf,
    fac: &'a Factory,
    root: PathBuf,
    issue: Option<u64>,
    issue_url: String,
    gates: String,
}

impl Ctx<'_> {
    fn prompt(&self, p: &PhaseSettings, attempt: u32, feedback: &str) -> Result<String> {
        let issue = self.issue.map(|n| n.to_string()).unwrap_or_default();
        let wt = self.worktree.to_string_lossy();
        factory::prompt(
            &self.root,
            &factory::PromptVars {
                skill: &p.skill,
                unit: &self.unit,
                issue: &issue,
                issue_url: &self.issue_url,
                worktree: &wt,
                gates: &self.gates,
                phase: &p.name,
                attempt,
                feedback,
            },
        )
    }

    fn log(&self, mut ev: Value) {
        if let Some(o) = ev.as_object_mut() {
            o.insert("ts".into(), json!(clock::iso(Clock::from_env().now())));
            o.insert("unit".into(), json!(self.unit));
            o.insert("pid".into(), json!(std::process::id()));
        }
        let dir = self.common.join("ns");
        let _ = fs::create_dir_all(&dir);
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("runs.jsonl"))
        {
            let _ = writeln!(f, "{ev}");
        }
    }

    fn head(&self) -> String {
        git::run(&self.worktree, &["rev-parse", "--short", "HEAD"]).unwrap_or_default()
    }
}

struct Finish {
    outcome: Outcome,
    reason: String,
    phase: Option<String>,
    needs_human: bool,
    reset_at: Option<i64>,
}

fn finish(outcome: Outcome, reason: impl Into<String>, phase: Option<&str>) -> Finish {
    Finish {
        outcome,
        reason: reason.into(),
        phase: phase.map(String::from),
        needs_human: false,
        reset_at: None,
    }
}

/// Load the user config and export its forge tokens, so every child process inherits them.
pub fn load_config_and_export_forge() -> Result<Option<Config>> {
    let cfg = config::load(&config::path())
        .map_err(|e| SfError::usage(format!("{e:#}"), "check it with:\n  ns doctor"))?;
    if let Some(c) = &cfg {
        forge::export(&c.forge)?;
    }
    Ok(cfg)
}

pub fn execute(args: &RunArgs, shared: &mut Shared) -> Result<RunResult> {
    let start = std::env::current_dir().context("cannot read current directory")?;
    let repo = Repo::discover(&start)?;
    let root = factory::root(args.factory.as_deref(), &repo.root);
    let fac = factory::load(&root)?;
    let cfg = load_config_and_export_forge()?;
    let problems = fac.problems(cfg.as_ref());
    if !problems.is_empty() {
        return Err(SfError::usage(
            format!(
                "the factory definition has problems: {}",
                problems.join("; ")
            ),
            "check it with:\n  ns factory validate",
        )
        .into());
    }
    if let Some(f) = &args.from {
        if !PHASES.contains(&f.as_str()) {
            return Err(SfError::usage(
                format!("--from {f:?} is not a phase"),
                format!(
                    "phases: {}\n  ns run 142-uart-timeout --from verify",
                    PHASES.join(", ")
                ),
            )
            .into());
        }
    }
    let gates = args.gates.clone().unwrap_or_else(|| fac.gates.clone());
    if !["stop", "auto"].contains(&gates.as_str()) {
        return Err(SfError::usage(
            format!("--gates {gates:?}: use stop or auto"),
            "ns run 142-uart-timeout --gates auto",
        )
        .into());
    }
    let subscription = fac.subscription();
    let mut commands = BTreeMap::new();
    for p in PHASES {
        commands.insert(
            *p,
            phase_command(cfg.as_ref(), &fac.phase(p), subscription)?,
        );
    }
    if subscription && !args.dry_run && commands.values().any(|c| billing::is_claude(c)) {
        billing::check_login()?;
    }

    // Unit and issue.
    let mut issue_url = String::from("none");
    let mut unit = args.unit.clone();
    if let Some(n) = args.issue {
        match gh_json(
            &repo.root,
            &["issue", "view", &n.to_string(), "--json", "title,url"],
        ) {
            Ok(v) => {
                issue_url = v["url"].as_str().unwrap_or("").to_string();
                if unit.is_none() {
                    let s = slug(v["title"].as_str().unwrap_or(""));
                    unit = Some(if s.is_empty() {
                        n.to_string()
                    } else {
                        format!("{n}-{s}")
                    });
                }
            }
            Err(e) if unit.is_none() => {
                return Err(SfError::usage(
                    format!("cannot read issue #{n} to name the unit: {e:#}"),
                    "pass the unit id yourself:\n  ns run 142-uart-timeout --issue 142",
                )
                .into())
            }
            Err(_) => issue_url = format!("#{n}"),
        }
    }
    let Some(unit) = unit else {
        return Err(SfError::usage(
            "name a unit or an issue",
            "ns run 142-uart-timeout\n  ns run --issue 142",
        )
        .into());
    };
    worktree::check_unit_id(&unit, "ns run 142-uart-timeout --issue 142")?;

    if args.dry_run {
        return dry_run(
            &repo, &fac, &root, &unit, args, &issue_url, &gates, &commands,
        );
    }

    let _lock = Lock::acquire(&repo.common_dir, &unit)?;
    let wt = worktree::ensure(&repo, &unit, args.base.as_deref(), &fac.worktree.setup)?;
    let ctx = Ctx {
        unit: unit.clone(),
        worktree: PathBuf::from(&wt.path),
        artifacts: PathBuf::from(&wt.artifacts),
        common: repo.common_dir.clone(),
        fac: &fac,
        root,
        issue: args.issue,
        issue_url,
        gates,
    };
    let mut phases: Vec<Value> = Vec::new();
    let mut last_artifact: Option<String> = None;
    let result = if let Some(failed) = wt.setup.iter().find(|r| r.exit != Some(0)) {
        finish(
            Outcome::Stuck,
            format!("worktree setup command {:?} failed", failed.run),
            None,
        )
    } else {
        drive(
            &ctx,
            args,
            shared,
            &commands,
            &mut phases,
            &mut last_artifact,
        )?
    };
    let pr = known_pr(&ctx.artifacts);
    ctx.log(json!({
        "event": "end",
        "outcome": result.outcome.label(),
        "reason": result.reason,
        "phase": result.phase,
        "pr": pr,
        "cost_usd": shared.spent_usd,
    }));
    let cost: f64 = phases.iter().filter_map(|p| p["cost_usd"].as_f64()).sum();
    let json = json!({
        "unit": unit,
        "outcome": result.outcome.label(),
        "phase": result.phase,
        "reason": result.reason,
        "pr": pr,
        "cost_usd": cost,
        "reset_at": result.reset_at,
        "artifact": last_artifact,
        "worktree": ctx.worktree.to_string_lossy(),
        "phases": phases,
    });
    Ok(RunResult {
        unit,
        outcome: result.outcome,
        reason: result.reason,
        needs_human: result.needs_human,
        reset_at: result.reset_at,
        artifact: last_artifact,
        cost_usd: cost,
        json,
    })
}

#[allow(clippy::too_many_arguments)]
fn dry_run(
    repo: &Repo,
    fac: &Factory,
    root: &Path,
    unit: &str,
    args: &RunArgs,
    issue_url: &str,
    gates: &str,
    commands: &BTreeMap<&'static str, Vec<String>>,
) -> Result<RunResult> {
    let existing = worktree::existing(repo, unit)?;
    let wt = existing
        .clone()
        .unwrap_or_else(|| worktree::planned_path(repo, unit));
    let head = match &existing {
        Some(p) => git::run(p, &["rev-parse", "--short", "HEAD"]).unwrap_or_default(),
        None => String::new(),
    };
    let state = read_state(&wt.join(".ns").join(unit), head, args.issue.is_some());
    let decision = match &args.from {
        Some(f) => run(
            PHASES
                .iter()
                .find(|p| **p == f.as_str())
                .copied()
                .unwrap_or("build"),
            "",
            "--from",
        ),
        None => decide(&state),
    };
    let ctx = Ctx {
        unit: unit.to_string(),
        worktree: wt.clone(),
        artifacts: wt.join(".ns").join(unit),
        common: repo.common_dir.clone(),
        fac,
        root: root.to_path_buf(),
        issue: args.issue,
        issue_url: issue_url.to_string(),
        gates: gates.to_string(),
    };
    let (decision_json, prompt, command) = match &decision {
        Decision::Run {
            phase,
            feedback,
            why,
        } => {
            let p = fac.phase(phase);
            (
                json!({"action": "run", "phase": phase, "attempt": 1, "why": why}),
                Some(ctx.prompt(&p, 1, feedback)?),
                Some(commands[phase].clone()),
            )
        }
        Decision::Done => (json!({"action": "done"}), None, None),
        Decision::Stuck(r) => (json!({"action": "stuck", "reason": r}), None, None),
    };
    let json = json!({
        "unit": unit,
        "dry_run": true,
        "worktree": wt.to_string_lossy(),
        "worktree_exists": existing.is_some(),
        "decision": decision_json,
        "prompt": prompt,
        "command": command,
        "budget_usd": fac.budget_usd(),
        "billing": fac.defaults.billing,
        "merge_policy": fac.merge.policy,
    });
    Ok(RunResult {
        unit: unit.to_string(),
        outcome: Outcome::Done,
        reason: "dry run".into(),
        needs_human: false,
        reset_at: None,
        artifact: None,
        cost_usd: 0.0,
        json,
    })
}

fn drive(
    ctx: &Ctx<'_>,
    args: &RunArgs,
    shared: &mut Shared,
    commands: &BTreeMap<&'static str, Vec<String>>,
    phases: &mut Vec<Value>,
    last_artifact: &mut Option<String>,
) -> Result<Finish> {
    let fac = ctx.fac;
    let subscription = fac.subscription();
    let mut attempts: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut forced: Option<Decision> = args.from.as_ref().map(|f| {
        run(
            PHASES
                .iter()
                .find(|p| **p == f.as_str())
                .copied()
                .unwrap_or("build"),
            "",
            "--from",
        )
    });
    let mut baseline = remote_default(&ctx.worktree);
    ctx.log(json!({
        "event": "start",
        "default_branch": baseline.as_ref().map(|b| &b.0),
        "default_sha": baseline.as_ref().map(|b| &b.1),
    }));
    loop {
        let state = read_state(&ctx.artifacts, ctx.head(), ctx.issue.is_some());
        let decision = forced.take().unwrap_or_else(|| decide(&state));
        let (phase, feedback, why) = match decision {
            Decision::Stuck(r) => return Ok(finish(Outcome::Stuck, r, None)),
            Decision::Done => {
                if !fac.merge.auto() {
                    return Ok(finish(Outcome::Done, "pr.md passed", Some("ship")));
                }
                match merge_step(ctx, &state, shared)? {
                    MergeStep::Finish(f) => return Ok(f),
                    MergeStep::Rebuild(fb) => {
                        forced = Some(run("build", &fb, "CI failed on the PR"));
                        continue;
                    }
                }
            }
            Decision::Run {
                phase,
                feedback,
                why,
            } => (phase, feedback, why),
        };
        let p = fac.phase(phase);
        let attempt = attempts.get(phase).copied().unwrap_or(0) + 1;
        if attempt > p.max_attempts {
            return Ok(finish(
                Outcome::Stuck,
                format!("{phase} is out of attempts ({})", p.max_attempts),
                Some(phase),
            ));
        }
        if let Some(b) = fac.budget_usd() {
            if shared.spent_usd >= b {
                return Ok(finish(
                    Outcome::Budget,
                    format!("spent ${:.2} of the ${b:.2} budget", shared.spent_usd),
                    Some(phase),
                ));
            }
        }
        let art_path = ctx.artifacts.join(artifact_of(phase));
        let moves = archive_for(&ctx.artifacts, phase, &ctx.head())?;
        let prompt = ctx.prompt(&p, attempt, &feedback)?;
        let transcript = ctx
            .common
            .join("ns")
            .join("transcripts")
            .join(&ctx.unit)
            .join(format!("{phase}-{attempt}.jsonl"));
        let env = [
            ("NS_UNIT", ctx.unit.clone()),
            ("NS_PHASE", phase.to_string()),
            ("NS_ATTEMPT", attempt.to_string()),
            ("NS_WORKTREE", ctx.worktree.to_string_lossy().into_owned()),
        ];
        eprintln!("ns run: {} {phase} attempt {attempt} ({why})", ctx.unit);
        let r = run_harness(
            &commands[phase],
            &prompt,
            &ctx.worktree,
            &env,
            Duration::from_secs(p.timeout_minutes * 60),
            &transcript,
            subscription,
        )?;
        let t = ClaudeStreamJson.parse(&r.stdout);
        let cost = t.cost_usd.unwrap_or(0.0);
        shared.spent_usd += cost;
        let mut rec = json!({
            "phase": phase,
            "attempt": attempt,
            "decision": why,
            "exit": r.exit,
            "timed_out": r.timed_out,
            "wall_s": (r.wall_s * 10.0).round() / 10.0,
            "cost_usd": cost,
            "input_tokens": t.input_tokens,
            "output_tokens": t.output_tokens,
            "transcript": transcript.to_string_lossy(),
        });
        if let Some(limit) = billing::usage_limit(&r.stdout, shared.clock.now()) {
            rec["status"] = json!("paused");
            rec["reason"] = json!(limit.message);
            restore(&moves);
            ctx.log(merge_obj(json!({"event": "phase"}), &rec));
            phases.push(rec);
            let mut f = finish(
                Outcome::Paused,
                format!("usage limit: {}", limit.message),
                Some(phase),
            );
            f.reset_at = limit.reset_at;
            return Ok(f);
        }
        attempts.insert(phase, attempt);
        let art = read_art(&art_path);
        let written = art.is_some();
        if !written {
            restore(&moves);
        }
        rec["written"] = json!(written);
        rec["status"] = json!(if written {
            art.as_ref().map(|a| a.status.clone())
        } else {
            None
        });
        rec["sha"] = json!(ctx.head());
        if written {
            *last_artifact = Some(art_path.to_string_lossy().into_owned());
        } else {
            let why = if r.timed_out {
                format!("timed out after {} min", p.timeout_minutes)
            } else {
                match r.exit {
                    Some(0) => "no artifact written".to_string(),
                    Some(c) => format!("exited {c}, no artifact written"),
                    None => "killed by a signal".to_string(),
                }
            };
            rec["reason"] = json!(why);
        }
        ctx.log(merge_obj(json!({"event": "phase"}), &rec));
        phases.push(rec);

        if let Some(f) = guards(ctx, &mut baseline, phase)? {
            return Ok(f);
        }
        if phase == "triage" && !written && r.exit == Some(0) && !r.timed_out && !art_path.exists()
        {
            return Ok(finish(
                Outcome::Stuck,
                "triage wrote no brief: the issue needs a human or ns-define",
                Some("triage"),
            ));
        }
    }
}

fn merge_obj(mut a: Value, b: &Value) -> Value {
    if let (Some(x), Some(y)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in y {
            x.insert(k.clone(), v.clone());
        }
    }
    a
}

/// Hard rules after every phase: the default branch didn't move because of this run, and no PR
/// was merged by a phase.
fn guards(
    ctx: &Ctx<'_>,
    baseline: &mut Option<(String, String)>,
    phase: &str,
) -> Result<Option<Finish>> {
    if let Some((branch, sha)) = baseline.clone() {
        if let Some(now) = remote_sha(&ctx.worktree, &branch) {
            if now != sha {
                let ours = git::ok(
                    &ctx.worktree,
                    &["merge-base", "--is-ancestor", &now, "HEAD"],
                );
                if ours {
                    ctx.log(json!({"event": "breach", "rule": "default branch moved", "phase": phase, "from": sha, "to": now}));
                    return Ok(Some(finish(
                        Outcome::Stuck,
                        format!("default branch moved: {branch} went from {sha} to {now}, a commit of this unit"),
                        Some(phase),
                    )));
                }
                ctx.log(json!({"event": "default_moved_by_other_actor", "phase": phase, "from": sha, "to": now}));
                *baseline = Some((branch, now));
            }
        }
    }
    if let Some(n) = known_pr(&ctx.artifacts) {
        let v = gh_json(
            &ctx.worktree,
            &["pr", "view", &n.to_string(), "--json", "state"],
        )?;
        if v["state"].as_str() == Some("MERGED") {
            ctx.log(
                json!({"event": "breach", "rule": "PR merged by run", "phase": phase, "pr": n}),
            );
            return Ok(Some(finish(
                Outcome::Stuck,
                format!("PR merged by run: #{n} was merged during the {phase} phase, outside ns run's merge step"),
                Some(phase),
            )));
        }
    }
    Ok(None)
}

enum MergeStep {
    Finish(Finish),
    Rebuild(String),
}

fn human(reason: impl Into<String>) -> MergeStep {
    let mut f = finish(Outcome::Done, reason, Some("merge"));
    f.needs_human = true;
    MergeStep::Finish(f)
}

/// `merge.policy = "auto"`: squash-merge this unit's PR when CI is green, review.md passed at
/// HEAD, and no file that needs human review changed. The only place ns merges anything.
fn merge_step(ctx: &Ctx<'_>, state: &State, shared: &Shared) -> Result<MergeStep> {
    if !state.arts.contains_key("ship") {
        return Ok(human("no pr.md"));
    }
    let Some(n) = known_pr(&ctx.artifacts) else {
        return Ok(human("pr.md has no pr: number; needs a human merge"));
    };
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let view = gh_json(
        wt,
        &[
            "pr",
            "view",
            &ns,
            "--json",
            "state,headRefOid,mergeStateStatus",
        ],
    )?;
    match view["state"].as_str() {
        Some("OPEN") => {}
        Some("MERGED") => {
            return Ok(MergeStep::Finish(finish(
                Outcome::Stuck,
                format!("PR merged by run: #{n} was merged before ns run's merge step"),
                Some("merge"),
            )))
        }
        other => {
            return Ok(MergeStep::Finish(finish(
                Outcome::Stuck,
                format!("PR #{n} is {}", other.unwrap_or("unknown")),
                Some("merge"),
            )))
        }
    }
    let head = git::run(wt, &["rev-parse", "HEAD"]).unwrap_or_default();
    let pr_head = view["headRefOid"].as_str().unwrap_or("");
    if !same_sha(pr_head, &head) {
        return Ok(human(format!(
            "PR head {pr_head} is not the reviewed HEAD {head}; needs a human merge"
        )));
    }
    match state.arts.get("review") {
        Some(r) if r.status == "pass" && same_sha(r.sha.as_deref().unwrap_or(""), &head) => {}
        _ => return Ok(human("review.md is not pass at HEAD; needs a human merge")),
    }

    // Strict required checks: a PR behind its base gets the base merged in, then fresh checks.
    let default = remote_default(wt)
        .map(|d| d.0)
        .unwrap_or_else(|| "main".into());
    let conflict = || {
        ctx.log(json!({"event": "conflict", "pr": n}));
        MergeStep::Rebuild(format!(
            "PR #{n} conflicts with {default}: rebase onto {default} and resolve conflicts, then force-push with --force-with-lease"
        ))
    };
    let mut merge_head = head.clone();
    match view["mergeStateStatus"].as_str() {
        Some("DIRTY") => return Ok(conflict()),
        Some("BEHIND") => {
            if gh(wt, &["pr", "update-branch", &ns]).is_err() {
                return Ok(conflict());
            }
            let v = gh_json(
                wt,
                &["pr", "view", &ns, "--json", "headRefOid,mergeStateStatus"],
            )?;
            if v["mergeStateStatus"].as_str() == Some("DIRTY") {
                return Ok(conflict());
            }
            ctx.log(json!({"event": "update_branch", "pr": n}));
            merge_head = v["headRefOid"].as_str().unwrap_or(&head).to_string();
        }
        _ => {}
    }

    // Wait for CI, bounded.
    let mut cmd = Command::new("gh");
    cmd.args(["pr", "checks", &ns, "--watch"])
        .current_dir(wt)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mins = ctx.fac.merge.ci_timeout_minutes;
    let (_, timed_out, _) = run_process(cmd, None, Duration::from_secs(mins * 60))
        .context("cannot run gh pr checks")?;
    if timed_out {
        return Ok(MergeStep::Finish(finish(
            Outcome::Stuck,
            format!("CI on PR #{n} did not finish within {mins} min"),
            Some("merge"),
        )));
    }
    let checks = gh_json(
        wt,
        &["pr", "checks", &ns, "--json", "name,state,bucket,link"],
    )
    .unwrap_or(Value::Array(Vec::new()));
    let checks = checks.as_array().cloned().unwrap_or_default();
    if checks.is_empty() {
        return Ok(human(format!(
            "no CI checks reported on PR #{n}; needs a human merge"
        )));
    }
    let failed: Vec<&Value> = checks
        .iter()
        .filter(|c| !matches!(c["bucket"].as_str(), Some("pass") | Some("skipping")))
        .collect();
    if !failed.is_empty() {
        let names: Vec<String> = failed
            .iter()
            .map(|c| {
                format!(
                    "{} ({})",
                    c["name"].as_str().unwrap_or("?"),
                    c["bucket"].as_str().or(c["state"].as_str()).unwrap_or("?")
                )
            })
            .collect();
        let mut fb = format!("CI failed on PR #{n}: {}\n", names.join(", "));
        let run_re = regex::Regex::new(r"/actions/runs/(\d+)").unwrap();
        for c in &failed {
            if let Some(id) = c["link"].as_str().and_then(|l| run_re.captures(l)) {
                if let Ok(log) = gh(wt, &["run", "view", &id[1], "--log-failed"]) {
                    let start = log.len().saturating_sub(3000);
                    let start = (start..log.len())
                        .find(|i| log.is_char_boundary(*i))
                        .unwrap_or(0);
                    fb.push_str(&format!(
                        "\nLog tail ({}):\n{}\n",
                        c["name"].as_str().unwrap_or("?"),
                        &log[start..]
                    ));
                }
            }
        }
        ctx.log(json!({"event": "ci_failed", "pr": n, "checks": names}));
        return Ok(MergeStep::Rebuild(fb));
    }

    let files: Vec<String> = gh(wt, &["pr", "diff", &ns, "--name-only"])?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let hits = ctx.fac.merge.human_review_hits(&files);
    if !hits.is_empty() {
        ctx.log(json!({"event": "human_review", "pr": n, "paths": hits}));
        return Ok(human(format!(
            "changes files that need human review; needs a human merge ({})",
            hits.join(", ")
        )));
    }
    let merged = gh(
        wt,
        &[
            "pr",
            "merge",
            &ns,
            "--squash",
            "--delete-branch",
            "--match-head-commit",
            &merge_head,
        ],
    );
    let state_now = gh_json(wt, &["pr", "view", &ns, "--json", "state"])
        .ok()
        .and_then(|v| v["state"].as_str().map(String::from));
    if state_now.as_deref() == Some("MERGED") {
        ctx.log(json!({"event": "merged", "pr": n, "sha": head, "spent_usd": shared.spent_usd}));
        return Ok(MergeStep::Finish(finish(
            Outcome::Merged,
            format!("squash-merged PR #{n}"),
            Some("merge"),
        )));
    }
    Ok(MergeStep::Finish(finish(
        Outcome::Stuck,
        format!(
            "gh pr merge #{n} did not merge it: {}",
            merged.err().map(|e| format!("{e:#}")).unwrap_or_default()
        ),
        Some("merge"),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(status: &str, sha: &str) -> Art {
        Art {
            status: status.into(),
            sha: Some(sha.into()),
            pr: None,
            body: format!("{status} body"),
        }
    }

    fn state(arts: &[(&'static str, Art)]) -> State {
        State {
            arts: arts.iter().cloned().collect(),
            head: "abc1234".into(),
            has_issue: true,
        }
    }

    fn phase_of(d: &Decision) -> &str {
        match d {
            Decision::Run { phase, .. } => phase,
            Decision::Done => "done",
            Decision::Stuck(_) => "stuck",
        }
    }

    fn next(arts: &[(&'static str, Art)]) -> String {
        phase_of(&decide(&state(arts))).to_string()
    }

    #[test]
    fn stuck_reason_is_the_first_body_line() {
        let blocked = |body: &str| {
            let mut a = art("blocked", "abc1234");
            a.body = body.into();
            match decide(&state(&[("verify", a)])) {
                Decision::Stuck(r) => r,
                _ => panic!("not stuck"),
            }
        };
        assert_eq!(
            blocked("\n  Needs a board on the bench.  \nlater text\n"),
            "evidence.md is blocked: Needs a board on the bench."
        );
        // A heading marker is dropped and its text kept: the first line is the blocker even
        // when it is written as a heading, and later body text is never used.
        assert_eq!(
            blocked("## Needs hardware\n\nlater text\n"),
            "evidence.md is blocked: Needs hardware"
        );
        let long = blocked(&"x".repeat(300));
        assert_eq!(long, format!("evidence.md is blocked: {}", "x".repeat(200)));
    }

    #[test]
    fn state_table() {
        assert_eq!(next(&[]), "triage");
        let mut s = state(&[]);
        s.has_issue = false;
        assert_eq!(phase_of(&decide(&s)), "stuck");
        let brief = ("triage", art("pass", ""));
        let build = ("build", art("pass", "abc1234"));
        let ev = ("verify", art("pass", "abc12"));
        let rv = ("review", art("pass", "abc1234"));
        assert_eq!(next(std::slice::from_ref(&brief)), "build");
        assert_eq!(next(&[brief.clone(), ("build", art("fail", ""))]), "build");
        assert_eq!(next(&[brief.clone(), build.clone()]), "verify");
        // A shorter sha prefix still matches HEAD.
        assert_eq!(next(&[brief.clone(), build.clone(), ev.clone()]), "review");
        let stale = ("verify", art("pass", "fff0000"));
        assert_eq!(next(&[brief.clone(), build.clone(), stale]), "verify");
        let d = decide(&state(&[
            brief.clone(),
            build.clone(),
            ("verify", art("fail", "abc1234")),
        ]));
        assert!(
            matches!(&d, Decision::Run { phase: "build", feedback, .. } if feedback == "fail body")
        );
        let d = decide(&state(&[
            brief.clone(),
            build.clone(),
            ev.clone(),
            ("review", art("fail", "abc1234")),
        ]));
        assert!(matches!(&d, Decision::Run { phase: "build", .. }));
        let base = [brief.clone(), build.clone(), ev.clone(), rv.clone()];
        assert_eq!(next(&base), "ship");
        let with = |extra: (&'static str, Art)| {
            let mut v = base.to_vec();
            v.push(extra);
            next(&v)
        };
        let mut sf = art("fail", "abc1234");
        sf.body = "evidence is stale; ns-review must rerun".into();
        assert_eq!(with(("ship", sf)), "review");
        assert_eq!(with(("ship", art("fail", "abc1234"))), "stuck");
        assert_eq!(with(("ship", art("pass", "abc1234"))), "done");
        // A pass at an older sha ships again once verify and review hold at HEAD.
        assert_eq!(with(("ship", art("pass", "0000000"))), "ship");
        assert_eq!(
            next(&[
                brief.clone(),
                build.clone(),
                ev.clone(),
                ("review", art("blocked", "abc1234"))
            ]),
            "stuck"
        );
        assert_eq!(next(&[("triage", art("fail", ""))]), "stuck");
    }

    #[test]
    fn archiving_keeps_only_current_downstream_passes() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let w = |f: &str, status: &str, sha: &str| {
            fs::write(
                d.join(f),
                format!("---\nstatus: {status}\nsha: {sha}\npr: 12\n---\nbody\n"),
            )
            .unwrap()
        };
        w("build.md", "pass", "abc1234");
        w("evidence.md", "pass", "abc1234");
        w("review.md", "fail", "abc1234");
        w("pr.md", "pass", "0000000");
        let moves = archive_for(d, "build", "abc1234").unwrap();
        assert!(!d.join("build.md").exists());
        assert!(d.join("evidence.md").exists());
        assert!(!d.join("review.md").exists());
        assert!(d.join("history/build-1.md").exists());
        assert!(d.join("history/review-1.md").exists());
        assert!(d.join("history/pr-1.md").exists());
        assert_eq!(known_pr(d), Some(12));
        restore(&moves);
        assert!(d.join("build.md").exists() && d.join("pr.md").exists());
        // A restored slot is free again; a second archive takes the next number.
        archive_for(d, "build", "abc1234").unwrap();
        w("build.md", "pass", "abc1234");
        archive_for(d, "build", "abc1234").unwrap();
        assert!(d.join("history/build-1.md").exists());
        assert!(d.join("history/build-2.md").exists());
    }

    #[test]
    fn sha_and_pr_are_read_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("evidence.md");
        for (sha, want) in [
            ("0e05787", "0e05787"),
            ("1234567", "1234567"),
            ("\"00ab12c\"", "00ab12c"),
            ("3f9c2e1   # HEAD", "3f9c2e1"),
        ] {
            fs::write(
                &p,
                format!("---\nstatus: pass\nsha: {sha}\npr: 0012\n---\nbody\n"),
            )
            .unwrap();
            let a = read_art(&p).unwrap();
            assert_eq!(a.sha.as_deref(), Some(want));
            assert_eq!(a.pr.as_deref(), Some("0012"));
            assert_eq!(a.status, "pass");
        }
    }

    #[test]
    fn helpers() {
        assert!(same_sha("abc1234", "abc1234def"));
        assert!(!same_sha("", "abc"));
        assert!(!same_sha("abc1", "abd1"));
        assert_eq!(pr_number("https://github.com/o/r/pull/12"), Some(12));
        assert_eq!(pr_number("#7"), Some(7));
        assert_eq!(pr_number("<url>"), None);
        assert_eq!(slug("feat(cli): Add `ns run`!"), "feat-cli-add-ns-run");
        assert!(slug(&"word ".repeat(30)).len() <= 40);
    }

    #[test]
    fn claude_command_gets_stream_json_and_no_bare() {
        let cfg = config::parse(
            "[harness.claude]\ncommand = [\"claude\"]\ncommand_write = [\"claude\", \"-p\", \"--bare\", \"--output-format\", \"text\"]\n",
        )
        .unwrap();
        let f = factory::parse("").unwrap();
        let c = phase_command(Some(&cfg), &f.phase("build"), true).unwrap();
        assert_eq!(
            c,
            [
                "claude",
                "-p",
                "--output-format",
                "stream-json",
                "--verbose"
            ]
        );
        let c = phase_command(None, &f.phase("build"), true).unwrap();
        assert!(c.contains(&"bypassPermissions".to_string()));
        assert!(!c.contains(&"--model".to_string()));
    }
}
