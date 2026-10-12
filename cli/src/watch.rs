//! `ns watch`: pull ready issues from GitHub and run them, each as its own `ns run`, up to
//! `--parallel` at once (docs/factory/watch.md).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::clock;
use crate::error::SfError;
use crate::factory::{self, Factory, Queue as QueueConfig};
use crate::git::{self, Repo};
use crate::install;
use crate::pin;
use crate::review_md::{self, Status};
use crate::run::{self, Outcome, RunResult, Shared};
use crate::skills_sync::{self, Sync};
use crate::stop;
use crate::worktree::BRANCH_PREFIX;

pub mod ci;
mod fix_main;
pub mod forge;
mod scheduler;
use scheduler::overlap;
mod self_update;
mod snapshot;
mod split;
mod triage;

use forge::{ClosingPrs, Forge, Post, PrState};
pub use snapshot::NightDir;

pub struct WatchArgs {
    pub once: bool,
    pub until: Option<String>,
    pub max_units: Option<u32>,
    pub parallel: Option<u32>,
    pub dry_run: bool,
    pub factory: Option<PathBuf>,
    pub no_self_update: bool,
    /// The night so far, from the `ns` that handed off to this one.
    pub resume_night: Option<PathBuf>,
}

/// Seconds to wait after a usage limit with no reset time in the message.
const PAUSE_RETRY_S: i64 = 30 * 60;

#[derive(Debug, Clone)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    pub body: String,
    pub team: bool,
}

/// Issue numbers named on a first line `Blocked by: #a, #b`. An agent's `🤖` disclosure line
/// before it doesn't count (skills/ns-contract, Disclosure).
pub fn blockers(body: &str) -> Vec<u64> {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines
        .next()
        .filter(|l| !l.starts_with('🤖'))
        .or_else(|| lines.next())
        .unwrap_or("");
    let lower = first.to_lowercase();
    let Some(rest) = lower.strip_prefix("blocked by:") else {
        return Vec::new();
    };
    let re = regex::Regex::new(r"#(\d+)").unwrap();
    re.captures_iter(rest)
        .filter_map(|c| c[1].parse().ok())
        .collect()
}

/// Issues an open PR body closes (`Closes #n`, also fixes/resolves).
pub fn closed_by(body: &str) -> Vec<u64> {
    let re =
        regex::Regex::new(r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(\d+)").unwrap();
    re.captures_iter(body)
        .filter_map(|c| c[1].parse().ok())
        .collect()
}

fn rank(order: &[String], labels: &[String]) -> usize {
    order
        .iter()
        .position(|o| labels.contains(o))
        .unwrap_or(order.len())
}

pub(crate) struct Queue {
    pub(crate) ready: Vec<Issue>,
    skipped: Vec<Value>,
}

/// Open issues, never PRs, and the open and merged PRs that close each.
struct Listing {
    issues: Vec<Issue>,
    has_pr: ClosingPrs,
    merged_pr: ClosingPrs,
}

impl Listing {
    /// `labels` narrows the list to issues carrying that label.
    fn read(forge: &dyn Forge, labels: Option<&str>) -> Result<Listing> {
        Ok(Listing {
            issues: forge.open_issues(labels)?,
            has_pr: forge.closing_prs(PrState::Open)?,
            merged_pr: forge.closing_prs(PrState::Merged)?,
        })
    }

    /// Why `ns watch` leaves the issue alone: its author is outside the team, or a PR closes it.
    /// With `resume_own`, an open PR from the issue's own unit branch doesn't count: that unit
    /// stopped before its merge step ended, and `ns run` resumes it there.
    fn passed_over(&self, i: &Issue, resume_own: bool) -> Option<String> {
        if !i.team {
            return Some("author outside the team".into());
        }
        if let Some((pr, head)) = self.has_pr.get(&i.number) {
            if !(resume_own && *head == format!("{BRANCH_PREFIX}{}", unit_id(i))) {
                return Some(format!("open PR #{pr} closes it"));
            }
        }
        self.merged_pr
            .get(&i.number)
            .map(|(pr, _)| format!("merged PR #{pr} closes it"))
    }
}

/// The unit `ns watch` runs for the issue, `<n>-<slug of the title>` or `<n>` for a title with
/// no slug: the name `ns run --issue <n>` would read from the forge.
fn unit_id(i: &Issue) -> String {
    run::unit_name(i.number, &i.title)
}

fn skip(i: &Issue, reason: impl Into<Value>) -> Value {
    json!({"number": i.number, "title": i.title, "reason": reason.into()})
}

/// The queue's sort key: higher priority first, then category, then the oldest issue.
fn order(q: &QueueConfig, i: &Issue) -> (usize, usize, u64) {
    (
        rank(&q.priority, &i.labels),
        rank(&q.order, &i.labels),
        i.number,
    )
}

/// Ready issues, minus those in `finished`: GitHub can list an issue as open
/// for a few seconds after its closing PR merges.
pub(crate) fn queue(forge: &dyn Forge, fac: &Factory, finished: &BTreeSet<u64>) -> Result<Queue> {
    let q = &fac.queue;
    let listing = Listing::read(forge, Some(&q.ready_label))?;
    let mut open_cache: BTreeMap<u64, bool> = BTreeMap::new();
    let mut ready = Vec::new();
    let mut skipped = Vec::new();
    for i in &listing.issues {
        if finished.contains(&i.number) {
            continue;
        }
        if let Some(why) = listing.passed_over(i, true) {
            skipped.push(skip(i, why));
            continue;
        }
        let mut open_blockers = Vec::new();
        for b in blockers(&i.body) {
            let open = match open_cache.get(&b) {
                Some(o) => *o,
                None => {
                    // An unreadable blocker counts as open: never start work that may be blocked.
                    let o = !forge.is_closed(b).unwrap_or(false);
                    open_cache.insert(b, o);
                    o
                }
            };
            if open {
                open_blockers.push(format!("#{b}"));
            }
        }
        if !open_blockers.is_empty() {
            skipped.push(skip(
                i,
                format!("blocked by open {}", open_blockers.join(", ")),
            ));
            continue;
        }
        ready.push(i.clone());
    }
    ready.sort_by_key(|i| order(q, i));
    Ok(Queue { ready, skipped })
}

const LABEL_EDIT_ATTEMPTS: u32 = 2;

struct StatusDrift {
    stray: Vec<String>,
    missing: bool,
}

impl StatusDrift {
    fn is_clean(&self) -> bool {
        self.stray.is_empty() && !self.missing
    }
}

/// A state label: any `status:` label, or one of the queue's own.
fn is_status(q: &QueueConfig, label: &str) -> bool {
    label.starts_with("status:")
        || [
            &q.ready_label,
            &q.in_progress_label,
            &q.done_label,
            &q.stuck_label,
            &q.split_label,
            &q.triage_label,
        ]
        .iter()
        .any(|o| *o == label)
}

fn status_drift(
    forge: &dyn Forge,
    q: &QueueConfig,
    n: u64,
    end: Option<&str>,
) -> Result<StatusDrift> {
    let labels = forge.labels(n)?;
    let stray = labels
        .iter()
        .filter(|l| Some(l.as_str()) != end && is_status(q, l))
        .cloned()
        .collect();
    Ok(StatusDrift {
        stray,
        missing: end.is_some_and(|e| !labels.iter().any(|l| l == e)),
    })
}

fn set_status(forge: &dyn Forge, q: &QueueConfig, n: u64, end: Option<&str>) -> Result<()> {
    for _ in 0..LABEL_EDIT_ATTEMPTS {
        let drift = status_drift(forge, q, n, end)?;
        if drift.is_clean() {
            return Ok(());
        }
        forge.edit_labels(n, &drift.stray, end.filter(|_| drift.missing))?;
    }
    let drift = status_drift(forge, q, n, end)?;
    if drift.is_clean() {
        return Ok(());
    }
    let mut problems = Vec::new();
    if !drift.stray.is_empty() {
        problems.push(format!(
            "has stray status labels {}",
            drift.stray.join(", ")
        ));
    }
    if let Some(e) = end.filter(|_| drift.missing) {
        problems.push(format!("is missing {e}"));
    }
    Err(SfError::general(format!(
        "#{n} {} after {LABEL_EDIT_ATTEMPTS} label edits (want {})",
        problems.join(" and "),
        end.unwrap_or("no status label")
    ))
    .into())
}

/// Open issues carrying `in_progress_label`: those no live `ns run` holds, left so by a night
/// that ended mid-unit, and those one does, each with its run lock's holder.
struct InProgress {
    stale: Vec<Issue>,
    held: Vec<(Issue, Value)>,
}

fn in_progress(repo: &Repo, forge: &dyn Forge, q: &QueueConfig) -> Result<InProgress> {
    let holders = run::Lock::run_holders(&repo.common_dir)?;
    let mut found = InProgress {
        stale: Vec::new(),
        held: Vec::new(),
    };
    for i in forge.open_issues(Some(&q.in_progress_label))? {
        if !i.labels.contains(&q.in_progress_label) {
            continue;
        }
        match holders.iter().find(|h| holds(h, i.number)) {
            Some(h) => found.held.push((i, h.clone())),
            None => found.stale.push(i),
        }
    }
    found.stale.sort_by_key(|i| i.number);
    Ok(found)
}

/// Whether a run lock's `holder` is running issue `n`'s unit.
fn holds(holder: &Value, n: u64) -> bool {
    if holder["issue"].as_u64() == Some(n) {
        return true;
    }
    holder["unit"]
        .as_str()
        .is_some_and(|u| u == n.to_string() || u.starts_with(&format!("{n}-")))
}

/// Put each stale in-progress issue back to `ready_label`, keeping its worktree and artifacts so
/// its unit resumes, and push its number onto `requeued`.
fn requeue_stale(
    repo: &Repo,
    forge: &dyn Forge,
    q: &QueueConfig,
    requeued: &mut Vec<u64>,
) -> Result<()> {
    let found = in_progress(repo, forge, q)?;
    for (i, h) in &found.held {
        eprintln!(
            "ns watch: #{} is held by a live ns run (pid {}, unit {}); left in progress",
            i.number,
            run::holder_pid(h),
            h["unit"].as_str().unwrap_or("?")
        );
    }
    for i in &found.stale {
        set_status(forge, q, i.number, Some(&q.ready_label))?;
        eprintln!(
            "ns watch: #{} was in progress with no live ns run; back to {}",
            i.number, q.ready_label
        );
        run::log_event(
            &repo.common_dir,
            json!({"event": "requeued", "issue": i.number, "reason": "in progress with no live ns run"}),
        );
        requeued.push(i.number);
    }
    Ok(())
}

/// `git fetch origin` and the `origin/<default>` ref to base new worktrees on.
fn fresh_base(root: &Path) -> Option<String> {
    let (branch, _) = run::remote_default(root)?;
    git::run(root, &["fetch", "--quiet", "origin"]).ok()?;
    Some(format!("origin/{branch}"))
}

/// Whether a phase of `fac` loads the installed skills, which a sync changes under it: a codex
/// phase (docs/factory/run.md, "Pinned skills and phase settings").
fn reads_installed_skills(fac: &Factory) -> bool {
    factory::PHASES
        .iter()
        .any(|p| pin::mode(&fac.phase(p).harness) == pin::Mode::Partial)
}

/// The installed skills whose symlinks resolve into the main checkout.
fn installed_skills(repo: &Repo) -> skills_sync::Installed {
    let targets = install::targets(&install::Harness::ALL, Vec::new());
    skills_sync::installed_into(&targets, &repo.root)
}

/// The commit at `base` to drain for, when it has skill changes the main checkout lacks and is
/// not in `tried`, announced on stderr and in the run log.
fn skills_drain(repo: &Repo, base: &str, tried: &BTreeSet<String>) -> Option<String> {
    let (from, to) = skills_sync::pending(&repo.root, base, &installed_skills(repo))?;
    if tried.contains(&to) {
        return None;
    }
    eprintln!(
        "ns watch: {base} has skill changes the skills checkout lacks ({} -> {}); draining: no new unit until the running ones end",
        self_update::short(&from),
        self_update::short(&to)
    );
    run::log_event(
        &repo.common_dir,
        json!({"event": "skills_drain", "from": from, "to": to}),
    );
    Some(to)
}

/// Fast-forward the main checkout when installed skills resolve into it, so the unit runs
/// the skills merged so far tonight. A checkout that can't move is reported, not changed;
/// `warned` holds the last warning so a checkout that stays stuck warns once. `issue` is the
/// unit the sync runs before, `None` after a drain.
fn sync_skills(repo: &Repo, base: &str, issue: Option<u64>, warned: &mut Option<String>) {
    let installed = installed_skills(repo);
    let short =
        |sha: &str| git::run(&repo.root, &["rev-parse", "--short", sha]).unwrap_or_default();
    let mut ev = json!({"issue": issue, "checkout": repo.root, "skills": installed.names});
    match skills_sync::sync(&repo.root, base, &installed) {
        Sync::NotInstalled | Sync::Current => return,
        Sync::Updated { from, to } => {
            eprintln!(
                "ns watch: skills checkout {} fast-forwarded {} -> {}",
                repo.root.display(),
                short(&from),
                short(&to)
            );
            ev["event"] = json!("skills_synced");
            ev["from"] = json!(from);
            ev["to"] = json!(to);
            match skills_sync::relink(&installed) {
                Ok(changed) => {
                    for c in &changed {
                        eprintln!("ns watch: skills {c}");
                    }
                    ev["relinked"] = json!(changed);
                }
                Err(e) => {
                    eprintln!("ns watch: warning: could not relink skills: {e:#}");
                    ev["relink_error"] = json!(format!("{e:#}"));
                }
            }
            *warned = None;
        }
        Sync::Stale { reason } => {
            if warned.as_deref() == Some(reason.as_str()) {
                return;
            }
            eprintln!(
                "ns watch: warning: installed skills may be stale ({}): {reason}",
                installed.names.join(", ")
            );
            ev["event"] = json!("skills_stale");
            ev["reason"] = json!(reason);
            *warned = Some(reason);
        }
    }
    run::log_event(&repo.common_dir, ev);
}

pub fn run(args: WatchArgs) -> Result<ExitCode> {
    let start = std::env::current_dir().context("cannot read current directory")?;
    let repo = Repo::discover(&start)?;
    // Read once: a file broken between units must not end the night.
    let mut loaded = run::Loaded::read(args.factory.as_deref(), args.dry_run)?;
    let fac = &loaded.fac;
    for (what, f) in [
        ("config", &loaded.files["config"]),
        ("factory", &loaded.files["factory"]),
    ] {
        eprintln!(
            "ns watch: {what} {} sha256 {}",
            f["path"].as_str().unwrap_or(""),
            f["sha256"].as_str().unwrap_or("none (file missing)")
        );
    }
    let q = fac.queue.clone();
    let forge = forge::open(&q, &repo.root)?;
    let forge = forge.as_ref();
    let mut night = Night::new();
    night.skills_drain = reads_installed_skills(fac);
    let deadline = match &args.until {
        Some(s) => {
            let Some((h, m)) = clock::parse_hm(s) else {
                return Err(SfError::usage(
                    format!("--until {s:?} is not HH:MM"),
                    "ns watch --until 06:30",
                )
                .into());
            };
            Some(clock::next_local(night.shared.clock.now(), h, m))
        }
        None => None,
    };
    // A hand-off carries the night on: its deadline wins over --until read again now. A state
    // this ns can't read starts a fresh night rather than ending the one the old ns began, but
    // fails a dry run, which is how the old ns checks this ns before it hands off.
    let carried = match args.resume_night.as_deref().map(self_update::Carried::take) {
        Some(Ok(c)) => Some(c),
        Some(Err(e)) if args.dry_run => return Err(e),
        Some(Err(e)) => {
            eprintln!("ns watch: warning: starting a fresh night: {e:#}");
            None
        }
        None => None,
    };
    let deadline = carried.as_ref().map_or(deadline, |c| c.deadline);
    night.shared.until = deadline;
    let max_units = if args.once {
        Some(1)
    } else {
        args.max_units.or(fac.limits.max_units)
    };
    let parallel = args.parallel.or(fac.limits.parallel).unwrap_or(1);
    if parallel == 0 {
        return Err(SfError::usage(
            "--parallel 0 runs nothing; give 1 or more (in [limits] parallel too)",
            "ns watch --parallel 2",
        )
        .into());
    }

    if args.dry_run && args.resume_night.is_some() {
        // The old ns's check of this binary: the arguments and the state are what it validates.
        // The queue is not, and a gh or network failure here must not blacklist a good commit.
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "resume_night": "ok",
                "max_units": max_units,
                "parallel": parallel,
                "until": deadline.map(clock::local_iso),
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }

    if args.dry_run {
        let main_ci = ci::read(forge, fac)?;
        if let Some(n) = main_ci.notice() {
            eprintln!("ns watch: {n}");
        }
        let qu = queue(forge, fac, &BTreeSet::new())?;
        let wip = in_progress(&repo, forge, &q)?;
        // The units live runs hold, by unit id, and their `touches`.
        let held = wip.held.iter().map(|(i, h)| (i, h["unit"].as_str()));
        let running = overlap::running_units(&repo, forge, held);
        let rule = (!running.is_empty()).then(|| overlap::rule(&repo, &q.overlap_ignore));
        let ready: Vec<Value> = qu
            .ready
            .iter()
            .map(|i| {
                let overlaps = rule.as_ref().map_or_else(Vec::new, |r| {
                    overlap::overlapped(r, &overlap::predicted(forge, i), &running)
                });
                json!({
                    "number": i.number,
                    "title": i.title,
                    "priority_label": q.priority.get(rank(&q.priority, &i.labels)),
                    "order_label": q.order.get(rank(&q.order, &i.labels)),
                    "overlaps": overlaps,
                })
            })
            .collect();
        // With the pass off, nothing would be triaged, so list nothing.
        let tr = if triage::on(fac) {
            triage::candidates(forge, fac, &BTreeSet::new())?
        } else {
            triage::Candidates::default()
        };
        let requeue: Vec<u64> = wip.stale.iter().map(|i| i.number).collect();
        // The night's first pass treats every candidate as backlog: it takes them in order
        // while no issue is ready, and none while one is. The night requeues stale issues
        // before that pass, so they count as ready.
        let next_pass: Vec<u64> = if qu.ready.is_empty() && requeue.is_empty() {
            tr.issues.iter().map(|(i, _)| i.number).collect()
        } else {
            Vec::new()
        };
        let triage: Vec<Value> = tr
            .issues
            .iter()
            .map(|(i, why)| {
                json!({
                    "number": i.number,
                    "title": i.title,
                    "reason": why,
                    "priority_label": q.priority.get(rank(&q.priority, &i.labels)),
                })
            })
            .collect();
        let parents =
            split::plan(forge, &q, split::sweep_due(&repo.common_dir)).unwrap_or_else(|e| {
                eprintln!("ns watch: split parents not checked: {e:#}");
                Vec::new()
            });
        // As in the night itself, a cleanup that can't plan doesn't stop the rest.
        let clean = crate::clean::plan(&repo).unwrap_or_else(|e| {
            eprintln!("ns watch: cleanup skipped: {e:#}");
            crate::clean::Plan::default()
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "requeue": requeue,
                "clean": clean.merged.iter().map(|m| &m.unit).collect::<Vec<_>>(),
                "split_parents": parents.iter().map(split::Parent::to_json).collect::<Vec<_>>(),
                "queue": ready,
                "skipped": qu.skipped,
                "triage_pass": triage::on(fac),
                "triage": triage,
                "triage_next_pass": next_pass,
                "triage_skipped": tr.skipped,
                "human_merge_label": q.human_merge_label,
                "main_ci": main_ci,
                "queue_held": main_ci.holds_queue(),
                "max_units": max_units,
                "parallel": parallel,
                "until": deadline.map(clock::local_iso),
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }

    let _watch = run::Lock::watch(&repo.common_dir)?;
    stop::install().context("cannot handle SIGINT and SIGTERM")?;
    // A stop the old ns held pending across its exec arrives now, at the handler just set.
    stop::unblock();
    let facts = snapshot::Facts {
        watch_pid: std::process::id(),
        until: deadline,
        gate: loaded.gate.clone(),
    };
    let snap = snapshot::Snapshot::take(
        &repo.common_dir,
        &loaded.root,
        &crate::config::path(),
        &facts,
    )?;
    for link in &snap.skipped {
        eprintln!(
            "ns watch: warning: {} is a symlink; the units won't see it",
            link.display()
        );
    }
    // A night resumed after a hand-off said it when it began.
    if carried.is_none() {
        if let Some(n) = ci::read_or_warn(forge, fac).and_then(|m| m.notice()) {
            eprintln!("ns watch: {n}");
        }
    }
    night.shared.night = Some(snap.night().clone());
    let (mut tonight, started) = match carried {
        Some(c) => night.resume(c)?,
        None => (Tonight::default(), 0),
    };
    // The triage pass, which runs here, reads the snapshot's prompts as every unit does.
    loaded.root = snap.factory();
    let update = self_update::SelfUpdate::new(
        &repo,
        args.no_self_update,
        std::env::args_os().skip(1).collect(),
    );
    let plan = scheduler::Plan {
        repo: &repo,
        forge,
        loaded: &loaded,
        snap: &snap,
        deadline,
        max_units,
        parallel: parallel as usize,
        started,
        update,
    };
    let mut failed = None;
    let stopped = match requeue_stale(&repo, forge, &q, &mut tonight.requeued)
        .and_then(|()| scheduler::Scheduler::new(plan).run(&mut night, &mut tonight))
    {
        Ok(stopped) => stopped,
        // The summary still reports what the other units did, and the error.
        Err(e) => {
            failed = Some(e);
            "error".into()
        }
    };
    // A signal that came after the last check still decides the exit code, so name it too.
    let stopped = stop::requested().map_or(stopped, |sig| stop::name(sig).into());
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "units": tonight.units,
            "triaged": night.triage.records,
            "requeued": tonight.requeued,
            "cleaned": tonight.cleaned,
            "split_parents": tonight.split_parents,
            "stopped": stopped,
            "until": deadline.map(clock::local_iso),
            "cost_usd": night.shared.spent(),
            "started_with": loaded.files,
            "error": failed.as_ref().map(|e| format!("{e:#}")),
        }))?
    );
    // A stop decides the exit code over an error, which still goes to stderr as well.
    if let Some(e) = failed {
        if stop::requested().is_none() {
            return Err(e);
        }
        eprintln!("ns watch: {e:#}");
    }
    Ok(match stop::requested() {
        Some(sig) => ExitCode::from((128 + sig) as u8),
        None => ExitCode::SUCCESS,
    })
}

/// What the night's work leaves for its summary.
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Tonight {
    units: Vec<Value>,
    /// Issues found in progress at start with no live `ns run`, returned to the queue.
    requeued: Vec<u64>,
    /// Units whose worktree and branch the night removed, their change merged.
    cleaned: Vec<String>,
    /// Merged units cleanup tried tonight, removed or not: each is tried once a night.
    cleanup_tried: BTreeSet<String>,
    /// Split parents closed, sent to a human, or moved to `split_label` by this `ns`. Left out of
    /// the state a self-update carries, whose layout is pinned; the run log has every one.
    #[serde(skip)]
    split_parents: Vec<Value>,
}

/// Whether the outcome is surely the unit's own end: a merge, a split or a clean `done`. Under a
/// stop, any other outcome may come from the stop itself, such as a `gh` call that Ctrl-C killed,
/// so the issue goes back to the queue instead.
fn ended_cleanly(r: &RunResult) -> bool {
    matches!(r.outcome, Outcome::Merged | Outcome::Split)
        || (r.outcome == Outcome::Done && !r.needs_human)
}

const PAUSED_PAST_UNTIL: &str = "usage limit resets after --until";

/// What one `ns watch` carries from run to run.
struct Night {
    shared: Shared,
    /// Consecutive runs, units or triage-only, whose every attempt failed instantly: the
    /// harness, not the work.
    harness_fails: u32,
    /// Issues whose unit ended tonight.
    finished: BTreeSet<u64>,
    skills_warned: Option<String>,
    /// The skills sync waits for a drain instead of running before each unit, as a phase reads
    /// the installed skills.
    skills_drain: bool,
    triage: triage::Tally,
}

impl Night {
    /// The night so far, for the new `ns` to resume. `started` is the units claimed.
    fn carry(
        &self,
        deadline: Option<i64>,
        started: u32,
        tonight: &Tonight,
    ) -> self_update::Carried {
        self_update::Carried {
            version: self_update::Carried::VERSION,
            deadline,
            spent_usd: self.shared.spent(),
            started,
            harness_fails: self.harness_fails,
            finished: self.finished.clone(),
            triage: self.triage.clone(),
            tonight: tonight.clone(),
        }
    }

    /// Take on the night a previous `ns` carried, once `shared.night` exists to take its spend.
    /// The rest of it, for the caller: the night's `Tonight` and the units claimed so far. The
    /// deadline is the caller's, read before this.
    fn resume(&mut self, carried: self_update::Carried) -> Result<(Tonight, u32)> {
        let self_update::Carried {
            version: _,
            deadline: _,
            spent_usd,
            started,
            harness_fails,
            finished,
            triage,
            tonight,
        } = carried;
        self.harness_fails = harness_fails;
        self.finished = finished;
        self.triage = triage;
        if spent_usd > 0.0 {
            self.shared.add_spend("before the self-update", spent_usd)?;
        }
        Ok((tonight, started))
    }

    fn new() -> Night {
        let mut shared = Shared::new();
        shared.watch_pid = Some(std::process::id());
        Night {
            shared,
            harness_fails: 0,
            finished: BTreeSet::new(),
            skills_warned: None,
            skills_drain: false,
            triage: triage::Tally::default(),
        }
    }

    /// Why the night ends before the next run: `--until` has passed or the budget is spent.
    fn over(&self, deadline: Option<i64>, fac: &Factory) -> Option<&'static str> {
        if deadline.is_some_and(|d| self.shared.clock.now() >= d) {
            return Some("until");
        }
        fac.budget_usd()
            .filter(|b| self.shared.spent() >= *b)
            .map(|_| "budget")
    }

    /// `git fetch origin`, then keep installed skills current unless they wait for a drain;
    /// the base for the run's worktree.
    fn base(&mut self, repo: &Repo, issue: u64) -> Option<String> {
        let base = fresh_base(&repo.root);
        if let Some(b) = base.as_deref().filter(|_| !self.skills_drain) {
            sync_skills(repo, b, Some(issue), &mut self.skills_warned);
        }
        base
    }

    /// Count a finished run toward the harness breaker; true when the harness is failing.
    fn harness(&mut self, r: &RunResult) -> bool {
        let failing = r.outcome == Outcome::Stuck && harness_failing(&r.json);
        self.harness_fails = if failing { self.harness_fails + 1 } else { 0 };
        failing
    }

    /// When a paused run can resume: its reset time, or a retry later when it gave none.
    fn resume_at(&self, r: &RunResult) -> i64 {
        let now = self.shared.clock.now();
        r.reset_at
            .filter(|t| *t > now)
            .unwrap_or(now + PAUSE_RETRY_S)
    }

    fn sleep_until(&mut self, reset: i64) -> Result<()> {
        eprintln!(
            "ns watch: usage limit, sleeping until {}",
            clock::local_iso(reset)
        );
        self.shared.clock.sleep_until(reset)
    }
}

/// The Critical and Important findings in changed code `review.md` leaves open, one
/// `<id>. <title> (<location>)` line each, so the stuck comment tells a human what is left to
/// finish. A pre-existing finding is an escape filed elsewhere (D29), not the unit's to finish.
fn open_findings(review: &str) -> Vec<String> {
    review_md::parse(review)
        .into_iter()
        .filter(|f| f.against_unit() && f.status == Status::Open)
        .map(|f| match f.location {
            Some(l) => format!("{}. {} ({l})", f.id, f.title),
            None => format!("{}. {}", f.id, f.title),
        })
        .collect()
}

/// Two units in a row that fail like this stop the night instead of draining the queue.
const HARNESS_FAIL_LIMIT: u32 = 2;

/// Every attempt exited non-zero within seconds, having produced no output tokens:
/// the harness itself is failing (an unrecognised usage limit, an auth or network
/// error), not the work.
fn harness_failing(run: &Value) -> bool {
    let Some(phases) = run["phases"].as_array() else {
        return false;
    };
    !phases.is_empty()
        && phases.iter().all(|p| {
            p["exit"].as_i64().is_some_and(|c| c != 0)
                && p["output_tokens"].as_u64().unwrap_or(0) == 0
                && p["wall_s"].as_f64().unwrap_or(f64::MAX) < 30.0
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use forge::IssueState;

    #[test]
    fn parses_blockers_and_closers() {
        assert_eq!(blockers("Blocked by: #3, #12\nmore"), [3, 12]);
        assert_eq!(blockers("\nblocked by: #4"), [4]);
        assert!(blockers("text\nBlocked by: #3").is_empty());
        let disclosed = Post::new("Blocked by: #7\n\nbody");
        assert_eq!(blockers(disclosed.as_str()), [7]);
        assert_eq!(
            blockers("🤖 *Posted by an AI agent (Claude Code).*\nBlocked by: #8"),
            [8]
        );
        assert!(
            blockers("🤖 *Posted by nightshift, an AI agent.*\n\ntext\nBlocked by: #3").is_empty()
        );
        assert_eq!(closed_by("Closes #5. Also fixes #6"), [5, 6]);
        assert!(closed_by("see #5").is_empty());
    }

    #[test]
    fn open_findings_keeps_open_criticals_and_importants() {
        let review = "\
Open after at least 2 fix attempts: C1, I2, I4.

### C1. Drops every `SKU`
- Location: `a.py:1`
- Status: open
### I1. Done already
- Location: `a.py:2`
- Status: dismissed: noted: open question for later
### I2. No location given
- Status: open. The fix did not land.
### I4. Formatted loosely
- **Location:** `a.py:4`
- **Status:** Open
### I5. Location after status
- Status: open
- Location: `a.py:5`
### I6. Closed by a section
## Suggestion
- Status: open
### S1. A suggestion
- Location: `a.py:3`
- Status: open
### Cycle 3 notes
- Status: open
";
        assert_eq!(
            open_findings(review),
            [
                "C1. Drops every SKU (a.py:1)",
                "I2. No location given",
                "I4. Formatted loosely (a.py:4)",
                "I5. Location after status (a.py:5)"
            ]
        );
        assert!(open_findings("no findings here").is_empty());
    }

    #[test]
    fn open_findings_reads_packed_one_line_and_range_forms() {
        let review = "\
## Important
### I1. Packed fields
- Location: `a.rs:585-605`. Raised by: architecture. Fix: split it. Status: open
### I2-I3 (cycle 1). Grouped
- Status: fixed (cycle 1, abc)
- I4. Two-line bullet inside a heading finding is not its own finding
## Critical
- C1. One-line finding (security). Open.
- C2. One-line finding, fixed in cycle 2 (correctness). Fixed.
### C3. An escape
- Scope: pre-existing
- Status: open
```
### I9. In a code fence
- Status: open
```
";
        assert_eq!(
            open_findings(review),
            [
                "I1. Packed fields (a.rs:585-605)",
                "C1. One-line finding (security). Open."
            ]
        );
    }

    #[test]
    fn a_run_lock_holder_holds_its_issue_or_its_unit_ids_issue() {
        let h = json!({"pid": 1, "unit": "13-fix-uart", "issue": 13});
        assert!(holds(&h, 13));
        assert!(!holds(&h, 1));
        assert!(!holds(&h, 130));
        // A unit run without --issue names its issue only through the unit id.
        let by_unit = json!({"pid": 1, "unit": "13-fix-uart", "issue": null});
        assert!(holds(&by_unit, 13));
        assert!(!holds(&by_unit, 1));
        assert!(holds(&json!({"pid": 1, "unit": "13"}), 13));
        assert!(!holds(&json!({"pid": 1, "unit": "130-x"}), 13));
        // A holder that hasn't written its record yet is known by its lock's file name.
        assert!(holds(
            &json!({"pid": null, "unit": "99-x", "issue": null}),
            99
        ));
        assert!(!holds(
            &json!({"pid": null, "unit": "uart", "issue": null}),
            99
        ));
    }

    #[test]
    fn a_unit_id_is_the_one_ns_run_names_for_the_issue() {
        let issue = |title: &str| Issue {
            number: 13,
            title: title.into(),
            labels: Vec::new(),
            body: String::new(),
            team: true,
        };
        assert_eq!(unit_id(&issue("Fix UART")), "13-fix-uart");
        assert_eq!(unit_id(&issue("")), "13");
    }

    /// A forge in memory: issues, the closed ones, and each issue's labels, edited in place.
    /// Check logs by where they are; issues filed, as `(title, body, labels)`. Of the closed,
    /// those in `not_planned` were closed as not planned. Sub-issues, team comments and the
    /// issues a `Part of` search finds by parent; the issues it closed, the comments it posted
    /// and the labels it created, in order. A call named in `fails` fails.
    #[derive(Default)]
    pub(super) struct Fake {
        pub issues: Vec<Issue>,
        pub closed: BTreeSet<u64>,
        pub not_planned: BTreeSet<u64>,
        pub subs: BTreeMap<u64, Vec<u64>>,
        pub comments: BTreeMap<u64, Vec<String>>,
        pub linked: BTreeMap<u64, Vec<Issue>>,
        pub searches: std::cell::Cell<usize>,
        pub comment_reads: std::cell::RefCell<Vec<u64>>,
        pub closes: std::cell::RefCell<Vec<u64>>,
        pub posted: std::cell::RefCell<Vec<(u64, String)>>,
        pub created: std::cell::RefCell<Vec<String>>,
        pub fails: BTreeSet<&'static str>,
        pub labels: std::cell::RefCell<BTreeMap<u64, Vec<String>>>,
        pub main: ci::HeadChecks,
        pub logs: BTreeMap<String, String>,
        pub green: Option<String>,
        pub green_fails: bool,
        pub filed: std::cell::RefCell<Vec<(String, String, Vec<String>)>>,
    }

    impl Forge for Fake {
        fn open_issues(&self, label: Option<&str>) -> Result<Vec<Issue>> {
            if self.fails.contains("open_issues") {
                anyhow::bail!("HTTP 502");
            }
            Ok(self
                .issues
                .iter()
                .filter(|i| label.is_none_or(|l| i.labels.iter().any(|x| x == l)))
                .cloned()
                .collect())
        }
        fn closing_prs(&self, _: PrState) -> Result<ClosingPrs> {
            Ok(ClosingPrs::new())
        }
        fn is_closed(&self, n: u64) -> Result<bool> {
            match n {
                99 => anyhow::bail!("unreadable"),
                _ => Ok(self.closed.contains(&n)),
            }
        }
        fn state(&self, n: u64) -> Result<IssueState> {
            Ok(match self.is_closed(n)? {
                false => IssueState::Open,
                true if self.not_planned.contains(&n) => IssueState::NotPlanned,
                true => IssueState::Completed,
            })
        }
        fn sub_issues(&self, n: u64) -> Result<Vec<u64>> {
            Ok(self.subs.get(&n).cloned().unwrap_or_default())
        }
        fn team_comments(&self, n: u64) -> Result<Vec<String>> {
            self.comment_reads.borrow_mut().push(n);
            if self.fails.contains("team_comments") {
                anyhow::bail!("HTTP 502");
            }
            Ok(self.comments.get(&n).cloned().unwrap_or_default())
        }
        fn part_of(&self, n: u64) -> Result<Vec<Issue>> {
            self.searches.set(self.searches.get() + 1);
            Ok(self.linked.get(&n).cloned().unwrap_or_default())
        }
        fn ensure_label(&self, label: &str, _: &str) -> Result<()> {
            if self.fails.contains("ensure_label") {
                anyhow::bail!("HTTP 502");
            }
            self.created.borrow_mut().push(label.into());
            Ok(())
        }
        fn labels(&self, n: u64) -> Result<Vec<String>> {
            Ok(self.labels.borrow().get(&n).cloned().unwrap_or_default())
        }
        fn edit_labels(&self, n: u64, remove: &[String], add: Option<&str>) -> Result<()> {
            if self.fails.contains("edit_labels") {
                anyhow::bail!("HTTP 502");
            }
            let mut all = self.labels.borrow_mut();
            let ls = all.entry(n).or_default();
            ls.retain(|l| !remove.contains(l));
            ls.extend(add.map(String::from));
            Ok(())
        }
        fn comment(&self, n: u64, body: &Post) -> Result<()> {
            if self.fails.contains("comment") {
                anyhow::bail!("HTTP 502");
            }
            self.posted.borrow_mut().push((n, body.as_str().into()));
            Ok(())
        }
        fn close(&self, n: u64) -> Result<()> {
            if self.fails.contains("close") {
                anyhow::bail!("HTTP 502");
            }
            self.closes.borrow_mut().push(n);
            Ok(())
        }
        fn add_pr_label(&self, _: u64, _: &str) -> Result<()> {
            Ok(())
        }
        fn remove_pr_label(&self, _: u64, _: &str) -> Result<()> {
            Ok(())
        }
        fn comment_pr(&self, _: u64, _: &Post) -> Result<()> {
            Ok(())
        }
        fn main_checks(&self) -> Result<ci::HeadChecks> {
            Ok(self.main.clone())
        }
        fn create_issue(&self, title: &str, body: &Post, labels: &[&str]) -> Result<u64> {
            let mut filed = self.filed.borrow_mut();
            let labels = labels.iter().map(|l| l.to_string()).collect();
            filed.push((title.into(), body.as_str().into(), labels));
            Ok(100 + filed.len() as u64 - 1)
        }
        fn check_log(&self, log: &str) -> Result<String> {
            match self.logs.get(log) {
                Some(l) => Ok(l.clone()),
                None => anyhow::bail!("no log at {log}"),
            }
        }
        fn last_green(&self, red: &str) -> Result<Option<String>> {
            if self.green_fails {
                anyhow::bail!("HTTP 502");
            }
            Ok(self.green.clone().filter(|g| g != red))
        }
    }

    /// Main's CI state for checks `(name, outcome, required)` on head `abc123`.
    fn main_ci(fac: &Factory, checks: &[(&str, ci::Outcome, bool)]) -> ci::MainCi {
        let forge = Fake {
            main: ci::HeadChecks {
                sha: "abc123".into(),
                checks: checks
                    .iter()
                    .map(|&(name, outcome, required)| ci::Check {
                        name: name.into(),
                        outcome,
                        required,
                        log: None,
                    })
                    .collect(),
            },
            ..Fake::default()
        };
        ci::read(&forge, fac).unwrap()
    }

    #[test]
    fn main_ci_is_red_green_pending_or_none() {
        use ci::{Outcome::*, State};
        let fac = Factory::default();
        let state = |checks: &[(&str, ci::Outcome, bool)]| {
            let m = main_ci(&fac, checks);
            let failing: Vec<String> = m.failing.iter().map(|c| c.name.clone()).collect();
            (m.state, failing, m.holds_queue())
        };
        // Completed with a failing required check: red, naming it, not the optional one.
        assert_eq!(
            state(&[
                ("test", Fail, true),
                ("lint", Pass, true),
                ("bench", Fail, false)
            ]),
            (State::Red, vec!["test".to_string()], true)
        );
        // Still running with one check already failed, required or not: red.
        assert_eq!(
            state(&[("bench", Fail, false), ("test", Running, true)]),
            (State::Red, vec!["bench".to_string()], true)
        );
        // Still running with no failure yet: pending, not red.
        assert_eq!(
            state(&[("lint", Pass, true), ("test", Running, true)]),
            (State::Pending, vec![], false)
        );
        // Every required check passed, an optional one failed: green.
        assert_eq!(
            state(&[("test", Pass, true), ("bench", Fail, false)]),
            (State::Green, vec![], false)
        );
        // No CI at all: not red, and said once at start.
        let none = main_ci(&fac, &[]);
        assert_eq!((none.state, none.holds_queue()), (State::None, false));
        assert!(none.notice().unwrap().contains("no CI"));
        assert_eq!(none.sha.as_deref(), Some("abc123"));
        assert_eq!(main_ci(&fac, &[("test", Pass, true)]).notice(), None);
    }

    #[test]
    fn fix_main_first_off_reports_off_whatever_main_ci_is() {
        let mut fac = Factory::default();
        fac.queue.fix_main_first = false;
        let m = main_ci(&fac, &[("test", ci::Outcome::Fail, true)]);
        assert_eq!(m.state, ci::State::Off);
        assert!(!m.holds_queue());
        assert!(m.notice().unwrap().contains("off"));
        assert_eq!(serde_json::to_value(&m).unwrap(), json!({"state": "off"}));
    }

    #[test]
    fn the_queue_and_labels_go_through_the_forge() {
        let fac = Factory::default();
        let ready = fac.queue.ready_label.clone();
        let issue = |number, body: &str| Issue {
            number,
            title: format!("Fix {number}"),
            labels: vec![ready.clone()],
            body: body.into(),
            team: true,
        };
        let forge = Fake {
            issues: vec![
                issue(2, "Blocked by: #1"),
                issue(3, "Blocked by: #4"),
                issue(5, "Blocked by: #99"),
            ],
            closed: BTreeSet::from([1]),
            ..Fake::default()
        };
        let qu = queue(&forge, &fac, &BTreeSet::new()).unwrap();
        assert_eq!(qu.ready.iter().map(|i| i.number).collect::<Vec<_>>(), [2]);
        assert_eq!(qu.skipped[0]["reason"], "blocked by open #4");
        assert_eq!(qu.skipped[1]["reason"], "blocked by open #99");

        let q = &fac.queue;
        forge
            .labels
            .borrow_mut()
            .insert(2, vec![ready.clone(), "type:fix".into()]);
        set_status(&forge, q, 2, Some(&q.in_progress_label)).unwrap();
        assert_eq!(
            forge.labels(2).unwrap(),
            ["type:fix", q.in_progress_label.as_str()]
        );
        forge.edit_labels(2, &[], Some(&ready)).unwrap();
        set_status(&forge, q, 2, Some(&q.in_progress_label)).unwrap();
        assert_eq!(
            forge.labels(2).unwrap(),
            ["type:fix", q.in_progress_label.as_str()]
        );
    }

    #[test]
    fn ranks_by_first_matching_order_label() {
        let order: Vec<String> = vec!["type:fix".into(), "type:feat".into()];
        assert_eq!(rank(&order, &["type:feat".into()]), 1);
        assert_eq!(rank(&order, &["area:cli".into()]), 2);
    }
}
