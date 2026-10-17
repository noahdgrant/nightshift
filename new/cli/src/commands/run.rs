use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::{Args as ClapArgs, ValueEnum};
use serde_json::{json, Value};

use crate::runlog::{Resume, RunLog};
use crate::state::{Outcome, State};
use crate::states::{self, ship, Context};
use crate::{forge, harness, vcs, workspace};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to take to a merged change: its number on the forge.
    unit: u64,
    /// How many times a state may run before the unit is stuck.
    #[arg(long, default_value_t = 2)]
    max_attempts: u32,
    /// Stop between states for the human, or go on.
    #[arg(long, value_enum, default_value_t = Gates::Stop)]
    gates: Gates,
    /// Merge the change request once its checks pass. Without it, `ship` stops at ready to merge.
    #[arg(long)]
    merge: bool,
    /// How often to look at the change request's checks.
    #[arg(long, default_value_t = 30)]
    ci_poll_seconds: u64,
    /// How long to wait for the checks to finish before the state goes wrong.
    #[arg(long, default_value_t = 1800)]
    ci_timeout_seconds: u64,
}

/// Whether `ns run` stops for the human between states.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Gates {
    /// Stop before each state after the first one this run starts. Run again to go on.
    Stop,
    /// Go on without stopping.
    Auto,
}

/// Walks the unit through its states, from where it last stopped.
pub fn run(args: Args) -> Result<()> {
    let unit = args.unit.to_string();
    let from = std::env::current_dir()?;
    let vcs = vcs::get();
    let root = vcs.root_find(&from)?;
    let log = RunLog::open(vcs.as_ref(), &root)?;
    let start = match log.resume(&unit)? {
        Resume::Fresh => State::START,
        Resume::At(state) => state,
        Resume::Finished => {
            println!(
                "{}",
                json!({ "unit": unit, "state": State::Done.to_string() })
            );
            return Ok(());
        }
    };
    let mut ctx = Context {
        vcs,
        forge: forge::get(),
        harness: harness::get(),
        from,
        unit: args.unit,
        workspace: None,
        ship: ship::Options {
            merge: args.merge,
            poll: Duration::from_secs(args.ci_poll_seconds),
            timeout: Duration::from_secs(args.ci_timeout_seconds),
        },
    };
    if start != State::Setup {
        let workspace = workspace::locate(&root, &unit)?;
        if start != State::CleanUp && !workspace.path.exists() {
            bail!(
                "the workspace of unit {unit} is gone: {}",
                workspace.path.display()
            );
        }
        ctx.workspace = Some(workspace);
    }
    let phase = |state, _attempt| match states::run(&mut ctx, state) {
        Ok(outcome) => outcome,
        Err(e) => {
            eprintln!("ns: {state} went wrong: {e:#}");
            Outcome::Redo
        }
    };
    let mut sink = |event: Value| -> Result<()> {
        println!("{event}");
        if event.get("to").is_some() {
            log.append(&event)?;
        }
        Ok(())
    };
    walk(
        &unit,
        start,
        args.gates,
        args.max_attempts,
        phase,
        &mut sink,
    )
}

/// Moves the unit from `start` to `done`, or to `stuck` when a state would run more than
/// `max_attempts` times in all. With `Gates::Stop` it also stops, ready to go on, before every
/// state after the first. `phase` runs a state given its attempt number (from 1). Every move is
/// passed to `sink` as one JSON event.
fn walk(
    unit: &str,
    start: State,
    gates: Gates,
    max_attempts: u32,
    mut phase: impl FnMut(State, u32) -> Outcome,
    sink: &mut impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    let mut state = start;
    let mut runs: HashMap<State, u32> = HashMap::new();
    sink(json!({ "unit": unit, "state": state.to_string() }))?;
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
        sink(event)?;
        state = to;
        if gates == Gates::Stop && matches!(outcome, Outcome::Pass) && state.next().is_some() {
            sink(json!({ "unit": unit, "waiting": state.to_string() }))?;
            return Ok(());
        }
    }
    if state == State::Stuck {
        bail!("unit {unit} is stuck");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moves(unit_outcomes: impl FnMut(State, u32) -> Outcome) -> (Vec<String>, bool) {
        let mut events = Vec::new();
        let result = walk(
            "u1",
            State::START,
            Gates::Auto,
            2,
            unit_outcomes,
            &mut |e| {
                events.push(e);
                Ok(())
            },
        );
        let moves = events
            .iter()
            .skip(1)
            .map(|v| {
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
            if s == State::CleanUp && attempt == 1 {
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
                "review>ship",
                "ship>clean_up",
                "clean_up>clean_up",
                "clean_up>done",
            ]
        );
    }

    #[test]
    fn a_state_that_keeps_failing_sticks_the_unit() {
        let (moves, ok) = moves(|s, _| {
            if s == State::CleanUp {
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
                "review>ship",
                "ship>clean_up",
                "clean_up>clean_up",
                "clean_up>stuck",
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
