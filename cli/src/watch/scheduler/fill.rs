//! Filling free slots, in the scheduler: the reads and the starts around `core::fill`'s
//! decisions (docs/factory/watch.md, "Parallel units").

use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::json;

use super::{overlap, red::Gate, FillError, Scheduler};
use crate::disk;
use crate::run;
use crate::stop;
use crate::watch::core::fill::{
    open_fill, Claims, DiskAct, FillAct, FillInput, Kind, Open, OpenAct, DISK_CRITICAL,
};
use crate::watch::transitions::{self, Event, State};
use crate::watch::{queue, split, triage, Issue, Night, Tonight};

impl Scheduler<'_> {
    /// Fill the free slots: paused units first, then, after the triage pass, ready issues
    /// from one read of the queue. A [`Kind::Refill`] skips the worktree cleanup and the
    /// update check.
    pub(super) fn fill(
        &mut self,
        night: &mut Night,
        tonight: &mut Tonight,
        kind: Kind,
    ) -> Result<(), FillError> {
        let open = Open {
            kind,
            paused: self.paused_until.is_some(),
            free: self.plan.parallel - self.running.len(),
            resume: self.resume.len(),
            stopping: self.stopping.as_ref(),
            claims: self.claims(night),
            draining: self.draining.is_some(),
        };
        for act in open_fill(&open) {
            match act {
                OpenAct::Housekeep => self.housekeep(tonight),
                OpenAct::Resume => self.resume_first(night)?,
                OpenAct::Stop(why) => self.stopping = Some(why),
                OpenAct::CheckDisk => {
                    if !self.disk_ok() {
                        return Ok(());
                    }
                }
                OpenAct::CheckBase => {
                    self.draining = self.check_base(night);
                    if self.draining.is_some() {
                        return Ok(());
                    }
                }
                OpenAct::Triage => return self.triage_and_claim(night),
            }
        }
        Ok(())
    }

    /// What bounds tonight's claims now.
    pub(super) fn claims(&self, night: &Night) -> Claims {
        Claims {
            started: self.started,
            max_units: self.plan.max_units,
            over: night.over(self.plan.deadline, &self.plan.loaded.fac),
        }
    }

    /// Check free disk space before a unit starts: false when the fill starts nothing, as space
    /// is low and the slot waits, or so low the night stops.
    fn disk_ok(&mut self) -> bool {
        let limits = self.plan.loaded.fac.limits.disk();
        let read = disk::read(disk::filesystems(self.plan.repo));
        let mut ok = true;
        for act in self.disk.check(read, limits) {
            let said = act.said(limits);
            if let Some(s) = &said {
                eprintln!("ns watch: {s}");
            }
            ok &= !act.holds();
            let common = &self.plan.repo.common_dir;
            match act {
                DiskAct::Unread { .. } | DiskAct::Wait => {}
                DiskAct::Low { path, free_gb } => {
                    let ev = json!({
                        "event": "disk_low",
                        "path": path,
                        "free_gb": free_gb,
                        "wait_gb": limits.wait_gb,
                    });
                    run::log_event(common, ev);
                }
                DiskAct::Critical { path, free_gb } => {
                    let ev = json!({
                        "event": "disk_critical",
                        "path": path,
                        "free_gb": free_gb,
                        "stop_gb": limits.stop_gb,
                    });
                    run::log_event(common, ev);
                    self.stop_all(DISK_CRITICAL);
                }
            }
        }
        ok
    }

    /// Units merged since go, after their records are saved, and split parents move on. Units
    /// running or paused are left alone, even one whose run hasn't taken its lock yet.
    fn housekeep(&mut self, tonight: &mut Tonight) {
        let busy: BTreeSet<u64> = (self.running.values().map(|w| w.issue.number))
            .chain(self.resume.iter().map(|i| i.number))
            .collect();
        let removed =
            crate::clean::between_units(self.plan.repo, &mut tonight.cleanup_tried, &busy);
        tonight.cleaned.extend(removed);
        self.split_parents(tonight);
    }

    /// Start the first unit a usage limit paused again; one that can't start goes back to ready.
    fn resume_first(&mut self, night: &mut Night) -> Result<(), FillError> {
        let issue = self.resume.remove(0);
        let n = issue.number;
        if let Err(e) = self.start(issue, night, true) {
            let q = &self.plan.loaded.fac.queue;
            let _ =
                transitions::apply(self.plan.forge, q, n, State::InProgress, Event::StartFailed);
            return Err(e.into());
        }
        Ok(())
    }

    /// The triage pass, then the claims `core::fill::plan_fill` makes of one read of the queue.
    fn triage_and_claim(&mut self, night: &mut Night) -> Result<(), FillError> {
        let (repo, forge, loaded) = (self.plan.repo, self.plan.forge, self.plan.loaded);
        // Red main holds triage too: the fix issue is the night's only work. The pass asks before
        // each triage-only run, as the first of them can turn main red.
        let held = || self.main_ci().is_some_and(|m| m.holds_queue());
        let mut run = |n, night: &mut Night| triage::run_here(repo, loaded, n, night);
        let fac = &loaded.fac;
        match triage::pass(forge, fac, self.plan.deadline, night, &held, &mut run)? {
            Some(triage::Halt::Stop(why)) => {
                self.stop_all(&why);
                return Ok(());
            }
            Some(triage::Halt::Paused(reset)) => {
                return self.pause(reset).map_err(FillError::Fatal);
            }
            None => {}
        }
        let mut taken: BTreeSet<u64> = night.finished.clone();
        taken.extend(self.running.values().map(|w| w.issue.number));
        taken.extend(self.resume.iter().map(|i| i.number));
        let ready = queue(forge, &loaded.fac, &taken)?.ready;
        // Read after the listing and as late as possible: the units start from this read.
        let gate = match self.main_ci() {
            Some(m) => self.main_fix(&m, &ready, &taken)?,
            None => Gate::Open,
        };
        let now = night.shared.clock.now();
        let input = FillInput {
            ready: &ready,
            gate: &gate,
            taken: &taken,
            free: self.plan.parallel - self.running.len(),
            claims: self.claims(night),
            holds: &self.holds,
        };
        let ignore = &loaded.fac.queue.overlap_ignore;
        let running = || {
            let units = self.running.values().map(|w| (&w.issue, None));
            overlap::running_units(repo, forge, units)
        };
        let acts = overlap::plan(forge, &input, now, running, || overlap::rule(repo, ignore));
        for act in acts {
            // A stop during the reads claims nothing more.
            if stop::requested().is_some() {
                break;
            }
            self.fill_act(act, now, night)?;
        }
        Ok(())
    }

    /// Do one of `plan_fill`'s acts.
    fn fill_act(&mut self, act: FillAct, now: i64, night: &mut Night) -> Result<()> {
        let common = &self.plan.repo.common_dir;
        match act {
            FillAct::Held { issue, overlaps } => {
                self.holds.hold(issue, now);
                eprintln!(
                    "ns watch: #{issue} waits: its touches overlap running {}",
                    overlaps.join(", ")
                );
                let ev = json!({"event": "overlap_held", "issue": issue, "overlaps": overlaps});
                run::log_event(common, ev);
            }
            FillAct::Forced {
                issue,
                waited,
                overlaps,
            } => {
                eprintln!(
                    "ns watch: #{issue} starts beside {} after waiting {waited}s",
                    overlaps.join(", ")
                );
                let ev = json!({
                    "event": "overlap_forced",
                    "issue": issue,
                    "waited_s": waited,
                    "overlaps": overlaps,
                });
                run::log_event(common, ev);
            }
            FillAct::Claim(issue) => self.claim(issue, night)?,
            FillAct::Stop(why) => self.stopping = Some(why),
        }
        Ok(())
    }

    /// Claim `issue` and start its unit; a claim or start that fails puts it back to ready.
    fn claim(&mut self, issue: Issue, night: &mut Night) -> Result<()> {
        let (forge, q) = (self.plan.forge, &self.plan.loaded.fac.queue);
        let n = issue.number;
        if let Err(e) = transitions::apply(forge, q, n, State::Ready, Event::Claim) {
            let _ = transitions::apply(forge, q, n, State::InProgress, Event::StartFailed);
            return Err(e);
        }
        if let Err(e) = self.start(issue, night, false) {
            let _ = transitions::apply(forge, q, n, State::InProgress, Event::StartFailed);
            return Err(e);
        }
        self.started += 1;
        Ok(())
    }

    /// Close the split parents whose children are done, and on the clone's first pass sweep the
    /// old split label. A forge that fails here holds nothing up.
    fn split_parents(&mut self, tonight: &mut Tonight) {
        let (forge, common) = (self.plan.forge, &self.plan.repo.common_dir);
        let q = &self.plan.loaded.fac.queue;
        let (parents, all_read) = match split::plan_read(forge, q, self.sweep) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("ns watch: split parents not checked: {e:#}");
                return;
            }
        };
        let done = split::apply(forge, q, &parents, &mut self.childless);
        for d in &done {
            eprintln!(
                "ns watch: split parent #{}: {}",
                d["number"],
                d["action"].as_str().unwrap_or("")
            );
            let mut ev = d.clone();
            ev["event"] = json!("split_parent");
            run::log_event(common, ev);
        }
        let finished = split::sweep_finished(all_read, &parents, &done);
        tonight.split_parents.extend(done);
        if self.sweep && finished {
            self.sweep = false;
            if let Err(e) = split::swept(common) {
                eprintln!("ns watch: the split sweep may run again: {e:#}");
            }
        }
    }
}
