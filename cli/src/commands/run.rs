use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use clap::{Args as ClapArgs, ValueEnum};
use serde_json::{json, Value};

use crate::artifact;
use crate::forge::ChangeState;
use crate::runlog::{Resume, RunLog};
use crate::state::{Outcome, State};
use crate::states::{self, clean_up, ship, Context, BUILT_SNAPSHOT};
use crate::{forge, harness, lock, os, status, vcs, workspace};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to take to a merged change: its number on the forge.
    unit: u64,
    /// Delete the unit's workspace and branch, whatever they hold, and start over from `setup`.
    #[arg(long)]
    fresh: bool,
    /// Stop between states for the human, or go on.
    #[arg(long, value_enum, default_value_t = Gates::Stop)]
    gates: Gates,
    #[command(flatten)]
    options: Options,
}

/// What a run of a unit is told, which `ns auto` passes on to each unit it runs.
#[derive(ClapArgs, Clone)]
pub struct Options {
    /// How many times a state may run before the unit is stuck.
    #[arg(long, default_value_t = 2)]
    max_attempts: u32,
    /// Merge the change request once its checks pass. Without it, `ship` stops at ready to merge.
    #[arg(long)]
    merge: bool,
    /// How often to look at the change request's checks.
    #[arg(long, default_value_t = 30)]
    ci_poll_seconds: u64,
    /// How long to wait for the checks to finish before the state goes wrong.
    #[arg(long, default_value_t = 1800)]
    ci_timeout_seconds: u64,
    /// How long to wait for the first check to be reported. After that a repository with no
    /// checks is taken to have none.
    #[arg(long, default_value_t = 120)]
    ci_start_seconds: u64,
    /// How long a harness session may take before it is killed and the state goes wrong.
    #[arg(long, default_value_t = 3600)]
    pub harness_timeout_seconds: u64,
    /// How long the repository's checks may take before they are killed and count as failed.
    #[arg(long, default_value_t = 1800)]
    checks_timeout_seconds: u64,
}

impl Args {
    /// A fresh or resumed run of `unit` that goes on between states without a human.
    pub fn unattended(unit: u64, options: Options) -> Args {
        Args {
            unit,
            fresh: false,
            gates: Gates::Auto,
            options,
        }
    }
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
    // Held until the function ends. Taken before anything of the unit's is read or deleted.
    let _lock = lock::lock_take(
        &vcs.state_dir_find(&root)?,
        &format!("unit-{unit}.lock"),
        &format!("unit {unit} is already being run"),
    )?;
    let log = RunLog::open(vcs.as_ref(), &root)?;
    let resume = if args.fresh {
        workspace::discard(vcs.as_ref(), &from, &unit)?;
        Resume::Fresh
    } else {
        log.resume(&unit)?
    };
    let mut ctx = Context {
        vcs,
        forge: forge::get(),
        harness: harness::get(),
        os: os::get(),
        from,
        unit: args.unit,
        workspace: None,
        ship: ship::Options {
            merge: args.options.merge,
            poll: Duration::from_secs(args.options.ci_poll_seconds),
            timeout: Duration::from_secs(args.options.ci_timeout_seconds),
            start: Duration::from_secs(args.options.ci_start_seconds),
        },
        harness_timeout: Duration::from_secs(args.options.harness_timeout_seconds),
        checks_timeout: Duration::from_secs(args.options.checks_timeout_seconds),
    };
    let planned = match resume {
        Resume::Fresh => State::START,
        Resume::At(state) | Resume::Stuck(state) => state,
        Resume::Finished => {
            println!(
                "{}",
                json!({ "unit": unit, "state": State::Done.to_string() })
            );
            return finished_clean_up(ctx);
        }
    };
    if planned != State::Setup {
        let workspace = workspace::locate(&root, &unit)?;
        if planned != State::CleanUp && !workspace.path.exists() {
            bail!(
                "the workspace of unit {unit} is gone: {}",
                workspace.path.display()
            );
        }
        ctx.workspace = Some(workspace);
    }
    let start = match resume {
        Resume::Stuck(stuck_in @ (State::Verify | State::Review)) => {
            stuck_check_resume(&ctx, &log, &unit, stuck_in)?
        }
        Resume::Fresh
        | Resume::At(_)
        | Resume::Finished
        | Resume::Stuck(
            State::Setup | State::Build | State::Ship | State::CleanUp | State::Done | State::Stuck,
        ) => planned,
    };
    status::unit_status_set(ctx.forge.as_ref(), &root, args.unit, status::IN_PROGRESS)?;
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
    let ended = walk(
        &unit,
        start,
        args.gates,
        args.options.max_attempts,
        phase,
        &mut sink,
    );
    let status = if matches!(ended, Ok(State::Done)) {
        finished_status_set(&ctx, &root, &unit)
    } else {
        // Waiting at a gate, stuck or gone wrong: a human is next.
        status::unit_status_set(
            ctx.forge.as_ref(),
            &root,
            args.unit,
            status::READY_FOR_HUMAN,
        )
    };
    match (ended, status) {
        (Err(e), Err(s)) => {
            eprintln!("ns: cannot set the status of unit {unit}: {s:#}");
            Err(e)
        }
        (Err(e), Ok(())) => Err(e),
        (Ok(_), status) => status,
    }
}

/// Where a unit stuck in the check `stuck_in` picks up. A workspace exactly as the last build left
/// it holds nothing the failed check could judge differently, so the unit goes back to `build`,
/// which is told the failing findings. Otherwise a human worked on it, and the check runs again.
/// The move is printed and logged like any other.
fn stuck_check_resume(ctx: &Context, log: &RunLog, unit: &str, stuck_in: State) -> Result<State> {
    let dir = &ctx
        .workspace
        .as_ref()
        .context("a stuck unit has no workspace")?
        .path;
    let snapshot = ctx.vcs.workspace_snapshot(dir)?;
    match artifact::read(dir, unit, BUILT_SNAPSHOT) {
        Ok(built) if built == snapshot => {
            let event = json!({
                "unit": unit,
                "from": State::Stuck.to_string(),
                "to": State::Build.to_string(),
                "reason": format!("nothing changed since {stuck_in} failed"),
            });
            println!("{event}");
            log.append(&event)?;
            Ok(State::Build)
        }
        _ => Ok(stuck_in),
    }
}

/// Cleans up after a unit whose run is done, once its change request has merged since: the
/// run ended with it open, and nothing else cleans up after the human merges. The unit's status
/// goes with it. A change request still open changes nothing.
fn finished_clean_up(mut ctx: Context) -> Result<()> {
    if clean_up::merged_clean_up(&mut ctx)? {
        let root = ctx.vcs.root_find(&ctx.from)?;
        status::unit_status_clear(ctx.forge.as_ref(), &root, ctx.unit)?;
    }
    Ok(())
}

/// Sets the status of a unit whose states are all done: none when its change request merged,
/// otherwise it is ready to merge and waits for a human.
fn finished_status_set(ctx: &Context, root: &Path, unit: &str) -> Result<()> {
    let branch = workspace::locate(root, unit)?.branch;
    let merged = matches!(
        ctx.forge.change_request_get(root, &branch)?,
        Some(cr) if cr.state == ChangeState::Merged
    );
    if merged {
        status::unit_status_clear(ctx.forge.as_ref(), root, ctx.unit)
    } else {
        status::unit_status_set(ctx.forge.as_ref(), root, ctx.unit, status::READY_FOR_HUMAN)
    }
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
) -> Result<State> {
    let mut state = start;
    let mut stuck_in = start;
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
        if to == State::Stuck {
            stuck_in = state;
        }
        state = to;
        if gates == Gates::Stop && matches!(outcome, Outcome::Pass) && state.next().is_some() {
            sink(json!({ "unit": unit, "waiting": state.to_string() }))?;
            return Ok(state);
        }
    }
    if state == State::Stuck {
        bail!("unit {unit} is stuck in {stuck_in}");
    }
    Ok(state)
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
