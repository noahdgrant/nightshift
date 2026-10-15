//! The scheduler's side of a self-update and a skills drain: finding what origin's default branch
//! has, adopting a build that ended, and handing off once no unit runs.

use super::{Drain, Scheduler};
use crate::stop;
use crate::watch::core::update::{self as core, HandOffStep, Idle};
use crate::watch::self_update::{probe_state, short, Built, HandOff};
use crate::watch::{fresh_base, skills_drain, sync_skills, Night, Tonight};

impl Scheduler<'_> {
    /// Look at origin's default branch: start building a new `ns` when it has `cli/` changes the
    /// binary new units run lacks, and return a skills sync to drain for when it has skill changes
    /// the checkout lacks and a phase reads the installed skills.
    pub(super) fn check_base(&mut self, night: &Night) -> Option<Drain> {
        if !core::checks_base(self.plan.update.is_some(), night.skills_drain) {
            return None;
        }
        let repo = self.plan.repo;
        let base = fresh_base(&repo.root)?;
        if let Some(u) = self.plan.update.as_mut() {
            if let Some(to) = u.pending(&repo.root, &base) {
                u.start(repo, to);
            }
        }
        if !night.skills_drain {
            return None;
        }
        let to = skills_drain(repo, &base, &self.skills_tried)?;
        Some(Drain { base, to })
    }

    /// A build ended: check and adopt its binary for new units, or give it up.
    pub(super) fn built(&mut self, b: Built, night: &Night, tonight: &Tonight) {
        let carried = night.carry(self.plan.deadline, self.started, tonight);
        let repo = self.plan.repo;
        let Some(u) = self.plan.update.as_mut() else {
            return;
        };
        match b {
            Built::Stopped(to) => {
                eprintln!(
                    "ns watch: self-update to {} abandoned: stopping",
                    short(&to)
                );
            }
            Built::Failed(to, e) => u.give_up(repo, to, e),
            Built::Ok(to) => match probe_state(&repo.common_dir, &carried) {
                Ok(state) => u.adopt(repo, to, state.path()),
                Err(e) => u.give_up(repo, to, e),
            },
        }
    }

    /// A skills drain or an adopted `ns`, with nothing left running or paused, and no stop: time
    /// to sync or hand off.
    pub(super) fn ready_to_hand_off(&self) -> bool {
        core::ready_to_hand_off(&self.idle())
    }

    fn idle(&self) -> Idle {
        Idle {
            draining: self.draining.is_some(),
            adopted: self.plan.update.as_ref().is_some_and(|u| u.adopted()),
            running: self.running.len(),
            resuming: self.resume.len(),
            paused: self.paused_until.is_some(),
            stopping: self.stopping.is_some(),
        }
    }

    /// Sync the skills drained for, else hand off to the adopted `ns`. A hand-off returns when
    /// its exec failed, which is logged and kept from being tried again, or when a stop came
    /// first and the loop's top winds down.
    pub(super) fn hand_off(&mut self, night: &mut Night, tonight: &Tonight) {
        let stop = stop::requested().is_some();
        match core::hand_off_step(&self.idle(), stop) {
            HandOffStep::Nothing => {}
            HandOffStep::DropDrain => self.draining = None,
            HandOffStep::Sync => {
                if let Some(Drain { base, to }) = self.draining.take() {
                    sync_skills(self.plan.repo, &base, None, &mut night.skills_warned);
                    self.skills_tried.insert(to);
                }
            }
            HandOffStep::Exec => {
                let carried = night.carry(self.plan.deadline, self.started, tonight);
                let repo = self.plan.repo;
                let Some(update) = self.plan.update.as_mut() else {
                    return;
                };
                if let HandOff::Failed(to, e) = update.hand_off(repo, &carried) {
                    update.give_up(repo, to, e);
                }
            }
        }
    }
}
