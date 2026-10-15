//! `ns run`: drive one unit through the phases (docs/factory/run.md).

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::billing;
use crate::clock::{self, Clock};
use crate::config::{self, Config};
use crate::error::SfError;
use crate::factory::{self, Factory, PhaseSettings, PHASES};
use crate::forge;
use crate::gate;
use crate::git::{self, Repo};
use crate::memcap::{self, Cap};
use crate::pin;
use crate::watch::NightDir;
use crate::worktree;

mod ci_gate;
mod core;
pub(crate) mod currency;
mod drive;
mod locks;
mod merge;
mod phase_run;
mod phases;
mod post_phase;

use currency::known_pr;
use drive::{drive, start_ledger};
use locks::lock_dir;
pub use locks::{holder_pid, run_lock, Lock, WORKTREE_LOCK};

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
    /// Start a new ledger instead of resuming `.ns/<unit>/state.json` (`--fresh`).
    pub fresh: bool,
    /// The night directory of the `ns watch` that started this run (`--night`).
    pub night: Option<PathBuf>,
    /// The CPU share every phase and gate gets (`--cpus`), or `None` to set nothing.
    pub cpus: Option<u32>,
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
    /// What `--cpus` set for every phase and gate, for the run log.
    pub cpus: Option<Value>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            spent_usd: 0.0,
            clock: Clock::from_env(),
            watch_pid: None,
            until: None,
            night: None,
            cpus: None,
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
    shared.cpus = args
        .cpus
        .map(|n| crate::cpus::log_value(crate::cpus::activate(n)));
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

// ---------------------------------------------------------------- gh and git helpers

pub fn gh(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = crate::proc::Proc::forge("gh")
        .args(args)
        .current_dir(cwd)
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
    let text = git::run_net(dir, &["ls-remote", "--symref", "origin", "HEAD"]).ok()?;
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
    let text = git::run_net(
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
    gate: GateSettings,
    env: PhaseEnv<'a>,
}

/// How the CI gate runs.
struct GateSettings {
    /// The CI gate commands; `None` runs no gate.
    commands: gate::Commands,
    /// The night directory of the `ns watch` this runs under.
    night: Option<PathBuf>,
}

/// What a phase runs under besides its prompt and command.
struct PhaseEnv<'a> {
    lock_dir: PathBuf,
    /// The memory cap phases and the gates after them run under.
    cap: Option<&'a Cap>,
    /// The `ns` binary the phase settings' hook runs.
    ns: PathBuf,
    /// The directory of this unit's phase `TMPDIR`s, `Repo::unit_tmp`.
    tmp: PathBuf,
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
        let link = crate::watch::forge::open(&fac.queue, &repo.root)
            .map_err(anyhow::Error::from)
            .and_then(|f| f.issue_link(n));
        match link {
            Ok(l) => {
                issue_url = l.url;
                if unit.is_none() {
                    unit = Some(unit_name(n, &l.title));
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
    let ensure = || worktree::ensure(&repo, &unit, Some(&base), &fac.worktree.setup);
    let wt = if shared.night.is_some() {
        ensure()?
    } else {
        crate::stop::scoped(ensure).context("cannot handle SIGINT and SIGTERM")??
    };
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
        gate: GateSettings {
            commands: gate.clone(),
            night: shared.night.as_ref().map(|n| n.path().to_path_buf()),
        },
        env: PhaseEnv {
            lock_dir: lock_dir(cfg.as_ref(), &repo.common_dir),
            cap: cap.as_ref(),
            ns: ns.clone(),
            tmp: repo.unit_tmp(&unit),
        },
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
        crate::clean::after_merge(
            &repo,
            &ctx.fac.queue,
            &unit,
            &ctx.worktree,
            issue,
            pr,
            record_saved,
        )
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
    let diffs = existing.is_some() && base.is_some();
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
        gate: GateSettings {
            commands: gate.clone(),
            night: None,
        },
        env: PhaseEnv {
            lock_dir: PathBuf::new(),
            cap: None,
            ns: PathBuf::new(),
            tmp: PathBuf::new(),
        },
    };
    // Decide as `drive` would at its first step, through the same `core::step`, without its log
    // or ledger writes.
    let (mut ledger, _) = start_ledger(args, &ctx.artifacts, &head);
    let state = drive::unit_state(&ctx, &ledger, head, diffs, drive::settings(fac, args));
    let mut action = core::step::step(&mut ledger, &state, core::step::Event::Start);
    if let core::step::Action::ReadRemote = action {
        let remote = existing.as_deref().and_then(remote_default);
        action = core::step::step(&mut ledger, &state, core::step::Event::Remote(remote));
    }
    let (decision_json, prompt, command) = dry_run_decision(&ctx, commands, action)?;
    let json = json!({
        "unit": unit,
        "dry_run": true,
        "worktree": wt.to_string_lossy(),
        "worktree_exists": existing.is_some(),
        "decision": decision_json,
        "prompt": prompt,
        "command": command,
        "gate": ctx.gate.commands.build,
        "gate_rest": ctx.gate.commands.rest,
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

/// What a dry run reports for the first action `core::step` returns: the decision, and the
/// prompt and command of a phase it would run.
fn dry_run_decision(
    ctx: &Ctx<'_>,
    commands: &BTreeMap<&'static str, Vec<String>>,
    action: core::step::Action,
) -> Result<(Value, Option<String>, Option<Vec<String>>)> {
    use core::step::Action;
    Ok(match action {
        Action::RunPhase(a) => {
            let p = ctx.fac.phase(a.phase);
            (
                json!({"action": "run", "phase": a.phase, "attempt": a.attempt, "why": a.why}),
                Some(ctx.prompt(&p, a.attempt, &a.prompt_feedback())?),
                Some(commands[a.phase].clone()),
            )
        }
        Action::Merge { .. } => (json!({"action": "done"}), None, None),
        Action::Finish(e) => {
            let reason = (e.outcome != Outcome::Done).then_some(e.reason);
            let mut v = json!({"action": e.outcome.label()});
            if let Some(r) = reason {
                v["reason"] = json!(r);
            }
            (v, None, None)
        }
        other => (json!({"action": format!("{other:?}")}), None, None),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::merge::marked_regions;
    use super::*;

    /// A repo cloned from a bare `origin` whose `main` holds plain `f.c` and `g.c`.
    pub(crate) fn cloned() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let (origin, wt) = (t.path().join("origin.git"), t.path().join("wt"));
        let g = crate::testutil::git_any;
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

    pub(super) fn commit_all(dir: &Path, msg: &str) {
        git::run(dir, &["add", "-A"]).unwrap();
        git::run(dir, &["commit", "-q", "-m", msg]).unwrap();
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
}
