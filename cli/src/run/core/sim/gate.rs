//! The gate in the sim: `gate::Gate::after` over [`Sim::gates`]' verdicts. It keeps the last
//! verdict for one invocation as `gate::Gate` does: a green gate at HEAD is not run again (a full
//! one covers a fast one), and a build that writes no artifact reuses a red verdict at HEAD and
//! runs no gate after a green one. A red verdict's output is the red gate's feedback; one that
//! starts `was killed by ` is a gate killed that way, and one that starts `ratchets: ` a gate that
//! failed the ratchets after it.

use std::path::Path;

use super::super::gate::{culprit, Suspect};
use super::super::merge::Red;
use super::super::step::{GateCall, GateRan, GateRed};
use super::super::Ledger;
use super::{Sim, Trace};
use crate::factory::LocalGate;
use crate::gate::Trigger;

/// One gate the sim ran, as the e2e tests' `gate` events record it.
#[derive(Debug, Clone, PartialEq)]
pub struct GateRun {
    /// The phase it followed, `merge` for the merge step's.
    pub phase: &'static str,
    /// That phase's attempt, or the merge step's run.
    pub attempt: u32,
    /// The full gate, not the fast one.
    pub full: bool,
    pub green: bool,
}

/// The last gate's verdict and the HEAD it ran at.
#[derive(Debug, Clone, Default)]
pub(super) enum Verdict {
    #[default]
    None,
    Green {
        sha: String,
        full: bool,
    },
    Red {
        sha: String,
        red: Red,
    },
}

/// Why the unit is stuck if build runs out of attempts on `red`, as `gate::Red::stuck_reason`
/// gives it.
pub(super) fn stuck_reason(red: &Red) -> Option<String> {
    red.killed.clone().or_else(|| {
        red.ratchets
            .as_ref()
            .map(|r| format!("the CI gate failed on {r}"))
    })
}

/// A red gate whose output is `output`, after `phase`.
pub(super) fn red(output: String, phase: &str) -> Red {
    let killed = output
        .strip_prefix("was killed by ")
        .map(|s| format!("the CI gate was killed by {s}"));
    let ratchets = output.strip_prefix("ratchets: ").map(String::from);
    let reason = match (&killed, &ratchets) {
        (Some(k), _) => format!("{k} after {phase}"),
        (None, Some(f)) => format!("the CI gate failed on {f} after {phase}"),
        (None, None) => format!("the CI gate failed after {phase}"),
    };
    Red {
        feedback: output,
        reason,
        killed,
        ratchets,
    }
}

impl Sim {
    /// The red gate's feedback the shell still holds.
    pub(super) fn red_feedback(&self) -> Option<String> {
        match &self.verdict {
            Verdict::Red { red, .. } => Some(red.feedback.clone()),
            _ => None,
        }
    }

    /// The gate `call` asks for after attempt `attempt` of `phase`; on red, the phase run its
    /// output blames.
    pub(super) fn gate_after(
        &mut self,
        call: GateCall,
        (phase, attempt): (&'static str, u32),
        l: &Ledger,
        t: &mut Trace,
    ) -> GateRan {
        if self.gates.is_none() {
            return GateRan::NoCommand;
        }
        let head = self.head();
        let full = matches!(
            call.trigger,
            Trigger::BuildPassed | Trigger::BuildWroteNothing
        ) || self.settings.local_gate == LocalGate::EveryStep;
        let red = match (&call.trigger, &self.verdict) {
            (Trigger::BuildWroteNothing, Verdict::Red { sha, red }) if *sha == head => Some(Red {
                reason: "the CI gate is still red".into(),
                ..red.clone()
            }),
            (Trigger::BuildWroteNothing, Verdict::Red { .. }) => {
                self.run_gate(phase, attempt, full, t)
            }
            (Trigger::BuildWroteNothing, _) => None,
            (_, Verdict::Green { sha, full: f }) if *sha == head && (*f || !full) => None,
            _ => self.run_gate(phase, attempt, full, t),
        };
        let Some(red) = red else {
            return GateRan::Green;
        };
        let now = self.remote_now();
        let failing = crate::gate::failing_paths(&red.feedback);
        let suspects: Vec<Suspect> = l
            .suspects()
            .map(|(index, m)| Suspect {
                index,
                phase: m.phase.clone(),
                changed: Some(self.changed(&m.from, &m.to)),
            })
            .collect();
        let tracked: Vec<String> = self.touched.values().flatten().cloned().collect();
        let culprit = culprit(&failing, &suspects, Path::new("/sim"), &tracked);
        GateRan::Red(GateRed {
            stuck_reason: stuck_reason(&red),
            feedback: red.feedback,
            reason: red.reason,
            moved_since: call
                .rebased_onto
                .is_some_and(|(_, onto)| now.is_some_and(|(_, sha)| sha != onto)),
            culprit: culprit.map(|c| c.0),
        })
    }

    /// Run the gate at HEAD: take the next verdict, record it and keep it.
    fn run_gate(
        &mut self,
        phase: &'static str,
        attempt: u32,
        full: bool,
        t: &mut Trace,
    ) -> Option<Red> {
        let next = self.gates.as_mut().and_then(|g| g.pop_front());
        let sha = self.head();
        let red = next.unwrap_or(Ok(())).err().map(|o| red(o, phase));
        t.gates.push(GateRun {
            phase,
            attempt,
            full,
            green: red.is_none(),
        });
        self.verdict = match &red {
            None => Verdict::Green { sha, full },
            Some(r) => Verdict::Red {
                sha,
                red: r.clone(),
            },
        };
        red
    }

    /// The files the commits after `from` up to `to` change.
    fn changed(&self, from: &str, to: &str) -> Vec<String> {
        let num = |sha: &str| sha[1..].parse::<u32>().expect("a sim commit");
        self.touched
            .range(num(from) + 1..=num(to))
            .flat_map(|(_, f)| f.clone())
            .collect()
    }
}
