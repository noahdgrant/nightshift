//! `ns run`: drive one unit through the phases (docs/FACTORY.md).

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::billing;
#[cfg(test)]
use crate::clock::MAX_PINNED_POLLS;
use crate::clock::{self, Clock};
use crate::config::{self, Config};
use crate::error::SfError;
use crate::eval::parser::{ClaudeStreamJson, OutputParser};
use crate::eval::trial::run_process;
use crate::factory::{self, artifact_of, Factory, PhaseSettings, PHASES};
use crate::forge;
use crate::gate;
use crate::git::same_sha;
use crate::git::{self, Repo};
use crate::markers;
use crate::memcap::{self, Cap};
use crate::worktree;

pub(crate) mod currency;

use currency::{
    archive_as, archive_for, decide, known_pr, read_art, read_state, restore, run, Art, Decision,
    State, UnitDiff,
};

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
    pub artifact: Option<String>,
    pub cost_usd: f64,
    pub json: Value,
}

/// State shared across the units of one `ns watch` (or the single unit of `ns run`).
pub struct Shared {
    pub spent_usd: f64,
    pub clock: Clock,
    /// The `ns watch` process running this unit, or `None` for a standalone `ns run`.
    pub watch_pid: Option<u32>,
    /// `ns watch --until`, or `None` for a standalone `ns run`. It bounds a runner lock wait.
    pub until: Option<i64>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            spent_usd: 0.0,
            clock: Clock::from_env(),
            watch_pid: None,
            until: None,
        }
    }
}

pub fn cli(args: RunArgs) -> Result<ExitCode> {
    let mut shared = Shared::new();
    let dry = args.dry_run;
    let loaded = Loaded::read(args.factory.as_deref(), args.dry_run)?;
    let r = execute(&args, &mut shared, &loaded)?;
    println!("{}", serde_json::to_string_pretty(&r.json)?);
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
    if billing::is_claude(argv) {
        billing::wait_for_bg_tasks(&mut cmd);
        billing::disable_auto_memory(&mut cmd);
        if subscription {
            billing::scrub(&mut cmd);
        }
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

// ---------------------------------------------------------------- lock

/// A per-repo lock: an OS `flock` on a file in the git common dir, so the kernel drops a dead
/// process's hold. While held the file records the holder as JSON; it is empty otherwise.
pub struct Lock {
    file: File,
}

/// The run lock's file prefix: `ns-run-<unit>.lock`, one `ns run` at a time per unit.
const RUN_LOCK_PREFIX: &str = "ns-run-";
/// The watch lock: one `ns watch` at a time per repo.
const WATCH_LOCK: &str = "ns-watch.lock";
/// The merge lock: one merge step's update, CI wait and merge at a time per repo.
const MERGE_LOCK: &str = "ns-merge.lock";
/// The worktree lock: one `ns worktree new` creating a worktree at a time per repo.
pub const WORKTREE_LOCK: &str = "ns-worktree.lock";
/// How many times, 10 ms apart, a lock whose holder names no one is tried before refusing.
const BRIEF_HOLD_TRIES: u32 = 10;
/// How long a wait for the merge or worktree lock sleeps between tries.
const LOCK_WAIT_POLL: Duration = Duration::from_millis(50);

pub fn run_lock(unit: &str) -> String {
    format!("{RUN_LOCK_PREFIX}{unit}.lock")
}

fn open_lock(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))
}

fn try_lock(f: &File, path: &Path) -> Result<bool> {
    try_flock(f).with_context(|| format!("cannot lock {}", path.display()))
}

impl Lock {
    fn held(file: File, holder: &Value) -> Result<Lock> {
        let lock = Lock { file };
        lock.file.set_len(0)?;
        writeln!(&lock.file, "{holder}")?;
        Ok(lock)
    }

    /// Take `<common>/<name>` and record `holder` in it, or return who holds it now.
    fn take(common: &Path, name: &str, holder: Value) -> Result<Result<Lock, Value>> {
        let path = common.join(name);
        let f = open_lock(&path)?;
        let mut tries = 0;
        while !try_lock(&f, &path)? {
            // A holder that hasn't written its record yet, or `run_holders` checking the lock,
            // holds it only for a moment.
            let held = read_holder(&path);
            tries += 1;
            if !held.is_null() || tries >= BRIEF_HOLD_TRIES {
                return Ok(Err(held));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(Ok(Lock::held(f, &holder)?))
    }

    /// Take `<common>/<name>` and record `holder` in it, waiting while another process holds
    /// it. `waiting` hears who holds it once, when the wait begins. The wait sleeps in real
    /// time, since the holder is another process, and ends on a stop. It gives up, returning
    /// who holds the lock, once the clock reaches the `deadline`.
    pub fn wait(
        common: &Path,
        name: &str,
        holder: Value,
        deadline: Option<(&Clock, i64)>,
        mut waiting: impl FnMut(&Value),
    ) -> Result<Result<Lock, Value>> {
        let path = common.join(name);
        let f = open_lock(&path)?;
        let mut first = true;
        while !try_lock(&f, &path)? {
            let held = read_holder(&path);
            if first {
                waiting(&held);
                first = false;
            }
            if deadline.is_some_and(|(clock, at)| clock.now() >= at) {
                return Ok(Err(held));
            }
            crate::stop::check()?;
            std::thread::sleep(LOCK_WAIT_POLL);
        }
        Ok(Ok(Lock::held(f, &holder)?))
    }

    /// The run lock for `unit`, refused with exit 5 while another `ns run` holds it.
    pub fn acquire(common: &Path, unit: &str, issue: Option<u64>) -> Result<Lock> {
        let me = json!({"pid": std::process::id(), "unit": unit, "issue": issue});
        let name = run_lock(unit);
        Self::take(common, &name, me)?.map_err(|held| {
            SfError::new(
                EXIT_LOCKED,
                format!(
                    "another ns run (pid {}, unit {}) holds {}",
                    held["pid"],
                    held["unit"].as_str().unwrap_or(unit),
                    common.join(&name).display()
                ),
            )
            .hint("one run per unit; wait for it to finish, or stop it")
            .into()
        })
    }

    /// The watch lock, held for the whole of one `ns watch`, refused with exit 5 while another
    /// `ns watch` holds it.
    pub fn watch(common: &Path) -> Result<Lock> {
        let me = json!({"pid": std::process::id()});
        Self::take(common, WATCH_LOCK, me)?.map_err(|held| {
            SfError::new(
                EXIT_LOCKED,
                format!("another ns watch (pid {}) is running", held["pid"]),
            )
            .hint("one ns watch at a time per repo; stop it first")
            .into()
        })
    }

    /// Who holds a run lock now: `{pid, unit, issue}` for each live `ns run`, sorted by unit.
    /// A holder that hasn't written its record yet is `{pid: null, unit, issue: null}`, its
    /// unit read from the file name.
    pub fn run_holders(common: &Path) -> Result<Vec<Value>> {
        let entries = match fs::read_dir(common) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", common.display())),
        };
        let mut holders = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("cannot read {}", common.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(unit) = name
                .strip_prefix(RUN_LOCK_PREFIX)
                .and_then(|n| n.strip_suffix(".lock"))
                .filter(|u| worktree::valid_unit_id(u))
            else {
                continue;
            };
            let path = entry.path();
            let f = File::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
            if try_lock(&f, &path)? {
                unlock_flock(&f);
                continue;
            }
            let held = read_holder(&path);
            holders.push(if held.is_object() {
                held
            } else {
                json!({"pid": null, "unit": unit, "issue": null})
            });
        }
        holders.sort_by(|a, b| a["unit"].as_str().cmp(&b["unit"].as_str()));
        Ok(holders)
    }
}

/// A lock holder's pid for a message: `?` when the holder hasn't written its record yet.
pub fn holder_pid(holder: &Value) -> String {
    holder["pid"]
        .as_u64()
        .map_or_else(|| "?".into(), |p| p.to_string())
}

fn read_holder(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_default();
    serde_json::from_str(text.trim()).unwrap_or(Value::Null)
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        unlock_flock(&self.file);
    }
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
    let taken = Lock::wait(common, MERGE_LOCK, me, deadline, |by| {
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

/// The locks a phase's runner names, held from before the phase starts until it ends. Each is
/// an OS lock (`flock`) on `<dir>/<name>.lock`, so the kernel drops a dead run's hold and its
/// file is taken over. A lock held by a live run makes this one wait, up to a bound.
#[derive(Debug)]
pub struct RunnerLocks(Vec<File>);

/// The longest pause between two tries at a held lock.
const LOCK_POLL_MAX_S: i64 = 5;

/// Which bound ended a wait for a runner lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// The waiting phase's own timeout, counted from when the wait began.
    Timeout,
    /// `ns watch --until`.
    Until,
}

impl Bound {
    fn label(self) -> &'static str {
        match self {
            Bound::Timeout => "timeout",
            Bound::Until => "until",
        }
    }
}

/// How a wait for the runner locks ended.
#[derive(Debug)]
pub enum Acquired {
    Held(RunnerLocks),
    GaveUp(GaveUp),
}

/// A wait for a runner lock that hit its bound. Every lock taken before it is released.
#[derive(Debug)]
pub struct GaveUp {
    pub lock: String,
    pub held_by: Value,
    pub bound: Bound,
}

impl GaveUp {
    pub fn reason(&self) -> String {
        format!(
            "gave up waiting for lock {} (held by pid {}, unit {}) at {}",
            self.lock,
            self.held_by["pid"],
            self.held_by["unit"].as_str().unwrap_or("?"),
            match self.bound {
                Bound::Timeout => "the phase timeout",
                Bound::Until => "--until",
            }
        )
    }
}

impl RunnerLocks {
    /// Take `names` sorted and without duplicates, so two runs can't deadlock.
    /// `waiting` hears each lock this run has to wait for, and who holds it. A held lock is
    /// retried with a backoff on `clock` until `timeout` after the wait began, or `until`,
    /// whichever is first; a lock taken at or past `until` counts as a give-up too.
    pub fn acquire(
        dir: &Path,
        names: &[String],
        unit: &str,
        clock: &Clock,
        timeout: Duration,
        until: Option<i64>,
        mut waiting: impl FnMut(&str, &Value),
    ) -> Result<Acquired> {
        let mut names = names.to_vec();
        names.sort();
        names.dedup();
        if !names.is_empty() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let holder = |path: &Path| {
            let text = fs::read_to_string(path).unwrap_or_default();
            serde_json::from_str(text.trim()).unwrap_or(Value::Null)
        };
        let mut give_up = None;
        let mut held = RunnerLocks(Vec::new());
        for name in &names {
            let path = dir.join(format!("{name}.lock"));
            let mut f = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .with_context(|| format!("cannot open {}", path.display()))?;
            let try_lock =
                |f: &File| try_flock(f).with_context(|| format!("cannot lock {}", path.display()));
            if !try_lock(&f)? {
                let (at, bound) =
                    *give_up.get_or_insert_with(|| give_up_at(clock.now(), timeout, until));
                let first = holder(&path);
                waiting(name, &first);
                if !poll_until(clock, at, || try_lock(&f))? {
                    return Ok(Acquired::GaveUp(GaveUp {
                        lock: name.clone(),
                        held_by: Some(holder(&path))
                            .filter(|h| !h.is_null())
                            .unwrap_or(first),
                        bound,
                    }));
                }
                if until.is_some_and(|u| clock.now() >= u) {
                    unlock_flock(&f);
                    return Ok(Acquired::GaveUp(GaveUp {
                        lock: name.clone(),
                        held_by: first,
                        bound: Bound::Until,
                    }));
                }
            }
            f.set_len(0)?;
            writeln!(f, "{}", json!({"pid": std::process::id(), "unit": unit}))?;
            held.0.push(f);
        }
        Ok(Acquired::Held(held))
    }
}

/// Run `try_once` until it succeeds, backing off from 1 s to `LOCK_POLL_MAX_S` between tries
/// on `clock`. `Ok(false)` once the clock reaches `at` without a success.
fn poll_until(clock: &Clock, at: i64, mut try_once: impl FnMut() -> Result<bool>) -> Result<bool> {
    let mut step = 1;
    let mut polls = 0;
    while !try_once()? {
        clock.check_polls(polls)?;
        polls += 1;
        let now = clock.now();
        if now >= at {
            return Ok(false);
        }
        clock.sleep_until((now + step).min(at))?;
        step = (step * 2).min(LOCK_POLL_MAX_S);
    }
    Ok(true)
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

/// When a wait that begins at `now` gives up: `timeout` later (a partial second counts as a
/// whole one), or at `until` if that comes first.
fn give_up_at(now: i64, timeout: Duration, until: Option<i64>) -> (i64, Bound) {
    let t = now + timeout.as_millis().div_ceil(1000) as i64;
    match until {
        Some(u) if u <= t => (u, Bound::Until),
        _ => (t, Bound::Timeout),
    }
}

impl Drop for RunnerLocks {
    fn drop(&mut self) {
        for f in &self.0 {
            let _ = f.set_len(0);
            unlock_flock(f);
        }
    }
}

#[cfg(unix)]
fn unlock_flock(f: &File) {
    use std::os::unix::io::AsRawFd;
    // SAFETY: the fd is open for as long as `f` lives.
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
}

#[cfg(not(unix))]
fn unlock_flock(_: &File) {}

/// Try to take an exclusive `flock` on `f`; `Ok(false)` means another holds it.
#[cfg(unix)]
fn try_flock(f: &File) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    loop {
        // SAFETY: the fd is open for as long as `f` lives.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EWOULDBLOCK) => return Ok(false),
            _ => return Err(e),
        }
    }
}

#[cfg(not(unix))]
fn try_flock(_: &File) -> std::io::Result<bool> {
    Ok(true)
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
    /// The CI gate command; `None` runs no gate.
    gate: Option<String>,
    cap: Option<&'a Cap>,
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

/// The factory definition and user config, read once at the start of `ns run` or `ns watch`.
pub struct Loaded {
    pub root: PathBuf,
    pub fac: Factory,
    pub cfg: Option<Config>,
    /// The build gate command, read at start like the files: `ns watch` moves the main checkout
    /// between units, and its `docs/agents/stack.md` must not change the night's gate.
    pub gate: Option<String>,
    /// The path and sha256 of each file read, so a morning reader can tell what ran.
    pub files: Value,
    /// The memory cap phases and gates run under, probed once at start.
    pub cap: Option<Cap>,
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
        if let Some(c) = &cfg {
            forge::export(&c.forge)?;
        }
        let files = json!({
            "config": file_hash(&cfg_path),
            "factory": file_hash(&root.join(factory::FILE)),
        });
        let gate = gate::command(&fac, &repo.root);
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
        })
    }
}

/// `[runners] lock_dir` from the user config, else `<git-common-dir>/ns/locks`.
fn lock_dir(cfg: Option<&Config>, common: &Path) -> PathBuf {
    match cfg.and_then(|c| c.runners.lock_dir.as_deref()) {
        Some(d) => config::expand_tilde(d),
        None => common.join("ns").join("locks"),
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
            &repo, fac, root, gate, &unit, args, &issue_url, &gates, &commands,
        );
    }

    let _lock = Lock::acquire(&repo.common_dir, &unit, args.issue)?;
    let base = resolve_base(&repo, args)?;
    let wt = worktree::ensure(&repo, &unit, Some(&base), &fac.worktree.setup)?;
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
    };
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
                "cost_usd": shared.spent_usd,
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
        "cost_usd": shared.spent_usd,
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
        "pr": pr,
        "cost_usd": cost,
        "reset_at": result.reset_at,
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
    gate: &Option<String>,
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
        "gate": ctx.gate,
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
    let mut last_failure: BTreeMap<&'static str, String> = BTreeMap::new();
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
            let mut reason = format!("{phase} is out of attempts ({})", p.max_attempts);
            if let Some(t) = last_failure.get(phase) {
                reason.push_str(&format!(": {t}"));
            }
            return Ok(finish(Outcome::Stuck, reason, Some(phase)));
        }
        if let Some(reason) = budget_spent(fac.budget_usd(), shared.spent_usd) {
            return Ok(finish(Outcome::Budget, reason, Some(phase)));
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
        let held = match recheck_budget(held, fac.budget_usd(), shared.spent_usd, phase) {
            Ok(held) => held,
            Err(f) => return Ok(f),
        };
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
        let env = [
            ("NS_UNIT", ctx.unit.clone()),
            ("NS_PHASE", phase.to_string()),
            ("NS_ATTEMPT", attempt.to_string()),
            ("NS_WORKTREE", ctx.worktree.to_string_lossy().into_owned()),
            ("NS_RUN_PID", std::process::id().to_string()),
            (
                "NS_WATCH_PID",
                shared.watch_pid.map(|p| p.to_string()).unwrap_or_default(),
            ),
        ];
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
            (phase == "review" && head_moved).then_some(gate::Trigger::ReviewMovedHead)
        } else {
            gate_trigger(phase, art.as_ref(), head_moved)
        };
        if let (Some(cmd), Some(trigger)) = (&ctx.gate, trigger) {
            let job = gate::Job {
                cmd,
                worktree: &ctx.worktree,
                log_dir: ctx.common.join("ns").join("transcripts").join(&ctx.unit),
                unit: &ctx.unit,
                timeout: gate::timeout(ctx.fac.phase("build").timeout_minutes),
                head: ctx.head(),
                phase,
                attempt,
                cap: ctx.cap,
            };
            let red = gate_state.after(trigger, &job, &|ev| ctx.log(ev));
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
                if let Some(k) = red.killed {
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
        ("review", _) if head_moved => Some(gate::Trigger::ReviewMovedHead),
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
    Unchanged,
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
            return Ok(UpdatedHead::Unchanged);
        }
        clock.sleep_until((now + wait).min(deadline))?;
        wait = (wait * 2).min(REGISTER_BACKOFF_MAX);
    }
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

/// `merge.policy = "auto"`: squash-merge this unit's PR when CI is green, review.md passed and is
/// current at the PR head, and no file or marked region that needs human review changed. The only
/// place ns merges anything.
fn merge_step(ctx: &Ctx<'_>, state: &State, shared: &Shared) -> Result<MergeStep> {
    if !state.arts.contains_key("ship") {
        return Ok(human("no pr.md"));
    }
    let Some(n) = known_pr(&ctx.artifacts) else {
        return Ok(human("pr.md has no pr: number; needs a human merge"));
    };
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let view = gh_json(wt, &["pr", "view", &ns, "--json", "state,headRefOid"])?;
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
    if !state.current(pr_head) {
        return Ok(human(format!(
            "PR head {pr_head} is not current with HEAD {head}; needs a human merge"
        )));
    }
    if !state.arts.get("review").is_some_and(|r| state.passes(r)) {
        return Ok(human(
            "review.md is not pass and current; needs a human merge",
        ));
    }

    // One merge at a time per repo, so a PR is updated and checked against the main the merge
    // before it left. Its merge state is read once the lock is held.
    let _merging =
        match take_merge_lock(&ctx.common, &ctx.unit, &shared.clock, shared.until, |ev| {
            ctx.log(ev)
        })? {
            Ok(lock) => lock,
            Err(f) => return Ok(MergeStep::Finish(f)),
        };

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
    let register_minutes = ctx.fac.merge.ci_register_timeout;
    let mut merge_head = pr_head.to_string();
    match merge_state(wt, &ns, register_minutes, &shared.clock)?.as_str() {
        Some("DIRTY") => return Ok(conflict()),
        Some("BEHIND") => {
            if gh(wt, &["pr", "update-branch", &ns]).is_err() {
                return Ok(conflict());
            }
            match head_after_update(wt, &ns, pr_head, register_minutes, &shared.clock)? {
                UpdatedHead::Moved(h) => merge_head = h,
                UpdatedHead::Dirty => return Ok(conflict()),
                UpdatedHead::Unchanged => {
                    return Ok(human(format!(
                        "PR #{n} head {pr_head} did not change after update-branch within {register_minutes} min; needs a human merge"
                    )));
                }
            }
            ctx.log(json!({"event": "update_branch", "pr": n}));
        }
        _ => {}
    }

    match checks_registered(wt, &merge_head, register_minutes, &shared.clock)? {
        Registered::Yes => {}
        Registered::No => {
            return Ok(human(format!(
                "no CI checks registered within {register_minutes} min on PR #{n}; needs a human merge"
            )));
        }
        Registered::QueryFailed(err) => {
            return Ok(human(format!(
                "could not query CI checks on PR #{n}: {err}; needs a human merge"
            )));
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
    match marked_regions(wt, &default) {
        Ok(regions) if regions.is_empty() => {}
        Ok(regions) => {
            ctx.log(json!({"event": "human_review", "pr": n, "regions": regions}));
            return Ok(human(format!(
                "changes code in a human-review region; needs a human merge ({})",
                regions.join(", ")
            )));
        }
        Err(e) => {
            return Ok(human(format!(
                "cannot check human-review regions: {e:#}; needs a human merge"
            )))
        }
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
    use super::*;

    #[test]
    fn lock_dir_defaults_under_the_common_dir_and_expands_a_tilde() {
        let common = Path::new("/repo/.git");
        assert_eq!(lock_dir(None, common), common.join("ns").join("locks"));
        let mut cfg = Config::default();
        assert_eq!(
            lock_dir(Some(&cfg), common),
            common.join("ns").join("locks")
        );
        cfg.runners.lock_dir = Some("~/shared/locks".into());
        assert_eq!(
            lock_dir(Some(&cfg), common),
            config::expand_tilde("~/shared/locks")
        );
        assert!(!lock_dir(Some(&cfg), common).starts_with(common));
        cfg.runners.lock_dir = Some("/abs/locks".into());
        assert_eq!(lock_dir(Some(&cfg), common), Path::new("/abs/locks"));
    }

    #[test]
    fn acquire_takes_a_repeated_name_once() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = ["b", "a", "b"].map(String::from).into();
        let clock = Clock::pinned(1000);
        let held = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, None, |lock, _| {
            panic!("waited on {lock}")
        })
        .unwrap()
        .held();
        assert_eq!(held.0.len(), 2);
    }

    const MIN: Duration = Duration::from_secs(60);

    impl Acquired {
        fn held(self) -> RunnerLocks {
            match self {
                Acquired::Held(h) => h,
                Acquired::GaveUp(g) => panic!("gave up: {}", g.reason()),
            }
        }

        fn gave_up(self) -> GaveUp {
            match self {
                Acquired::GaveUp(g) => g,
                Acquired::Held(_) => panic!("took the locks"),
            }
        }
    }

    struct Hold(File);

    impl Drop for Hold {
        fn drop(&mut self) {
            unlock_flock(&self.0);
        }
    }

    /// Another process's hold on `<dir>/<name>.lock`, naming pid 42 and unit `other`.
    fn hold(dir: &Path, name: &str) -> Hold {
        let path = dir.join(format!("{name}.lock"));
        fs::write(&path, "{\"pid\":42,\"unit\":\"other\"}\n").unwrap();
        let f = File::open(&path).unwrap();
        assert!(try_flock(&f).unwrap());
        Hold(f)
    }

    fn is_free(dir: &Path, name: &str) -> bool {
        let f = File::open(dir.join(format!("{name}.lock"))).unwrap();
        let free = try_flock(&f).unwrap();
        unlock_flock(&f);
        free
    }

    #[test]
    fn acquire_gives_up_at_the_phase_timeout_and_releases_what_it_took() {
        let dir = tempfile::tempdir().unwrap();
        let _b = hold(dir.path(), "b");
        let clock = Clock::pinned(1000);
        let names: Vec<String> = ["b", "a"].map(String::from).into();
        let mut waits = Vec::new();
        let g = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, Some(2000), |l, by| {
            waits.push((l.to_string(), by.clone()))
        })
        .unwrap()
        .gave_up();
        assert_eq!(clock.now(), 1060);
        assert_eq!((g.lock.as_str(), g.bound), ("b", Bound::Timeout));
        assert_eq!(g.held_by, json!({"pid": 42, "unit": "other"}));
        assert_eq!(waits, [("b".to_string(), g.held_by.clone())]);
        assert!(is_free(dir.path(), "a"));
        assert_eq!(fs::read_to_string(dir.path().join("a.lock")).unwrap(), "");
        assert_eq!(
            g.reason(),
            "gave up waiting for lock b (held by pid 42, unit other) at the phase timeout"
        );
    }

    #[test]
    fn acquire_gives_up_at_until_when_it_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        let _a = hold(dir.path(), "a");
        let clock = Clock::pinned(1000);
        let names = vec!["a".to_string()];
        let g = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, Some(1030), |_, _| {})
            .unwrap()
            .gave_up();
        assert_eq!(clock.now(), 1030);
        assert_eq!(g.bound, Bound::Until);
        assert_eq!(
            g.reason(),
            "gave up waiting for lock a (held by pid 42, unit other) at --until"
        );
    }

    #[test]
    fn acquire_counts_a_partial_second_of_timeout_as_a_whole_one() {
        let dir = tempfile::tempdir().unwrap();
        let _a = hold(dir.path(), "a");
        let clock = Clock::pinned(1000);
        let names = vec!["a".to_string()];
        let timeout = Duration::from_millis(1500);
        let g = RunnerLocks::acquire(
            dir.path(),
            &names,
            "u",
            &clock,
            timeout,
            Some(1002),
            |_, _| {},
        )
        .unwrap()
        .gave_up();
        assert_eq!((clock.now(), g.bound), (1002, Bound::Until));
    }

    #[test]
    fn acquire_takes_a_lock_its_holder_releases_in_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Some(hold(dir.path(), "a"));
        let clock = Clock::pinned(1000);
        let names = vec!["a".to_string()];
        let held = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, None, |_, _| {
            a.take();
        })
        .unwrap()
        .held();
        let other = File::open(dir.path().join("a.lock")).unwrap();
        assert!(!try_flock(&other).unwrap());
        let text = fs::read_to_string(dir.path().join("a.lock")).unwrap();
        assert!(
            text.contains(&format!("\"pid\":{}", std::process::id())),
            "{text}"
        );
        drop(held);
        assert!(is_free(dir.path(), "a"));
    }

    #[test]
    fn acquire_times_out_from_when_the_first_wait_began() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Some(hold(dir.path(), "a"));
        let _b = hold(dir.path(), "b");
        let clock = Clock::pinned(1000);
        let names: Vec<String> = ["a", "b"].map(String::from).into();
        let g = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, None, |l, _| {
            if l == "a" {
                clock.sleep_until(1030).unwrap();
                a.take();
                assert!(is_free(dir.path(), "a"));
            }
        })
        .unwrap()
        .gave_up();
        assert_eq!((g.lock.as_str(), g.bound), ("b", Bound::Timeout));
        assert_eq!(clock.now(), 1060);
    }

    #[test]
    fn acquire_gives_up_when_until_passed_while_it_waited() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = Some(hold(dir.path(), "b"));
        let clock = Clock::pinned(1000);
        let names: Vec<String> = ["a", "b"].map(String::from).into();
        let g = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, Some(1030), |_, _| {
            clock.sleep_until(1030).unwrap();
            b.take();
        })
        .unwrap()
        .gave_up();
        assert_eq!((g.lock.as_str(), g.bound), ("b", Bound::Until));
        assert!(is_free(dir.path(), "a"));
        assert!(is_free(dir.path(), "b"));
    }

    #[test]
    fn polling_backs_off_one_two_four_then_five_seconds() {
        let clock = Clock::pinned(1000);
        let mut seen = Vec::new();
        let got = poll_until(&clock, 1 << 40, || {
            seen.push(clock.now());
            Ok(seen.len() == 6)
        })
        .unwrap();
        assert!(got);
        assert_eq!(seen, [1000, 1001, 1003, 1007, 1012, 1017]);
    }

    #[test]
    fn a_polling_loop_that_never_succeeds_fails_on_a_pinned_clock() {
        let clock = Clock::pinned(1000);
        let mut tries = 0;
        let err = poll_until(&clock, 1 << 40, || {
            tries += 1;
            Ok(false)
        })
        .unwrap_err();
        assert_eq!(tries, MAX_PINNED_POLLS + 1);
        assert!(err.to_string().contains("pinned clock (NS_NOW)"), "{err}");
    }

    #[test]
    fn polling_at_the_bound_tries_once_and_does_not_sleep() {
        let clock = Clock::pinned(1000);
        let mut tries = 0;
        let got = poll_until(&clock, 1000, || {
            tries += 1;
            Ok(false)
        })
        .unwrap();
        assert!(!got);
        assert_eq!((tries, clock.now()), (1, 1000));
    }

    #[test]
    fn polling_never_sleeps_past_the_bound() {
        let clock = Clock::pinned(1000);
        assert!(!poll_until(&clock, 1004, || Ok(false)).unwrap());
        assert_eq!(clock.now(), 1004);
    }

    #[test]
    fn a_partial_second_of_timeout_rounds_up() {
        let t = Duration::from_millis(1500);
        assert_eq!(give_up_at(1000, t, None), (1002, Bound::Timeout));
        assert_eq!(give_up_at(1000, MIN, None), (1060, Bound::Timeout));
    }

    #[test]
    fn a_tie_between_the_timeout_and_until_goes_to_until() {
        assert_eq!(give_up_at(1000, MIN, Some(1060)), (1060, Bound::Until));
        assert_eq!(give_up_at(1000, MIN, Some(1061)), (1060, Bound::Timeout));
        assert_eq!(give_up_at(1000, MIN, Some(1059)), (1059, Bound::Until));
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

    #[test]
    fn a_lock_wait_takes_the_lock_once_its_holder_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let mut other = Some(hold(dir.path(), "ns-merge"));
        let mut waits = Vec::new();
        let me = json!({"pid": 1, "unit": "u"});
        let lock = Lock::wait(dir.path(), MERGE_LOCK, me, None, |by| {
            waits.push(by.clone());
            other = None;
        })
        .unwrap()
        .unwrap();
        assert_eq!(waits, [json!({"pid": 42, "unit": "other"})]);
        let path = dir.path().join(MERGE_LOCK);
        assert_eq!(read_holder(&path), json!({"pid": 1, "unit": "u"}));
        assert!(!is_free(dir.path(), "ns-merge"));
        drop(lock);
        assert!(is_free(dir.path(), "ns-merge"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn a_free_lock_is_taken_without_a_wait() {
        let dir = tempfile::tempdir().unwrap();
        let me = json!({"pid": 1, "unit": "u"});
        let lock = Lock::wait(dir.path(), WORKTREE_LOCK, me, None, |_| panic!("waited")).unwrap();
        assert!(lock.is_ok());
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

    #[test]
    fn run_holders_lists_each_held_run_lock_by_unit() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Lock::run_holders(&dir.path().join("missing"))
            .unwrap()
            .is_empty());
        let _b = hold(dir.path(), "ns-run-b-2");
        // Taken, but its record not yet written.
        let _a = hold(dir.path(), "ns-run-a-1");
        fs::write(dir.path().join("ns-run-a-1.lock"), "").unwrap();
        fs::write(
            dir.path().join("ns-run-c.lock"),
            "{\"pid\":7,\"unit\":\"c\"}",
        )
        .unwrap();
        let _bad = hold(dir.path(), "ns-run-Bad");
        let _watch = hold(dir.path(), "ns-watch");
        let _merge = hold(dir.path(), "ns-merge");
        assert_eq!(
            Lock::run_holders(dir.path()).unwrap(),
            [
                json!({"pid": null, "unit": "a-1", "issue": null}),
                json!({"pid": 42, "unit": "other"}),
            ]
        );
    }

    #[test]
    fn a_run_lock_is_per_unit() {
        let dir = tempfile::tempdir().unwrap();
        let a = Lock::acquire(dir.path(), "a", Some(1)).unwrap();
        let _b = Lock::acquire(dir.path(), "b", None).unwrap();
        let err = Lock::acquire(dir.path(), "a", None).err().unwrap();
        let sf = err.downcast_ref::<SfError>().unwrap();
        assert_eq!(sf.code, EXIT_LOCKED);
        assert!(
            err.to_string().starts_with(&format!(
                "another ns run (pid {}, unit a) holds ",
                std::process::id()
            )),
            "{err}"
        );
        drop(a);
        Lock::acquire(dir.path(), "a", None).unwrap();
    }
}
