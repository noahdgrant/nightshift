//! `ns run`: drive one unit through the phases (docs/FACTORY.md).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::billing;
use crate::clock::{self, Clock};
use crate::config::{self, Config};
use crate::error::SfError;
use crate::eval::parser::{ClaudeStreamJson, OutputParser};
use crate::eval::trial::run_process;
use crate::factory::{self, artifact_of, Factory, LocalGate, PhaseSettings, PHASES};
use crate::forge;
use crate::gate;
use crate::git::same_sha;
use crate::git::{self, Repo};
use crate::markers;
use crate::memcap::{self, Cap};
use crate::pin;
use crate::watch::NightDir;
use crate::worktree;

pub(crate) mod currency;
mod locks;

use currency::{
    archive_as, archive_for, decide, known_pr, read_art, read_state, restore, run, Art, Decision,
    State, UnitDiff,
};
pub use locks::{holder_pid, run_lock, Lock, WORKTREE_LOCK};
use locks::{lock_dir, Acquired, Bound, RunnerLocks, MERGE_LOCK};

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
    /// Run the triage phase once and stop (`ns watch`'s triage pass), whatever `from` says.
    pub triage_only: bool,
    /// The night directory of the `ns watch` that started this run (`--night`).
    pub night: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Merged,
    Stuck,
    Budget,
    Paused,
    /// Triage split the issue into child issues: nothing left for this unit to do.
    Split,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::Merged => "merged",
            Outcome::Stuck => "stuck",
            Outcome::Budget => "budget",
            Outcome::Paused => "paused",
            Outcome::Split => "split",
        }
    }

    fn from_label(label: &str) -> Option<Outcome> {
        [
            Outcome::Done,
            Outcome::Merged,
            Outcome::Stuck,
            Outcome::Budget,
            Outcome::Paused,
            Outcome::Split,
        ]
        .into_iter()
        .find(|o| o.label() == label)
    }

    fn exit(self) -> ExitCode {
        match self {
            Outcome::Done | Outcome::Merged | Outcome::Split => ExitCode::SUCCESS,
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
    /// Paused by the night's hold (`ns run --night`), not by a usage limit of its own.
    pub held: bool,
    pub artifact: Option<String>,
    pub cost_usd: f64,
    /// When the run ended, on its clock.
    pub ended_at: Option<i64>,
    pub json: Value,
}

impl RunResult {
    /// The result an `ns run` printed, or `None` when `json` is no run result.
    pub fn from_json(json: Value) -> Option<RunResult> {
        let outcome = Outcome::from_label(json["outcome"].as_str()?)?;
        let unit = json["unit"]
            .as_str()
            .filter(|u| worktree::valid_unit_id(u))?
            .to_string();
        Some(RunResult {
            unit,
            outcome,
            reason: json["reason"].as_str().unwrap_or("").to_string(),
            needs_human: json["needs_human"].as_bool().unwrap_or(false),
            reset_at: json["reset_at"].as_i64(),
            held: json["held"].as_bool().unwrap_or(false),
            artifact: json["artifact"].as_str().map(String::from),
            cost_usd: json["cost_usd"].as_f64().unwrap_or(0.0),
            ended_at: json["ended_at"].as_i64(),
            json,
        })
    }
}

/// State shared across the units of one `ns watch` (or the single unit of `ns run`).
pub struct Shared {
    /// What this process spent, when no night sums the spend.
    spent_usd: f64,
    pub clock: Clock,
    /// The `ns watch` process running this unit, or `None` for a standalone `ns run`.
    pub watch_pid: Option<u32>,
    /// `ns watch --until`, or `None` for a standalone `ns run`. It bounds a runner lock wait.
    pub until: Option<i64>,
    /// The night of the `ns watch` this runs under: its hold and the spend of all its units.
    pub night: Option<NightDir>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            spent_usd: 0.0,
            clock: Clock::from_env(),
            watch_pid: None,
            until: None,
            night: None,
        }
    }

    /// Dollars spent: by every unit of the night under `ns watch`, else by this process.
    pub fn spent(&self) -> f64 {
        match &self.night {
            Some(n) => n.spent(),
            None => self.spent_usd,
        }
    }

    pub(crate) fn add_spend(&mut self, unit: &str, usd: f64) -> Result<()> {
        match &self.night {
            Some(n) => n.add_spend(unit, usd),
            None => {
                self.spent_usd += usd;
                Ok(())
            }
        }
    }

    /// A usage limit another unit of the night hit, as the finish of a run that must not start
    /// `phase` before it resets.
    fn held(&self, phase: &str) -> Result<Option<Finish>> {
        let Some(n) = &self.night else {
            return Ok(None);
        };
        let now = self.clock.now();
        Ok(n.held_until()?.filter(|r| *r > now).map(|reset| {
            let mut f = finish(
                Outcome::Paused,
                format!(
                    "usage limit: another unit of this ns watch hit it; held until {}",
                    clock::local_iso(reset)
                ),
                Some(phase),
            );
            f.reset_at = Some(reset);
            f.held = true;
            f
        }))
    }
}

pub fn cli(args: RunArgs) -> Result<ExitCode> {
    let mut shared = Shared::new();
    let night = match &args.night {
        Some(dir) => {
            crate::stop::install_for_watch().context("cannot handle SIGINT and SIGTERM")?;
            Some(NightDir::open(dir)?)
        }
        None => None,
    };
    let dry = args.dry_run;
    let mut loaded = Loaded::read(args.factory.as_deref(), args.dry_run)?;
    if let Some((dir, facts)) = night {
        shared.watch_pid = Some(facts.watch_pid);
        shared.until = facts.until;
        shared.night = Some(dir);
        loaded.gate = facts.gate;
    }
    let r = execute(&args, &mut shared, &loaded)?;
    println!("{}", serde_json::to_string_pretty(&r.json)?);
    // Under `ns watch`, a stop that came while the run ended still names the signal.
    if let Some(sig) = crate::stop::requested() {
        return Ok(ExitCode::from((128 + sig) as u8));
    }
    Ok(if dry {
        ExitCode::SUCCESS
    } else {
        r.outcome.exit()
    })
}

// ---------------------------------------------------------------- harness

pub fn phase_command(
    cfg: Option<&Config>,
    p: &PhaseSettings,
    subscription: bool,
    common_dir: &Path,
    worktree: &Path,
    unit: &str,
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
        pin::apply(
            &mut argv,
            &pin::settings_path(common_dir, unit),
            worktree,
            unit,
            &format!("phase {}", p.name),
        )?;
        if subscription {
            argv = billing::strip_bare(argv);
        }
    }
    Ok(argv)
}

/// The worktree `ensure` made is the one the phase commands were rendered for: their
/// `--settings` and `--add-dir` paths sit under it.
fn same_worktree(predicted: &Path, actual: &Path) -> Result<()> {
    if predicted != actual {
        anyhow::bail!(
            "the unit's worktree is {} but its phase commands were built for {}; run again",
            actual.display(),
            predicted.display()
        );
    }
    Ok(())
}

/// A phase's timeout. `NS_PHASE_TIMEOUT_MS` overrides it: `<ms>` for every phase, or
/// comma-separated `<phase>=<ms>` for the named phases only.
fn phase_timeout(phase: &str, minutes: u64) -> Duration {
    let var = std::env::var("NS_PHASE_TIMEOUT_MS").ok();
    timeout_override(var.as_deref(), phase).unwrap_or_else(|| Duration::from_secs(minutes * 60))
}

/// The override for `phase` in an `NS_PHASE_TIMEOUT_MS` value; a named entry beats a bare one.
fn timeout_override(var: Option<&str>, phase: &str) -> Option<Duration> {
    let mut bare = None;
    for part in var?.split(',') {
        match part.split_once('=') {
            Some((p, ms)) if p.trim() == phase => {
                if let Ok(ms) = ms.trim().parse::<u64>() {
                    return Some(Duration::from_millis(ms));
                }
            }
            Some(_) => {}
            None => {
                if bare.is_none() {
                    bare = part.trim().parse::<u64>().ok();
                }
            }
        }
    }
    bare.map(Duration::from_millis)
}

struct PhaseRun {
    exit: Option<i32>,
    timed_out: bool,
    /// Why the run failed at the memory cap or by a signal `ns` didn't send.
    killed: Option<String>,
    wall_s: f64,
    stdout: String,
}

#[allow(clippy::too_many_arguments)]
fn run_harness(
    argv: &[String],
    prompt: &str,
    cwd: &Path,
    env: &[(&str, String)],
    timeout: Duration,
    transcript: &Path,
    subscription: bool,
    cap: Option<&Cap>,
) -> Result<PhaseRun> {
    if let Some(d) = transcript.parent() {
        fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    let out = File::create(transcript)
        .with_context(|| format!("cannot create {}", transcript.display()))?;
    let stderr = transcript.with_extension("stderr");
    let err = File::create(&stderr)?;
    let not_found = || -> anyhow::Error {
        SfError::usage(
            format!("harness binary `{}` is not on PATH", argv[0]),
            "install it, or set [defaults] harness in nightshift.toml; check with:\n  ns factory validate",
        )
        .into()
    };
    // Under a cap the spawned program is systemd-run or prlimit, so a missing harness would
    // be a failed attempt rather than the usage error: look for it first.
    if cap.is_some() {
        let found = if argv[0].contains(std::path::MAIN_SEPARATOR) {
            crate::which::which(&cwd.join(&argv[0]).to_string_lossy())
        } else {
            crate::which::which(&argv[0])
        };
        if found.is_none() {
            return Err(not_found());
        }
    }
    let (mut cmd, watch) = memcap::command(cap, &argv[0]);
    crate::git::scrub(&mut cmd);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdout(out)
        .stderr(err);
    for (k, v) in env {
        cmd.env(k, v);
    }
    if subscription {
        billing::scrub(&mut cmd, argv);
    }
    if billing::is_claude(argv) {
        billing::wait_for_bg_tasks(&mut cmd);
        billing::disable_auto_memory(&mut cmd);
    }
    let (status, timed_out, wall_s) =
        match run_process(cmd, Some(prompt.as_bytes().to_vec()), timeout) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found()),
            Err(e) => return Err(e).with_context(|| format!("cannot start {}", argv[0])),
        };
    let killed = watch
        .cut_short(status, timed_out, || {
            String::from_utf8_lossy(&gate::read_window(&stderr)).into_owned()
        })
        .map(memcap::Cut::reason);
    Ok(PhaseRun {
        exit: status.and_then(|s| s.code()),
        timed_out,
        killed,
        wall_s,
        stdout: fs::read_to_string(transcript).unwrap_or_default(),
    })
}

/// Take the merge lock for `unit`, waiting while another unit merges, each wait logged by
/// `log`. A wait that reaches `until` ends the run as `budget`.
fn take_merge_lock(
    common: &Path,
    unit: &str,
    clock: &Clock,
    until: Option<i64>,
    log: impl Fn(Value),
) -> Result<Result<Lock, Finish>> {
    let me = json!({"pid": std::process::id(), "unit": unit});
    let deadline = until.map(|u| (clock, u));
    let taken = Lock::wait(common, MERGE_LOCK, &me, deadline, |by| {
        eprintln!(
            "ns run: {unit} merge waits for the merge lock (held by pid {}, unit {})",
            holder_pid(by),
            by["unit"].as_str().unwrap_or("?")
        );
        log(json!({"event": "lock_wait", "phase": "merge", "lock": "merge", "held_by": by}));
    })?;
    Ok(taken.map_err(|by| {
        let reason = format!(
            "gave up waiting for the merge lock (held by pid {}, unit {}) at --until",
            holder_pid(&by),
            by["unit"].as_str().unwrap_or("?")
        );
        eprintln!("ns run: {unit} merge {reason}");
        log(json!({
            "event": "lock_wait_timeout",
            "phase": "merge",
            "lock": "merge",
            "held_by": by,
            "bound": Bound::Until.label(),
        }));
        finish(Outcome::Budget, reason, Some("merge"))
    }))
}

/// Why a spend of `spent` dollars is over `budget`, or `None` while it is under it.
fn budget_spent(budget: Option<f64>, spent: f64) -> Option<String> {
    let b = budget.filter(|b| spent >= *b)?;
    Some(format!("spent ${spent:.2} of the ${b:.2} budget"))
}

/// A budget spent while the runner locks were awaited stops the phase before its harness
/// starts, with the locks released.
fn recheck_budget(
    held: RunnerLocks,
    budget: Option<f64>,
    spent: f64,
    phase: &str,
) -> Result<RunnerLocks, Finish> {
    match budget_spent(budget, spent) {
        Some(reason) => Err(finish(Outcome::Budget, reason, Some(phase))),
        None => Ok(held),
    }
}

// ---------------------------------------------------------------- gh and git helpers

pub fn gh(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = crate::git::scrub(&mut Command::new("gh"))
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

/// Append one event to `<common>/ns/runs.jsonl`, stamped with `ts` and `pid`.
pub fn log_event(common: &Path, mut ev: Value) {
    if let Some(o) = ev.as_object_mut() {
        o.insert("ts".into(), json!(clock::iso(Clock::from_env().now())));
        o.insert("pid".into(), json!(std::process::id()));
    }
    let dir = common.join("ns");
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("runs.jsonl"))
    {
        // One write per line, so lines from runs of other units never interleave.
        let _ = f.write_all(format!("{ev}\n").as_bytes());
    }
}

/// `origin`'s default branch and its sha, or `None` without an origin remote or when the name
/// isn't [`ref_safe`].
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
    Some((branch.filter(|b| ref_safe(b))?, sha?))
}

/// Whether a branch name from a remote is safe to hand git as an argument: never an option.
fn ref_safe(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
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

/// The unit for issue `n`: `<n>-<slug of the title>`, or `<n>` for a title with no slug.
pub fn unit_name(n: u64, title: &str) -> String {
    match slug(title) {
        s if s.is_empty() => n.to_string(),
        s => format!("{n}-{s}"),
    }
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

fn resolve_base(repo: &Repo, args: &RunArgs) -> Result<String> {
    match &args.base {
        Some(b) => Ok(b.clone()),
        None => repo.default_base(),
    }
}

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
    base: String,
    lock_dir: PathBuf,
    /// The CI gate commands; `None` runs no gate.
    gate: gate::Commands,
    cap: Option<&'a Cap>,
    /// The night directory of the `ns watch` this runs under.
    night: Option<PathBuf>,
    /// The `ns` binary the phase settings' hook runs.
    ns: PathBuf,
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
            o.insert("unit".into(), json!(self.unit));
        }
        log_event(&self.common, ev);
    }

    fn head(&self) -> String {
        git::run(&self.worktree, &["rev-parse", "--short", "HEAD"]).unwrap_or_default()
    }

    /// Run the gate `cmd` at HEAD after `phase` when `trigger` calls for it. A stop during the
    /// run is an error with the stop set (`crate::stop::requested`).
    fn gate(
        &self,
        gate_state: &mut gate::Gate,
        trigger: gate::Trigger,
        cmd: &str,
        phase: &str,
        attempt: u32,
    ) -> Result<Option<gate::Red>> {
        let job = self.gate_job(gate_state, cmd, &self.worktree, self.head(), phase, attempt);
        gate_state.after(trigger, &job, &|ev| self.log(ev))
    }

    /// The gate `cmd` run in `worktree`, which holds `head`. A fast gate gets the paths that
    /// differ in `worktree` from where the full gate last went green, else from the base.
    fn gate_job<'j>(
        &'j self,
        gate_state: &gate::Gate,
        cmd: &'j str,
        worktree: &'j Path,
        head: String,
        phase: &'j str,
        attempt: u32,
    ) -> gate::Job<'j> {
        let full = self.gate.full(cmd);
        let files = (!full)
            .then(|| {
                let since = gate_state
                    .full_green()
                    .map(String::from)
                    .or_else(|| git::run(worktree, &["merge-base", &self.base, "HEAD"]).ok())?;
                git::run(worktree, &["diff", "--name-only", &since]).ok()
            })
            .flatten();
        gate::Job {
            cmd,
            full,
            files,
            worktree,
            log_dir: self.common.join("ns").join("transcripts").join(&self.unit),
            unit: &self.unit,
            timeout: gate::timeout(self.fac.phase("build").timeout_minutes),
            head,
            phase,
            attempt,
            cap: self.cap,
            ratchets: gate::Ratchets {
                factory: &self.root,
                night: self.night.as_deref(),
            },
        }
    }
}

struct Finish {
    outcome: Outcome,
    reason: String,
    phase: Option<String>,
    needs_human: bool,
    reset_at: Option<i64>,
    /// Paused by the night's hold, not by a usage limit of its own.
    held: bool,
}

fn finish(outcome: Outcome, reason: impl Into<String>, phase: Option<&str>) -> Finish {
    Finish {
        outcome,
        reason: reason.into(),
        phase: phase.map(String::from),
        needs_human: false,
        reset_at: None,
        held: false,
    }
}

/// The factory definition and user config, read once at the start of `ns run` or `ns watch`.
pub struct Loaded {
    pub root: PathBuf,
    pub fac: Factory,
    pub cfg: Option<Config>,
    /// The build gate command, read at start like the files: `ns watch` moves the main checkout
    /// between units, and its `docs/agents/stack.md` must not change the night's gate.
    pub gate: gate::Commands,
    /// The path and sha256 of each file read, so a morning reader can tell what ran.
    pub files: Value,
    /// The memory cap phases and gates run under, probed once at start.
    pub cap: Option<Cap>,
    /// The running `ns` binary, which the phase hook guard and the gate run by path.
    pub ns: PathBuf,
}

impl Loaded {
    /// Read both files and export the config's forge tokens, so every child process inherits them.
    /// A dry run probes no memory cap: it runs nothing.
    pub fn read(factory: Option<&Path>, dry_run: bool) -> Result<Loaded> {
        let start = std::env::current_dir().context("cannot read current directory")?;
        let repo = Repo::discover(&start)?;
        let root = factory::root(factory, &repo.root);
        let fac = factory::load(&root)?;
        for w in fac.warnings() {
            eprintln!("ns: warning: {}: {w}", root.join(factory::FILE).display());
        }
        let cfg_path = config::path();
        let cfg = config::load(&cfg_path)
            .map_err(|e| SfError::usage(format!("{e:#}"), "check it with:\n  ns doctor"))?;
        let origin = forge::Origin::read(&repo.root, fac.queue.source.as_deref(), cfg.as_ref());
        forge::start(&origin, dry_run)?;
        let files = json!({
            "config": file_hash(&cfg_path),
            "factory": file_hash(&root.join(factory::FILE)),
        });
        let ns = crate::exe::ns_exe()?;
        let gate = gate::Commands::read(&fac, &repo.root, &ns);
        let cap = match fac.limits.memory_mb.filter(|mb| *mb > 0 && !dry_run) {
            Some(mb) => {
                let cap = memcap::detect(mb)?;
                match &cap.fallback {
                    None => eprintln!(
                        "ns: phases and gates run under a {mb} MB memory cap (cgroup, systemd-run)"
                    ),
                    Some(why) => eprintln!(
                        "ns: phases and gates run under a {mb} MB memory cap (RLIMIT_AS, prlimit); no cgroup: {why}"
                    ),
                }
                Some(cap)
            }
            None => None,
        };
        Ok(Loaded {
            root,
            fac,
            cfg,
            gate,
            files,
            cap,
            ns,
        })
    }
}

/// `{"path", "sha256"}`, with a null hash when the file does not exist.
fn file_hash(p: &Path) -> Value {
    let sha = fs::read(p)
        .ok()
        .map(|b| crate::eval::hex(&Sha256::digest(b)));
    json!({"path": p, "sha256": sha})
}

pub fn execute(args: &RunArgs, shared: &mut Shared, loaded: &Loaded) -> Result<RunResult> {
    let start = std::env::current_dir().context("cannot read current directory")?;
    let repo = Repo::discover(&start)?;
    let Loaded {
        root,
        fac,
        cfg,
        gate,
        cap,
        ns,
        ..
    } = loaded;
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
                    unit = Some(unit_name(n, v["title"].as_str().unwrap_or("")));
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
    let wt_path =
        worktree::existing(&repo, &unit)?.unwrap_or_else(|| worktree::planned_path(&repo, &unit));
    let subscription = fac.subscription();
    let mut commands = BTreeMap::new();
    for p in PHASES {
        commands.insert(
            *p,
            phase_command(
                cfg.as_ref(),
                &fac.phase(p),
                subscription,
                &repo.common_dir,
                &wt_path,
                &unit,
            )?,
        );
    }
    if subscription && !args.dry_run && commands.values().any(|c| billing::is_claude(c)) {
        billing::check_login()?;
    }

    if args.dry_run {
        return dry_run(
            &repo, fac, root, gate, &unit, args, &issue_url, &gates, &commands,
        );
    }

    let _lock = Lock::acquire(&repo.common_dir, &unit, args.issue)?;
    let base = resolve_base(&repo, args)?;
    let wt = worktree::ensure(&repo, &unit, Some(&base), &fac.worktree.setup)?;
    same_worktree(&wt_path, Path::new(&wt.path))?;
    let pinned = pin::pin(Path::new(&wt.path), &repo.common_dir, &unit, &base, ns)?;
    let ctx = Ctx {
        unit: unit.clone(),
        worktree: PathBuf::from(&wt.path),
        artifacts: PathBuf::from(&wt.artifacts),
        common: repo.common_dir.clone(),
        fac,
        root: root.clone(),
        issue: args.issue,
        issue_url,
        gates,
        base,
        lock_dir: lock_dir(cfg.as_ref(), &repo.common_dir),
        gate: gate.clone(),
        cap: cap.as_ref(),
        night: shared.night.as_ref().map(|n| n.path().to_path_buf()),
        ns: ns.clone(),
    };
    for ev in pinned {
        ctx.log(ev);
    }
    let mut phases: Vec<Value> = Vec::new();
    let mut last_artifact: Option<String> = None;
    let result = {
        // A stop that killed a setup command interrupts the unit rather than leaving it stuck.
        let driven = if let Some(failed) = wt.setup.iter().find(|r| r.exit != Some(0)) {
            crate::stop::check().map(|()| {
                finish(
                    Outcome::Stuck,
                    format!("worktree setup command {:?} failed", failed.run),
                    None,
                )
            })
        } else {
            drive(
                &ctx,
                args,
                shared,
                &commands,
                &mut phases,
                &mut last_artifact,
            )
        };
        if let Some(sig) = driven.as_ref().err().and(crate::stop::requested()) {
            ctx.log(json!({
                "event": "end",
                "outcome": "interrupted",
                "reason": format!("stopped by {}", crate::stop::name(sig)),
                "cost_usd": shared.spent(),
            }));
        }
        driven?
    };
    let pr = known_pr(&ctx.artifacts);
    ctx.log(json!({
        "event": "end",
        "outcome": result.outcome.label(),
        "reason": result.reason,
        "phase": result.phase,
        "pr": pr,
        "cost_usd": shared.spent(),
    }));
    let mut record_saved = false;
    if matches!(
        result.outcome,
        Outcome::Done | Outcome::Merged | Outcome::Stuck
    ) {
        let meta = crate::quality::Meta {
            unit: unit.clone(),
            issue: ctx.issue.or_else(|| crate::quality::issue_of(&unit)),
            pr,
            outcome: Some(result.outcome.label().to_string()),
        };
        let ev = crate::quality::record_unit(&repo, &ctx.worktree, &meta);
        record_saved = ev["error"].is_null() && ev["unparsed"].is_null();
        ctx.log(ev);
    }
    // A merge needs a PR number, so a merged unit always has one.
    let cleanup = pr.filter(|_| result.outcome == Outcome::Merged).map(|pr| {
        let issue = ctx.issue.or_else(|| crate::quality::issue_of(&unit));
        crate::clean::after_merge(&repo, &unit, &ctx.worktree, issue, pr, record_saved)
    });
    let cost: f64 = phases.iter().filter_map(|p| p["cost_usd"].as_f64()).sum();
    let json = json!({
        "unit": unit,
        "outcome": result.outcome.label(),
        "phase": result.phase,
        "reason": result.reason,
        "needs_human": result.needs_human,
        "pr": pr,
        "cost_usd": cost,
        "reset_at": result.reset_at,
        "held": result.held,
        "ended_at": shared.clock.now(),
        "artifact": last_artifact,
        "worktree": ctx.worktree.to_string_lossy(),
        "cleanup": cleanup,
        "phases": phases,
    });
    Ok(RunResult {
        unit,
        outcome: result.outcome,
        reason: result.reason,
        needs_human: result.needs_human,
        reset_at: result.reset_at,
        held: result.held,
        artifact: last_artifact,
        cost_usd: cost,
        ended_at: Some(shared.clock.now()),
        json,
    })
}

#[allow(clippy::too_many_arguments)]
fn dry_run(
    repo: &Repo,
    fac: &Factory,
    root: &Path,
    gate: &gate::Commands,
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
    let base = resolve_base(repo, args).ok();
    let unit_diff = existing
        .clone()
        .zip(base.clone())
        .map(|(p, b)| UnitDiff::new(p, b, &head));
    let state = read_state(
        &wt.join(".ns").join(unit),
        head,
        args.issue.is_some(),
        unit_diff,
    );
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
        base: base.unwrap_or_default(),
        lock_dir: PathBuf::new(),
        gate: gate.clone(),
        cap: None,
        night: None,
        ns: PathBuf::new(),
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
        Decision::Split(r) => (json!({"action": "split", "reason": r}), None, None),
    };
    let json = json!({
        "unit": unit,
        "dry_run": true,
        "worktree": wt.to_string_lossy(),
        "worktree_exists": existing.is_some(),
        "decision": decision_json,
        "prompt": prompt,
        "command": command,
        "gate": ctx.gate.build,
        "gate_rest": ctx.gate.rest,
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
        held: false,
        artifact: None,
        cost_usd: 0.0,
        ended_at: None,
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
    let mut last_failure: BTreeMap<&'static str, String> = BTreeMap::new();
    let mut reruns: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut passed: BTreeSet<&'static str> = BTreeSet::new();
    let from = args
        .triage_only
        .then_some("triage")
        .or(args.from.as_deref());
    let mut forced: Option<Decision> = from.map(|f| {
        run(
            PHASES.iter().find(|p| **p == f).copied().unwrap_or("build"),
            "",
            "--from",
        )
    });
    let mut baseline = remote_default(&ctx.worktree);
    let mut gate_state = gate::Gate::default();
    let mut merges = 0;
    ctx.log(json!({
        "event": "start",
        "default_branch": baseline.as_ref().map(|b| &b.0),
        "default_sha": baseline.as_ref().map(|b| &b.1),
        "memory_cap": ctx.cap.map(|c| json!({
            "mb": c.mb,
            "via": c.via.label(),
            "fallback": c.fallback,
        })),
    }));
    loop {
        crate::stop::check()?;
        let state = read_state(
            &ctx.artifacts,
            ctx.head(),
            ctx.issue.is_some(),
            Some(UnitDiff::new(
                ctx.worktree.clone(),
                ctx.base.clone(),
                &ctx.head(),
            )),
        );
        let decision = forced.take().unwrap_or_else(|| decide(&state));
        let (phase, feedback, why) = match decision {
            Decision::Stuck(r) => {
                if last_artifact.is_none() {
                    *last_artifact = PHASES
                        .iter()
                        .find(|p| state.arts.get(**p).is_some_and(|a| a.status == "blocked"))
                        .map(|p| {
                            ctx.artifacts
                                .join(artifact_of(p))
                                .to_string_lossy()
                                .into_owned()
                        });
                }
                return Ok(finish(Outcome::Stuck, r, None));
            }
            Decision::Split(r) => {
                let brief = artifact_of("triage");
                let kept = archive_as(&ctx.artifacts, brief, brief.trim_end_matches(".md"))?;
                *last_artifact = kept.map(|p| p.to_string_lossy().into_owned());
                return Ok(finish(Outcome::Split, r, Some("triage")));
            }
            Decision::Done => {
                if !fac.merge.auto() {
                    return Ok(finish(Outcome::Done, "pr.md passed", Some("ship")));
                }
                merges += 1;
                match merge_step(ctx, &state, shared, &mut gate_state, merges)? {
                    MergeStep::Finish(f) => return Ok(f),
                    MergeStep::Rebuild(red) => {
                        if let Some(k) = red.stuck_reason() {
                            last_failure.insert("build", k);
                        }
                        forced = Some(run("build", &red.feedback, red.reason));
                        continue;
                    }
                    MergeStep::Reship(feedback) => {
                        forced = Some(run("ship", &feedback, "the PR conflicts with its base"));
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
        // The first run of a verify or review after it passed fails nothing: HEAD moved under
        // it. Ship moves HEAD, so its attempts still bound a ship that changes the diff each time.
        if matches!(phase, "verify" | "review") && passed.remove(phase) {
            *reruns.entry(phase).or_default() += 1;
        }
        if attempt - reruns.get(phase).copied().unwrap_or(0) > p.max_attempts {
            let mut reason = format!("{phase} is out of attempts ({})", p.max_attempts);
            if let Some(t) = last_failure.get(phase) {
                reason.push_str(&format!(": {t}"));
            }
            return Ok(finish(Outcome::Stuck, reason, Some(phase)));
        }
        if let Some(reason) = budget_spent(fac.budget_usd(), shared.spent()) {
            return Ok(finish(Outcome::Budget, reason, Some(phase)));
        }
        if let Some(f) = shared.held(phase)? {
            return Ok(f);
        }
        let timeout = phase_timeout(phase, p.timeout_minutes);
        eprintln!("ns run: {} {phase} attempt {attempt} ({why})", ctx.unit);
        let locks = RunnerLocks::acquire(
            &ctx.lock_dir,
            &fac.locks(&p),
            &ctx.unit,
            &shared.clock,
            timeout,
            shared.until,
            |lock, by| {
                eprintln!(
                    "ns run: {} {phase} waits for lock {lock} (held by pid {}, unit {})",
                    ctx.unit, by["pid"], by["unit"]
                );
                ctx.log(json!({"event": "lock_wait", "phase": phase, "lock": lock, "held_by": by}));
            },
        )?;
        let held = match locks {
            Acquired::Held(held) => held,
            Acquired::GaveUp(g) => {
                let reason = g.reason();
                eprintln!("ns run: {} {phase} {reason}", ctx.unit);
                ctx.log(json!({
                    "event": "lock_wait_timeout",
                    "phase": phase,
                    "attempt": attempt,
                    "lock": g.lock,
                    "held_by": g.held_by,
                    "bound": g.bound.label(),
                }));
                if g.bound == Bound::Until {
                    return Ok(finish(Outcome::Budget, reason, Some(phase)));
                }
                phases.push(json!({
                    "phase": phase,
                    "attempt": attempt,
                    "decision": why,
                    "written": false,
                    "reason": reason,
                }));
                attempts.insert(phase, attempt);
                last_failure.insert(phase, format!("the last attempt {reason}"));
                forced = Some(Decision::Run {
                    phase,
                    feedback,
                    why,
                });
                continue;
            }
        };
        let held = match recheck_budget(held, fac.budget_usd(), shared.spent(), phase) {
            Ok(held) => held,
            Err(f) => return Ok(f),
        };
        // A unit that waited for a lock while another hit a usage limit waits for the reset.
        if let Some(f) = shared.held(phase)? {
            return Ok(f);
        }
        let art_path = ctx.artifacts.join(artifact_of(phase));
        let moves = archive_for(&ctx.artifacts, phase, &state)?;
        let mut prompt = ctx.prompt(&p, attempt, &feedback)?;
        if args.triage_only {
            prompt.push_str(factory::TRIAGE_ONLY);
        }
        let transcript = ctx
            .common
            .join("ns")
            .join("transcripts")
            .join(&ctx.unit)
            .join(format!("{phase}-{attempt}.jsonl"));
        let mut env = vec![
            ("NS_UNIT", ctx.unit.clone()),
            ("NS_PHASE", phase.to_string()),
            ("NS_ATTEMPT", attempt.to_string()),
            ("NS_WORKTREE", ctx.worktree.to_string_lossy().into_owned()),
            (
                pin::SETTINGS_VAR,
                pin::settings_path(&ctx.common, &ctx.unit)
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("NS_RUN_PID", std::process::id().to_string()),
            (
                "NS_WATCH_PID",
                shared.watch_pid.map(|p| p.to_string()).unwrap_or_default(),
            ),
        ];
        if !crate::git::ssh_command_set(&ctx.worktree) {
            env.push(("GIT_SSH_COMMAND", crate::git::SSH_KEEPALIVE.to_string()));
        }
        pin::write_settings(&ctx.common, &ctx.unit, &ctx.ns)?;
        let r = run_harness(
            &commands[phase],
            &prompt,
            &ctx.worktree,
            &env,
            timeout,
            &transcript,
            subscription,
            ctx.cap,
        );
        drop(held);
        if let Some(sig) = r.as_ref().err().and(crate::stop::requested()) {
            let ev = json!({
                "event": "phase",
                "phase": phase,
                "attempt": attempt,
                "decision": why,
                "transcript": transcript.to_string_lossy(),
            });
            return Err(set_aside(ctx, phase, &moves, sig, ev)?);
        }
        let r = r?;
        let t = ClaudeStreamJson.parse(&r.stdout);
        let cost = t.cost_usd.unwrap_or(0.0);
        shared.add_spend(&ctx.unit, cost)?;
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
        // A phase cut short wrote nothing, whatever it left behind.
        let cut_short = r.timed_out || r.killed.is_some();
        if cut_short {
            let file = artifact_of(phase);
            let kind = if r.timed_out { "timeout" } else { "killed" };
            let stem = format!("{}-{kind}", file.trim_end_matches(".md"));
            if let Some(dst) = archive_as(&ctx.artifacts, file, &stem)? {
                rec["archived"] = json!(dst.to_string_lossy());
            }
        }
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
        if r.timed_out {
            last_failure.insert(
                phase,
                format!(
                    "the last attempt {}; raise its timeout_minutes or split the unit",
                    gate::timed_out_after(timeout)
                ),
            );
        } else if let Some(k) = &r.killed {
            last_failure.insert(phase, format!("the last attempt {k}"));
        } else {
            last_failure.remove(phase);
        }
        let art = read_art(&art_path);
        let written = art.is_some();
        if art.as_ref().is_some_and(|a| a.status == "pass") {
            passed.insert(phase);
        }
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
                gate::timed_out_after(timeout)
            } else if let Some(k) = &r.killed {
                k.clone()
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
        if args.triage_only {
            return Ok(match (r.timed_out, r.killed, r.exit) {
                (true, _, _) => finish(Outcome::Stuck, gate::timed_out_after(timeout), Some(phase)),
                (false, Some(k), _) => finish(Outcome::Stuck, k, Some(phase)),
                (false, None, Some(0)) => finish(Outcome::Done, "triage ran", Some(phase)),
                (false, None, Some(c)) => {
                    finish(Outcome::Stuck, format!("exited {c}"), Some(phase))
                }
                (false, None, None) => finish(Outcome::Stuck, "killed by a signal", Some(phase)),
            });
        }
        if phase == "triage" && !written && r.exit == Some(0) && !cut_short && !art_path.exists() {
            return Ok(finish(
                Outcome::Stuck,
                "triage wrote no brief: the issue needs a human or ns-define",
                Some("triage"),
            ));
        }
        let head_moved = !same_sha(&state.head, &ctx.head());
        if cut_short && phase == "build" {
            if let Some(fb) = gate_state.red_feedback() {
                let how = r.killed.as_deref().unwrap_or("timed out");
                forced = Some(run(
                    "build",
                    fb,
                    format!("{how} with the CI gate still red"),
                ));
            }
            continue;
        }
        let trigger = if cut_short {
            head_moved.then_some(gate::Trigger::PhaseMovedHead)
        } else {
            gate_trigger(phase, art.as_ref(), head_moved)
        };
        if let Some((cmd, trigger)) = trigger.and_then(|t| Some((ctx.gate.for_trigger(t)?, t))) {
            let red = ctx.gate(&mut gate_state, trigger, cmd, phase, attempt);
            if let Some(sig) = red.as_ref().err().and(crate::stop::requested()) {
                let mut ev = json!({"event": "gate", "phase": phase, "attempt": attempt});
                if written {
                    return Err(set_aside(ctx, phase, &moves, sig, ev)?);
                }
                // The phase wrote nothing and its moves are already undone: nothing to set aside.
                ev["interrupted"] = json!(crate::stop::name(sig));
                ctx.log(ev);
                return Err(crate::stop::error(sig));
            }
            if let Some(red) = red? {
                if let Some(k) = red.stuck_reason() {
                    last_failure.insert("build", k);
                }
                forced = Some(run("build", &red.feedback, red.reason));
            }
        }
    }
}

/// A stop ended `phase`, or the gate run after it, before it was done: set aside what the phase
/// wrote and put back what it replaced, so the next run decides from the state before the
/// phase, and log `ev` with what happened. Returns the stop's error.
fn set_aside(
    ctx: &Ctx<'_>,
    phase: &str,
    moves: &[(PathBuf, PathBuf)],
    sig: i32,
    mut ev: Value,
) -> Result<anyhow::Error> {
    let file = artifact_of(phase);
    let stem = format!("{}-interrupted", file.trim_end_matches(".md"));
    let archived = archive_as(&ctx.artifacts, file, &stem)?;
    restore(moves);
    ev["interrupted"] = json!(crate::stop::name(sig));
    ev["archived"] = json!(archived.map(|d| d.to_string_lossy().into_owned()));
    ctx.log(ev);
    Ok(crate::stop::error(sig))
}

fn gate_trigger(phase: &str, art: Option<&Art>, head_moved: bool) -> Option<gate::Trigger> {
    match (phase, art) {
        ("build", Some(a)) if a.status == "pass" => Some(gate::Trigger::BuildPassed),
        ("build", None) => Some(gate::Trigger::BuildWroteNothing),
        ("build", _) => None,
        _ if head_moved => Some(gate::Trigger::PhaseMovedHead),
        _ => None,
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
                let reachable = git::ok(
                    &ctx.worktree,
                    &["merge-base", "--is-ancestor", &now, "HEAD"],
                );
                let mut prs = vec![];
                if reachable {
                    match merged_by_others(ctx, &sha, &now) {
                        Ok(p) => prs = p,
                        Err(why) => {
                            ctx.log(json!({"event": "breach", "rule": "default branch moved", "phase": phase, "from": sha, "to": now, "why": why}));
                            return Ok(Some(finish(
                                Outcome::Stuck,
                                format!("default branch moved: {branch} went from {sha} to {now}, which this unit holds and may have pushed: {why}"),
                                Some(phase),
                            )));
                        }
                    }
                }
                ctx.log(json!({"event": "default_moved_by_other_actor", "phase": phase, "from": sha, "to": now, "prs": prs}));
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

/// The most commits one move of the default branch is checked for, so a long push is still
/// decided with a bounded number of GitHub queries.
const MOVE_CHECK_MAX: usize = 50;

/// The default branch moved from `from` to `now`, a commit the unit's HEAD holds. That is
/// another actor's move only when every commit it added along the first-parent line came from
/// a merged PR that isn't this unit's: the unit holds `now` because it rebased onto it. Returns
/// those PRs, or why the move counts as this unit's.
fn merged_by_others(ctx: &Ctx<'_>, from: &str, now: &str) -> std::result::Result<Vec<u64>, String> {
    let text = git::run(
        &ctx.worktree,
        &[
            "rev-list",
            "--first-parent",
            &format!("--max-count={}", MOVE_CHECK_MAX + 1),
            &format!("{from}..{now}"),
        ],
    )
    .map_err(|e| format!("cannot list the commits it added: {e:#}"))?;
    let commits: Vec<&str> = text.split_whitespace().collect();
    if commits.is_empty() {
        return Err("it added no commit".into());
    }
    if commits.len() > MOVE_CHECK_MAX {
        return Err(format!("it added more than {MOVE_CHECK_MAX} commits"));
    }
    let own_pr = known_pr(&ctx.artifacts);
    let own_branch = format!("{}{}", worktree::BRANCH_PREFIX, ctx.unit);
    let mut prs = vec![];
    for c in commits {
        let pulls = gh_json(
            &ctx.worktree,
            &[
                "api",
                &format!("repos/{{owner}}/{{repo}}/commits/{c}/pulls"),
            ],
        )
        .map_err(|e| format!("cannot tell who made {c}: {e:#}"))?;
        let n = other_merged_pr(&pulls, own_pr, &own_branch)
            .ok_or_else(|| format!("{c} is in no merged PR of another branch"))?;
        if !prs.contains(&n) {
            prs.push(n);
        }
    }
    Ok(prs)
}

/// A merged PR in GitHub's `commits/<sha>/pulls` list, when the list names no PR that is
/// `own_pr` or comes from `own_branch`.
fn other_merged_pr(pulls: &Value, own_pr: Option<u64>, own_branch: &str) -> Option<u64> {
    let pulls = pulls.as_array()?;
    let own = |p: &Value| {
        p["number"].as_u64().is_some_and(|n| Some(n) == own_pr)
            || p["head"]["ref"].as_str() == Some(own_branch)
    };
    if pulls.iter().any(own) {
        return None;
    }
    pulls.iter().find_map(|p| {
        let merged = p["merged_at"].as_str().is_some_and(|s| !s.is_empty());
        p["number"].as_u64().filter(|_| merged)
    })
}

const REGISTER_BACKOFF_START: i64 = 5;
const REGISTER_BACKOFF_MAX: i64 = 30;

enum Registered {
    Yes,
    No,
    QueryFailed(String),
}

fn check_count(wt: &Path, sha: &str, kind: &str) -> Result<u64> {
    let out = gh(
        wt,
        &[
            "api",
            &format!("repos/{{owner}}/{{repo}}/commits/{sha}/{kind}"),
            "--jq",
            ".total_count",
        ],
    )?;
    let out = out.trim();
    out.parse()
        .map_err(|_| anyhow!("unexpected output from the {kind} query: {out:?}"))
}

/// GitHub may not have registered any check run or status right after a push or update-branch.
fn checks_registered(
    wt: &Path,
    sha: &str,
    timeout_minutes: u64,
    clock: &Clock,
) -> Result<Registered> {
    let deadline = clock.now() + (timeout_minutes * 60) as i64;
    let mut wait = REGISTER_BACKOFF_START;
    let mut polls = 0;
    loop {
        clock.check_polls(polls)?;
        polls += 1;
        let counts = ["check-runs", "status"].map(|kind| check_count(wt, sha, kind));
        if counts.iter().any(|c| matches!(c, Ok(n) if *n > 0)) {
            return Ok(Registered::Yes);
        }
        let now = clock.now();
        if now >= deadline {
            return Ok(match counts.into_iter().find_map(|c| c.err()) {
                Some(e) => Registered::QueryFailed(e.to_string()),
                None => Registered::No,
            });
        }
        clock.sleep_until((now + wait).min(deadline))?;
        wait = (wait * 2).min(REGISTER_BACKOFF_MAX);
    }
}

/// The PR's merge state once GitHub has worked it out. Right after its base moves, as when the
/// unit before it merged, GitHub reports `UNKNOWN` while it recomputes, so poll with the register
/// backoff until it reports anything else, or `timeout_minutes` pass, and return the last read.
fn merge_state(wt: &Path, pr: &str, timeout_minutes: u64, clock: &Clock) -> Result<Value> {
    let deadline = clock.now() + (timeout_minutes * 60) as i64;
    let mut wait = REGISTER_BACKOFF_START;
    let mut polls = 0;
    loop {
        clock.check_polls(polls)?;
        polls += 1;
        let state = gh_json(wt, &["pr", "view", pr, "--json", "mergeStateStatus"])?
            ["mergeStateStatus"]
            .take();
        let now = clock.now();
        if state != "UNKNOWN" || now >= deadline {
            return Ok(state);
        }
        clock.sleep_until((now + wait).min(deadline))?;
        wait = (wait * 2).min(REGISTER_BACKOFF_MAX);
    }
}

enum UpdatedHead {
    Moved(String),
    Dirty,
    /// The head stayed put; carries the last merge state GitHub reported.
    Unchanged(Value),
}

/// GitHub may still report the old head right after update-branch. A view with no head keeps the
/// old one.
fn head_after_update(
    wt: &Path,
    pr: &str,
    old: &str,
    timeout_minutes: u64,
    clock: &Clock,
) -> Result<UpdatedHead> {
    let deadline = clock.now() + (timeout_minutes * 60) as i64;
    let mut wait = REGISTER_BACKOFF_START;
    let mut polls = 0;
    loop {
        clock.check_polls(polls)?;
        polls += 1;
        let v = gh_json(
            wt,
            &["pr", "view", pr, "--json", "headRefOid,mergeStateStatus"],
        )?;
        if v["mergeStateStatus"].as_str() == Some("DIRTY") {
            return Ok(UpdatedHead::Dirty);
        }
        match v["headRefOid"].as_str() {
            Some(h) if h == old => {}
            h => return Ok(UpdatedHead::Moved(h.unwrap_or(old).to_string())),
        }
        let now = clock.now();
        if now >= deadline {
            return Ok(UpdatedHead::Unchanged(v["mergeStateStatus"].clone()));
        }
        clock.sleep_until((now + wait).min(deadline))?;
        wait = (wait * 2).min(REGISTER_BACKOFF_MAX);
    }
}

enum MergeStep {
    Finish(Finish),
    Rebuild(gate::Red),
    /// Back to ship with this `{feedback}`.
    Reship(String),
}

/// Back to build with `feedback` because CI on the PR failed.
fn ci_failed(feedback: String) -> MergeStep {
    MergeStep::Rebuild(gate::Red {
        feedback,
        reason: "CI failed on the PR".into(),
        killed: None,
        ratchets: None,
    })
}

/// Back to build to rebase PR #`n` onto `default`, which it conflicts with, or to ship under
/// `[gate] local = "build-only"`.
fn conflicts(ctx: &Ctx<'_>, n: u64, default: &str) -> MergeStep {
    ctx.log(json!({"event": "conflict", "pr": n}));
    let feedback = format!(
        "PR #{n} conflicts with {default}: rebase onto {default} and resolve conflicts, then force-push with --force-with-lease"
    );
    if ctx.fac.gate.local == LocalGate::BuildOnly {
        MergeStep::Reship(feedback)
    } else {
        ci_failed(feedback)
    }
}

fn human(reason: impl Into<String>) -> MergeStep {
    let mut f = finish(Outcome::Done, reason, Some("merge"));
    f.needs_human = true;
    MergeStep::Finish(f)
}

/// `merge.policy = "auto"`: squash-merge this unit's PR when CI is green, review.md passed and is
/// current at the PR head, the CI gate passed at that head, and no file or marked region that
/// needs human review changed. The only place ns merges anything. `run` numbers this merge step
/// in the invocation, for its gate log.
fn merge_step(
    ctx: &Ctx<'_>,
    state: &State,
    shared: &Shared,
    gate_state: &mut gate::Gate,
    run: u32,
) -> Result<MergeStep> {
    let (n, head, pr_head) = match open_pr(ctx, state)? {
        Ok(pr) => pr,
        Err(step) => return Ok(step),
    };
    let pr_head = pr_head.as_str();
    let ns = n.to_string();
    let wt = &ctx.worktree;

    // One merge at a time per repo, so a PR is updated and checked against the main the merge
    // before it left. Its merge state is read once the lock is held.
    let _merging =
        match take_merge_lock(&ctx.common, &ctx.unit, &shared.clock, shared.until, |ev| {
            ctx.log(ev)
        })? {
            Ok(lock) => lock,
            Err(f) => return Ok(MergeStep::Finish(f)),
        };

    // Strict required checks: a PR behind its base gets the base merged in, then fresh checks,
    // unless the local merge queue gates the merge result instead. Under build-only GitHub's
    // checks on the PR head with the base merged in are the merge queue, and no local gate runs.
    let remote = ctx.fac.gate.local == LocalGate::BuildOnly;
    let queued = !remote && ctx.fac.merge.local_queue && ctx.gate.rest.is_some();
    let default = remote_default(wt)
        .map(|d| d.0)
        .unwrap_or_else(|| "main".into());
    let conflict = || conflicts(ctx, n, &default);
    let register_minutes = ctx.fac.merge.ci_register_timeout;
    let mut merge_head = pr_head.to_string();
    let behind = match merge_state(wt, &ns, register_minutes, &shared.clock)?.as_str() {
        Some("DIRTY") => return Ok(conflict()),
        Some("BEHIND") => !queued,
        _ if remote => match fetch_tip(wt, &default) {
            Ok(tip) => !git::ok(wt, &["merge-base", "--is-ancestor", &tip, pr_head]),
            Err(e) => {
                return Ok(human(format!(
                    "cannot fetch {default} to update PR #{n}: {e:#}; needs a human merge"
                )))
            }
        },
        _ => false,
    };
    if behind {
        match update_branch(ctx, n, pr_head, &default, &shared.clock, &conflict)? {
            Ok(h) => merge_head = h,
            Err(step) => return Ok(step),
        }
    }

    if let Some(cmd) = ctx.gate.rest.as_ref().filter(|_| !remote) {
        let step = if queued {
            gate_merge_result(ctx, gate_state, cmd, n, pr_head, &default, run)?
        } else {
            gate_before_merge(ctx, gate_state, cmd, n, &merge_head, run)?
        };
        if let Some(step) = step {
            return Ok(step);
        }
    }

    if let Some(step) = wait_for_ci(ctx, n, &merge_head, &shared.clock)? {
        return Ok(step);
    }

    if let Some(step) = human_review(ctx, n, &default)? {
        return Ok(step);
    }
    // No --delete-branch: gh would check out the default branch in the unit's worktree first.
    let merged = gh(
        wt,
        &[
            "pr",
            "merge",
            &ns,
            "--squash",
            "--match-head-commit",
            &merge_head,
        ],
    );
    let state_now = gh_json(wt, &["pr", "view", &ns, "--json", "state"])
        .ok()
        .and_then(|v| v["state"].as_str().map(String::from));
    if state_now.as_deref() == Some("MERGED") {
        ctx.log(json!({"event": "merged", "pr": n, "sha": head, "spent_usd": shared.spent()}));
        let branch = format!("{}{}", crate::worktree::BRANCH_PREFIX, ctx.unit);
        let path = format!("repos/{{owner}}/{{repo}}/git/refs/heads/{branch}");
        if let Err(e) = gh(wt, &["api", &path, "-X", "DELETE"]) {
            ctx.log(json!({"event": "branch_not_deleted", "pr": n, "branch": branch, "error": format!("{e:#}")}));
        }
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

/// The open PR `pr.md` names, if the merge step may go on: its number, HEAD, and the PR head.
fn open_pr(ctx: &Ctx<'_>, state: &State) -> Result<Result<(u64, String, String), MergeStep>> {
    if !state.arts.contains_key("ship") {
        return Ok(Err(human("no pr.md")));
    }
    let Some(n) = known_pr(&ctx.artifacts) else {
        return Ok(Err(human("pr.md has no pr: number; needs a human merge")));
    };
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let view = gh_json(wt, &["pr", "view", &ns, "--json", "state,headRefOid"])?;
    match view["state"].as_str() {
        Some("OPEN") => {}
        Some("MERGED") => {
            return Ok(Err(MergeStep::Finish(finish(
                Outcome::Stuck,
                format!("PR merged by run: #{n} was merged before ns run's merge step"),
                Some("merge"),
            ))))
        }
        other => {
            return Ok(Err(MergeStep::Finish(finish(
                Outcome::Stuck,
                format!("PR #{n} is {}", other.unwrap_or("unknown")),
                Some("merge"),
            ))))
        }
    }
    let head = git::run(wt, &["rev-parse", "HEAD"]).unwrap_or_default();
    let pr_head = view["headRefOid"].as_str().unwrap_or("");
    if !state.current(pr_head) {
        return Ok(Err(human(format!(
            "PR head {pr_head} is not current with HEAD {head}; needs a human merge"
        ))));
    }
    if !state.arts.get("review").is_some_and(|r| state.passes(r)) {
        return Ok(Err(human(
            "review.md is not pass and current; needs a human merge",
        )));
    }
    Ok(Ok((n, head, pr_head.to_string())))
}

/// Wait, bounded, for CI on the PR at `merge_head`: `None` when every check passed or skipped,
/// else how the merge step ends.
fn wait_for_ci(
    ctx: &Ctx<'_>,
    n: u64,
    merge_head: &str,
    clock: &Clock,
) -> Result<Option<MergeStep>> {
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let register_minutes = ctx.fac.merge.ci_register_timeout;
    match checks_registered(wt, merge_head, register_minutes, clock)? {
        Registered::Yes => {}
        Registered::No => {
            return Ok(Some(human(format!(
                "no CI checks registered within {register_minutes} min on PR #{n}; needs a human merge"
            ))));
        }
        Registered::QueryFailed(err) => {
            return Ok(Some(human(format!(
                "could not query CI checks on PR #{n}: {err}; needs a human merge"
            ))));
        }
    }

    // Wait for CI, bounded.
    let mut cmd = Command::new("gh");
    crate::git::scrub(&mut cmd);
    cmd.args(["pr", "checks", &ns, "--watch"])
        .current_dir(wt)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mins = ctx.fac.merge.ci_timeout_minutes;
    let (_, timed_out, _) = run_process(cmd, None, Duration::from_secs(mins * 60))
        .context("cannot run gh pr checks")?;
    if timed_out {
        return Ok(Some(MergeStep::Finish(finish(
            Outcome::Stuck,
            format!("CI on PR #{n} did not finish within {mins} min"),
            Some("merge"),
        ))));
    }
    let checks = gh_json(
        wt,
        &["pr", "checks", &ns, "--json", "name,state,bucket,link"],
    )
    .unwrap_or(Value::Array(Vec::new()));
    let checks = checks.as_array().cloned().unwrap_or_default();
    if checks.is_empty() {
        return Ok(Some(human(format!(
            "no CI checks reported on PR #{n}; needs a human merge"
        ))));
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
        let fb = ci_feedback(wt, n, &names, &failed);
        ctx.log(json!({"event": "ci_failed", "pr": n, "checks": names, "where": "github"}));
        return Ok(Some(ci_failed(fb)));
    }
    ctx.log(json!({"event": "ci_passed", "pr": n, "sha": merge_head, "where": "github"}));
    Ok(None)
}

/// The `{feedback}` for `failed` checks: their `names` and the tail of each one's failed log.
fn ci_feedback(wt: &Path, n: u64, names: &[String], failed: &[&Value]) -> String {
    let mut fb = format!("CI failed on PR #{n}: {}\n", names.join(", "));
    let run_re = regex::Regex::new(r"/actions/runs/(\d+)").unwrap();
    for c in failed {
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
    fb
}

/// Whether the PR changes a file or marked region that needs human review: `None` when it
/// doesn't, else how the merge step ends.
fn human_review(ctx: &Ctx<'_>, n: u64, default: &str) -> Result<Option<MergeStep>> {
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let files: Vec<String> = gh(wt, &["pr", "diff", &ns, "--name-only"])?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let hits = ctx.fac.merge.human_review_hits(&files);
    if !hits.is_empty() {
        ctx.log(json!({"event": "human_review", "pr": n, "paths": hits}));
        return Ok(Some(human(format!(
            "changes files that need human review; needs a human merge ({})",
            hits.join(", ")
        ))));
    }
    match marked_regions(wt, default) {
        Ok(regions) if regions.is_empty() => {}
        Ok(regions) => {
            ctx.log(json!({"event": "human_review", "pr": n, "regions": regions}));
            return Ok(Some(human(format!(
                "changes code in a human-review region; needs a human merge ({})",
                regions.join(", ")
            ))));
        }
        Err(e) => {
            return Ok(Some(human(format!(
                "cannot check human-review regions: {e:#}; needs a human merge"
            ))))
        }
    }
    Ok(None)
}

/// `gh pr update-branch`, or a merge of `default` with git when GitHub refuses it for the
/// `workflow` scope: the new PR head, or how the merge step ends.
fn update_branch(
    ctx: &Ctx<'_>,
    n: u64,
    pr_head: &str,
    default: &str,
    clock: &Clock,
    conflict: &dyn Fn() -> MergeStep,
) -> Result<Result<String, MergeStep>> {
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let register_minutes = ctx.fac.merge.ci_register_timeout;
    let mut via = "api";
    if let Err(e) = gh(wt, &["pr", "update-branch", &ns]) {
        let error = format!("{e:#}");
        let scope = workflow_refusal(&error);
        let mut ev = json!({"event": "update_branch_failed", "pr": n, "error": error});
        if scope {
            ev["workflow_scope"] = json!(true);
        }
        ctx.log(ev);
        if !scope {
            return Ok(Err(conflict()));
        }
        match merge_default_with_git(wt, default, pr_head) {
            Ok(true) => via = "git",
            Ok(false) => return Ok(Err(conflict())),
            Err(e) => {
                return Ok(Err(human(format!(
                        "PR #{n}: GitHub refused update-branch without the `workflow` scope, and merging {default} with git failed: {e:#}; needs a human merge"
                    ))));
            }
        }
    }
    let head = match head_after_update(wt, &ns, pr_head, register_minutes, clock)? {
        UpdatedHead::Moved(h) => h,
        UpdatedHead::Dirty => return Ok(Err(conflict())),
        UpdatedHead::Unchanged(merge_state) => {
            let error = format!(
                    "PR #{n} head {pr_head} did not change after update-branch within {register_minutes} min"
                );
            ctx.log(json!({"event": "update_branch_failed", "pr": n, "error": error, "merge_state": merge_state}));
            return Ok(Err(human(format!("{error}; needs a human merge"))));
        }
    };
    ctx.log(json!({"event": "update_branch", "pr": n, "via": via}));
    Ok(Ok(head))
}

/// Check out the PR head `merge_head` and run the gate `cmd` there, unless it already went green
/// at that head: `None` to go on to the merge, else how the merge step ends.
fn gate_before_merge(
    ctx: &Ctx<'_>,
    gate_state: &mut gate::Gate,
    cmd: &str,
    n: u64,
    merge_head: &str,
    run: u32,
) -> Result<Option<MergeStep>> {
    if let Err(e) = check_out(&ctx.worktree, merge_head) {
        return Ok(Some(human(format!(
            "cannot check out PR #{n} head {merge_head} for the CI gate: {e:#}; needs a human merge"
        ))));
    }
    let red = ctx.gate(gate_state, gate::Trigger::BeforeMerge, cmd, "merge", run);
    Ok(merge_gate_stopped(ctx, red, run)?.map(MergeStep::Rebuild))
}

/// `red`, or the stop that interrupted the merge step's gate, logged.
fn merge_gate_stopped(
    ctx: &Ctx<'_>,
    red: Result<Option<gate::Red>>,
    run: u32,
) -> Result<Option<gate::Red>> {
    if let Some(sig) = red.as_ref().err().and(crate::stop::requested()) {
        ctx.log(json!({
            "event": "gate",
            "phase": "merge",
            "attempt": run,
            "interrupted": crate::stop::name(sig),
        }));
        return Err(crate::stop::error(sig));
    }
    red
}

/// The local merge queue: merge the PR head `pr_head` into the fresh tip of `origin/<default>`
/// in a scratch worktree, no push, and run the gate `cmd` on the result, unless it already ran
/// on that pair. `None` to go on to the merge, else how the merge step ends.
fn gate_merge_result(
    ctx: &Ctx<'_>,
    gate_state: &mut gate::Gate,
    cmd: &str,
    n: u64,
    pr_head: &str,
    default: &str,
    run: u32,
) -> Result<Option<MergeStep>> {
    let wt = &ctx.worktree;
    let base = match fetch_tip(wt, default) {
        Ok(b) => b,
        Err(e) => {
            return Ok(Some(human(format!(
                "cannot fetch {default} for the local merge queue: {e:#}; needs a human merge"
            ))))
        }
    };
    if let Err(e) = check_out(wt, pr_head) {
        return Ok(Some(human(format!(
            "cannot check out PR #{n} head {pr_head} for the local merge queue: {e:#}; needs a human merge"
        ))));
    }
    // Merging a base the PR head already holds leaves the PR head: gate it as the head.
    if git::ok(wt, &["merge-base", "--is-ancestor", &base, pr_head]) {
        let step = gate_before_merge(ctx, gate_state, cmd, n, pr_head, run)?;
        if matches!(step, Some(MergeStep::Rebuild(_))) {
            log_merge_gate_failed(ctx, n, pr_head, &base, false);
        }
        return Ok(step);
    }
    let reused = gate_state.merge_verdict(pr_head, &base);
    let red = match reused.clone() {
        Some(red) => red,
        None => {
            let scratch = match Scratch::add(wt, &ctx.common, &ctx.unit, pr_head) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(Some(human(format!(
                    "cannot add a worktree for the local merge queue: {e:#}; needs a human merge"
                ))))
                }
            };
            match merge_into(&scratch.path, &base) {
                Ok(true) => {}
                Ok(false) => return Ok(Some(conflicts(ctx, n, default))),
                Err(e) => {
                    return Ok(Some(human(format!(
                        "cannot merge PR #{n} into {default} for the local merge queue: {e:#}; needs a human merge"
                    ))))
                }
            }
            let job = ctx.gate_job(
                gate_state,
                cmd,
                &scratch.path,
                pr_head.to_string(),
                "merge",
                run,
            );
            let red = gate_state.on_merge_result(&job, default, &base, &|ev| ctx.log(ev));
            merge_gate_stopped(ctx, red, run)?
        }
    };
    if red.is_some() {
        log_merge_gate_failed(ctx, n, pr_head, &base, reused.is_some());
    }
    Ok(red.map(MergeStep::Rebuild))
}

fn log_merge_gate_failed(ctx: &Ctx<'_>, n: u64, pr_head: &str, base: &str, reused: bool) {
    ctx.log(json!({
        "event": "merge_gate_failed",
        "pr": n,
        "sha": pr_head,
        "base": base,
        "reused": reused,
    }));
}

/// The tip of `origin/<branch>`, freshly fetched.
fn fetch_tip(wt: &Path, branch: &str) -> Result<String> {
    let out = git::net(wt, &["fetch", "-q", "origin", branch])
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git fetch origin {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    git::run(wt, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"])
}

/// A detached worktree under `<common>/ns/merge-queue/<unit>` for the local merge queue,
/// removed when dropped, so a failure or a stop leaves none behind.
struct Scratch {
    repo: PathBuf,
    path: PathBuf,
}

impl Scratch {
    /// Add it at `sha`, first removing one a killed run left at that path. No hook runs.
    fn add(repo: &Path, common: &Path, unit: &str, sha: &str) -> Result<Scratch> {
        let dir = common.join("ns").join("merge-queue");
        let s = Scratch {
            repo: repo.to_path_buf(),
            path: dir.join(unit),
        };
        s.remove();
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = s.path.to_string_lossy();
        git::run(
            repo,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "worktree",
                "add",
                "-q",
                "--detach",
                &path,
                sha,
            ],
        )?;
        Ok(s)
    }

    fn remove(&self) {
        let _ = fs::remove_dir_all(&self.path);
        let _ = git::run(&self.repo, &["worktree", "prune"]);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Merge `base` into the detached HEAD at `dir` without committing, so no merge hook runs:
/// `Ok(false)` is a conflict. The identity is a placeholder git asks for anyway.
fn merge_into(dir: &Path, base: &str) -> Result<bool> {
    let out = git::command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=ns",
            "-c",
            "user.email=ns@localhost",
            "merge",
            "-q",
            "--no-ff",
            "--no-commit",
            base,
        ])
        .output()
        .context("failed to run git")?;
    if out.status.success() {
        return Ok(true);
    }
    if git::run(dir, &["diff", "--name-only", "--diff-filter=U"]).is_ok_and(|p| !p.is_empty()) {
        return Ok(false);
    }
    Err(anyhow!(
        "git merge {base} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// Point the worktree's branch at exactly `sha`, fetched from origin when it is not here: the PR
/// head the merge goes ahead with, which after a rebase need not descend from HEAD.
fn check_out(wt: &Path, sha: &str) -> Result<()> {
    if git::run(wt, &["rev-parse", "HEAD"]).is_ok_and(|h| same_sha(&h, sha)) {
        return Ok(());
    }
    let here = git::run(wt, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok();
    if !here {
        let branch = git::run(wt, &["symbolic-ref", "--short", "HEAD"])?;
        let out = git::net(wt, &["fetch", "-q", "origin", &branch])
            .output()
            .context("failed to run git")?;
        if !out.status.success() {
            return Err(anyhow!(
                "git fetch origin {branch} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    git::run(wt, &["reset", "-q", "--hard", &format!("{sha}^{{commit}}")])?;
    Ok(())
}

/// Whether a failed `gh pr update-branch` was GitHub refusing to write a workflow file: the
/// token lacks the `workflow` scope (an OAuth or personal token) or the `workflows` permission
/// (a GitHub App).
fn workflow_refusal(error: &str) -> bool {
    error.contains("refusing to allow") && error.contains("workflow")
}

/// Merge the fresh tip of `origin/<default>` into the worktree's branch and push it over
/// `pr_head` with a lease, as update-branch would have. `Ok(false)` is a merge conflict,
/// aborted; any other failure is an error. HEAD must be `pr_head`, since the lease would
/// otherwise replace a PR head the merge was not built on. A failed push resets the branch to
/// the HEAD it started from, so the worktree still matches the PR. The merge runs
/// with `core.hooksPath=/dev/null`, since `--no-verify` leaves `post-merge` running; the push
/// uses `--no-verify`, as `pre-push` is the only hook it runs. Both skip the checkout's hooks
/// because the forge's CI checks the merge result; the merge does carry unit code.
fn merge_default_with_git(wt: &Path, default: &str, pr_head: &str) -> Result<bool> {
    let net = |args: &[&str]| -> Result<()> {
        let out = git::net(wt, args).output().context("failed to run git")?;
        if !out.status.success() {
            return Err(anyhow!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    };
    let start = git::run(wt, &["rev-parse", "HEAD"])?;
    if !same_sha(&start, pr_head) {
        return Err(anyhow!(
            "HEAD {start} is not the PR head {pr_head}, so a merge of {default} would replace it"
        ));
    }
    net(&["fetch", "-q", "origin", default])?;
    let branch = git::run(wt, &["symbolic-ref", "--short", "HEAD"])?;
    let theirs = format!("origin/{default}");
    let merge = git::command()
        .arg("-C")
        .arg(wt)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "merge",
            "-q",
            "--no-edit",
            "FETCH_HEAD",
            "-m",
        ])
        .arg(format!("Merge {theirs} into {branch}"))
        .output()
        .context("failed to run git")?;
    if !merge.status.success() {
        let merging = git::ok(wt, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
        let unmerged = git::run(wt, &["diff", "--name-only", "--diff-filter=U"])
            .is_ok_and(|paths| !paths.is_empty());
        let aborted = !merging || git::run(wt, &["merge", "--abort"]).is_ok();
        if unmerged && aborted {
            return Ok(false);
        }
        return Err(anyhow!(
            "git merge failed: {}",
            String::from_utf8_lossy(&merge.stderr).trim()
        ));
    }
    let lease = format!("--force-with-lease=refs/heads/{branch}:{pr_head}");
    let spec = format!("HEAD:refs/heads/{branch}");
    if let Err(e) = net(&["push", "-q", "--no-verify", &lease, "origin", &spec]) {
        return Err(match git::run(wt, &["reset", "-q", "--keep", &start]) {
            Ok(_) => e,
            Err(reset) => anyhow!("{e:#}; resetting to {start} failed too: {reset:#}"),
        });
    }
    Ok(true)
}

/// Human-review regions that HEAD's changes since its merge base with `default` touch, counting
/// fences at the freshly fetched tip of `default`. A failed fetch is an error, never a stale tip.
fn marked_regions(wt: &Path, default: &str) -> Result<Vec<String>> {
    git::run(wt, &["fetch", "-q", "origin", default])?;
    let tip = git::run(wt, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"])?;
    let base = git::run(wt, &["merge-base", "HEAD", &tip])?;
    markers::touched_between(wt, &base, "HEAD", Some(&tip))
}

#[cfg(test)]
mod tests {
    use super::locks::tests::{hold, MIN};
    use super::*;

    #[test]
    fn workflow_refusal_needs_both_the_refusal_and_a_workflow() {
        assert!(workflow_refusal(
            "gh pr update-branch 12 failed: GraphQL: refusing to allow an OAuth App to create or update workflow `.github/workflows/ci.yml` without `workflow` scope"
        ));
        assert!(workflow_refusal(
            "refusing to allow a GitHub App to create or update workflow `.github/workflows/ci.yml` without `workflows` permission"
        ));
        assert!(!workflow_refusal("refusing to allow a merge: conflict"));
        assert!(!workflow_refusal("workflow run failed: conflict"));
    }

    #[test]
    fn other_merged_pr_names_only_a_merged_pr_of_another_branch() {
        let pr = |n: u64, merged: Value, head: &str| json!({"number": n, "merged_at": merged, "head": {"ref": head}});
        let when = json!("2026-10-10T01:00:00Z");
        let mine = "ns/7-fix";
        let check = |list: Vec<Value>, own: Option<u64>| other_merged_pr(&json!(list), own, mine);
        assert_eq!(check(vec![pr(3, when.clone(), "ns/3-b")], None), Some(3));
        assert_eq!(
            check(vec![pr(3, when.clone(), "ns/3-b")], Some(12)),
            Some(3)
        );
        assert_eq!(check(vec![], None), None);
        assert_eq!(check(vec![pr(3, Value::Null, "ns/3-b")], None), None);
        assert_eq!(check(vec![pr(3, json!(""), "ns/3-b")], None), None);
        assert_eq!(check(vec![pr(12, when.clone(), "ns/3-b")], Some(12)), None);
        assert_eq!(check(vec![pr(9, when.clone(), mine)], None), None);
        assert_eq!(
            check(
                vec![pr(3, when.clone(), "ns/3-b"), pr(12, Value::Null, "x")],
                Some(12)
            ),
            None
        );
        assert_eq!(
            check(
                vec![pr(3, Value::Null, "a"), pr(4, when.clone(), "b")],
                Some(12)
            ),
            Some(4)
        );
        assert_eq!(
            other_merged_pr(&json!({"message": "Not Found"}), None, mine),
            None
        );
    }

    #[test]
    fn a_budget_spent_during_a_lock_wait_stops_the_phase_and_frees_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let names = vec!["a".to_string()];
        let acquire = || {
            RunnerLocks::acquire(
                dir.path(),
                &names,
                "u",
                &Clock::pinned(1000),
                MIN,
                None,
                |_, _| {},
            )
            .unwrap()
            .held()
        };
        let held = recheck_budget(acquire(), Some(5.0), 4.0, "build");
        drop(held.ok().expect("under budget"));
        let Err(f) = recheck_budget(acquire(), Some(5.0), 5.0, "build") else {
            panic!("a spent budget kept the locks")
        };
        assert!(matches!(f.outcome, Outcome::Budget));
        assert_eq!(f.reason, "spent $5.00 of the $5.00 budget");
        assert_eq!(f.phase.as_deref(), Some("build"));
        acquire();
    }

    #[test]
    fn a_spent_budget_names_the_spend_and_the_limit() {
        assert_eq!(budget_spent(None, 99.0), None);
        assert_eq!(budget_spent(Some(5.0), 4.99), None);
        assert_eq!(
            budget_spent(Some(5.0), 5.0).as_deref(),
            Some("spent $5.00 of the $5.00 budget")
        );
    }

    #[test]
    fn marked_regions_errors_when_the_merge_base_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert!(marked_regions(dir.path(), "main").is_err());
        git::run(dir.path(), &["init", "-q"]).unwrap();
        assert!(marked_regions(dir.path(), "main").is_err());
    }

    /// A repo cloned from a bare `origin` whose `main` holds plain `f.c` and `g.c`.
    fn cloned() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let (origin, wt) = (t.path().join("origin.git"), t.path().join("wt"));
        let g = |dir: &Path, args: &[&str]| git::run(dir, args).unwrap();
        g(
            t.path(),
            &["init", "-q", "--bare", "-b", "main", "origin.git"],
        );
        g(t.path(), &["clone", "-q", "origin.git", "wt"]);
        for args in [
            &["config", "user.email", "t@example.com"][..],
            &["config", "user.name", "t"],
            &["config", "commit.gpgsign", "false"],
        ] {
            g(&wt, args);
        }
        fs::write(wt.join("f.c"), "a\nx = 1\nb\n").unwrap();
        fs::write(wt.join("g.c"), "a\nx = 1\nb\n").unwrap();
        g(&wt, &["add", "-A"]);
        g(&wt, &["commit", "-q", "-m", "base"]);
        g(&wt, &["push", "-q", "origin", "HEAD:main"]);
        (t, origin, wt)
    }

    fn commit_all(dir: &Path, msg: &str) {
        git::run(dir, &["add", "-A"]).unwrap();
        git::run(dir, &["commit", "-q", "-m", msg]).unwrap();
    }

    /// The local merge queue's scratch worktree is `ns` plumbing that commits nothing: its
    /// checkout and its merge run no hook, and the merge needs no identity of the user's. One a killed run left behind is replaced, and a drop
    /// removes it from disk and from `git worktree list`.
    #[test]
    fn the_merge_queue_scratch_worktree_runs_no_hook_and_is_removed_on_drop() {
        let (t, _origin, wt) = cloned();
        let g = |dir: &Path, args: &[&str]| git::run(dir, args).unwrap();
        g(&wt, &["switch", "-q", "-c", "ns/9-x"]);
        fs::write(wt.join("f.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "unit");
        let head = g(&wt, &["rev-parse", "HEAD"]);
        g(&wt, &["switch", "-q", "main"]);
        fs::write(wt.join("g.c"), "a\nx = 3\nb\n").unwrap();
        commit_all(&wt, "main moves");
        let base = g(&wt, &["rev-parse", "HEAD"]);
        let hooks = [
            "post-checkout",
            "pre-merge-commit",
            "commit-msg",
            "post-merge",
        ];
        for name in hooks {
            let hook = wt.join(".git/hooks").join(name);
            let ran = t.path().join(format!("ran-{name}"));
            crate::testutil::write_exe(
                &hook,
                format!("#!/bin/sh\ntouch '{}'\nexit 1\n", ran.display()),
            );
        }
        let common = t.path().join("common");
        std::mem::forget(Scratch::add(&wt, &common, "9-x", &head).unwrap());
        let s = Scratch::add(&wt, &common, "9-x", &head).unwrap();
        g(&wt, &["config", "user.name", ""]);
        g(&wt, &["config", "user.email", ""]);
        assert!(merge_into(&s.path, &base).unwrap());
        assert_eq!(g(&s.path, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            fs::read_to_string(s.path.join("f.c")).unwrap(),
            "a\nx = 2\nb\n"
        );
        assert_eq!(
            fs::read_to_string(s.path.join("g.c")).unwrap(),
            "a\nx = 3\nb\n"
        );
        let path = s.path.clone();
        drop(s);
        assert!(!path.exists());
        let list = g(&wt, &["worktree", "list", "--porcelain"]);
        assert!(!list.contains("9-x"), "{list}");
        for name in hooks {
            assert!(!t.path().join(format!("ran-{name}")).exists(), "{name} ran");
        }
    }

    /// A killed run can leave the scratch path as a plain directory git doesn't know, or as a
    /// registered worktree whose directory is gone: either way the next one is added there.
    #[test]
    fn a_scratch_worktree_replaces_whatever_a_killed_run_left_at_its_path() {
        let (t, _origin, wt) = cloned();
        let head = git::run(&wt, &["rev-parse", "HEAD"]).unwrap();
        let common = t.path().join("common");
        let path = common.join("ns/merge-queue/9-x");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("junk"), "left behind").unwrap();
        drop(Scratch::add(&wt, &common, "9-x", &head).unwrap());
        let s = Scratch::add(&wt, &common, "9-x", &head).unwrap();
        std::mem::forget(s);
        fs::remove_dir_all(&path).unwrap();
        let s = Scratch::add(&wt, &common, "9-x", &head).unwrap();
        assert!(s.path.join("f.c").is_file());
    }

    /// The update-branch merge runs with no hook (`--no-verify` would leave `post-merge`) and its
    /// push skips `pre-push`, because the forge's CI checks the merge result, and the merge does
    /// carry unit code.
    #[test]
    fn merging_the_default_branch_runs_none_of_the_checkouts_hooks() {
        let (t, _origin, wt) = cloned();
        let g = |dir: &Path, args: &[&str]| git::run(dir, args).unwrap();
        g(&wt, &["switch", "-q", "-c", "ns/9-x"]);
        fs::write(wt.join("f.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "unit");
        g(&wt, &["push", "-q", "origin", "ns/9-x"]);
        let other = t.path().join("other");
        g(t.path(), &["clone", "-q", "origin.git", "other"]);
        for args in [
            &["config", "user.email", "t@example.com"][..],
            &["config", "user.name", "t"],
            &["config", "commit.gpgsign", "false"],
        ] {
            g(&other, args);
        }
        fs::write(other.join("g.c"), "a\nx = 3\nb\n").unwrap();
        commit_all(&other, "main moves");
        g(&other, &["push", "-q", "origin", "HEAD:main"]);
        for name in ["pre-push", "pre-merge-commit", "commit-msg", "post-merge"] {
            let hook = wt.join(".git/hooks").join(name);
            crate::testutil::write_exe(&hook, format!("#!/bin/sh\ntouch ran-{name}\nexit 1\n"));
        }
        let head = g(&wt, &["rev-parse", "HEAD"]);
        assert!(merge_default_with_git(&wt, "main", &head).unwrap());
        for name in ["pre-push", "pre-merge-commit", "commit-msg", "post-merge"] {
            assert!(!wt.join(format!("ran-{name}")).exists(), "{name} ran");
        }
    }

    #[test]
    fn ref_safe_takes_ordinary_branch_names_only() {
        for ok in ["main", "release/1.2", "feature_x-1"] {
            assert!(ref_safe(ok), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "--upload-pack=/tmp/evil",
            "é",
            "a b",
            "a;b",
            "a$b",
            "a\\b",
        ] {
            assert!(!ref_safe(bad), "{bad}");
        }
    }

    #[test]
    fn remote_default_resolves_an_ordinary_default_branch() {
        let (_t, _origin, wt) = cloned();
        let (branch, sha) = remote_default(&wt).unwrap();
        assert_eq!(branch, "main");
        assert_eq!(sha, git::run(&wt, &["rev-parse", "HEAD"]).unwrap());
    }

    #[test]
    fn a_hostile_default_branch_never_runs_a_command() {
        // No dot-led path components, which a ref name can't hold.
        let bin = tempfile::Builder::new().prefix("nsbin").tempdir().unwrap();
        let (script, proof) = (bin.path().join("evil"), bin.path().join("ran"));
        crate::testutil::write_exe(
            &script,
            format!("#!/bin/sh\ntouch {}\nexit 1\n", proof.display()),
        );
        let (_t, origin, wt) = cloned();
        let hostile = format!("refs/heads/--upload-pack={}", script.display());
        git::run(&origin, &["update-ref", &hostile, "main"]).unwrap();
        git::run(&origin, &["symbolic-ref", "HEAD", &hostile]).unwrap();
        if let Some((branch, _)) = remote_default(&wt) {
            let _ = marked_regions(&wt, &branch);
        }
        assert!(
            !proof.exists(),
            "the remote's default branch ran {}",
            script.display()
        );
        assert!(remote_default(&wt).is_none());
    }

    #[test]
    fn marked_regions_sees_a_fence_that_landed_after_the_pr_branched() {
        let (_t, _origin, wt) = cloned();
        git::run(&wt, &["checkout", "-q", "-b", "pr"]).unwrap();
        fs::write(wt.join("f.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "pr edits f.c");
        // Spelled out at run time so this file holds no real markers.
        let (start, end) = ("ns:human-review start", "ns:human-review end");
        git::run(&wt, &["checkout", "-q", "-b", "fence", "main"]).unwrap();
        fs::write(
            wt.join("f.c"),
            format!("a\n// {start}\nx = 1\n// {end}\nb\n"),
        )
        .unwrap();
        commit_all(&wt, "fence f.c");
        // With no fetch refspec origin/main stays at the branch point; only FETCH_HEAD sees the fence.
        git::run(&wt, &["config", "--unset", "remote.origin.fetch"]).unwrap();
        git::run(&wt, &["push", "-q", "origin", "fence:main"]).unwrap();
        git::run(&wt, &["checkout", "-q", "pr"]).unwrap();
        assert_eq!(marked_regions(&wt, "main").unwrap(), ["f.c:1-5"]);
        fs::write(wt.join("f.c"), "a\nx = 1\nb\n").unwrap();
        fs::write(wt.join("g.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "pr edits only g.c");
        assert!(marked_regions(&wt, "main").unwrap().is_empty());
    }

    #[test]
    fn marked_regions_ignores_a_fence_the_pr_merged_in_from_the_default_branch() {
        let (_t, _origin, wt) = cloned();
        git::run(&wt, &["checkout", "-q", "-b", "pr"]).unwrap();
        fs::write(wt.join("g.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "pr edits g.c");
        let (start, end) = ("ns:human-review start", "ns:human-review end");
        git::run(&wt, &["checkout", "-q", "-b", "fence", "main"]).unwrap();
        fs::write(
            wt.join("f.c"),
            format!("a\n// {start}\nx = 1\n// {end}\nb\n"),
        )
        .unwrap();
        commit_all(&wt, "fence f.c");
        git::run(&wt, &["config", "--unset", "remote.origin.fetch"]).unwrap();
        git::run(&wt, &["push", "-q", "origin", "fence:main"]).unwrap();
        git::run(&wt, &["checkout", "-q", "pr"]).unwrap();
        git::run(&wt, &["merge", "-q", "--no-edit", "fence"]).unwrap();
        assert!(marked_regions(&wt, "main").unwrap().is_empty());
    }

    #[test]
    fn marked_regions_errors_when_the_fetch_fails() {
        let (_t, origin, wt) = cloned();
        assert!(marked_regions(&wt, "main").unwrap().is_empty());
        fs::remove_dir_all(origin).unwrap();
        let err = marked_regions(&wt, "main").unwrap_err();
        assert!(
            format!("{err:#}").starts_with("git fetch -q origin main failed"),
            "{err:#}"
        );
    }

    #[test]
    fn marked_regions_errors_when_head_shares_no_history_with_the_tip() {
        let (_t, _origin, wt) = cloned();
        git::run(&wt, &["checkout", "-q", "--orphan", "unrelated"]).unwrap();
        commit_all(&wt, "unrelated");
        assert!(marked_regions(&wt, "main").is_err());
    }

    #[test]
    fn timeout_override_parses_bare_named_and_malformed_values() {
        let ms = |v: Option<&str>, p: &str| timeout_override(v, p).map(|d| d.as_millis());
        assert_eq!(ms(None, "build"), None);
        assert_eq!(ms(Some(""), "build"), None);
        assert_eq!(ms(Some("1500"), "build"), Some(1500));
        assert_eq!(ms(Some("1500"), "review"), Some(1500));
        assert_eq!(ms(Some("build=1500"), "build"), Some(1500));
        assert_eq!(ms(Some("build=1500"), "review"), None);
        assert_eq!(ms(Some("build=1,review=2"), "review"), Some(2));
        assert_eq!(ms(Some(" build = 7 , review = 8 "), "review"), Some(8));
        assert_eq!(ms(Some(" 9 "), "ship"), Some(9));
        assert_eq!(ms(Some("abc"), "build"), None);
        assert_eq!(ms(Some("build=abc"), "build"), None);
        assert_eq!(ms(Some("build=,review=3"), "review"), Some(3));
        assert_eq!(ms(Some("5,build=1"), "build"), Some(1));
        assert_eq!(ms(Some("build=1,5"), "build"), Some(1));
        assert_eq!(ms(Some("build=1,5"), "review"), Some(5));
        assert_eq!(ms(Some("build=abc,5"), "build"), Some(5));
    }

    #[test]
    fn helpers() {
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
        let (wt, common) = (Path::new("/wt"), Path::new("/c"));
        let c = phase_command(Some(&cfg), &f.phase("build"), true, common, wt, "u").unwrap();
        let mut want: Vec<String> = [
            "claude",
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
        ]
        .map(String::from)
        .into();
        want.extend(pin::settings_args(
            &pin::settings_path(common, "u"),
            wt,
            "u",
        ));
        assert_eq!(c, want);
        let c = phase_command(None, &f.phase("build"), true, common, wt, "u").unwrap();
        assert!(c.contains(&"bypassPermissions".to_string()));
        assert!(!c.contains(&"--model".to_string()));
        assert!(c.contains(&"--settings".to_string()));
    }

    #[test]
    fn a_worktree_other_than_the_predicted_one_is_an_error() {
        assert!(same_worktree(Path::new("/a"), Path::new("/a")).is_ok());
        let err = same_worktree(Path::new("/a"), Path::new("/b")).unwrap_err();
        assert!(err.to_string().contains("/b"), "{err}");
    }

    #[test]
    fn a_claude_command_that_carries_settings_is_a_usage_error() {
        for flag in [
            ["--settings", "x.json"],
            ["--settings=x.json", "-p"],
            ["--setting-sources", "user"],
            ["--setting-sources=user", "-p"],
        ] {
            let mut cmd = vec!["claude".to_string()];
            cmd.extend(flag.iter().map(|s| s.to_string()));
            let cfg = config::parse(&format!(
                "[harness.claude]\ncommand = [\"claude\"]\ncommand_write = {cmd:?}\n"
            ))
            .unwrap();
            let f = factory::parse("").unwrap();
            let err = phase_command(
                Some(&cfg),
                &f.phase("build"),
                true,
                Path::new("/c"),
                Path::new("/wt"),
                "u",
            )
            .unwrap_err()
            .to_string();
            assert!(err.contains(flag[0].split('=').next().unwrap()), "{err}");
        }
    }

    #[test]
    fn a_codex_phase_gets_no_hook_guard_settings() {
        let f = factory::parse("[defaults]\nharness = \"codex\"\n").unwrap();
        let c = phase_command(
            None,
            &f.phase("build"),
            true,
            Path::new("/c"),
            Path::new("/wt"),
            "u",
        )
        .unwrap();
        assert!(!c.contains(&"--settings".to_string()), "{c:?}");
    }

    #[test]
    fn a_merge_lock_wait_at_until_ends_the_run_as_budget() {
        let dir = tempfile::tempdir().unwrap();
        let _other = hold(dir.path(), "ns-merge");
        let events = std::cell::RefCell::new(Vec::new());
        let clock = Clock::pinned(1000);
        let f = take_merge_lock(dir.path(), "u", &clock, Some(1000), |ev| {
            events.borrow_mut().push(ev)
        })
        .unwrap()
        .err()
        .unwrap();
        assert_eq!(f.outcome, Outcome::Budget);
        assert_eq!(f.phase.as_deref(), Some("merge"));
        assert_eq!(
            f.reason,
            "gave up waiting for the merge lock (held by pid 42, unit other) at --until"
        );
        let events = events.into_inner();
        let by = json!({"pid": 42, "unit": "other"});
        assert_eq!(
            events,
            [
                json!({"event": "lock_wait", "phase": "merge", "lock": "merge", "held_by": by}),
                json!({"event": "lock_wait_timeout", "phase": "merge", "lock": "merge", "held_by": by, "bound": "until"}),
            ]
        );
    }

    #[test]
    fn a_merge_lock_before_until_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let clock = Clock::pinned(1000);
        let taken = take_merge_lock(dir.path(), "u", &clock, Some(1001), |ev| {
            events.borrow_mut().push(ev)
        })
        .unwrap();
        assert!(taken.is_ok());
        assert!(events.into_inner().is_empty());
    }
}
