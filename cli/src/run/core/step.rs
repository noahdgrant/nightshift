//! The transition function `ns run` drives: [`step`] takes what the shell observed (an
//! [`Event`]) and returns the one [`Action`] to perform next. Every routing decision is made here;
//! the shell gathers facts and performs actions, and never matches on a phase name to decide
//! anything (docs/design/factory-core/v1.md, "State and the transition function").

use serde_json::{json, Value};

use super::gate::gate_trigger;
use super::route::{blocked, carry_forward, in_review_loop, same_sha, Decision, State};
use super::{Admitted, Ledger, MergeBack, PhaseResult, Rebase, RESHIP_CAP};
use crate::factory::artifact_of;
use crate::gate::Trigger;
use crate::run::Outcome;

/// What the shell observed.
#[derive(Debug, Clone)]
pub enum Event {
    /// Decide the next phase from the state the shell just read (after [`Action::Decide`]).
    Start,
    /// The default branch and its sha, read for [`Action::ReadRemote`]; `None` without an origin.
    Remote(Option<(String, String)>),
    PhaseRan(PhaseRan),
    /// The runner locks were not acquired in time, for this reason.
    LockGaveUp(String),
    GuardFacts(GuardFacts),
    /// The invalid artifact [`Action::SetAside`] named is archived.
    Archived,
    GateRan(GateRan),
    /// The merge step sent the unit back.
    MergeEnded(MergeBack),
}

/// What a phase run came to, as the shell saw it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhaseRan {
    /// The artifact's status when the phase wrote one, valid or not; `None` when it wrote none
    /// (a phase cut short wrote none, whatever it left behind).
    pub artifact: Option<String>,
    /// What `ns artifact check` rejected in it.
    pub invalid: Option<Vec<String>>,
    /// The phase's artifact file exists after a run that wrote none put back what it replaced.
    pub present: bool,
    pub timed_out: bool,
    pub killed: Option<String>,
    /// The timeout, in words.
    pub timeout_text: String,
    pub exit: Option<i32>,
    pub head_before: String,
    pub head_after: String,
    /// The red gate's feedback the shell still holds, if the last gate went red.
    pub red_feedback: Option<String>,
}

impl PhaseRan {
    fn cut_short(&self) -> bool {
        self.timed_out || self.killed.is_some()
    }

    /// The valid artifact's status.
    fn status(&self) -> Option<String> {
        self.artifact.clone().filter(|_| self.invalid.is_none())
    }
}

/// What the hard rules after a phase found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuardFacts {
    /// Why the unit stops: the default branch moved because of this run, or a phase merged a PR.
    pub breach: Option<String>,
}

/// How the gate after a phase came out.
#[derive(Debug, Clone, PartialEq)]
pub enum GateRan {
    /// No gate command is set for the trigger.
    NoCommand,
    Green,
    Red(GateRed),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GateRed {
    pub feedback: String,
    pub reason: String,
    /// Why the unit would end stuck if build runs out of attempts on this red.
    pub stuck_reason: Option<String>,
    /// The default branch moved since the rebase [`GateCall::rebased_onto`] named.
    pub moved_since: bool,
    /// The index of the move (`Ledger::suspects`) the failure points at.
    pub culprit: Option<usize>,
}

/// The gate to run after a phase.
#[derive(Debug, Clone, PartialEq)]
pub struct GateCall {
    pub trigger: Trigger,
    /// The default branch and the sha a free `BaseMoved` rebase went onto: the shell reports
    /// whether the branch moved since.
    pub rebased_onto: Option<(String, String)>,
}

/// What the shell does next.
#[derive(Debug, Clone)]
pub enum Action {
    /// Save the ledger, read the unit's state, and send [`Event::Start`].
    Decide,
    /// Read the default branch and its sha, and send [`Event::Remote`].
    ReadRemote,
    RunPhase(Admitted),
    /// Check the hard rules after the phase, and send [`Event::GuardFacts`].
    CheckGuards,
    /// Archive the phase's invalid artifact, log why, and send [`Event::Archived`]. With `undo`,
    /// also put back what the phase replaced: an invalid pass inside a review loop leaves the
    /// loop as it was before the pass.
    SetAside {
        phase: &'static str,
        errors: Vec<String>,
        undo: bool,
    },
    RunGate(GateCall),
    /// Run the merge step, numbered `run` in this invocation, and send [`Event::MergeEnded`].
    Merge {
        run: u32,
    },
    Finish(Ending),
}

/// How the run ends.
#[derive(Debug, Clone, PartialEq)]
pub struct Ending {
    pub outcome: Outcome,
    pub reason: String,
    pub phase: Option<&'static str>,
    /// The artifact a human reads next.
    pub artifact: Option<Last>,
    /// A run-log event to write before the run ends.
    pub log: Option<Value>,
}

/// The artifact a human reads next.
#[derive(Debug, Clone, PartialEq)]
pub enum Last {
    /// This file, unless the run already names a later one.
    File(&'static str),
    /// This file, archived first.
    Archived(&'static str),
    /// This file, its frontmatter rewritten `status: blocked`, `next: human` with this reason.
    Blocked(&'static str, String),
}

fn end(outcome: Outcome, reason: impl Into<String>, phase: Option<&'static str>) -> Action {
    Action::Finish(Ending {
        outcome,
        reason: reason.into(),
        phase,
        artifact: None,
        log: None,
    })
}

/// Where the ledger is in one turn: what it asked the shell for, and what it keeps meanwhile.
#[derive(Debug, Clone, PartialEq)]
pub enum Turn {
    /// Waiting for the default branch, to admit this phase with a free rebase.
    Admitting {
        phase: &'static str,
        feedback: String,
        why: String,
        rebase: Rebase,
        max_attempts: u32,
    },
    Running(Admitted),
    Guarding(Admitted, PhaseRan),
    SettingAside,
    Gating(Admitted),
    Merging,
}

/// Take `ev` and return what to do next. `s` is read only on [`Event::Start`]; for the other
/// events the shell may pass the state it read last.
pub fn step(l: &mut Ledger, s: &State, ev: Event) -> Action {
    let turn = l.turn.take();
    match (ev, turn) {
        (Event::Start, _) => decide_next(l, s),
        (
            Event::Remote(remote),
            Some(Turn::Admitting {
                phase,
                feedback,
                why,
                rebase,
                max_attempts,
            }),
        ) => admit(l, phase, feedback, why, Some(rebase), remote, max_attempts),
        (Event::PhaseRan(ran), Some(Turn::Running(a))) => phase_ran(l, a, ran),
        (Event::LockGaveUp(reason), Some(Turn::Running(a))) => {
            l.lock_gave_up(a, &reason);
            Action::Decide
        }
        (Event::GuardFacts(g), Some(Turn::Guarding(a, ran))) => guarded(l, s, a, &ran, g),
        (Event::Archived, Some(Turn::SettingAside)) => Action::Decide,
        (Event::GateRan(g), Some(Turn::Gating(a))) => gated(l, &a, g),
        (Event::MergeEnded(back), Some(Turn::Merging)) => match l.merge_back(back) {
            Ok(()) => Action::Decide,
            Err(reason) => Action::Finish(Ending {
                log: Some(json!({"event": "reship_cap", "reships": RESHIP_CAP, "cap": RESHIP_CAP})),
                ..ending(Outcome::Stuck, reason, Some("ship"))
            }),
        },
        (ev, turn) => end(
            Outcome::Stuck,
            format!("ns run got {ev:?} out of turn ({turn:?}); this is a bug in ns"),
            None,
        ),
    }
}

fn ending(outcome: Outcome, reason: String, phase: Option<&'static str>) -> Ending {
    Ending {
        outcome,
        reason,
        phase,
        artifact: None,
        log: None,
    }
}

/// The top of the loop: the decision the state table (or a forced one) makes at `s`.
fn decide_next(l: &mut Ledger, s: &State) -> Action {
    let mut s2 = s.clone();
    s2.carried.extend(carry_forward(l, s));
    let (decision, rebase) = l.next(&s2);
    match decision {
        Decision::Stuck(r) => Action::Finish(Ending {
            artifact: blocked(s).map(|p| Last::File(artifact_of(p))),
            ..ending(Outcome::Stuck, r, None)
        }),
        Decision::LoopSpent(r) => Action::Finish(Ending {
            artifact: Some(Last::Blocked(artifact_of("review"), r.clone())),
            ..ending(Outcome::Stuck, r, None)
        }),
        Decision::Split(r) => Action::Finish(Ending {
            artifact: Some(Last::Archived(artifact_of("triage"))),
            ..ending(Outcome::Split, r, Some("triage"))
        }),
        Decision::Done if !s.settings.merge_auto => {
            end(Outcome::Done, "pr.md passed", Some("ship"))
        }
        Decision::Done => {
            l.merging();
            l.turn = Some(Turn::Merging);
            Action::Merge { run: l.merges }
        }
        Decision::Run {
            phase,
            feedback,
            why,
        } => {
            let max_attempts = s.settings.max_attempts(phase);
            match rebase {
                Some(rebase) => {
                    l.turn = Some(Turn::Admitting {
                        phase,
                        feedback,
                        why,
                        rebase,
                        max_attempts,
                    });
                    Action::ReadRemote
                }
                None => admit(l, phase, feedback, why, None, None, max_attempts),
            }
        }
    }
}

fn admit(
    l: &mut Ledger,
    phase: &'static str,
    feedback: String,
    why: String,
    rebase: Option<Rebase>,
    remote: Option<(String, String)>,
    max_attempts: u32,
) -> Action {
    match l.admit(phase, feedback, why, rebase, remote, max_attempts) {
        Ok(a) => {
            l.turn = Some(Turn::Running(a.clone()));
            Action::RunPhase(a)
        }
        Err(d) => Action::Finish(Ending {
            log: d
                .follows_base
                .then(|| json!({"event": "ship_after_base_moved", "attempt": d.attempt})),
            ..ending(Outcome::Stuck, d.reason, Some(phase))
        }),
    }
}

fn phase_ran(l: &mut Ledger, a: Admitted, ran: PhaseRan) -> Action {
    l.phase_ended(
        &a,
        &PhaseResult {
            status: ran.status(),
            timed_out: ran.timed_out,
            killed: ran.killed.clone(),
            timeout_text: ran.timeout_text.clone(),
        },
    );
    l.turn = Some(Turn::Guarding(a, ran));
    Action::CheckGuards
}

/// After the guards passed: a rewrite of an invalid artifact, the end of a triage-only run, the
/// gate, or the next decision.
fn guarded(l: &mut Ledger, s: &State, a: Admitted, ran: &PhaseRan, g: GuardFacts) -> Action {
    let phase = a.phase;
    if let Some(reason) = g.breach {
        return end(Outcome::Stuck, reason, Some(phase));
    }
    let head_moved = !same_sha(&ran.head_before, &ran.head_after);
    if head_moved {
        l.head_moved(phase, &ran.head_before, &ran.head_after);
    }
    let triage_only = s.settings.triage_only;
    if let (Some(errors), false) = (&ran.invalid, triage_only) {
        l.artifact_invalid(&a, artifact_of(phase), errors);
        l.turn = Some(Turn::SettingAside);
        return Action::SetAside {
            phase,
            errors: errors.clone(),
            undo: phase == "review" && in_review_loop(s),
        };
    }
    if triage_only {
        return triage_only_end(ran, phase);
    }
    if phase == "triage"
        && ran.artifact.is_none()
        && ran.exit == Some(0)
        && !ran.cut_short()
        && !ran.present
    {
        return end(
            Outcome::Stuck,
            "triage wrote no brief: the issue needs a human or ns-define",
            Some("triage"),
        );
    }
    if ran.cut_short() && phase == "build" {
        if let Some(fb) = &ran.red_feedback {
            let how = ran.killed.as_deref().unwrap_or("timed out");
            l.cut_short_build(fb, how);
        }
        return Action::Decide;
    }
    let wrote_no_fix = ran.cut_short() || ran.exit != Some(0) || ran.artifact.is_none();
    if phase == "review-fix" && wrote_no_fix && l.cut_short_gate_fix() {
        return Action::Decide;
    }
    match gate_trigger(
        phase,
        ran.artifact.as_deref(),
        head_moved,
        s.settings.local_gate,
    ) {
        Some(trigger) => {
            let rebased_onto = match &a.rebase {
                Some((Rebase::BaseMoved, branch, onto)) => Some((branch.clone(), onto.clone())),
                _ => None,
            };
            l.turn = Some(Turn::Gating(a));
            Action::RunGate(GateCall {
                trigger,
                rebased_onto,
            })
        }
        None => Action::Decide,
    }
}

/// A triage-only run ends on triage's exit.
fn triage_only_end(ran: &PhaseRan, phase: &'static str) -> Action {
    match (ran.timed_out, &ran.killed, ran.exit) {
        (true, _, _) => end(Outcome::Stuck, ran.timeout_text.clone(), Some(phase)),
        (false, Some(k), _) => end(Outcome::Stuck, k.clone(), Some(phase)),
        (false, None, Some(0)) => end(Outcome::Done, "triage ran", Some(phase)),
        (false, None, Some(c)) => end(Outcome::Stuck, format!("exited {c}"), Some(phase)),
        (false, None, None) => end(Outcome::Stuck, "killed by a signal", Some(phase)),
    }
}

fn gated(l: &mut Ledger, a: &Admitted, g: GateRan) -> Action {
    match g {
        GateRan::NoCommand => {}
        GateRan::Green => l.gate_green(),
        GateRan::Red(r) => l.gate_red(
            a,
            &r.feedback,
            r.reason,
            r.stuck_reason,
            r.moved_since,
            r.culprit,
        ),
    }
    Action::Decide
}
