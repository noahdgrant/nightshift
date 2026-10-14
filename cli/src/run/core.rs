//! The unit's ledger: every attempt, credit and forced decision `ns run` keeps between phases,
//! and the rules that change them. Pure: the driver does the I/O and reports what happened, and
//! the ledger says what to do next (docs/design/factory-core/v1.md, "State and the transition
//! function").

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::currency::{decide, rebases_for_base, run, Decision, State};
use crate::factory::PHASES;

/// The most times one `ns run` sends a passed ship back because its PR conflicts with the base.
pub const RESHIP_CAP: u32 = 5;

/// Why a build or ship runs only to take in the newest default branch, which costs no attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rebase {
    /// Verify, review, ship or the merge step said the base moved.
    BaseMoved,
    /// The gate after a `BaseMoved` rebuild went red and the base moved again since. Never
    /// leads to another.
    MovedSinceRebase,
}

/// A phase the ledger let run: what to tell it, and which credits it was given.
#[derive(Debug, Clone, PartialEq)]
pub struct Admitted {
    pub phase: &'static str,
    pub attempt: u32,
    pub feedback: String,
    pub why: String,
    /// The free rebase this run makes, with the default branch and the sha it goes onto.
    pub rebase: Option<(Rebase, String, String)>,
    /// The ship after a free ship's verify and review reruns: free too.
    pub follows_base: bool,
    before_credit: Option<String>,
}

/// A phase the ledger did not let run, because it is out of attempts.
#[derive(Debug, Clone, PartialEq)]
pub struct Denied {
    pub reason: String,
    pub attempt: u32,
    /// The run would have been a free ship after a free ship's reruns, which the run log notes
    /// even when the cap stops it.
    pub follows_base: bool,
}

/// What a phase run came to, as the driver saw it.
#[derive(Debug, Clone, Default)]
pub struct PhaseResult {
    /// The artifact's status, `None` when the phase wrote none.
    pub status: Option<String>,
    pub timed_out: bool,
    pub killed: Option<String>,
    /// The timeout, in words, for the out-of-attempts reason.
    pub timeout_text: String,
}

/// What the merge step sent back, when it didn't finish the unit.
#[derive(Debug, Clone)]
pub enum MergeBack {
    /// CI or the local gate went red: build again.
    Rebuild {
        feedback: String,
        reason: String,
        stuck_reason: Option<String>,
    },
    /// The PR conflicts with its base: ship again.
    Reship(String),
    /// The base moved in a way only build can take in.
    Rebase { feedback: String, why: String },
}

/// The counters `ns run` keeps across phases. Serializable, so a restart resumes them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    attempts: BTreeMap<String, u32>,
    reruns: BTreeMap<String, u32>,
    last_failure: BTreeMap<String, String>,
    passed: BTreeSet<String>,
    forced: Option<Forced>,
    forced_rebase: Option<Rebase>,
    /// The default branch's sha the last free rebase went onto: each free one needs a new move.
    rebased_onto: Option<String>,
    reships: u32,
    /// A free ship that brought the base in passed: the ship after the verify and review reruns
    /// it caused is free too.
    ship_follows_base: bool,
    pub merges: u32,
}

/// A decision that skips `decide` on the next step, in a serializable form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Forced {
    phase: String,
    feedback: String,
    why: String,
}

/// The `PHASES` entry named `p`, so a deserialized name becomes the `'static` phase again.
fn phase_named(p: &str) -> &'static str {
    PHASES.iter().find(|x| **x == p).copied().unwrap_or("build")
}

impl Ledger {
    /// A fresh ledger; `from` forces the first phase (`--from`, `--triage-only`).
    pub fn new(from: Option<&str>) -> Self {
        let mut l = Self::default();
        if let Some(f) = from {
            l.force(run(phase_named(f), "", "--from"), None);
        }
        l
    }

    fn force(&mut self, d: Decision, rebase: Option<Rebase>) {
        if let Decision::Run {
            phase,
            feedback,
            why,
        } = d
        {
            self.forced = Some(Forced {
                phase: phase.to_string(),
                feedback,
                why,
            });
            self.forced_rebase = rebase;
        }
    }

    /// The decision to act on: a forced one, else the state table's, with the free rebase it
    /// makes.
    pub fn next(&mut self, s: &State) -> (Decision, Option<Rebase>) {
        match self.forced.take() {
            Some(f) => (
                run(phase_named(&f.phase), &f.feedback, f.why),
                self.forced_rebase.take(),
            ),
            None => {
                let d = decide(s);
                let r = rebases_for_base(s, &d).then_some(Rebase::BaseMoved);
                (d, r)
            }
        }
    }

    /// The decision is `Done` and the merge step is about to run.
    pub fn merging(&mut self) {
        self.ship_follows_base = false;
        self.merges += 1;
    }

    /// The merge step sent the unit back. `Err` carries the stuck reason when a cap is hit.
    pub fn merge_back(&mut self, back: MergeBack) -> Result<(), String> {
        match back {
            MergeBack::Rebuild {
                feedback,
                reason,
                stuck_reason,
            } => {
                if let Some(k) = stuck_reason {
                    self.last_failure.insert("build".into(), k);
                }
                self.force(run("build", &feedback, reason), None);
            }
            MergeBack::Reship(feedback) => {
                self.reships += 1;
                if self.reships > RESHIP_CAP {
                    return Err(format!(
                        "ship was sent back {RESHIP_CAP} times because the base moved (cap {RESHIP_CAP})"
                    ));
                }
                self.force(
                    run("ship", &feedback, "the PR conflicts with its base"),
                    Some(Rebase::BaseMoved),
                );
            }
            MergeBack::Rebase { feedback, why } => {
                self.force(run("build", &feedback, why), Some(Rebase::BaseMoved));
            }
        }
        Ok(())
    }

    fn rerun(&mut self, phase: &str, by: i64) {
        let n = self.reruns.entry(phase.to_string()).or_default();
        *n = n.saturating_add_signed(by as i32);
    }

    /// Charge `phase`'s next attempt and grant its credits. `remote` is the default branch and
    /// its sha now, read only when `rebase` is `Some`.
    pub fn admit(
        &mut self,
        phase: &'static str,
        feedback: String,
        why: String,
        rebase: Option<Rebase>,
        remote: Option<(String, String)>,
        max_attempts: u32,
    ) -> Result<Admitted, Denied> {
        let attempt = self.attempts.get(phase).copied().unwrap_or(0) + 1;
        // The first run of a verify or review after it passed fails nothing: HEAD moved under
        // it. Ship moves HEAD, so its attempts still bound a ship that changes the diff each time.
        if matches!(phase, "verify" | "review") && self.passed.remove(phase) {
            self.rerun(phase, 1);
        }
        // A build that only rebases because the base moved fails nothing either.
        let rebase = rebase.and_then(|r| {
            remote
                .filter(|(_, sha)| self.rebased_onto.as_ref() != Some(sha))
                .map(|(b, s)| (r, b, s))
        });
        let before_credit = self.rebased_onto.clone();
        if let Some((_, _, sha)) = &rebase {
            self.rerun(phase, 1);
            self.rebased_onto = Some(sha.clone());
        }
        let follows_base = match phase {
            "verify" | "review" => false,
            "ship" => std::mem::take(&mut self.ship_follows_base),
            _ => {
                self.ship_follows_base = false;
                false
            }
        };
        if follows_base {
            self.rerun(phase, 1);
        }
        if attempt - self.reruns.get(phase).copied().unwrap_or(0) > max_attempts {
            let mut reason = format!("{phase} is out of attempts ({max_attempts})");
            if let Some(t) = self.last_failure.get(phase) {
                reason.push_str(&format!(": {t}"));
            }
            return Err(Denied {
                reason,
                attempt,
                follows_base,
            });
        }
        Ok(Admitted {
            phase,
            attempt,
            feedback,
            why,
            rebase,
            follows_base,
            before_credit,
        })
    }

    /// The runner locks for `a` were not acquired in time: the attempt counts, its credits are
    /// handed back, and the same decision runs next.
    pub fn lock_gave_up(&mut self, a: Admitted, reason: &str) {
        self.attempts.insert(a.phase.into(), a.attempt);
        self.last_failure
            .insert(a.phase.into(), format!("the last attempt {reason}"));
        let kind = a.rebase.as_ref().map(|(k, _, _)| *k);
        if kind.is_some() {
            self.rerun(a.phase, -1);
            self.rebased_onto = a.before_credit.clone();
        }
        if a.follows_base {
            self.rerun(a.phase, -1);
            self.ship_follows_base = true;
        }
        self.force(run(a.phase, &a.feedback, a.why), kind);
    }

    /// `a` ran and came to `r`.
    pub fn phase_ended(&mut self, a: &Admitted, r: &PhaseResult) {
        self.attempts.insert(a.phase.into(), a.attempt);
        if r.timed_out {
            self.last_failure.insert(
                a.phase.into(),
                format!(
                    "the last attempt {}; raise its timeout_minutes or split the unit",
                    r.timeout_text
                ),
            );
        } else if let Some(k) = &r.killed {
            self.last_failure
                .insert(a.phase.into(), format!("the last attempt {k}"));
        } else {
            self.last_failure.remove(a.phase);
        }
        let pass = r.status.as_deref() == Some("pass");
        if (a.rebase.is_some() || a.follows_base) && !pass {
            self.rerun(a.phase, -1);
        }
        if pass {
            self.passed.insert(a.phase.into());
            self.ship_follows_base |= a.phase == "ship" && a.rebase.is_some();
        }
    }

    /// A build cut short while the gate is still red runs again with the gate's feedback.
    pub fn cut_short_build(&mut self, red_feedback: &str, how: &str) {
        self.force(
            run(
                "build",
                red_feedback,
                format!("{how} with the CI gate still red"),
            ),
            None,
        );
    }

    /// The gate after `a` went red. `moved_since` is true when `a` was a `BaseMoved` rebuild
    /// and the default branch moved again since, which earns one more free rebase.
    pub fn gate_red(
        &mut self,
        a: &Admitted,
        feedback: &str,
        reason: String,
        stuck_reason: Option<String>,
        moved_since: bool,
    ) {
        if let Some(k) = stuck_reason {
            self.last_failure.insert("build".into(), k);
        }
        if moved_since {
            if let Some((Rebase::BaseMoved, branch, _)) = &a.rebase {
                self.force(
                    run(
                        "build",
                        feedback,
                        format!("the CI gate failed after a rebase and {branch} moved since"),
                    ),
                    Some(Rebase::MovedSinceRebase),
                );
                return;
            }
        }
        self.force(run("build", feedback, reason), None);
    }
}

#[cfg(test)]
mod tests;
