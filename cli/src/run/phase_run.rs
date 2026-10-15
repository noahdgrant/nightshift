//! Running one admitted phase: its budget and hold checks, its runner locks, archiving the
//! artifacts it replaces, the harness, and the record of what it did. Reports what it observed as
//! a [`PhaseRan`]; the decision about what follows is `core::step`'s.

use std::fs;
use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use super::core::route::review_run;
use super::core::step::PhaseRan;
use super::core::Admitted;
use super::currency::{archive_as, archive_for, read_art, restore, Moves, State};
use super::drive::Io;
use super::locks::{Acquired, Bound, RunnerLocks};
use super::phases::{phase_timeout, phase_tmpdir, run_harness};
use super::{finish, Ctx, Finish, Outcome};
use crate::billing;
use crate::clock;
use crate::eval::parser::{ClaudeStreamJson, OutputParser};
use crate::factory::{self, artifact_of};
use crate::gate;
use crate::git;
use crate::pin;
use crate::review_runs::{self, ReviewRun};

/// What came of an admitted phase.
pub(super) enum Ran {
    /// It ran: what the shell saw, and what to undo if a stop interrupts the gate after it.
    Phase(PhaseRan, Undo),
    /// Its runner locks were not acquired in time, for this reason.
    LockGaveUp(String),
    /// The run ends before or instead of the phase: a spent budget, a hold, a usage limit.
    Finish(Finish),
}

/// What a phase changed in the unit's artifacts, for a stop during its gate to set aside.
pub(super) struct Undo {
    pub moves: Moves,
    pub written: bool,
}

/// Why a spend of `spent` dollars is over `budget`, or `None` while it is under it.
pub(super) fn budget_spent(budget: Option<f64>, spent: f64) -> Option<String> {
    let b = budget.filter(|b| spent >= *b)?;
    Some(format!("spent ${spent:.2} of the ${b:.2} budget"))
}

/// A budget spent while the runner locks were awaited stops the phase before its harness
/// starts, with the locks released.
pub(super) fn recheck_budget(
    held: RunnerLocks,
    budget: Option<f64>,
    spent: f64,
    phase: &str,
) -> Result<RunnerLocks, Finish> {
    match budget_spent(budget, spent) {
        Some(reason) => Err(finish(Outcome::Budget, reason, Some(phase))),
        None => Ok(held),
    }
}

/// Fetch `branch` from origin into the worktree's remote-tracking ref.
fn fetch_branch(worktree: &Path, branch: &str) -> Result<String> {
    git::run_net(worktree, &["fetch", "-q", "origin", branch])
}

/// Run `adm` in the unit at `state`.
pub(super) fn run_phase(io: &mut Io<'_, '_>, state: &State, adm: &Admitted) -> Result<Ran> {
    let (ctx, phase, attempt) = (io.ctx, adm.phase, adm.attempt);
    if adm.follows_base {
        ctx.log(json!({"event": "ship_after_base_moved", "attempt": attempt}));
    }
    let mut p = ctx.fac.phase(factory::config_phase(phase));
    p.name = phase.to_string();
    let timeout = phase_timeout(phase, p.timeout_minutes);
    let held = match take_locks(io, adm, timeout)? {
        Ok(held) => held,
        Err(ran) => return Ok(ran),
    };
    let moves = archive_for(&ctx.artifacts, phase, state)?;
    if let Some((_, branch, sha)) = &adm.rebase {
        let _ = fetch_branch(&ctx.worktree, branch);
        ctx.log(json!({
            "event": "rebase_after_base_moved",
            "phase": phase,
            "attempt": attempt,
            "onto": sha,
            "cause": adm.why,
        }));
    }
    let mut prompt = ctx.prompt(&p, attempt, &adm.prompt_feedback())?;
    if io.args.triage_only {
        prompt.push_str(factory::TRIAGE_ONLY);
    }
    let transcript = ctx
        .common
        .join("ns")
        .join("transcripts")
        .join(&ctx.unit)
        .join(format!("{phase}-{attempt}.jsonl"));
    let mut env = phase_env(ctx, io.shared.watch_pid, phase, attempt);
    let tmp = match phase_tmpdir(&ctx.env.tmp, phase, attempt) {
        Ok(tmp) => tmp,
        Err(e) => {
            restore(&moves);
            return Err(e);
        }
    };
    env.push(("TMPDIR", tmp.to_string_lossy().into_owned()));
    pin::write_settings(&ctx.common, &ctx.unit, &ctx.env.ns)?;
    let started = io.shared.clock.now();
    let r = run_harness(
        &io.commands[factory::config_phase(phase)],
        &prompt,
        &ctx.worktree,
        &env,
        timeout,
        &transcript,
        ctx.fac.subscription(),
        ctx.env.cap,
    );
    drop(held);
    if let Some(sig) = r.as_ref().err().and(crate::stop::requested()) {
        let ev = json!({
            "event": "phase",
            "phase": phase,
            "attempt": attempt,
            "decision": adm.why,
            "transcript": transcript.to_string_lossy(),
        });
        return Err(set_aside(ctx, phase, &moves, sig, ev)?);
    }
    let r = r?;
    let done = Harnessed {
        r: &r,
        transcript: &transcript,
        tmp: &tmp,
        timeout,
        started,
    };
    record(io, state, adm, done, moves)
}

/// The budget and hold checks, then the phase's runner locks: the locks, or what the phase came
/// to without them.
fn take_locks(
    io: &mut Io<'_, '_>,
    adm: &Admitted,
    timeout: std::time::Duration,
) -> Result<Result<RunnerLocks, Ran>> {
    let (ctx, phase, attempt) = (io.ctx, adm.phase, adm.attempt);
    let fac = ctx.fac;
    if let Some(reason) = budget_spent(fac.budget_usd(), io.shared.spent()) {
        return Ok(Err(Ran::Finish(finish(
            Outcome::Budget,
            reason,
            Some(phase),
        ))));
    }
    if let Some(f) = io.shared.held(phase)? {
        return Ok(Err(Ran::Finish(f)));
    }
    eprintln!(
        "ns run: {} {phase} attempt {attempt} ({})",
        ctx.unit, adm.why
    );
    let locks = RunnerLocks::acquire(
        &ctx.env.lock_dir,
        &fac.locks(&fac.phase(phase)),
        &ctx.unit,
        &io.shared.clock,
        timeout,
        io.shared.until,
        |lock, by| {
            eprintln!(
                "ns run: {} {phase} waits for lock {lock} (held by pid {}, unit {})",
                ctx.unit, by["pid"], by["unit"]
            );
            ctx.log(json!({"event": "lock_wait", "phase": phase, "lock": lock, "held_by": by}));
        },
    )?;
    let held = match locks {
        Acquired::Held(held) => held,
        Acquired::GaveUp(g) => {
            let reason = g.reason();
            eprintln!("ns run: {} {phase} {reason}", ctx.unit);
            ctx.log(json!({
                "event": "lock_wait_timeout",
                "phase": phase,
                "attempt": attempt,
                "lock": g.lock,
                "held_by": g.held_by,
                "bound": g.bound.label(),
            }));
            if g.bound == Bound::Until {
                return Ok(Err(Ran::Finish(finish(
                    Outcome::Budget,
                    reason,
                    Some(phase),
                ))));
            }
            io.phases.push(json!({
                "phase": phase,
                "attempt": attempt,
                "decision": adm.why,
                "written": false,
                "reason": reason,
            }));
            return Ok(Err(Ran::LockGaveUp(reason)));
        }
    };
    let held = match recheck_budget(held, fac.budget_usd(), io.shared.spent(), phase) {
        Ok(held) => held,
        Err(f) => return Ok(Err(Ran::Finish(f))),
    };
    // A unit that waited for a lock while another hit a usage limit waits for the reset.
    if let Some(f) = io.shared.held(phase)? {
        return Ok(Err(Ran::Finish(f)));
    }
    Ok(Ok(held))
}

/// The environment a phase's harness runs with, besides `TMPDIR`.
fn phase_env(
    ctx: &Ctx<'_>,
    watch_pid: Option<u32>,
    phase: &str,
    attempt: u32,
) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("NS_UNIT", ctx.unit.clone()),
        ("NS_PHASE", phase.to_string()),
        ("NS_ATTEMPT", attempt.to_string()),
        ("NS_WORKTREE", ctx.worktree.to_string_lossy().into_owned()),
        (
            pin::SETTINGS_VAR,
            pin::settings_path(&ctx.common, &ctx.unit)
                .to_string_lossy()
                .into_owned(),
        ),
        ("NS_RUN_PID", std::process::id().to_string()),
        (
            "NS_WATCH_PID",
            watch_pid.map(|p| p.to_string()).unwrap_or_default(),
        ),
    ];
    if !crate::git::ssh_command_set(&ctx.worktree) {
        env.push(("GIT_SSH_COMMAND", crate::git::SSH_KEEPALIVE.to_string()));
    }
    env
}

/// A harness run that finished.
#[derive(Clone, Copy)]
struct Harnessed<'h> {
    r: &'h super::phases::PhaseRun,
    transcript: &'h std::path::Path,
    tmp: &'h std::path::Path,
    timeout: std::time::Duration,
    /// When the harness started, on the run's clock.
    started: i64,
}

/// Account for a finished harness run: its spend, its artifact, its record in the run log.
fn record(
    io: &mut Io<'_, '_>,
    state: &State,
    adm: &Admitted,
    h: Harnessed<'_>,
    moves: Moves,
) -> Result<Ran> {
    let (ctx, phase, attempt, r) = (io.ctx, adm.phase, adm.attempt, h.r);
    let t = ClaudeStreamJson.parse(&r.stdout);
    let cost = t.cost_usd.unwrap_or(0.0);
    io.shared.add_spend(&ctx.unit, cost)?;
    let mut rec = json!({
        "phase": phase,
        "attempt": attempt,
        "decision": adm.why,
        "exit": r.exit,
        "timed_out": r.timed_out,
        "wall_s": (r.wall_s * 10.0).round() / 10.0,
        "cost_usd": cost,
        "input_tokens": t.input_tokens,
        "output_tokens": t.output_tokens,
        "transcript": h.transcript.to_string_lossy(),
    });
    // A phase cut short wrote nothing, whatever it left behind.
    if r.timed_out || r.killed.is_some() {
        let file = artifact_of(phase);
        let kind = if r.timed_out { "timeout" } else { "killed" };
        let stem = format!("{}-{kind}", file.trim_end_matches(".md"));
        if let Some(dst) = archive_as(&ctx.artifacts, file, &stem)? {
            rec["archived"] = json!(dst.to_string_lossy());
        }
    }
    if let Some(limit) = billing::usage_limit(&r.stdout, io.shared.clock.now()) {
        rec["status"] = json!("paused");
        rec["reason"] = json!(limit.message);
        restore(&moves);
        ctx.log(merge_obj(json!({"event": "phase"}), &rec));
        io.phases.push(rec);
        let mut f = finish(
            Outcome::Paused,
            format!("usage limit: {}", limit.message),
            Some(phase),
        );
        f.reset_at = limit.reset_at;
        return Ok(Ran::Finish(f));
    }
    let art_path = ctx.artifacts.join(artifact_of(phase));
    let art = read_art(&art_path);
    let written = art.is_some();
    if art.as_ref().is_some_and(|a| a.invalid.is_none()) {
        if let Err(e) = record_review_run(io, state, phase, h.started, &mut rec) {
            return unrecorded(io, phase, &moves, rec, &e);
        }
    }
    if art.as_ref().is_some_and(|a| a.status == "pass") {
        // A failed phase's scratch files stay for a human to read, until `ns clean`. A pass
        // whose scratch dir will not go is logged, not failed: `ns clean` removes the leftover.
        if let Err(e) = fs::remove_dir_all(h.tmp) {
            ctx.log(json!({
                "event": "tmpdir_not_removed",
                "phase": phase,
                "path": h.tmp.display().to_string(),
                "error": e.to_string(),
            }));
        }
    }
    if !written {
        restore(&moves);
    }
    let timeout_text = gate::timed_out_after(h.timeout);
    rec["written"] = json!(written);
    rec["status"] = json!(art.as_ref().map(|a| a.status.clone()));
    rec["sha"] = json!(ctx.head());
    if written {
        *io.last_artifact = Some(art_path.to_string_lossy().into_owned());
    } else {
        rec["reason"] = json!(match (r.timed_out, &r.killed, r.exit) {
            (true, _, _) => timeout_text.clone(),
            (false, Some(k), _) => k.clone(),
            (false, None, Some(0)) => "no artifact written".to_string(),
            (false, None, Some(c)) => format!("exited {c}, no artifact written"),
            (false, None, None) => "killed by a signal".to_string(),
        });
    }
    ctx.log(merge_obj(json!({"event": "phase"}), &rec));
    io.phases.push(rec);
    let ran = PhaseRan {
        artifact: art.as_ref().map(|a| a.status.clone()),
        invalid: art.and_then(|a| a.invalid),
        present: art_path.exists(),
        timed_out: r.timed_out,
        killed: r.killed.clone(),
        timeout_text,
        exit: r.exit,
        head_before: state.head.clone(),
        head_after: ctx.head(),
        red_feedback: io.gate_state.red_feedback().map(String::from),
    };
    Ok(Ran::Phase(ran, Undo { moves, written }))
}

/// Add a review pass or fix cycle that wrote its artifact to `ns`'s record of the review loop,
/// with the times `ns` saw, and name its cycle in the run log's `phase` event.
fn record_review_run(
    io: &Io<'_, '_>,
    state: &State,
    phase: &str,
    started: i64,
    rec: &mut Value,
) -> Result<()> {
    let ctx = io.ctx;
    let Some((cycle, kind, fresh)) = review_run(phase, state) else {
        return Ok(());
    };
    let run = ReviewRun {
        phase: phase.to_string(),
        cycle,
        kind: kind.to_string(),
        started: clock::iso(started),
        ended: clock::iso(io.shared.clock.now()),
        sha: ctx.head(),
    };
    rec["review_cycle"] = json!({"cycle": cycle, "kind": kind});
    review_runs::append(&ctx.artifacts, run, fresh).map(|_| ())
}

/// A review run `ns` could not record ends the unit stuck, with what it wrote set aside and what
/// it replaced put back: a run the record doesn't hold must not count as no cycle.
fn unrecorded(
    io: &mut Io<'_, '_>,
    phase: &str,
    moves: &Moves,
    mut rec: Value,
    e: &anyhow::Error,
) -> Result<Ran> {
    let ctx = io.ctx;
    let file = artifact_of(phase);
    let stem = format!("{}-unrecorded", file.trim_end_matches(".md"));
    let archived = archive_as(&ctx.artifacts, file, &stem)?;
    restore(moves);
    let reason = format!("ns could not record the {phase} run in its review record: {e:#}");
    rec["status"] = json!("unrecorded");
    rec["reason"] = json!(reason);
    rec["archived"] = json!(archived.map(|d| d.to_string_lossy().into_owned()));
    ctx.log(merge_obj(json!({"event": "phase"}), &rec));
    io.phases.push(rec);
    Ok(Ran::Finish(finish(Outcome::Stuck, reason, Some(phase))))
}

/// A stop ended `phase`, or the gate run after it, before it was done: set aside what the phase
/// wrote and put back what it replaced, so the next run decides from the state before the
/// phase, and log `ev` with what happened. Returns the stop's error.
pub(super) fn set_aside(
    ctx: &Ctx<'_>,
    phase: &str,
    moves: &Moves,
    sig: i32,
    mut ev: Value,
) -> Result<anyhow::Error> {
    let file = artifact_of(phase);
    let stem = format!("{}-interrupted", file.trim_end_matches(".md"));
    let archived = archive_as(&ctx.artifacts, file, &stem)?;
    restore(moves);
    ev["interrupted"] = json!(crate::stop::name(sig));
    ev["archived"] = json!(archived.map(|d| d.to_string_lossy().into_owned()));
    ctx.log(ev);
    Ok(crate::stop::error(sig))
}

fn merge_obj(mut a: Value, b: &Value) -> Value {
    if let (Some(x), Some(y)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in y {
            x.insert(k.clone(), v.clone());
        }
    }
    a
}

#[cfg(test)]
mod tests;
