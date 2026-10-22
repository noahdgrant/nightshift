use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context as _, Result};
use clap::{Args as ClapArgs, ValueEnum};
use serde_json::{json, Value};

use crate::artifact;
use crate::forge::ChangeState;
use crate::runlog::{Move, Reason, Resume, RunLog};
use crate::state::{Outcome, State};
use crate::states::{self, clean_up, ship, Context, BUILT_SNAPSHOT};
use crate::transcripts::Transcripts;
use crate::{auto, forge, harness, lock, manifest, os, status, vcs, workspace};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to take to a merged change: its number on the forge.
    unit: u64,
    /// Delete the unit's workspace and branch, whatever they hold, and start over from `setup`.
    #[arg(long)]
    fresh: bool,
    /// Run the unit although the units that block it are open.
    #[arg(long)]
    ignore_blockers: bool,
    /// Stop between states for the human, or go on.
    #[arg(long, value_enum, default_value_t = Gates::Stop)]
    gates: Gates,
    #[command(flatten)]
    options: Options,
}

/// What a run of a unit is told, which `ns auto` passes on to each unit it runs.
#[derive(ClapArgs, Clone)]
pub struct Options {
    /// How many times a state may run before the unit is stuck. Without it, `[run] max_attempts`
    /// in `.nightshift/nightshift.toml`, or 3 when the repository does not set it.
    #[arg(long)]
    pub max_attempts: Option<u32>,
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
            ignore_blockers: false,
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
    let max_attempts = match args.options.max_attempts {
        Some(n) => n,
        None => manifest::max_attempts_read(&root)?,
    };
    let keep = manifest::transcript_days_read(&root)?;
    // Read here so a bad value stops `ns` before it runs anything.
    manifest::review_sessions_read(&root)?;
    let queue = manifest::merge_queue_read(&root)?;
    let max_base_moves = manifest::max_base_moves_read(&root)?;
    // Held until the function ends. Taken before anything of the unit's is read or deleted.
    let _lock = lock::lock_take(
        &vcs.state_dir_find(&root)?,
        &format!("unit-{unit}.lock"),
        &format!("unit {unit} is already being run"),
    )?;
    Transcripts::open(vcs.as_ref(), &root)?.prune(keep, SystemTime::now());
    let log = RunLog::open(vcs.as_ref(), &root)?;
    // A unit that has begun is not stopped, so that resuming it never hits a blocker.
    let begun = !args.fresh
        && (!matches!(log.resume(&unit)?, Resume::Fresh)
            || workspace::locate(&root, &unit)?.path.exists());
    if !begun && !args.ignore_blockers {
        blockers_check(forge::get().as_ref(), &root, args.unit)?;
    }
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
            queue,
            max_base_moves,
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
    // The assignee is for people to read; a unit does not depend on it.
    if let Err(e) = ctx.forge.unit_assignee_add(&root, args.unit) {
        eprintln!("ns: cannot assign the unit: {e:#}");
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
    let ended = walk(&unit, start, args.gates, max_attempts, phase, &mut sink);
    if ended.is_err() {
        if let Err(e) = unproven_comment_add(&ctx, &log, &root, &unit) {
            eprintln!("ns: cannot say what verification left unproven: {e:#}");
        }
    }
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

/// Stops a unit that open units block, after printing them as one JSON line. The blockers are those
/// the forge names and those its `Blocked by:` line names.
fn blockers_check(forge: &dyn forge::Forge, root: &Path, unit: u64) -> Result<()> {
    let open = forge.unit_list(root)?;
    let Some(this) = open.iter().find(|u| u.number == unit) else {
        return Ok(());
    };
    let native = HashMap::from([(unit, forge::blockers_get(forge, root, unit))]);
    let blockers = auto::blockers_of(this, &open, &native);
    if blockers.is_empty() {
        return Ok(());
    }
    println!("{}", json!({ "unit": unit, "blocked_by": blockers }));
    bail!("unit {unit} is blocked; run it anyway with --ignore-blockers")
}

/// Tells the forge what verification could not prove, for a unit that got stuck because of it.
fn unproven_comment_add(ctx: &Context, log: &RunLog, root: &Path, unit: &str) -> Result<()> {
    let stuck_unproven = matches!(
        log.last_move(unit)?,
        Some(Move {
            to: State::Stuck,
            reason: Some(Reason::Inconclusive),
            ..
        })
    );
    if !stuck_unproven {
        return Ok(());
    }
    let dir = &ctx
        .workspace
        .as_ref()
        .context("a stuck unit has no workspace")?
        .path;
    let note = states::unproven_note(dir, unit)?;
    ctx.forge.unit_comment_add(root, ctx.unit, &note)
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
    // Nothing was wrong with the build, so the build has nothing to change.
    if matches!(
        log.last_move(unit)?,
        Some(Move {
            reason: Some(Reason::Inconclusive),
            ..
        })
    ) {
        return Ok(stuck_in);
    }
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
            Outcome::Unprovable | Outcome::BaseMoved => State::Stuck,
            Outcome::Fail | Outcome::Redo | Outcome::Inconclusive => {
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
            event["reason"] = match outcome {
                Outcome::Redo => "went_wrong",
                Outcome::Pass | Outcome::Fail => "failed",
                Outcome::Inconclusive | Outcome::Unprovable => "inconclusive",
                Outcome::BaseMoved => "base_moved",
            }
            .into();
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
        moves_with(2, unit_outcomes)
    }

    fn moves_with(
        max_attempts: u32,
        unit_outcomes: impl FnMut(State, u32) -> Outcome,
    ) -> (Vec<String>, bool) {
        let mut events = Vec::new();
        let result = walk(
            "u1",
            State::START,
            Gates::Auto,
            max_attempts,
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

    #[test]
    fn an_inconclusive_state_runs_again_and_one_that_is_unprovable_sticks() {
        let (moves, ok) = moves(|s, attempt| match (s, attempt) {
            (State::Verify, 1) => Outcome::Inconclusive,
            _ => Outcome::Pass,
        });
        assert!(ok);
        assert_eq!(
            &moves[1..4],
            ["build>verify", "verify>verify", "verify>review"]
        );
        let (moves, ok) = moves_with(1, |s, _| match s {
            State::Verify => Outcome::Unprovable,
            _ => Outcome::Pass,
        });
        assert!(!ok);
        assert_eq!(moves.last().unwrap(), "verify>stuck");
        let (moves, _) = self::moves(|s, _| match s {
            State::Verify => Outcome::Inconclusive,
            _ => Outcome::Pass,
        });
        assert_eq!(moves.last().unwrap(), "verify>stuck");
    }

    /// The reason on each event that has one, for a unit whose every state ends as `outcome`.
    fn reasons(outcome: fn() -> Outcome) -> Vec<String> {
        let mut events = Vec::new();
        let _ = walk(
            "u1",
            State::START,
            Gates::Auto,
            2,
            |_, _| outcome(),
            &mut |e| {
                events.push(e);
                Ok(())
            },
        );
        events
            .iter()
            .filter_map(|e| e["reason"].as_str().map(String::from))
            .collect()
    }

    #[test]
    fn a_state_that_returns_an_error_is_logged_as_gone_wrong_and_a_failed_check_as_failed() {
        assert_eq!(reasons(|| Outcome::Redo), ["went_wrong", "went_wrong"]);
        assert_eq!(reasons(|| Outcome::Fail), ["failed", "failed"]);
        assert_eq!(
            reasons(|| Outcome::Inconclusive),
            ["inconclusive", "inconclusive"]
        );
        assert_eq!(reasons(|| Outcome::Unprovable), ["inconclusive"]);
    }
}
