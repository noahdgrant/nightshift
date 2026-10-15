//! Settling a unit that ended, in the scheduler: reading its result and the night's facts, then
//! doing what `core::settle` decides, the forge's part first.

use std::fs;
use std::process::ExitStatus;

use anyhow::Result;
use serde_json::{json, Value};

use super::child::stop_signal;
use super::Scheduler;
use crate::clock;
use crate::error::SfError;
use crate::factory::Queue as QueueConfig;
use crate::run::{self, Outcome, RunResult};
use crate::stop;
use crate::watch::ci::MainCi;
use crate::watch::core::red::{self as core_red, MAIN_RED_UNFIXED};
use crate::watch::core::settle::{self as core_settle, SettleAct, SettleFacts};
use crate::watch::transitions::{self, Event, State};
use crate::watch::{Forge, Issue, Night, Post, Tonight};

/// A unit that ended: its issue, its result, main's CI if its settle read it, and its record so
/// far.
struct Ended<'a> {
    issue: &'a Issue,
    r: &'a RunResult,
    main: Option<&'a MainCi>,
    rec: &'a Value,
}

impl Scheduler<'_> {
    /// Record the unit worker `n` ran, which ended with `status`, and move its issue on.
    pub(super) fn settle(
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
            return self.settle_lost(&issue, n, status, tonight);
        };
        let rec = json!({
            "issue": issue.number,
            "unit": r.unit,
            "worker": n,
            "outcome": r.outcome.label(),
            "reason": r.reason,
            "pr": r.json["pr"],
            "cost_usd": r.cost_usd,
        });
        let review = core_settle::review_path(&r).and_then(|a| fs::read_to_string(a).ok());
        let (facts, main) = self.settle_facts(&issue, n, &r, review.as_deref(), night);
        let acts = core_settle::settle(&r, facts);
        let q = &self.plan.loaded.fac.queue;
        let (acts, failed) = forge_part(self.plan.forge, q, issue.number, acts);
        let ended = Ended {
            issue: &issue,
            r: &r,
            main: main.as_ref(),
            rec: &rec,
        };
        for act in acts {
            self.night_part(act, &ended, night, tonight)?;
        }
        failed
    }

    /// What `core::settle` needs of the night for issue `issue`, which worker `n` ran to `r`, and
    /// main's CI when the fix unit ended and it was read.
    fn settle_facts<'r>(
        &self,
        issue: &Issue,
        n: usize,
        r: &RunResult,
        review: Option<&'r str>,
        night: &mut Night,
    ) -> (SettleFacts<'r>, Option<MainCi>) {
        let mut facts = SettleFacts {
            stopping: stop::requested().is_some(),
            over: night.over(self.plan.deadline, &self.plan.loaded.fac),
            review,
            ..SettleFacts::default()
        };
        if core_settle::interrupted(r, facts.stopping) {
            eprintln!("ns watch: #{} interrupted (worker {n})", issue.number);
            return (facts, None);
        }
        facts.failing = night.harness(r);
        facts.harness_fails = night.harness_fails;
        let shown = if facts.failing {
            "harness_failing"
        } else {
            r.outcome.label()
        };
        eprintln!("ns watch: #{} {shown} (worker {n})", issue.number);
        if r.outcome == Outcome::Paused {
            facts.resume_at = resume_at(r, night);
        }
        let mut main = None;
        if self.red.may_be_unfixed(issue.number, r.outcome) && core_settle::checks_main(r, &facts) {
            main = self.main_ci();
            facts.unfixed = core_red::unfixed(true, main.as_ref());
        }
        (facts, main)
    }

    /// Do one of the acts `forge_part` leaves to the scheduler for the unit that `ended`.
    fn night_part(
        &mut self,
        act: SettleAct,
        ended: &Ended,
        night: &mut Night,
        tonight: &mut Tonight,
    ) -> Result<()> {
        let Ended {
            issue,
            r,
            main,
            rec,
        } = *ended;
        let num = issue.number;
        match act {
            SettleAct::Unfixed => {
                eprintln!(
                    "ns watch: fix issue #{num} ended {} and main is still red; the night stops",
                    r.outcome.label()
                );
                run::log_event(
                    &self.plan.repo.common_dir,
                    json!({"event": "main_red_unfixed", "sha": main.and_then(|m| m.sha.clone()), "issue": num, "outcome": r.outcome.label()}),
                );
                self.stop_all(MAIN_RED_UNFIXED);
            }
            SettleAct::StopAll(why) => self.stop_all(&why),
            SettleAct::Pause(reset) => {
                // Paused first, so a hold that can't be written still leaves it to give back.
                self.resume.push(issue.clone());
                self.pause(reset)?;
                eprintln!(
                    "ns watch: #{num} paused on a usage limit until {}; every unit pauses at its next phase",
                    clock::local_iso(self.paused_until.unwrap_or(reset))
                );
            }
            SettleAct::Record {
                outcome,
                reset_at,
                finished,
            } => {
                let mut rec = rec.clone();
                rec["outcome"] = json!(outcome);
                if let Some(t) = reset_at {
                    rec["reset_at"] = json!(clock::local_iso(t));
                }
                tonight.units.push(rec);
                if finished {
                    night.finished.insert(num);
                }
            }
            SettleAct::Label { .. }
            | SettleAct::Close
            | SettleAct::UnflagPr(_)
            | SettleAct::FlagPr { .. }
            | SettleAct::Comment(_) => unreachable!("forge_part does the forge's acts"),
        }
        Ok(())
    }

    /// Settle a unit that left no result: back to the queue, and under a stop recorded as
    /// interrupted; otherwise the night fails, as its `ns run` already said why.
    fn settle_lost(
        &mut self,
        issue: &Issue,
        n: usize,
        status: Option<ExitStatus>,
        tonight: &mut Tonight,
    ) -> Result<()> {
        let q = &self.plan.loaded.fac.queue;
        let forge = self.plan.forge;
        let _ = transitions::apply(
            forge,
            q,
            issue.number,
            State::InProgress,
            Event::Interrupted,
        );
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
        Err(SfError::new(
            code.unwrap_or(1),
            format!(
                "ns run --issue {} ended with {how} and no result; its error is above",
                issue.number
            ),
        )
        .into())
    }
}

/// Do the forge's part of `acts` for issue `n`, in order: its labels, comment and close, and its
/// PR's label. Returns the acts left to the scheduler and whether the forge's part failed. A
/// label move that fails at once drops every act after it; one deferred, or a comment, fails
/// the settle after the rest, but the unit's record is dropped.
pub(super) fn forge_part(
    forge: &dyn Forge,
    q: &QueueConfig,
    n: u64,
    acts: Vec<SettleAct>,
) -> (Vec<SettleAct>, Result<()>) {
    let mut left = Vec::new();
    let mut deferred = None;
    for act in acts {
        match act {
            SettleAct::Label { event, defer } => {
                if let Err(e) = transitions::apply(forge, q, n, State::InProgress, event) {
                    if !defer {
                        return (left, Err(e));
                    }
                    deferred.get_or_insert(e);
                }
            }
            // GitHub may not have closed it yet; an already-closed issue makes gh fail.
            SettleAct::Close => {
                if let Err(e) = forge.close(n) {
                    eprintln!("ns watch: #{n} not closed: {e}");
                }
            }
            SettleAct::UnflagPr(pr) => {
                if let Err(e) = forge.remove_pr_label(pr, &q.human_merge_label) {
                    eprintln!(
                        "ns watch: PR #{pr} still labelled {}? {e:#}",
                        q.human_merge_label
                    );
                }
            }
            SettleAct::FlagPr { pr, body } => {
                flag_human_merge(forge, &q.human_merge_label, pr, &body)
            }
            SettleAct::Comment(body) => {
                if let Err(e) = forge.comment(n, &Post::new(&body)) {
                    deferred.get_or_insert(e);
                }
            }
            SettleAct::Record { .. } if deferred.is_some() => {}
            other => left.push(other),
        }
    }
    (left, deferred.map_or(Ok(()), Err))
}

/// Label a `done` unit's PR with `label` and comment `body`, why it needs a human merge, so the
/// PRs waiting on a human are one `gh pr list --label` away. A failure is reported, never fatal.
fn flag_human_merge(forge: &dyn Forge, label: &str, pr: u64, body: &str) {
    if let Err(e) = forge.add_pr_label(pr, label) {
        eprintln!("ns watch: PR #{pr} not labelled {label}: {e:#}");
    }
    if let Err(e) = forge.comment_pr(pr, &Post::new(body)) {
        eprintln!("ns watch: PR #{pr} not commented: {e:#}");
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

#[cfg(test)]
mod tests;
