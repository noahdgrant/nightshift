use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context as _, Result};
use chrono::{DateTime, Local};
use clap::{Args as ClapArgs, ValueEnum};
use serde_json::{json, Value};

use crate::artifact;
use crate::forge::ChangeState;
use crate::manifest::Place;
use crate::notify::{self, Facts, Move as Notice, Settings};
use crate::runlog::{Move, Reason, Resume, RunLog};
use crate::state::{Outcome, State};
use crate::states::ship::Refresh;
use crate::states::{self, clean_up, ship, Context, Done, BUILT_SNAPSHOT};
use crate::transcripts::Transcripts;
use crate::{auto, config, forge, harness, lock, manifest, os, status, vcs, workspace};

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
    /// Whether `ns auto` started the run, which never refreshes an open change request.
    #[arg(skip)]
    from_auto: bool,
    /// The moment `ns auto` stops, which a usage limit must not make a run wait past.
    #[arg(skip)]
    until: Option<DateTime<Local>>,
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
    /// How long to wait for the checks to finish before the state goes wrong. Without it,
    /// `[ship] ci_timeout_seconds` in `.nightshift/nightshift.toml`, or 1800 when the repository
    /// does not set it.
    #[arg(long)]
    pub ci_timeout_seconds: Option<u64>,
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
    /// The model every session uses, instead of the ones `[model]` in
    /// `.nightshift/nightshift.toml` names.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub model: Option<String>,
}

impl Args {
    /// A fresh or resumed run of `unit` that goes on between states without a human.
    pub fn unattended(unit: u64, options: Options, until: Option<DateTime<Local>>) -> Args {
        Args {
            unit,
            fresh: false,
            ignore_blockers: false,
            gates: Gates::Auto,
            options,
            from_auto: true,
            until,
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
    // Read first so a bad value stops `ns` before it runs anything.
    let notify = config::notify()?;
    let from = std::env::current_dir()?;
    let vcs = vcs::get();
    let root = vcs.root_find(&from)?;
    let max_attempts = match args.options.max_attempts {
        Some(n) => n,
        None => manifest::max_attempts_read(&root)?,
    };
    let ci_timeout = match args.options.ci_timeout_seconds {
        Some(n) => n,
        None => manifest::ci_timeout_read(&root)?,
    };
    let keep = manifest::transcript_days_read(&root)?;
    // Read here so a bad value stops `ns` before it runs anything.
    manifest::review_sessions_read(&root)?;
    manifest::retrospective_share_read(&root)?;
    manifest::model_read(&root, Place::Build, None)?;
    manifest::retrospective_read(&root)?;
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
            timeout: Duration::from_secs(ci_timeout),
            start: Duration::from_secs(args.options.ci_start_seconds),
            queue,
            max_base_moves,
        },
        harness_timeout: Duration::from_secs(args.options.harness_timeout_seconds),
        checks_timeout: Duration::from_secs(args.options.checks_timeout_seconds),
        model: args.options.model.clone(),
        until: args.until,
    };
    let planned = match resume {
        Resume::Fresh => State::START,
        Resume::At(state) | Resume::Stuck(state) => state,
        Resume::Finished => return finished(ctx, &log, args.from_auto),
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
    // `ns auto` never resumes a stuck unit, so it never takes one over either.
    let taken_over =
        match resume {
            // A unit stuck in `setup` has no workspace, and one stuck in `clean_up` is past the change
            // request. In `ship` the change request is `ns`'s own while it is open, so only a merge or
            // a close by a person moves the unit.
            Resume::Stuck(
                stuck_in @ (State::Build | State::Verify | State::Review | State::Ship),
            ) if !args.from_auto => takeover_find(&ctx, &log, &root, &unit, stuck_in)?,
            _ => None,
        };
    let start = match (taken_over, resume) {
        (Some(state), _) => state,
        (None, Resume::Stuck(stuck_in @ (State::Verify | State::Review))) => {
            stuck_check_resume(&ctx, &log, &unit, stuck_in)?
        }
        (
            None,
            Resume::Fresh
            | Resume::At(_)
            | Resume::Finished
            | Resume::Stuck(
                State::Setup
                | State::Build
                | State::Ship
                | State::Retrospective
                | State::CleanUp
                | State::Done
                | State::Stuck,
            ),
        ) => planned,
    };
    // What the unit was labeled before, to give back when a usage limit stops the run.
    let label_before = ctx
        .forge
        .unit_get(&root, args.unit)?
        .labels
        .into_iter()
        .find(|l| l.starts_with(auto::STATE_PREFIX));
    status::unit_status_set(ctx.forge.as_ref(), &root, args.unit, status::IN_PROGRESS)?;
    // The assignee is for people to read; a unit does not depend on it.
    if let Err(e) = ctx.forge.unit_assignee_add(&root, args.unit) {
        eprintln!("ns: cannot assign the unit: {e:#}");
    }
    let halted = std::cell::RefCell::new(None);
    let phase = |state, _attempt| match states::run(&mut ctx, state) {
        Ok(done) => done,
        // A usage limit is not the state going wrong: the run stops and the state is not counted.
        Err(e) if harness::usage_limit_of(&e).is_some() => {
            *halted.borrow_mut() = Some(e);
            Outcome::Halt.into()
        }
        Err(e) => {
            eprintln!("ns: {state} went wrong: {e:#}");
            Outcome::Redo.into()
        }
    };
    let mut sink = |mut event: Value| -> Result<()> {
        if let Some(place) = event["from"]
            .as_str()
            .and_then(State::parse)
            .and_then(session_place)
        {
            event["model"] =
                manifest::model_read(&root, place, args.options.model.as_deref())?.into();
        }
        println!("{event}");
        if event.get("to").is_some() {
            log.append(&event)?;
        }
        Ok(())
    };
    let ended = walk(&unit, start, args.gates, max_attempts, phase, &mut sink);
    if let Some(e) = halted.into_inner() {
        // Nothing was logged for the state that hit the limit, so a run resumes where this began.
        let restored = match &label_before {
            Some(label) => status::unit_status_set(ctx.forge.as_ref(), &root, args.unit, label),
            None => status::unit_status_clear(ctx.forge.as_ref(), &root, args.unit),
        };
        if let Err(r) = restored {
            eprintln!("ns: cannot give unit {unit} its status back: {r:#}");
        }
        println!(
            "{}",
            json!({ "unit": args.unit, "stopped": "usage_limit", "reason": format!("{e:#}") })
        );
        return Err(e.context(format!(
            "stopped: unit {unit} is left as it was; run it again after the limit resets"
        )));
    }
    if ended.is_err() {
        if let Err(e) = unproven_comment_add(&ctx, &log, &root, &unit) {
            eprintln!("ns: cannot say what verification left unproven: {e:#}");
        }
    }
    let told = Told {
        ctx: &ctx,
        log: &log,
        root: &root,
        settings: notify.as_ref(),
    };
    let status = if matches!(ended, Ok(State::Done)) {
        finished_status_set(&told, &unit)
    } else {
        // Stuck, or gone wrong, is a unit a person looks at first; waiting at a gate is only
        // waiting for a person to go on.
        let label = if ended.is_err() {
            status::STUCK
        } else {
            status::WAITING_FOR_HUMAN
        };
        let set = status::unit_status_set(ctx.forge.as_ref(), &root, args.unit, label);
        match &ended {
            Err(_) => {
                let reason = log
                    .last_move(&unit)
                    .ok()
                    .flatten()
                    .and_then(|m| m.reason)
                    .map_or("", Reason::name);
                told.tell(Notice::Stuck, "stuck", reason);
            }
            Ok(state) => told.tell(Notice::WaitingForHuman, &state.to_string(), "gate"),
        }
        set
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

/// Where the sessions of `state` take their model from, for a state that starts any.
fn session_place(state: State) -> Option<Place> {
    match state {
        State::Build => Some(Place::Build),
        State::Verify => Some(Place::Verify),
        State::Review => Some(Place::Review),
        _ => None,
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

/// Where a stuck unit goes when a person opened its change request by hand: `ship` for an open
/// one, so `ns` waits for it and merges it as for a unit it shipped itself, `retrospective` for one
/// that has merged, and `clean_up` for a closed one, where it reports it and removes nothing, so
/// the unit ends `stuck`. `None` when the forge has none for the unit's branch, or when the unit
/// is stuck in `ship` and the change request is open. An open or merged
/// one must be at the commit the workspace holds, or `ns` stops before it changes anything. The
/// take-over is printed and logged with the change request's number.
fn takeover_find(
    ctx: &Context,
    log: &RunLog,
    root: &Path,
    unit: &str,
    stuck_in: State,
) -> Result<Option<State>> {
    let workspace = ctx
        .workspace
        .as_ref()
        .context("a stuck unit has no workspace")?;
    let Some(cr) = ctx.forge.change_request_get(root, &workspace.branch)? else {
        return Ok(None);
    };
    let state = match cr.state {
        ChangeState::Open => State::Ship,
        ChangeState::Merged => State::Retrospective,
        ChangeState::Closed => State::CleanUp,
    };
    // A unit stuck in `ship` with its own open change request resumes as it always did.
    if state == stuck_in {
        return Ok(None);
    }
    // A merged change request can be an older one that reused the branch name, and `clean_up`
    // would throw away the workspace's newer work; a closed one makes `clean_up` remove nothing.
    if cr.state != ChangeState::Closed {
        let head = ctx.vcs.workspace_head_find(&workspace.path)?;
        if head != cr.head {
            bail!(
                "the workspace of unit {unit} is at {head}, which is not the head of the \
                 change request {}, at {}; nothing was changed",
                cr.url,
                cr.head
            );
        }
    }
    let mut event = json!({ "unit": unit, "took_over": cr.number });
    if let Some(merged_at) = cr.merged_at {
        event["merged_at"] = merged_at.into();
    }
    println!("{event}");
    log.append(&event)?;
    Ok(Some(state))
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

/// Looks again at a unit whose run is done and prints it as one JSON line. A change request that
/// is still open and behind its base branch is refreshed first, unless `ns auto` asked: the line
/// then says whether it was current, refreshed, or sent back to `build`. One that has merged since
/// is cleaned up after, and the unit's status goes with it. A change request still open changes
/// nothing else.
fn finished(mut ctx: Context, log: &RunLog, from_auto: bool) -> Result<()> {
    let root = ctx.vcs.root_find(&ctx.from)?;
    let unit = ctx.unit.to_string();
    let mut line = json!({ "unit": unit, "state": State::Done.to_string() });
    if ctx.ship.queue && !from_auto {
        if let Some(refresh) = open_refresh(&mut ctx, log, &root, &unit)? {
            match refresh {
                Refresh::Current => line["refresh"] = "current".into(),
                Refresh::Refreshed => line["refresh"] = "refreshed".into(),
                Refresh::Failed => return sent_back(&ctx, log, &unit),
            }
        }
    }
    println!("{line}");
    if clean_up::merged_clean_up(&mut ctx)? {
        status::unit_status_clear(ctx.forge.as_ref(), &root, ctx.unit)?;
    }
    Ok(())
}

/// Brings the change request of a finished unit up to date with the base branch when it is open
/// and behind. `None` when it is not open.
fn open_refresh(
    ctx: &mut Context,
    log: &RunLog,
    root: &Path,
    unit: &str,
) -> Result<Option<Refresh>> {
    let wanted = workspace::locate(root, unit)?;
    let open = matches!(
        ctx.forge.change_request_get(root, &wanted.branch)?,
        Some(cr) if cr.state == ChangeState::Open
    );
    if !open {
        return Ok(None);
    }
    if !wanted.path.exists() {
        bail!(
            "the workspace of unit {unit} is gone: {}",
            wanted.path.display()
        );
    }
    let dir = wanted.path.clone();
    ctx.workspace = Some(wanted);
    let Some(tip) = ship::base_behind(ctx.vcs.as_ref(), log, root, &dir, unit)? else {
        return Ok(Some(Refresh::Current));
    };
    let refresh = ship::open_refresh(ctx, &tip)?;
    if refresh == Refresh::Refreshed {
        log.base_tested_append(unit, &tip)?;
    }
    Ok(Some(refresh))
}

/// Moves a finished unit back to `build` because refreshing its change request failed, and prints
/// the move with the evidence, so a person sees why before merging.
fn sent_back(ctx: &Context, log: &RunLog, unit: &str) -> Result<()> {
    let dir = &ctx
        .workspace
        .as_ref()
        .context("a refresh ran before there was a workspace")?
        .path;
    let event = json!({
        "unit": unit,
        "from": State::Done.to_string(),
        "to": State::Build.to_string(),
        "reason": "failed",
    });
    log.append(&event)?;
    let mut line = event;
    line["evidence"] = artifact::read(dir, unit, "evidence.md")?.into();
    println!("{line}");
    Ok(())
}

/// What `ns run` needs to tell a person about the unit it runs.
struct Told<'a> {
    ctx: &'a Context,
    log: &'a RunLog,
    root: &'a Path,
    settings: Option<&'a Settings>,
}

impl Told<'_> {
    /// Runs the notify command for `mv`, with the unit's title and the address of its change
    /// request, or of the unit before it has one. Skips the forge when nothing would run.
    fn tell(&self, mv: Notice, state: &str, reason: &str) {
        if !self.settings.is_some_and(|s| s.on.contains(&mv)) {
            return;
        }
        let ctx = self.ctx;
        let unit = match ctx.forge.unit_get(self.root, ctx.unit) {
            Ok(unit) => unit,
            Err(e) => return eprintln!("ns: cannot tell about unit {}: {e:#}", ctx.unit),
        };
        let branch = workspace::locate(self.root, &ctx.unit.to_string()).map(|w| w.branch);
        let url = match branch.map(|b| ctx.forge.change_request_get(self.root, &b)) {
            Ok(Ok(Some(cr))) => cr.url,
            _ => unit.url,
        };
        let facts = Facts::Unit {
            unit: ctx.unit,
            title: &unit.title,
            state,
            reason,
            url: &url,
        };
        notify::notify(ctx.os.as_ref(), self.log, self.settings, mv, &facts);
    }
}

/// Sets the status of a unit whose states are all done: none when its change request merged,
/// otherwise it is ready to merge and waits for a person. Tells the person either way.
fn finished_status_set(told: &Told, unit: &str) -> Result<()> {
    let ctx = told.ctx;
    let branch = workspace::locate(told.root, unit)?.branch;
    let merged = matches!(
        ctx.forge.change_request_get(told.root, &branch)?,
        Some(cr) if cr.state == ChangeState::Merged
    );
    let set = if merged {
        status::unit_status_clear(ctx.forge.as_ref(), told.root, ctx.unit)
    } else {
        status::unit_status_set(
            ctx.forge.as_ref(),
            told.root,
            ctx.unit,
            status::WAITING_FOR_HUMAN,
        )
    };
    if merged {
        told.tell(Notice::Merged, "done", "");
    } else {
        told.tell(Notice::WaitingForHuman, "done", "merge");
    }
    set
}

/// Moves the unit from `start` to `done`, or to `stuck` when a state would run more than
/// `max_attempts` times in all. With `Gates::Stop` it also stops, ready to go on, before every
/// state after the first. `phase` runs a state given its attempt number (from 1). Every move is
/// passed to `sink` as one JSON event.
fn walk<D: Into<Done>>(
    unit: &str,
    start: State,
    gates: Gates,
    max_attempts: u32,
    mut phase: impl FnMut(State, u32) -> D,
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
        let Done { outcome, facts } = phase(state, attempt).into();
        if matches!(outcome, Outcome::Halt) {
            bail!("unit {unit} was stopped in {state}");
        }
        let to = match outcome {
            Outcome::Pass => next,
            Outcome::Unmerged => State::CleanUp,
            Outcome::Halt => unreachable!("a halted run returns before it moves"),
            Outcome::Unprovable | Outcome::BaseMoved | Outcome::Unresumable => State::Stuck,
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
        // `to_attempt` is the attempt the state moved to will be in, so a reader of the log needs
        // no count of its own.
        let to_attempt = runs.get(&to).copied().unwrap_or(0) + 1;
        let mut event = json!({
            "unit": unit,
            "from": state.to_string(),
            "to": to.to_string(),
            "to_attempt": to_attempt,
        });
        if to != next {
            event["attempt"] = attempt.into();
            event["reason"] = match outcome {
                Outcome::Redo | Outcome::Unresumable => "went_wrong",
                Outcome::Halt => unreachable!("a halted run returns before it moves"),
                Outcome::Pass | Outcome::Unmerged | Outcome::Fail => "failed",
                Outcome::Inconclusive | Outcome::Unprovable => "inconclusive",
                Outcome::BaseMoved => "base_moved",
            }
            .into();
        }
        // What the state produced rides on the move out of it, and is repeated at a gate.
        facts.onto(&mut event);
        sink(event)?;
        if to == State::Stuck {
            stuck_in = state;
        }
        state = to;
        if gates == Gates::Stop
            && matches!(outcome, Outcome::Pass | Outcome::Unmerged)
            && state.next().is_some()
        {
            let mut waiting = json!({ "unit": unit, "waiting": state.to_string() });
            facts.onto(&mut waiting);
            sink(waiting)?;
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
    use crate::states::verdict::Verdict;
    use crate::states::{Facts, SessionVerdict};

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

    fn facts_of(facts: Facts) -> Done {
        let mut done = Done::from(Outcome::Pass);
        done.facts = facts;
        done
    }

    fn check_of(verdict: Verdict, findings: usize, artifact: &str) -> Facts {
        Facts::check(Some(verdict), findings, std::path::Path::new(artifact))
    }

    /// Every event of a walk from `setup` whose states pass, each with the facts `facts` gives.
    fn events_with(gates: Gates, facts: impl Fn(State) -> Done) -> Vec<Value> {
        let mut events = Vec::new();
        let _ = walk("u1", State::START, gates, 2, |s, _| facts(s), &mut |e| {
            events.push(e);
            Ok(())
        });
        events
    }

    fn verify_facts(s: State) -> Done {
        match s {
            State::Verify => facts_of(check_of(Verdict::Pass, 2, "/w/.ns/1/verify-1.md")),
            _ => Outcome::Pass.into(),
        }
    }

    #[test]
    fn a_move_carries_the_facts_its_state_set_and_a_move_with_none_is_unchanged() {
        let events = events_with(Gates::Auto, verify_facts);
        assert_eq!(
            events[3],
            json!({
                "unit": "u1", "from": "verify", "to": "review", "to_attempt": 1,
                "verdict": "PASS", "findings": 2, "artifact": "/w/.ns/1/verify-1.md",
            })
        );
        assert_eq!(
            events[1].to_string(),
            r#"{"from":"setup","to":"build","to_attempt":1,"unit":"u1"}"#
        );
    }

    #[test]
    fn a_waiting_event_repeats_the_facts_of_the_move_before_it() {
        let mut events = Vec::new();
        let _ = walk(
            "u1",
            State::Verify,
            Gates::Stop,
            2,
            |s, _| verify_facts(s),
            &mut |e| {
                events.push(e);
                Ok(())
            },
        );
        assert_eq!(
            events[2],
            json!({
                "unit": "u1", "waiting": "review",
                "verdict": "PASS", "findings": 2, "artifact": "/w/.ns/1/verify-1.md",
            })
        );
    }

    #[test]
    fn a_waiting_event_after_a_move_with_no_facts_has_only_unit_and_waiting() {
        let mut events = Vec::new();
        let _ = walk(
            "u1",
            State::Setup,
            Gates::Stop,
            2,
            |_, _| Outcome::Pass,
            &mut |e| {
                events.push(e);
                Ok(())
            },
        );
        assert_eq!(events[2], json!({ "unit": "u1", "waiting": "build" }));
    }

    #[test]
    fn a_review_move_and_the_waiting_after_it_carry_its_sessions() {
        let review = |s: State| match s {
            State::Review => facts_of(Facts {
                sessions: Some(vec![SessionVerdict {
                    name: "Behavior".into(),
                    verdict: Some(Verdict::Fail),
                }]),
                ..check_of(Verdict::Fail, 2, "/w/.ns/1/review-1.md")
            }),
            _ => Outcome::Pass.into(),
        };
        let mut events = Vec::new();
        let _ = walk(
            "u1",
            State::Review,
            Gates::Stop,
            2,
            |s, _| review(s),
            &mut |e| {
                events.push(e);
                Ok(())
            },
        );
        for event in &events[1..] {
            assert_eq!(event["sessions"][0]["name"], "Behavior", "{event}");
            assert_eq!(event["findings"], 2, "{event}");
            assert_eq!(event["verdict"], "FAIL", "{event}");
        }
        assert_eq!(events[2]["waiting"], "ship");
    }

    #[test]
    fn a_build_move_has_an_artifact_and_no_verdict_or_findings() {
        let events = events_with(Gates::Auto, |s| match s {
            State::Build => facts_of(Facts {
                artifact: Some("/w/.ns/1/build.md".into()),
                ..Facts::default()
            }),
            _ => Outcome::Pass.into(),
        });
        assert_eq!(events[2]["artifact"], "/w/.ns/1/build.md");
        assert!(events[2].get("verdict").is_none() && events[2].get("findings").is_none());
        assert!(events[3].get("artifact").is_none(), "{}", events[3]);
    }

    #[test]
    fn a_caller_that_reads_only_the_old_keys_sees_the_same_events() {
        let old = ["unit", "from", "to", "to_attempt", "state", "waiting"];
        let keep = |events: Vec<Value>| -> Vec<Value> {
            events
                .into_iter()
                .map(|e| {
                    let kept: serde_json::Map<String, Value> = e
                        .as_object()
                        .unwrap()
                        .iter()
                        .filter(|(k, _)| old.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    Value::Object(kept)
                })
                .collect()
        };
        let plain = events_with(Gates::Stop, |_| Outcome::Pass.into());
        let rich = events_with(Gates::Stop, verify_facts);
        assert_eq!(keep(plain), keep(rich));
    }

    #[test]
    fn a_halted_state_stops_the_walk_with_no_move_and_no_attempt_spent() {
        let mut events = Vec::new();
        let mut calls = 0;
        let result = walk(
            "u1",
            State::Build,
            Gates::Auto,
            1,
            |_, _| {
                calls += 1;
                Outcome::Halt
            },
            &mut |e| {
                events.push(e);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
        assert_eq!(events.len(), 1, "only the starting state is reported");
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
                "ship>retrospective",
                "retrospective>clean_up",
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
                "ship>retrospective",
                "retrospective>clean_up",
                "clean_up>clean_up",
                "clean_up>stuck",
            ]
        );
    }

    #[test]
    fn a_ship_that_did_not_merge_skips_retrospective() {
        let (moves, ok) = moves(|s, _| match s {
            State::Ship => Outcome::Unmerged,
            _ => Outcome::Pass,
        });
        assert!(ok);
        assert_eq!(&moves[4..], ["ship>clean_up", "clean_up>done"]);
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
                "ship>retrospective",
                "retrospective>clean_up",
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
    fn every_move_logs_the_attempt_of_the_state_it_goes_to() {
        let mut events = Vec::new();
        let _ = walk(
            "u1",
            State::START,
            Gates::Auto,
            2,
            |s, _| {
                if s == State::Verify {
                    Outcome::Fail
                } else {
                    Outcome::Pass
                }
            },
            &mut |e| {
                events.push(e);
                Ok(())
            },
        );
        let seen: Vec<(String, u64)> = events
            .iter()
            .skip(1)
            .map(|e| {
                (
                    format!(
                        "{}>{}",
                        e["from"].as_str().unwrap(),
                        e["to"].as_str().unwrap()
                    ),
                    e["to_attempt"].as_u64().unwrap(),
                )
            })
            .collect();
        let expected: Vec<(String, u64)> = [
            ("setup>build", 1),
            ("build>verify", 1),
            ("verify>build", 2),
            ("build>verify", 2),
            ("verify>stuck", 1),
        ]
        .iter()
        .map(|(m, n)| (m.to_string(), *n))
        .collect();
        assert_eq!(seen, expected);
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
                "ship>retrospective",
                "retrospective>clean_up",
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
