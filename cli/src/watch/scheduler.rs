//! The night's scheduler (docs/factory/watch.md, "Parallel units"). Each unit runs as its own child
//! `ns run` on the night's snapshot, up to `parallel` at once. The scheduler fills free slots
//! (triage pass, then the queue, claiming each issue before its unit starts), settles each unit
//! that ends, pauses every unit on a usage limit, and on a stop lets running units finish or,
//! for a signal, passes the signal on and returns their issues to the queue.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::json;

mod child;
mod fill;
pub(super) mod overlap;
mod red;
#[cfg(test)]
mod scenarios;
mod settle;
mod update;

use self::child::{signal, spawn, SIGTERM};
use super::core::fill::{Disk, Holds, Kind, Stopping};
use super::core::red::Red;
use super::core::settle::pause_until;
use super::self_update::{Built, SelfUpdate};
use super::snapshot::{NightDir, Snapshot};
use super::transitions::{self, Event, State};
use super::{split, Forge, Issue, Night, Tonight, PAUSED_PAST_UNTIL};
use crate::clock::Clock;
use crate::git::Repo;
use crate::run::{self, Loaded};
use crate::stop;

/// How often the scheduler looks for a unit that ended, a stop, or a reset that came.
const POLL: Duration = Duration::from_millis(50);

/// How long a free slot waits beside running units before the scheduler fills again, so an
/// issue triaged or labelled ready since the last fill starts without waiting for a unit to end.
const REFILL: Duration = Duration::from_secs(60);
/// Tests set this (milliseconds) to shorten [`REFILL`].
const REFILL_ENV: &str = "NS_REFILL_MS";

fn refill_override() -> Option<Duration> {
    std::env::var(REFILL_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .map(|ms| Duration::from_millis(ms).max(POLL))
}

/// When a free slot is filled again. On a real clock, once the interval has passed. On a pinned
/// clock (`NS_NOW`) time passes only as the night moves it, so the free slot's wait moves it, a
/// second each poll, and the refill comes once the interval, in whole seconds, has passed on it;
/// that happens only with [`REFILL_ENV`] set, so a test that doesn't ask for refills gets none.
enum Refill {
    Real(Instant),
    Pinned(Option<i64>),
}

impl Refill {
    fn start(clock: &Clock) -> Refill {
        Refill::after(clock, refill_override())
    }

    /// The refill `interval` from now, or [`REFILL`] on a real clock when it's `None`.
    fn after(clock: &Clock, interval: Option<Duration>) -> Refill {
        match clock.pinned_at() {
            None => Refill::Real(Instant::now() + interval.unwrap_or(REFILL)),
            Some(now) => {
                Refill::Pinned(interval.map(|d| now + d.as_millis().div_ceil(1000) as i64))
            }
        }
    }

    /// Whether the refill is due, after this poll's share of a pinned clock's wait.
    fn due(&self, clock: &Clock) -> bool {
        match *self {
            Refill::Real(at) => Instant::now() >= at,
            Refill::Pinned(None) => false,
            Refill::Pinned(Some(at)) => {
                if clock.now() >= at {
                    return true;
                }
                // A stop this reports is seen at the top of the next poll.
                let _ = clock.sleep_until(clock.now() + 1);
                false
            }
        }
    }
}

/// What the night is set to do, fixed at start.
pub(super) struct Plan<'a> {
    pub repo: &'a Repo,
    pub forge: &'a dyn Forge,
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
    /// The refill interval passed with a slot free: fill again, without the cleanup and update check.
    Refill,
    /// A self-update's build ended.
    Built(Built),
}

/// What a fill failed on.
enum FillError {
    /// A listing, claim or start that failed; a refill logs it and waits for the next.
    Transient(anyhow::Error),
    /// A failure no night goes on after: a pause's hold that cannot be written, so a unit would
    /// run on unheld through a usage limit.
    Fatal(anyhow::Error),
}

impl From<anyhow::Error> for FillError {
    fn from(e: anyhow::Error) -> Self {
        FillError::Transient(e)
    }
}

/// What the night drains for, no new unit starting until the running ones end: a skills sync of
/// the main checkout to `to` at `base`, for phases that read the installed skills.
struct Drain {
    base: String,
    to: String,
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
    /// What to do once the running units end; no new unit starts meanwhile.
    draining: Option<Drain>,
    /// Commits the skills checkout was drained for tonight; none is drained for again, so a
    /// checkout that can't move doesn't hold the night.
    skills_tried: BTreeSet<String>,
    started: u32,
    results: u64,
    /// What the night remembers of red main between fills.
    red: Red,
    /// The split parents' pass still has the clone's one sweep of the old split label to run.
    sweep: bool,
    /// Split parents with no children found, reported once a night.
    childless: BTreeSet<u64>,
    /// Ready issues held back tonight for overlapping a running unit.
    holds: Holds,
    /// What the night remembers of free disk space.
    disk: Disk,
}

impl<'a> Scheduler<'a> {
    pub fn new(plan: Plan<'a>) -> Scheduler<'a> {
        Scheduler {
            started: plan.started,
            sweep: split::sweep_due(&plan.repo.common_dir),
            plan,
            running: BTreeMap::new(),
            resume: Vec::new(),
            paused_until: None,
            stopping: None,
            forwarded: false,
            failed: None,
            draining: None,
            skills_tried: BTreeSet::new(),
            results: 0,
            red: Red::default(),
            childless: BTreeSet::new(),
            holds: Holds::default(),
            disk: Disk::default(),
        }
    }

    /// Run units until the night ends; why it ended.
    pub fn run(&mut self, night: &mut Night, tonight: &mut Tonight) -> Result<String> {
        let mut next_fill = Kind::Full;
        loop {
            let kind = std::mem::replace(&mut next_fill, Kind::Full);
            if let Some(sig) = stop::requested() {
                self.pass_on(sig);
            }
            // An adopted ns takes over before the fill, so no unit starts on the old one first.
            if self.ready_to_hand_off() {
                self.hand_off(night, tonight);
                continue;
            }
            if self.failed.is_none() && stop::requested().is_none() {
                if kind == Kind::Refill {
                    let ev = json!({"event": "refill", "at": night.shared.clock.now()});
                    run::log_event(&self.plan.repo.common_dir, ev);
                }
                // A stop that cut the fill short is handled at the loop's top, not as a failure.
                match self.fill(night, tonight, kind) {
                    Err(FillError::Fatal(e)) => self.fail(e),
                    Err(FillError::Transient(e)) if stop::requested().is_none() => {
                        if kind == Kind::Refill {
                            eprintln!("ns watch: refill failed: {e:#}");
                        } else {
                            self.fail(e);
                        }
                    }
                    Ok(()) | Err(FillError::Transient(_)) => {}
                }
            }
            // Units paused on a usage limit resume in the fill above, and finish first.
            if self.ready_to_hand_off() {
                self.hand_off(night, tonight);
                continue;
            }
            if self.running.is_empty() {
                let Some(reset) = self.paused_until.filter(|_| self.may_resume()) else {
                    // Nothing runs, but a build may yet give the night a newer ns to hand off to,
                    // or a refill find the disk space a unit waits for.
                    let building = self.plan.update.as_ref().is_some_and(|u| u.building());
                    if self.stopping.is_none() && (building || self.disk.waiting()) {
                        match self.next(night) {
                            Wake::Built(b) => self.built(b, night, tonight),
                            Wake::Refill => next_fill = Kind::Refill,
                            Wake::Ended(..) | Wake::Reset | Wake::Stop => {}
                        }
                        continue;
                    }
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
                Wake::Refill => next_fill = Kind::Refill,
                Wake::Built(b) => self.built(b, night, tonight),
                Wake::Stop => {}
            }
        }
        if let Some(u) = self.plan.update.as_mut() {
            let why = if stop::requested().is_some() {
                "stopping"
            } else {
                "the night ended"
            };
            u.abandon(why);
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

    /// A slot is free and a fill could start something: nothing stopping, draining or paused.
    fn may_refill(&self) -> bool {
        self.free()
            && self.stopping.is_none()
            && self.draining.is_none()
            && self.paused_until.is_none()
    }

    fn free(&self) -> bool {
        self.running.len() < self.plan.parallel
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
        let cpus = crate::cpus::share_here(self.plan.parallel);
        let clock = &night.shared.clock;
        let exe = self.plan.update.as_ref().and_then(|u| u.child_exe());
        let child = match spawn(
            self.plan.snap,
            &issue,
            base.clone(),
            cpus,
            clock,
            &result,
            exe,
        ) {
            Ok(c) => c,
            // A new ns that can't start a unit is given up; the unit starts on this one.
            Err(e) if exe.is_some() => {
                if let Some(u) = self.plan.update.as_mut() {
                    let to = u.adopted_commit();
                    u.give_up(self.plan.repo, to, e);
                }
                spawn(self.plan.snap, &issue, base, cpus, clock, &result, None)?
            }
            Err(e) => return Err(e),
        };
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

    /// Wait for a unit to end, the pause's reset to come, a stop to pass on, or a free slot's
    /// time to fill again. On a pinned clock (`NS_NOW`) time doesn't pass while units run, so a
    /// pause lasts until every unit has ended; only a free slot's wait moves it ([`Refill`]).
    fn next(&mut self, night: &Night) -> Wake {
        let refill = Refill::start(&night.shared.clock);
        loop {
            if stop::requested().is_some() && !self.forwarded {
                return Wake::Stop;
            }
            if let Some(b) = self.plan.update.as_mut().and_then(|u| u.poll()) {
                return Wake::Built(b);
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
            if self.may_refill() && refill.due(clock) {
                return Wake::Refill;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Pause every unit until `reset`, or until the latest reset tonight when that's later.
    fn pause(&mut self, reset: i64) -> Result<()> {
        let deadline = self.plan.deadline;
        let past = extend_pause(
            self.plan.snap.night(),
            &mut self.paused_until,
            reset,
            deadline,
        )?;
        if past {
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

    /// Return the units still paused to the queue: the night ended before their reset.
    fn give_back_paused(&mut self, tonight: &mut Tonight) {
        let q = &self.plan.loaded.fac.queue;
        for issue in self.resume.drain(..) {
            let given_back = transitions::apply(
                self.plan.forge,
                q,
                issue.number,
                State::InProgress,
                Event::PausedGivenBack,
            );
            if let Err(e) = given_back {
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

/// Hold every unit until `reset`, or until the latest reset in `paused` when that's later
/// (`core::settle::pause_until`); whether the hold lasts past `deadline`. The hold is written
/// first, so a hold that can't be written leaves `paused` unchanged.
fn extend_pause(
    night: &NightDir,
    paused: &mut Option<i64>,
    reset: i64,
    deadline: Option<i64>,
) -> Result<bool> {
    let (until, past) = pause_until(*paused, reset, deadline);
    night.hold(until)?;
    *paused = Some(until);
    Ok(past)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many polls until `r` is due on `clock`, up to `most`.
    fn polls_until_due(r: &Refill, clock: &Clock, most: u32) -> Option<u32> {
        (0..most).find(|_| r.due(clock))
    }

    #[test]
    fn a_pinned_refill_moves_the_clock_a_second_a_poll_until_the_interval_passes() {
        let clock = Clock::pinned(1000);
        let r = Refill::after(&clock, Some(POLL * 60));
        assert_eq!(polls_until_due(&r, &clock, 10), Some(3));
        assert_eq!(clock.now(), 1003);
        // A part of a second is a whole one.
        let r = Refill::after(&clock, Some(POLL * 2));
        assert_eq!(polls_until_due(&r, &clock, 10), Some(1));
        assert_eq!(clock.now(), 1004);
    }

    #[test]
    fn a_pinned_clock_with_no_refill_interval_set_never_refills_or_moves() {
        let clock = Clock::pinned(1000);
        let r = Refill::after(&clock, None);
        assert_eq!(polls_until_due(&r, &clock, 2000), None);
        assert_eq!(clock.now(), 1000);
    }

    #[test]
    fn a_real_refill_waits_for_the_interval() {
        let clock = Clock::from_env();
        assert_eq!(clock.pinned_at(), None);
        assert!(!Refill::after(&clock, None).due(&clock));
        assert!(Refill::after(&clock, Some(Duration::ZERO)).due(&clock));
    }

    #[test]
    fn a_pause_whose_hold_cannot_be_written_is_not_applied() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("night.json"), r#"{"watch_pid": 1}"#).unwrap();
        let (night, _) = NightDir::open(tmp.path()).unwrap();
        let mut paused = Some(500);
        assert!(!extend_pause(&night, &mut paused, 900, None).unwrap());
        assert_eq!(paused, Some(900));
        std::fs::create_dir(tmp.path().join("hold.tmp")).unwrap();
        assert!(extend_pause(&night, &mut paused, 1200, None).is_err());
        assert_eq!(paused, Some(900));
        let mut none = None;
        assert!(extend_pause(&night, &mut none, 1200, None).is_err());
        assert_eq!(none, None);
    }
}
