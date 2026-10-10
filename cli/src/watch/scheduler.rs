//! The night's scheduler (docs/FACTORY.md, "Parallel units"). Each unit runs as its own child
//! `ns run` on the night's snapshot, up to `parallel` at once. The scheduler fills free slots
//! (triage pass, then the queue, claiming each issue before its unit starts), settles each unit
//! that ends, pauses every unit on a usage limit, and on a stop lets running units finish or,
//! for a signal, passes the signal on and returns their issues to the queue.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;

use super::self_update::{HandOff, SelfUpdate};
use super::snapshot::Snapshot;
use super::{
    comment, ended_cleanly, fresh_base, open_findings, queue, set_status, triage, Issue, Night,
    Tonight, DISCLAIMER, HARNESS_FAIL_LIMIT, PAUSED_PAST_UNTIL,
};
use crate::clock;
use crate::error::SfError;
use crate::git::Repo;
use crate::run::{self, gh, Loaded, Outcome, RunResult};
use crate::stop;
use crate::worktree::BRANCH_PREFIX;

#[cfg(unix)]
const SIGTERM: i32 = libc::SIGTERM;
#[cfg(not(unix))]
const SIGTERM: i32 = 15;

/// How often the scheduler looks for a unit that ended, a stop, or a reset that came.
const POLL: Duration = Duration::from_millis(50);

/// What the night is set to do, fixed at start.
pub(super) struct Plan<'a> {
    pub repo: &'a Repo,
    pub loaded: &'a Loaded,
    pub snap: &'a Snapshot,
    pub deadline: Option<i64>,
    pub max_units: Option<u32>,
    pub parallel: usize,
    /// Units claimed tonight before a self-update handed the night to this `ns`.
    pub started: u32,
    /// `None` when the night never updates its own binary.
    pub update: Option<SelfUpdate>,
}

/// A unit's `ns run`, running.
struct Worker {
    issue: Issue,
    child: Child,
    /// Where the child's stdout, its result, goes.
    result: PathBuf,
}

enum Wake {
    /// A worker's unit ended, with its exit status, or `None` when it can't be read.
    Ended(usize, Option<ExitStatus>),
    Reset,
    Stop,
}

/// Why no new unit starts.
enum Stopping {
    /// No new claims, but a unit paused on a usage limit still resumes: `--max-units`.
    Claims(String),
    /// Nothing starts, and paused units go back to the queue: `--until`, the budget, the
    /// harness breaker, a reset past `--until`, an error or a signal.
    All(String),
}

impl Stopping {
    fn reason(&self) -> &str {
        match self {
            Stopping::Claims(r) | Stopping::All(r) => r,
        }
    }
}

pub(super) struct Scheduler<'a> {
    plan: Plan<'a>,
    /// By worker number, 1 to `parallel`.
    running: BTreeMap<usize, Worker>,
    /// Units a usage limit paused, still claimed, waiting for the reset.
    resume: Vec<Issue>,
    /// The latest reset of a usage limit tonight that hasn't come yet.
    paused_until: Option<i64>,
    stopping: Option<Stopping>,
    /// The signal passed on to the running units, once.
    forwarded: bool,
    /// The first error; the night stops once the running units end.
    failed: Option<anyhow::Error>,
    /// The commit to update to once the running units end; no new unit starts meanwhile.
    draining: Option<String>,
    started: u32,
    results: u64,
}

impl<'a> Scheduler<'a> {
    pub fn new(plan: Plan<'a>) -> Scheduler<'a> {
        Scheduler {
            started: plan.started,
            plan,
            running: BTreeMap::new(),
            resume: Vec::new(),
            paused_until: None,
            stopping: None,
            forwarded: false,
            failed: None,
            draining: None,
            results: 0,
        }
    }

    /// Run units until the night ends; why it ended.
    pub fn run(&mut self, night: &mut Night, tonight: &mut Tonight) -> Result<String> {
        loop {
            if let Some(sig) = stop::requested() {
                self.pass_on(sig);
            }
            if self.failed.is_none() && stop::requested().is_none() {
                // A stop that cut the fill short is handled at the loop's top, not as a failure.
                if let Err(e) = self.fill(night, tonight) {
                    if stop::requested().is_none() {
                        self.fail(e);
                    }
                }
            }
            // Units paused on a usage limit resume in the fill above, and finish first.
            if self.ready_to_hand_off() {
                self.hand_off(night, tonight);
                continue;
            }
            if self.running.is_empty() {
                let Some(reset) = self.paused_until.filter(|_| self.may_resume()) else {
                    break;
                };
                // A stop ends the sleep early; the loop's top deals with it.
                if let Err(e) = night.sleep_until(reset) {
                    if stop::requested().is_none() {
                        self.fail(e);
                    }
                    continue;
                }
                self.lift_pause();
                continue;
            }
            match self.next(night) {
                Wake::Ended(n, status) => {
                    if let Err(e) = self.settle(n, status, night, tonight) {
                        self.fail(e);
                    }
                }
                Wake::Reset => self.lift_pause(),
                Wake::Stop => {}
            }
        }
        self.give_back_paused(tonight);
        if let Some(e) = self.failed.take() {
            return Err(e);
        }
        Ok(self
            .stopping
            .as_ref()
            .map_or("queue empty".into(), |s| s.reason().to_string()))
    }

    fn fail(&mut self, e: anyhow::Error) {
        if self.failed.is_none() {
            self.failed = Some(e);
        }
        self.stop_all("error");
    }

    /// Start nothing new from here on; the first reason given wins.
    fn stop_all(&mut self, reason: &str) {
        if !matches!(self.stopping, Some(Stopping::All(_))) {
            self.stopping = Some(Stopping::All(reason.into()));
        }
    }

    fn may_resume(&self) -> bool {
        !matches!(self.stopping, Some(Stopping::All(_)))
    }

    /// Stop on `sig`: pass it to every running unit, once, so each ends its phase and sets
    /// its work aside.
    fn pass_on(&mut self, sig: i32) {
        self.stopping = Some(Stopping::All(stop::name(sig).into()));
        if self.forwarded {
            return;
        }
        self.forwarded = true;
        for w in self.running.values() {
            signal(&w.child, sig);
        }
    }

    /// Fill the free slots: paused units first, then, after the triage pass, ready issues
    /// from one read of the queue.
    fn fill(&mut self, night: &mut Night, tonight: &mut Tonight) -> Result<()> {
        if self.paused_until.is_some() {
            return Ok(());
        }
        // At start and between units: units merged since go, after their records are saved.
        // Units running or paused are left alone, even one whose run hasn't taken its lock yet.
        let busy: BTreeSet<u64> = (self.running.values().map(|w| w.issue.number))
            .chain(self.resume.iter().map(|i| i.number))
            .collect();
        let removed =
            crate::clean::between_units(self.plan.repo, &mut tonight.cleanup_tried, &busy);
        tonight.cleaned.extend(removed);
        while self.free() && self.may_resume() && !self.resume.is_empty() {
            let issue = self.resume.remove(0);
            let n = issue.number;
            if let Err(e) = self.start(issue, night, true) {
                let q = &self.plan.loaded.fac.queue;
                let _ = set_status(&self.plan.repo.root, q, n, Some(&q.ready_label));
                return Err(e);
            }
        }
        if !self.free() || self.stopping.is_some() {
            return Ok(());
        }
        let plan = &self.plan;
        let fac = &plan.loaded.fac;
        if let Some(why) = self.over_claims(night) {
            self.stopping = Some(why);
            return Ok(());
        }
        if self.draining.is_none() {
            self.draining = self.pending_update();
        }
        if self.draining.is_some() {
            return Ok(());
        }
        match triage::pass(plan.repo, plan.loaded, plan.deadline, night)? {
            Some(triage::Halt::Stop(why)) => {
                self.stop_all(&why);
                return Ok(());
            }
            Some(triage::Halt::Paused(reset)) => return self.pause(reset),
            None => {}
        }
        let mut taken: BTreeSet<u64> = night.finished.clone();
        taken.extend(self.running.values().map(|w| w.issue.number));
        taken.extend(self.resume.iter().map(|i| i.number));
        let ready = queue(&plan.repo.root, fac, &taken)?.ready;
        for issue in ready {
            // A listing can name an issue twice; a stop during the read claims nothing more.
            if !self.free() || stop::requested().is_some() {
                break;
            }
            if !taken.insert(issue.number) {
                continue;
            }
            if let Some(why) = self.over_claims(night) {
                self.stopping = Some(why);
                break;
            }
            self.started += 1;
            let q = &self.plan.loaded.fac.queue;
            set_status(
                &self.plan.repo.root,
                q,
                issue.number,
                Some(&q.in_progress_label),
            )?;
            let n = issue.number;
            if let Err(e) = self.start(issue, night, false) {
                let _ = set_status(&self.plan.repo.root, q, n, Some(&q.ready_label));
                return Err(e);
            }
        }
        Ok(())
    }

    /// The commit to update to, when origin's default branch has `cli/` changes this `ns` lacks.
    fn pending_update(&self) -> Option<String> {
        let base = fresh_base(&self.plan.repo.root)?;
        self.plan
            .update
            .as_ref()?
            .pending(&self.plan.repo.root, &base)
    }

    /// Draining, with nothing left running or paused, and no stop: time to update.
    fn ready_to_hand_off(&self) -> bool {
        self.draining.is_some()
            && self.running.is_empty()
            && self.paused_until.is_none()
            && self.stopping.is_none()
    }

    /// Hand off to the commit being drained for. Returns when that failed, which is logged and
    /// kept from being tried again, or when a stop came first and the loop's top winds down.
    fn hand_off(&mut self, night: &Night, tonight: &Tonight) {
        let Some(to) = self.draining.take() else {
            return;
        };
        let carried = night.carry(self.plan.deadline, self.started, tonight);
        let repo = self.plan.repo;
        let Some(update) = self.plan.update.as_mut() else {
            return;
        };
        if let HandOff::Failed(e) = update.hand_off(repo, &to, &carried) {
            update.give_up(repo, to, e);
        }
    }

    fn free(&self) -> bool {
        self.running.len() < self.plan.parallel
    }

    /// Why no further issue may be claimed: `--max-units`, `--until` or the budget.
    fn over_claims(&self, night: &Night) -> Option<Stopping> {
        if self.plan.max_units.is_some_and(|m| self.started >= m) {
            return Some(Stopping::Claims("max_units".into()));
        }
        night
            .over(self.plan.deadline, &self.plan.loaded.fac)
            .map(|why| Stopping::All(why.into()))
    }

    /// Start `issue`'s unit as a child `ns run` on the lowest free worker number.
    fn start(&mut self, issue: Issue, night: &mut Night, resumed: bool) -> Result<()> {
        let base = night.base(self.plan.repo, issue.number);
        let n = (1..=self.plan.parallel)
            .find(|n| !self.running.contains_key(n))
            .expect("start is called with a free slot");
        self.results += 1;
        let result = self
            .plan
            .snap
            .night()
            .path()
            .join(format!("result-{}-{}.json", issue.number, self.results));
        let child = spawn(self.plan.snap, &issue, base, &night.shared.clock, &result)?;
        let again = if resumed { ", resumed" } else { "" };
        eprintln!(
            "ns watch: #{} {} (worker {n}{again})",
            issue.number, issue.title
        );
        run::log_event(
            &self.plan.repo.common_dir,
            json!({
                "event": "worker_start",
                "worker": n,
                "issue": issue.number,
                "run_pid": child.id(),
                "resumed": resumed,
            }),
        );
        self.running.insert(
            n,
            Worker {
                issue,
                child,
                result,
            },
        );
        Ok(())
    }

    /// Wait for a unit to end, the pause's reset to come, or a stop to pass on. On a pinned
    /// clock (`NS_NOW`) time doesn't pass while units run, so a pause lasts until every unit
    /// has ended.
    fn next(&mut self, night: &Night) -> Wake {
        loop {
            if stop::requested().is_some() && !self.forwarded {
                return Wake::Stop;
            }
            for (n, w) in self.running.iter_mut() {
                match w.child.try_wait() {
                    Ok(Some(status)) => return Wake::Ended(*n, Some(status)),
                    Ok(None) => {}
                    Err(e) => {
                        // Seldom anything but a child already reaped; wait once more for it.
                        eprintln!("ns watch: cannot check on worker {n}: {e}");
                        return Wake::Ended(*n, w.child.wait().ok());
                    }
                }
            }
            let clock = &night.shared.clock;
            if clock.pinned_at().is_none()
                && self.paused_until.is_some_and(|r| clock.now() >= r)
                && self.may_resume()
            {
                return Wake::Reset;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Pause every unit until `reset`, or until the latest reset tonight when that's later.
    fn pause(&mut self, reset: i64) -> Result<()> {
        let until = self.paused_until.map_or(reset, |p| p.max(reset));
        self.paused_until = Some(until);
        self.plan.snap.night().hold(until)?;
        if self.plan.deadline.is_some_and(|d| until >= d) {
            self.stop_all(PAUSED_PAST_UNTIL);
        }
        Ok(())
    }

    fn lift_pause(&mut self) {
        if self.paused_until.take().is_some() {
            if let Err(e) = self.plan.snap.night().release() {
                self.fail(e);
            }
        }
    }

    /// Record the unit worker `n` ran, which ended with `status`, and move its issue on.
    fn settle(
        &mut self,
        n: usize,
        status: Option<ExitStatus>,
        night: &mut Night,
        tonight: &mut Tonight,
    ) -> Result<()> {
        let w = self.running.remove(&n).expect("a running worker ended");
        let issue = w.issue;
        let text = fs::read_to_string(&w.result).unwrap_or_default();
        let _ = fs::remove_file(&w.result);
        let r = serde_json::from_str(&text)
            .ok()
            .and_then(RunResult::from_json);
        let sig = status.as_ref().and_then(stop_signal);
        // A signal that stopped a unit, from anyone but this watch, stops the night too.
        if let Some(s) = sig.filter(|_| !self.forwarded) {
            stop::request(s);
        }
        if let Some(t) = r.as_ref().and_then(|r| r.ended_at) {
            night.shared.clock.catch_up(t);
        }
        let q = &self.plan.loaded.fac.queue;
        let root = &self.plan.repo.root;
        run::log_event(
            &self.plan.repo.common_dir,
            json!({
                "event": "worker_end",
                "worker": n,
                "issue": issue.number,
                "unit": r.as_ref().map(|r| &r.unit),
                "outcome": r.as_ref().map(|r| r.outcome.label()),
                "exit": status.and_then(|s| s.code()),
            }),
        );
        let Some(r) = r else {
            let _ = set_status(root, q, issue.number, Some(&q.ready_label));
            if let Some(s) = stop::requested() {
                eprintln!("ns watch: #{} interrupted (worker {n})", issue.number);
                tonight.units.push(json!({
                    "issue": issue.number,
                    "worker": n,
                    "outcome": "interrupted",
                    "reason": format!("stopped by {}", stop::name(s)),
                }));
                return Ok(());
            }
            let how = status.map_or("an unknown status".into(), |s| match s.code() {
                Some(c) => format!("exit {c}"),
                None => s.to_string(),
            });
            let code = status.and_then(|s| s.code()).filter(|c| *c != 0);
            return Err(SfError::new(
                code.unwrap_or(1),
                format!(
                    "ns run --issue {} ended with {how} and no result; its error is above",
                    issue.number
                ),
            )
            .into());
        };
        let mut rec = json!({
            "issue": issue.number,
            "unit": r.unit,
            "worker": n,
            "outcome": r.outcome.label(),
            "reason": r.reason,
            "pr": r.json["pr"],
            "cost_usd": r.cost_usd,
        });
        if stop::requested().is_some() && !ended_cleanly(&r) {
            set_status(root, q, issue.number, Some(&q.ready_label))?;
            rec["outcome"] = json!("interrupted");
            eprintln!("ns watch: #{} interrupted (worker {n})", issue.number);
            tonight.units.push(rec);
            return Ok(());
        }
        let failing = night.harness(&r);
        let shown = if failing {
            "harness_failing"
        } else {
            r.outcome.label()
        };
        eprintln!("ns watch: #{} {shown} (worker {n})", issue.number);
        match r.outcome {
            Outcome::Merged => {
                let status = set_status(root, q, issue.number, None);
                // GitHub may not have closed it yet; an already-closed issue makes gh fail.
                let ns = issue.number.to_string();
                if let Err(e) = gh(root, &["issue", "close", &ns, "--reason", "completed"]) {
                    eprintln!("ns watch: #{ns} not closed: {e}");
                }
                status?;
            }
            Outcome::Split => set_status(root, q, issue.number, Some(&q.split_label))?,
            Outcome::Done if !r.needs_human => {
                set_status(root, q, issue.number, Some(&q.done_label))?
            }
            Outcome::Stuck if failing => {
                set_status(root, q, issue.number, Some(&q.ready_label))?;
                rec["outcome"] = json!("harness_failing");
                if night.harness_fails >= HARNESS_FAIL_LIMIT {
                    self.stop_all("harness failing");
                }
            }
            Outcome::Done | Outcome::Stuck => {
                let status = set_status(root, q, issue.number, Some(&q.stuck_label));
                let posted = comment(root, issue.number, &stuck_comment(&r));
                status?;
                posted?;
            }
            Outcome::Budget => {
                set_status(root, q, issue.number, Some(&q.ready_label))?;
                // A runner lock wait cut short by `--until` ends the run as `budget` too.
                let why = night
                    .over(self.plan.deadline, &self.plan.loaded.fac)
                    .unwrap_or("budget");
                self.stop_all(why);
            }
            Outcome::Paused => {
                let reset = resume_at(&r, night);
                rec["reset_at"] = json!(clock::local_iso(reset));
                tonight.units.push(rec);
                // Paused first, so a hold that can't be written still leaves it to give back.
                let n = issue.number;
                self.resume.push(issue);
                self.pause(reset)?;
                eprintln!(
                    "ns watch: #{} paused on a usage limit until {}; every unit pauses at its next phase",
                    n,
                    clock::local_iso(self.paused_until.unwrap_or(reset))
                );
                return Ok(());
            }
        }
        tonight.units.push(rec);
        night.finished.insert(issue.number);
        Ok(())
    }

    /// Return the units still paused to the queue: the night ended before their reset.
    fn give_back_paused(&mut self, tonight: &mut Tonight) {
        let q = &self.plan.loaded.fac.queue;
        for issue in self.resume.drain(..) {
            if let Err(e) = set_status(&self.plan.repo.root, q, issue.number, Some(&q.ready_label))
            {
                eprintln!(
                    "ns watch: #{} not returned to the queue: {e:#}",
                    issue.number
                );
            }
            if let Some(s) = stop::requested() {
                tonight.units.push(json!({
                    "issue": issue.number,
                    "outcome": "interrupted",
                    "reason": format!("stopped by {}", stop::name(s)),
                }));
            }
        }
        if let Err(e) = self.plan.snap.night().release() {
            eprintln!("ns watch: {e:#}");
        }
    }
}

impl Drop for Scheduler<'_> {
    /// Units still running when the scheduler goes, on a panic, are stopped and waited for,
    /// so none outlives the night directory it reads.
    fn drop(&mut self) {
        for w in self.running.values_mut() {
            signal(&w.child, SIGTERM);
            let _ = w.child.wait();
        }
    }
}

/// When a paused unit resumes: as [`Night::resume_at`], except that a unit the hold paused
/// resumes at the hold's reset, at once when that came while the unit was ending.
fn resume_at(r: &RunResult, night: &Night) -> i64 {
    match r.reset_at.filter(|_| r.held) {
        Some(t) => t.max(night.shared.clock.now()),
        None => night.resume_at(r),
    }
}

/// The comment on an issue whose unit got stuck or needs a human.
fn stuck_comment(r: &RunResult) -> String {
    let what = if r.outcome == Outcome::Stuck {
        "got stuck"
    } else {
        "needs a human"
    };
    let artifact = r
        .artifact
        .as_deref()
        .and_then(|a| Path::new(a).file_name())
        .map_or("none".to_string(), |f| {
            format!(
                "`.ns/{0}/{1}` on branch `{BRANCH_PREFIX}{0}`",
                r.unit,
                f.to_string_lossy()
            )
        });
    let findings = r
        .artifact
        .as_deref()
        .filter(|a| Path::new(a).file_name().is_some_and(|f| f == "review.md"))
        .and_then(|a| fs::read_to_string(a).ok())
        .map(|text| open_findings(&text))
        .filter(|f| !f.is_empty())
        .map_or(String::new(), |f| {
            format!(
                "\n\nOpen findings in `review.md`:\n```text\n{}\n```",
                f.join("\n")
            )
        });
    format!(
        "nightshift {what} on unit `{}`: {}{findings}\n\nLast artifact: {artifact}\n\n{DISCLAIMER}",
        r.unit, r.reason
    )
}

/// Start `ns run --issue <n>` on the night's snapshot, its result written to `result`. It gets
/// its own process group, so a terminal's Ctrl-C reaches only `ns watch`, which passes it on
/// once; and on Linux it gets SIGTERM if `ns watch` dies, so no unit outlives its night. The
/// kernel sends that signal when the thread that spawned the child ends, so units must be
/// started from the thread that runs the whole night, as the scheduler does.
fn spawn(
    snap: &Snapshot,
    issue: &Issue,
    base: Option<String>,
    clock: &clock::Clock,
    result: &Path,
) -> Result<Child> {
    // The binary this night runs, even if a newer one was installed over it since.
    let exe = if cfg!(target_os = "linux") {
        PathBuf::from("/proc/self/exe")
    } else {
        std::env::current_exe().context("cannot find the ns binary")?
    };
    let out =
        File::create(result).with_context(|| format!("cannot create {}", result.display()))?;
    let mut cmd = Command::new(exe);
    cmd.args(["run", "--issue", &issue.number.to_string()])
        .arg("--night")
        .arg(snap.night().path())
        .arg("--factory")
        .arg(snap.factory())
        .env("NS_CONFIG", snap.config())
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(Stdio::inherit());
    if let Some(b) = base {
        cmd.args(["--base", &b]);
    }
    // The unit starts at the night's time, which a pinned clock's sleeps have moved on.
    if let Some(t) = clock.pinned_at() {
        cmd.env("NS_NOW", t.to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        #[cfg(target_os = "linux")]
        {
            let watch = std::process::id() as libc::pid_t;
            // SAFETY: prctl(2) and getppid(2) are async-signal-safe and touch only this child.
            // A parent that died before the prctl is seen by getppid, and the child gives up.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != watch {
                        return Err(std::io::Error::other("ns watch is gone"));
                    }
                    Ok(())
                });
            }
        }
    }
    cmd.spawn()
        .with_context(|| format!("cannot start ns run --issue {}", issue.number))
}

/// Signal a unit's process group: its `ns run` and what runs in that group, a setup command or
/// a `gh` call, as a signal to the whole of `ns watch`'s group would. Its phases, each in a
/// session of its own, are ended by the `ns run`.
#[cfg(unix)]
fn signal(child: &Child, sig: i32) {
    // SAFETY: kill(2) on the process group of a child this process started and has not reaped:
    // the child leads the group, so the group id is still its own.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), sig);
    }
}

#[cfg(not(unix))]
fn signal(_: &Child, _: i32) {}

/// The SIGINT or SIGTERM that stopped a unit: its exit code 128 + the signal, or the signal
/// that killed it.
fn stop_signal(status: &ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .map(|c| c - 128)
            .or(status.signal())
            .filter(|s| [libc::SIGINT, libc::SIGTERM].contains(s))
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_the_hold_paused_resumes_at_once_once_the_reset_came() {
        let night = Night::new();
        let now = night.shared.clock.now();
        let paused = |reset: i64, held: bool| {
            RunResult::from_json(serde_json::json!({
                "unit": "2-a", "outcome": "paused", "reset_at": reset, "held": held,
            }))
            .unwrap()
        };
        // The hold's reset came while the unit ended: no new 30-minute wait.
        assert_eq!(resume_at(&paused(now - 1, true), &night), now);
        assert_eq!(resume_at(&paused(now + 60, true), &night), now + 60);
        // A unit's own limit whose reset has passed, the same reset or not, waits as before:
        // the provider is still refusing it.
        for reset in [now - 1, now - 5] {
            let own = paused(reset, false);
            assert_eq!(resume_at(&own, &night), night.resume_at(&own));
            assert!(resume_at(&own, &night) > now);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_unit_stopped_by_sigint_or_sigterm_names_the_signal() {
        use std::os::unix::process::ExitStatusExt;
        let exited = |c: i32| ExitStatus::from_raw(c << 8);
        assert_eq!(stop_signal(&exited(143)), Some(libc::SIGTERM));
        assert_eq!(stop_signal(&exited(130)), Some(libc::SIGINT));
        assert_eq!(stop_signal(&exited(0)), None);
        assert_eq!(stop_signal(&exited(1)), None);
        assert_eq!(stop_signal(&exited(137)), None);
        assert_eq!(
            stop_signal(&ExitStatus::from_raw(libc::SIGTERM)),
            Some(libc::SIGTERM)
        );
        assert_eq!(stop_signal(&ExitStatus::from_raw(libc::SIGKILL)), None);
    }
}
