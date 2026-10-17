use std::collections::HashMap;
use std::io::Write;

use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::{json, Value};

use crate::state::{Outcome, State};
use crate::states::{self, Context};
use crate::{forge, harness, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to take to a merged change: its number on the forge.
    unit: u64,
    /// How many times a state may run before the unit is stuck.
    #[arg(long, default_value_t = 2)]
    max_attempts: u32,
}

/// Walks the unit through its states. `setup`, `build` and `verify` do work so far.
pub fn run(args: Args) -> Result<()> {
    let mut ctx = Context {
        vcs: vcs::get(),
        forge: forge::get(),
        harness: harness::get(),
        from: std::env::current_dir()?,
        unit: args.unit,
        workspace: None,
    };
    let phase = |state, _attempt| match states::run(&mut ctx, state) {
        Ok(outcome) => outcome,
        Err(e) => {
            eprintln!("ns: {state} went wrong: {e:#}");
            Outcome::Redo
        }
    };
    walk(
        &args.unit.to_string(),
        args.max_attempts,
        phase,
        &mut std::io::stdout(),
    )
}

/// Moves the unit from the start state to `done`, or to `stuck` when a state would run more than
/// `max_attempts` times in all. `phase` runs a state given its attempt number (from 1). Each move
/// is printed as one JSON line.
fn walk(
    unit: &str,
    max_attempts: u32,
    mut phase: impl FnMut(State, u32) -> Outcome,
    out: &mut impl Write,
) -> Result<()> {
    let mut state = State::START;
    let mut runs: HashMap<State, u32> = HashMap::new();
    emit(out, json!({ "unit": unit, "state": state.to_string() }))?;
    while let Some(next) = state.next() {
        let attempt = runs.entry(state).or_insert(0);
        *attempt += 1;
        let attempt = *attempt;
        let outcome = phase(state, attempt);
        let to = match outcome {
            Outcome::Pass => next,
            Outcome::Fail | Outcome::Redo => {
                let back = match outcome {
                    Outcome::Fail => state.on_fail(),
                    _ => state,
                };
                if runs.get(&back).copied().unwrap_or(0) < max_attempts {
                    back
                } else {
                    State::Stuck
                }
            }
        };
        let mut event = json!({ "unit": unit, "from": state.to_string(), "to": to.to_string() });
        if to != next {
            event["attempt"] = attempt.into();
            event["reason"] = "failed".into();
        }
        emit(out, event)?;
        state = to;
    }
    if state == State::Stuck {
        bail!("unit {unit} is stuck");
    }
    Ok(())
}

fn emit(out: &mut impl Write, event: Value) -> Result<()> {
    writeln!(out, "{event}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moves(unit_outcomes: impl FnMut(State, u32) -> Outcome) -> (Vec<String>, bool) {
        let mut out = Vec::new();
        let result = walk("u1", 2, unit_outcomes, &mut out);
        let moves = String::from_utf8(out)
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| {
                let v: Value = serde_json::from_str(l).unwrap();
                format!(
                    "{}>{}",
                    v["from"].as_str().unwrap(),
                    v["to"].as_str().unwrap()
                )
            })
            .collect();
        (moves, result.is_ok())
    }

    #[test]
    fn a_failure_is_retried_then_passes() {
        let (moves, ok) = moves(|s, attempt| {
            if s == State::Review && attempt == 1 {
                Outcome::Fail
            } else {
                Outcome::Pass
            }
        });
        assert!(ok);
        assert_eq!(
            moves,
            [
                "setup>build",
                "build>verify",
                "verify>review",
                "review>review",
                "review>ship",
                "ship>clean_up",
                "clean_up>done",
            ]
        );
    }

    #[test]
    fn a_state_that_keeps_failing_sticks_the_unit() {
        let (moves, ok) = moves(|s, _| {
            if s == State::Review {
                Outcome::Fail
            } else {
                Outcome::Pass
            }
        });
        assert!(!ok);
        assert_eq!(
            moves,
            [
                "setup>build",
                "build>verify",
                "verify>review",
                "review>review",
                "review>stuck",
            ]
        );
    }

    #[test]
    fn a_failed_verify_goes_back_to_build() {
        let (moves, ok) = moves(|s, attempt| {
            if s == State::Verify && attempt == 1 {
                Outcome::Fail
            } else {
                Outcome::Pass
            }
        });
        assert!(ok);
        assert_eq!(
            moves,
            [
                "setup>build",
                "build>verify",
                "verify>build",
                "build>verify",
                "verify>review",
                "review>ship",
                "ship>clean_up",
                "clean_up>done",
            ]
        );
    }

    #[test]
    fn going_back_to_build_is_limited_by_its_attempts() {
        let (moves, ok) = moves(|s, _| {
            if s == State::Verify {
                Outcome::Fail
            } else {
                Outcome::Pass
            }
        });
        assert!(!ok);
        assert_eq!(
            moves,
            [
                "setup>build",
                "build>verify",
                "verify>build",
                "build>verify",
                "verify>stuck",
            ]
        );
    }

    #[test]
    fn a_state_that_goes_wrong_is_run_again_even_when_its_failure_goes_back() {
        let (moves, ok) = moves(|s, attempt| match (s, attempt) {
            (State::Verify, 1) => Outcome::Redo,
            _ => Outcome::Pass,
        });
        assert!(ok);
        assert_eq!(
            moves,
            [
                "setup>build",
                "build>verify",
                "verify>verify",
                "verify>review",
                "review>ship",
                "ship>clean_up",
                "clean_up>done",
            ]
        );
    }
}
