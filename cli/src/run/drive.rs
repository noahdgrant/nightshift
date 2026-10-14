//! The phase driver: decide the next phase, run it under its locks and budget, and gate it.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{json, Value};

use super::ci_gate::gate_trigger;
use super::core::{Ledger, MergeBack, PhaseResult, Rebase};
use super::currency::{
    archive_as, archive_for, known_pr, read_art, read_state, restore, Carried, Decision, State,
    UnitDiff, DELTA_MAX_LINES,
};
use super::locks::{Acquired, Bound, RunnerLocks};
use super::merge::{merge_step, MergeStep};
use super::phases::{phase_timeout, phase_tmpdir, run_harness};
use super::{finish, remote_default, remote_sha, Ctx, Finish, Outcome, RunArgs, Shared};
use crate::billing;
use crate::eval::parser::{ClaudeStreamJson, OutputParser};
use crate::factory::{self, artifact_of, PHASES};
use crate::gate;
use crate::git::{self, same_sha};
use crate::pin;
use crate::watch::forge::{self, CommitPr, PrStatus};
use crate::worktree;

/// Why a spend of `spent` dollars is over `budget`, or `None` while it is under it.
fn budget_spent(budget: Option<f64>, spent: f64) -> Option<String> {
    let b = budget.filter(|b| spent >= *b)?;
    Some(format!("spent ${spent:.2} of the ${b:.2} budget"))
}

/// A budget spent while the runner locks were awaited stops the phase before its harness
/// starts, with the locks released.
fn recheck_budget(
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

/// Drive the unit to an outcome. A run that finishes removes its ledger, so the next `ns run`
/// of the unit starts fresh; one that stops without finishing (a stop signal, a crash, a reboot)
/// leaves it for the next run to resume.
pub(super) fn drive(
    ctx: &Ctx<'_>,
    args: &RunArgs,
    shared: &mut Shared,
    commands: &BTreeMap<&'static str, Vec<String>>,
    phases: &mut Vec<Value>,
    last_artifact: &mut Option<String>,
) -> Result<Finish> {
    let r = drive_loop(ctx, args, shared, commands, phases, last_artifact);
    // A paused, out-of-budget or until-bound run is resumed later: its ledger stays.
    if r.as_ref().is_ok_and(|f| {
        matches!(
            f.outcome,
            Outcome::Done | Outcome::Merged | Outcome::Stuck | Outcome::Split
        )
    }) {
        let _ = fs::remove_file(ledger_path(&ctx.artifacts));
    }
    r
}

#[allow(
    clippy::too_many_lines,
    reason = "one loop over the phases; split it rather than grow it"
)]
fn drive_loop(
    ctx: &Ctx<'_>,
    args: &RunArgs,
    shared: &mut Shared,
    commands: &BTreeMap<&'static str, Vec<String>>,
    phases: &mut Vec<Value>,
    last_artifact: &mut Option<String>,
) -> Result<Finish> {
    let fac = ctx.fac;
    let subscription = fac.subscription();
    let (mut ledger, events) = start_ledger(args, &ctx.artifacts, &ctx.head());
    for ev in events {
        ctx.log(ev);
    }
    let mut baseline = remote_default(&ctx.worktree);
    let mut gate_state = gate::Gate::default();
    ctx.log(json!({
        "event": "start",
        "default_branch": baseline.as_ref().map(|b| &b.0),
        "default_sha": baseline.as_ref().map(|b| &b.1),
        "memory_cap": ctx.env.cap.map(|c| json!({
            "mb": c.mb,
            "via": c.via.label(),
            "fallback": c.fallback,
        })),
        "cpus": shared.cpus,
    }));
    loop {
        let head = ctx.head();
        ledger.saved_at(&head);
        save_ledger(ctx, &ledger);
        crate::stop::check()?;
        let state = read_state(
            &ctx.artifacts,
            head,
            ctx.issue.is_some(),
            Some(UnitDiff::new(
                ctx.worktree.clone(),
                ctx.base.clone(),
                &ctx.head(),
            )),
        );
        let mut state = state;
        carry_forward(ctx, &ledger, &mut state);
        let (decision, rebase) = ledger.next(&state);
        let (phase, feedback, why) = match decision {
            Decision::Stuck(r) => {
                if last_artifact.is_none() {
                    *last_artifact = PHASES
                        .iter()
                        .find(|p| state.arts.get(**p).is_some_and(|a| a.status == "blocked"))
                        .map(|p| {
                            ctx.artifacts
                                .join(artifact_of(p))
                                .to_string_lossy()
                                .into_owned()
                        });
                }
                return Ok(finish(Outcome::Stuck, r, None));
            }
            Decision::Split(r) => {
                let brief = artifact_of("triage");
                let kept = archive_as(&ctx.artifacts, brief, brief.trim_end_matches(".md"))?;
                *last_artifact = kept.map(|p| p.to_string_lossy().into_owned());
                return Ok(finish(Outcome::Split, r, Some("triage")));
            }
            Decision::Done => {
                if !fac.merge.auto() {
                    return Ok(finish(Outcome::Done, "pr.md passed", Some("ship")));
                }
                ledger.merging();
                let back = match merge_step(ctx, &state, shared, &mut gate_state, ledger.merges)? {
                    MergeStep::Finish(f) => return Ok(f),
                    MergeStep::Rebuild(red) => MergeBack::Rebuild {
                        stuck_reason: red.stuck_reason(),
                        feedback: red.feedback,
                        reason: red.reason,
                    },
                    MergeStep::Reship(feedback) => MergeBack::Reship(feedback),
                    MergeStep::Rebase(feedback, why) => MergeBack::Rebase { feedback, why },
                };
                if let Err(reason) = ledger.merge_back(back) {
                    let cap = super::core::RESHIP_CAP;
                    ctx.log(json!({"event": "reship_cap", "reships": cap, "cap": cap}));
                    return Ok(finish(Outcome::Stuck, reason, Some("ship")));
                }
                continue;
            }
            Decision::Run {
                phase,
                feedback,
                why,
            } => (phase, feedback, why),
        };
        let p = fac.phase(phase);
        let remote = rebase.and_then(|_| remote_default(&ctx.worktree));
        let admitted = ledger.admit(phase, feedback, why, rebase, remote, p.max_attempts);
        let (attempt, follows_base) = match &admitted {
            Ok(a) => (a.attempt, a.follows_base),
            Err(d) => (d.attempt, d.follows_base),
        };
        if follows_base {
            ctx.log(json!({"event": "ship_after_base_moved", "attempt": attempt}));
        }
        let adm = match admitted {
            Ok(a) => a,
            Err(d) => return Ok(finish(Outcome::Stuck, d.reason, Some(phase))),
        };
        let why = adm.why.clone();
        if let Some(reason) = budget_spent(fac.budget_usd(), shared.spent()) {
            return Ok(finish(Outcome::Budget, reason, Some(phase)));
        }
        if let Some(f) = shared.held(phase)? {
            return Ok(f);
        }
        let timeout = phase_timeout(phase, p.timeout_minutes);
        eprintln!("ns run: {} {phase} attempt {attempt} ({why})", ctx.unit);
        let locks = RunnerLocks::acquire(
            &ctx.env.lock_dir,
            &fac.locks(&p),
            &ctx.unit,
            &shared.clock,
            timeout,
            shared.until,
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
                    return Ok(finish(Outcome::Budget, reason, Some(phase)));
                }
                phases.push(json!({
                    "phase": phase,
                    "attempt": attempt,
                    "decision": why,
                    "written": false,
                    "reason": reason,
                }));
                ledger.lock_gave_up(adm, &reason);
                continue;
            }
        };
        let held = match recheck_budget(held, fac.budget_usd(), shared.spent(), phase) {
            Ok(held) => held,
            Err(f) => return Ok(f),
        };
        // A unit that waited for a lock while another hit a usage limit waits for the reset.
        if let Some(f) = shared.held(phase)? {
            return Ok(f);
        }
        let art_path = ctx.artifacts.join(artifact_of(phase));
        let moves = archive_for(&ctx.artifacts, phase, &state)?;
        let mut feedback = adm.feedback.clone();
        if let Some((_, branch, sha)) = &adm.rebase {
            // The phase takes in the default branch as it is now, not a commit noted earlier. Ship
            // merges the base in and never rebases (skills/ns-ship).
            let _ = git::run(&ctx.worktree, &["fetch", "-q", "origin", branch]);
            let how = if phase == "build" {
                "rebase onto"
            } else {
                "merge in"
            };
            feedback.push_str(&format!(
                "\n\nThe base moved: fetch origin and {how} the newest origin/{branch} ({sha}). This {phase} costs no attempt.\n"
            ));
            ctx.log(json!({
                "event": "rebase_after_base_moved",
                "phase": phase,
                "attempt": attempt,
                "onto": sha,
                "cause": why,
            }));
        }
        let mut prompt = ctx.prompt(&p, attempt, &feedback)?;
        if args.triage_only {
            prompt.push_str(factory::TRIAGE_ONLY);
        }
        let transcript = ctx
            .common
            .join("ns")
            .join("transcripts")
            .join(&ctx.unit)
            .join(format!("{phase}-{attempt}.jsonl"));
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
                shared.watch_pid.map(|p| p.to_string()).unwrap_or_default(),
            ),
        ];
        if !crate::git::ssh_command_set(&ctx.worktree) {
            env.push(("GIT_SSH_COMMAND", crate::git::SSH_KEEPALIVE.to_string()));
        }
        let tmp = match phase_tmpdir(&ctx.env.tmp, phase, attempt) {
            Ok(tmp) => tmp,
            Err(e) => {
                restore(&moves);
                return Err(e);
            }
        };
        env.push(("TMPDIR", tmp.to_string_lossy().into_owned()));
        pin::write_settings(&ctx.common, &ctx.unit, &ctx.env.ns)?;
        let r = run_harness(
            &commands[phase],
            &prompt,
            &ctx.worktree,
            &env,
            timeout,
            &transcript,
            subscription,
            ctx.env.cap,
        );
        drop(held);
        if let Some(sig) = r.as_ref().err().and(crate::stop::requested()) {
            let ev = json!({
                "event": "phase",
                "phase": phase,
                "attempt": attempt,
                "decision": why,
                "transcript": transcript.to_string_lossy(),
            });
            return Err(set_aside(ctx, phase, &moves, sig, ev)?);
        }
        let r = r?;
        let t = ClaudeStreamJson.parse(&r.stdout);
        let cost = t.cost_usd.unwrap_or(0.0);
        shared.add_spend(&ctx.unit, cost)?;
        let mut rec = json!({
            "phase": phase,
            "attempt": attempt,
            "decision": why,
            "exit": r.exit,
            "timed_out": r.timed_out,
            "wall_s": (r.wall_s * 10.0).round() / 10.0,
            "cost_usd": cost,
            "input_tokens": t.input_tokens,
            "output_tokens": t.output_tokens,
            "transcript": transcript.to_string_lossy(),
        });
        // A phase cut short wrote nothing, whatever it left behind.
        let cut_short = r.timed_out || r.killed.is_some();
        if cut_short {
            let file = artifact_of(phase);
            let kind = if r.timed_out { "timeout" } else { "killed" };
            let stem = format!("{}-{kind}", file.trim_end_matches(".md"));
            if let Some(dst) = archive_as(&ctx.artifacts, file, &stem)? {
                rec["archived"] = json!(dst.to_string_lossy());
            }
        }
        if let Some(limit) = billing::usage_limit(&r.stdout, shared.clock.now()) {
            rec["status"] = json!("paused");
            rec["reason"] = json!(limit.message);
            restore(&moves);
            ctx.log(merge_obj(json!({"event": "phase"}), &rec));
            phases.push(rec);
            let mut f = finish(
                Outcome::Paused,
                format!("usage limit: {}", limit.message),
                Some(phase),
            );
            f.reset_at = limit.reset_at;
            return Ok(f);
        }
        let art = read_art(&art_path);
        let invalid = art.as_ref().and_then(|a| a.invalid.clone());
        ledger.phase_ended(
            &adm,
            &PhaseResult {
                status: art
                    .as_ref()
                    .filter(|a| a.invalid.is_none())
                    .map(|a| a.status.clone()),
                timed_out: r.timed_out,
                killed: r.killed.clone(),
                timeout_text: gate::timed_out_after(timeout),
            },
        );
        let written = art.is_some();
        if art.as_ref().is_some_and(|a| a.status == "pass") {
            // A failed phase's scratch files stay for a human to read, until `ns clean`. A pass
            // whose scratch dir will not go is logged, not failed: `ns clean` removes the leftover.
            if let Err(e) = fs::remove_dir_all(&tmp) {
                ctx.log(json!({
                    "event": "tmpdir_not_removed",
                    "phase": phase,
                    "path": tmp.display().to_string(),
                    "error": e.to_string(),
                }));
            }
        }
        if !written {
            restore(&moves);
        }
        rec["written"] = json!(written);
        rec["status"] = json!(if written {
            art.as_ref().map(|a| a.status.clone())
        } else {
            None
        });
        rec["sha"] = json!(ctx.head());
        if written {
            *last_artifact = Some(art_path.to_string_lossy().into_owned());
        } else {
            let why = if r.timed_out {
                gate::timed_out_after(timeout)
            } else if let Some(k) = &r.killed {
                k.clone()
            } else {
                match r.exit {
                    Some(0) => "no artifact written".to_string(),
                    Some(c) => format!("exited {c}, no artifact written"),
                    None => "killed by a signal".to_string(),
                }
            };
            rec["reason"] = json!(why);
        }
        ctx.log(merge_obj(json!({"event": "phase"}), &rec));
        phases.push(rec);

        if let Some(f) = guards(ctx, &mut baseline, phase)? {
            return Ok(f);
        }
        let head_now = ctx.head();
        let head_moved = !same_sha(&state.head, &head_now);
        if head_moved {
            ledger.head_moved(phase, &state.head, &head_now);
        }
        if let (Some(errors), false) = (&invalid, args.triage_only) {
            let file = artifact_of(phase);
            let stem = format!("{}-invalid", file.trim_end_matches(".md"));
            let archived = archive_as(&ctx.artifacts, file, &stem)?;
            ctx.log(json!({
                "event": "artifact_invalid",
                "phase": phase,
                "attempt": attempt,
                "errors": errors,
                "archived": archived.map(|p| p.to_string_lossy().into_owned()),
            }));
            ledger.artifact_invalid(&adm, file, errors);
            continue;
        }
        if args.triage_only {
            return Ok(match (r.timed_out, r.killed, r.exit) {
                (true, _, _) => finish(Outcome::Stuck, gate::timed_out_after(timeout), Some(phase)),
                (false, Some(k), _) => finish(Outcome::Stuck, k, Some(phase)),
                (false, None, Some(0)) => finish(Outcome::Done, "triage ran", Some(phase)),
                (false, None, Some(c)) => {
                    finish(Outcome::Stuck, format!("exited {c}"), Some(phase))
                }
                (false, None, None) => finish(Outcome::Stuck, "killed by a signal", Some(phase)),
            });
        }
        if phase == "triage" && !written && r.exit == Some(0) && !cut_short && !art_path.exists() {
            return Ok(finish(
                Outcome::Stuck,
                "triage wrote no brief: the issue needs a human or ns-define",
                Some("triage"),
            ));
        }
        if cut_short && phase == "build" {
            if let Some(fb) = gate_state.red_feedback() {
                let how = r.killed.as_deref().unwrap_or("timed out");
                ledger.cut_short_build(fb, how);
            }
            continue;
        }
        if let Some((cmd, trigger)) =
            gate_trigger(phase, art.as_ref(), head_moved, ctx.fac.gate.local)
                .and_then(|t| Some((ctx.gate.commands.for_trigger(t)?, t)))
        {
            let red = ctx.gate(&mut gate_state, trigger, cmd, phase, attempt);
            if let Some(sig) = red.as_ref().err().and(crate::stop::requested()) {
                let mut ev = json!({"event": "gate", "phase": phase, "attempt": attempt});
                if written {
                    return Err(set_aside(ctx, phase, &moves, sig, ev)?);
                }
                // The phase wrote nothing and its moves are already undone: nothing to set aside.
                ev["interrupted"] = json!(crate::stop::name(sig));
                ctx.log(ev);
                return Err(crate::stop::error(sig));
            }
            if let Some(red) = red? {
                let moved_since = match &adm.rebase {
                    Some((Rebase::BaseMoved, branch, onto)) => {
                        remote_sha(&ctx.worktree, branch).is_some_and(|now| now != *onto)
                    }
                    _ => false,
                };
                let stuck = red.stuck_reason();
                let culprit = culprit(ctx, &ledger, &red.feedback);
                if let Some((i, who, paths)) = &culprit {
                    ctx.log(json!({"event": "gate_fault", "phase": phase, "attempt": attempt, "culprit": who, "move": i, "paths": paths}));
                }
                ledger.gate_red(
                    &adm,
                    &red.feedback,
                    red.reason,
                    stuck,
                    moved_since,
                    culprit.as_ref().map(|(i, _, _)| *i),
                );
            } else {
                ledger.gate_green();
            }
        }
    }
}

/// Mark the verify and review artifacts that only a small base merge by ship made stale, so they
/// carry forward (CORE-1): `state.carried`.
pub(super) fn carry_forward(ctx: &Ctx<'_>, ledger: &Ledger, state: &mut State) {
    for phase in ["verify", "review"] {
        let Some(sha) = state.arts.get(phase).and_then(|a| a.sha.clone()) else {
            continue;
        };
        if state.current(&sha)
            || !ledger.only_ship_moved(&sha, &state.head)
            || !git::took_in_base(&ctx.worktree, &ctx.base, &sha, &state.head)
        {
            continue;
        }
        let Some(lines) = git::interdiff_lines(&ctx.worktree, &ctx.base, &sha, &state.head) else {
            continue;
        };
        if lines <= DELTA_MAX_LINES {
            state.carried.insert(phase, Carried { from: sha, lines });
        }
    }
}

/// The phase whose commits a red gate's output points at, with the paths it matched: the newest
/// phase run that moved HEAD and changed a file the output names. `None` when the output names
/// no file, or none any phase changed (the base, or the environment).
fn culprit(ctx: &Ctx<'_>, ledger: &Ledger, output: &str) -> Option<(usize, String, Vec<String>)> {
    let failing = gate::failing_paths(output);
    if failing.is_empty() {
        return None;
    }
    for (i, m) in ledger.suspects() {
        // Ship's move may merge the base in: only the files whose own lines it changed count.
        let changed: Vec<String> = if m.phase == "ship" {
            match git::interdiff_files(&ctx.worktree, &ctx.base, &m.from, &m.to) {
                Some(f) => f,
                None => continue,
            }
        } else {
            match git::run(&ctx.worktree, &["diff", "--name-only", &m.from, &m.to]) {
                Ok(t) => t.lines().map(str::to_string).collect(),
                Err(_) => continue,
            }
        };
        let hit: Vec<String> = failing
            .iter()
            .filter(|f| changed.iter().any(|c| gate::same_file(c, f)))
            .cloned()
            .collect();
        if !hit.is_empty() {
            return Some((i, m.phase.clone(), hit));
        }
    }
    None
}

/// Where the unit's ledger is kept between `ns run` invocations.
fn ledger_path(artifacts: &std::path::Path) -> PathBuf {
    artifacts.join("state.json")
}

/// The ledger a run starts from, with the events to log about it: a new one under `--from`,
/// `--triage-only` or `--fresh`, else the saved one at `head`. A saved ledger that can't be read
/// or doesn't parse (a torn write, an older format) is dropped, and the run re-derives its state
/// from the artifacts.
pub(super) fn start_ledger(
    args: &RunArgs,
    artifacts: &std::path::Path,
    head: &str,
) -> (Ledger, Vec<Value>) {
    let from = args
        .triage_only
        .then_some("triage")
        .or(args.from.as_deref());
    if from.is_some() || args.fresh {
        return (Ledger::new(from), vec![]);
    }
    let path = ledger_path(artifacts);
    match load_ledger(artifacts) {
        Ok(None) => (Ledger::new(None), vec![]),
        Ok(Some(mut l)) => {
            let dropped = l.resumed_at(head);
            let ev = json!({"event": "ledger_resumed", "path": path.to_string_lossy(), "forced_dropped": dropped});
            (l, vec![ev])
        }
        Err(e) => {
            let ev = json!({"event": "state_rederived", "path": path.to_string_lossy(), "error": format!("{e:#}")});
            (Ledger::new(None), vec![ev])
        }
    }
}

/// The saved ledger, `None` when there is none.
fn load_ledger(artifacts: &std::path::Path) -> Result<Option<Ledger>> {
    let text = match fs::read_to_string(ledger_path(artifacts)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some(serde_json::from_str(&text)?))
}

/// Write the ledger through a temp file and a rename, so a crash leaves the old one or the new
/// one. A failed write is logged, not fatal: the run only loses what a restart would resume.
fn save_ledger(ctx: &Ctx<'_>, ledger: &Ledger) {
    let path = ledger_path(&ctx.artifacts);
    let tmp = path.with_extension("json.tmp");
    let write = serde_json::to_vec_pretty(ledger)
        .map_err(anyhow::Error::from)
        .and_then(|b| fs::write(&tmp, b).map_err(anyhow::Error::from))
        .and_then(|()| fs::rename(&tmp, &path).map_err(anyhow::Error::from));
    if let Err(e) = write {
        ctx.log(json!({"event": "ledger_not_saved", "path": path.to_string_lossy(), "error": format!("{e:#}")}));
    }
}

/// A stop ended `phase`, or the gate run after it, before it was done: set aside what the phase
/// wrote and put back what it replaced, so the next run decides from the state before the
/// phase, and log `ev` with what happened. Returns the stop's error.
fn set_aside(
    ctx: &Ctx<'_>,
    phase: &str,
    moves: &[(PathBuf, PathBuf)],
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

/// Hard rules after every phase: the default branch didn't move because of this run, and no PR
/// was merged by a phase.
fn guards(
    ctx: &Ctx<'_>,
    baseline: &mut Option<(String, String)>,
    phase: &str,
) -> Result<Option<Finish>> {
    if let Some((branch, sha)) = baseline.clone() {
        if let Some(now) = remote_sha(&ctx.worktree, &branch) {
            if now != sha {
                let reachable = git::ok(
                    &ctx.worktree,
                    &["merge-base", "--is-ancestor", &now, "HEAD"],
                );
                let mut prs = vec![];
                if reachable {
                    match merged_by_others(ctx, &sha, &now) {
                        Ok(p) => prs = p,
                        Err(why) => {
                            ctx.log(json!({"event": "breach", "rule": "default branch moved", "phase": phase, "from": sha, "to": now, "why": why}));
                            return Ok(Some(finish(
                                Outcome::Stuck,
                                format!("default branch moved: {branch} went from {sha} to {now}, which this unit holds and may have pushed: {why}"),
                                Some(phase),
                            )));
                        }
                    }
                }
                ctx.log(json!({"event": "default_moved_by_other_actor", "phase": phase, "from": sha, "to": now, "prs": prs}));
                *baseline = Some((branch, now));
            }
        }
    }
    if let Some(n) = known_pr(&ctx.artifacts) {
        let forge = forge::open(&ctx.fac.queue, &ctx.worktree)?;
        if forge.pr(n)?.status == PrStatus::Merged {
            ctx.log(
                json!({"event": "breach", "rule": "PR merged by run", "phase": phase, "pr": n}),
            );
            return Ok(Some(finish(
                Outcome::Stuck,
                format!("PR merged by run: #{n} was merged during the {phase} phase, outside ns run's merge step"),
                Some(phase),
            )));
        }
    }
    Ok(None)
}

/// The most commits one move of the default branch is checked for, so a long push is still
/// decided with a bounded number of forge queries.
const MOVE_CHECK_MAX: usize = 50;

/// The default branch moved from `from` to `now`, a commit the unit's HEAD holds. That is
/// another actor's move only when every commit it added along the first-parent line came from
/// a merged PR that isn't this unit's: the unit holds `now` because it rebased onto it. Returns
/// those PRs, or why the move counts as this unit's.
fn merged_by_others(ctx: &Ctx<'_>, from: &str, now: &str) -> std::result::Result<Vec<u64>, String> {
    let text = git::run(
        &ctx.worktree,
        &[
            "rev-list",
            "--first-parent",
            &format!("--max-count={}", MOVE_CHECK_MAX + 1),
            &format!("{from}..{now}"),
        ],
    )
    .map_err(|e| format!("cannot list the commits it added: {e:#}"))?;
    let commits: Vec<&str> = text.split_whitespace().collect();
    if commits.is_empty() {
        return Err("it added no commit".into());
    }
    if commits.len() > MOVE_CHECK_MAX {
        return Err(format!("it added more than {MOVE_CHECK_MAX} commits"));
    }
    let own_pr = known_pr(&ctx.artifacts);
    let own_branch = format!("{}{}", worktree::BRANCH_PREFIX, ctx.unit);
    let forge = forge::open(&ctx.fac.queue, &ctx.worktree)
        .map_err(|e| format!("cannot tell who made it: {e:#}"))?;
    let mut prs = vec![];
    for c in commits {
        let pulls = forge
            .commit_prs(c)
            .map_err(|e| format!("cannot tell who made {c}: {e:#}"))?;
        let n = other_merged_pr(&pulls, own_pr, &own_branch)
            .ok_or_else(|| format!("{c} is in no merged PR of another branch"))?;
        if !prs.contains(&n) {
            prs.push(n);
        }
    }
    Ok(prs)
}

/// A merged PR among the PRs that hold a commit, when none of them is `own_pr` or comes from
/// `own_branch`.
fn other_merged_pr(pulls: &[CommitPr], own_pr: Option<u64>, own_branch: &str) -> Option<u64> {
    let own = |p: &CommitPr| Some(p.number) == own_pr || p.head == own_branch;
    if pulls.iter().any(own) {
        return None;
    }
    pulls.iter().find(|p| p.merged).map(|p| p.number)
}

#[cfg(test)]
mod tests {
    use super::super::locks::tests::MIN;
    use super::*;
    use crate::clock::Clock;

    #[test]
    fn other_merged_pr_names_only_a_merged_pr_of_another_branch() {
        let pr = |number: u64, merged: bool, head: &str| CommitPr {
            number,
            head: head.into(),
            merged,
        };
        let mine = "ns/7-fix";
        let check = |list: Vec<CommitPr>, own: Option<u64>| other_merged_pr(&list, own, mine);
        assert_eq!(check(vec![pr(3, true, "ns/3-b")], None), Some(3));
        assert_eq!(check(vec![pr(3, true, "ns/3-b")], Some(12)), Some(3));
        assert_eq!(check(vec![], None), None);
        assert_eq!(check(vec![pr(3, false, "ns/3-b")], None), None);
        assert_eq!(check(vec![pr(12, true, "ns/3-b")], Some(12)), None);
        assert_eq!(check(vec![pr(9, true, mine)], None), None);
        assert_eq!(
            check(vec![pr(3, true, "ns/3-b"), pr(12, false, "x")], Some(12)),
            None
        );
        assert_eq!(
            check(vec![pr(3, false, "a"), pr(4, true, "b")], Some(12)),
            Some(4)
        );
    }

    #[test]
    fn a_budget_spent_during_a_lock_wait_stops_the_phase_and_frees_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let names = vec!["a".to_string()];
        let acquire = || {
            RunnerLocks::acquire(
                dir.path(),
                &names,
                "u",
                &Clock::pinned(1000),
                MIN,
                None,
                |_, _| {},
            )
            .unwrap()
            .held()
        };
        let held = recheck_budget(acquire(), Some(5.0), 4.0, "build");
        drop(held.ok().expect("under budget"));
        let Err(f) = recheck_budget(acquire(), Some(5.0), 5.0, "build") else {
            panic!("a spent budget kept the locks")
        };
        assert!(matches!(f.outcome, Outcome::Budget));
        assert_eq!(f.reason, "spent $5.00 of the $5.00 budget");
        assert_eq!(f.phase.as_deref(), Some("build"));
        acquire();
    }

    /// The phase `ledger` runs next on a unit with no artifacts.
    fn next_phase(ledger: &mut Ledger) -> String {
        let dir = tempfile::tempdir().unwrap();
        match ledger
            .next(&read_state(dir.path(), "abc1234".into(), true, None))
            .0
        {
            Decision::Run { phase, .. } => phase.to_string(),
            d => format!("{d:?}"),
        }
    }

    fn save(dir: &std::path::Path, ledger: &Ledger) {
        fs::write(ledger_path(dir), serde_json::to_vec(ledger).unwrap()).unwrap();
    }

    #[test]
    fn a_saved_ledger_resumes_and_a_missing_one_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let resume = RunArgs::default();
        let (mut l, events) = start_ledger(&resume, dir.path(), "abc1234");
        assert_eq!((next_phase(&mut l).as_str(), events.len()), ("triage", 0));
        let mut ledger = Ledger::new(Some("review"));
        ledger.saved_at("abc1234");
        save(dir.path(), &ledger);
        let (mut l, events) = start_ledger(&resume, dir.path(), "abc1234");
        assert_eq!(next_phase(&mut l), "review");
        assert_eq!(events[0]["event"], "ledger_resumed");
        assert_eq!(events[0]["forced_dropped"], false);
        for args in [
            RunArgs {
                fresh: true,
                ..RunArgs::default()
            },
            RunArgs {
                from: Some("build".into()),
                ..RunArgs::default()
            },
        ] {
            let (mut l, events) = start_ledger(&args, dir.path(), "abc1234");
            assert_ne!(next_phase(&mut l), "review", "{args:?}");
            assert!(events.is_empty(), "{args:?}");
        }
    }

    #[test]
    fn a_forced_decision_saved_at_another_head_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = Ledger::new(Some("review"));
        ledger.saved_at("abc1234");
        save(dir.path(), &ledger);
        let (mut l, events) = start_ledger(&RunArgs::default(), dir.path(), "def5678");
        assert_eq!(next_phase(&mut l), "triage");
        assert_eq!(events[0]["event"], "ledger_resumed");
        assert_eq!(events[0]["forced_dropped"], true);
        save(dir.path(), &Ledger::new(Some("review")));
        let (mut l, _) = start_ledger(&RunArgs::default(), dir.path(), "abc1234");
        assert_eq!(next_phase(&mut l), "triage", "saved at no recorded HEAD");
    }

    #[test]
    fn a_torn_ledger_is_rederived_and_logged() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(ledger_path(dir.path()), "{\"attempts\": {\"bui").unwrap();
        let (mut l, events) = start_ledger(&RunArgs::default(), dir.path(), "abc1234");
        assert_eq!(next_phase(&mut l), "triage");
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["event"], "state_rederived");
        assert!(events[0]["error"].as_str().is_some_and(|e| !e.is_empty()));
    }

    #[test]
    fn a_spent_budget_names_the_spend_and_the_limit() {
        assert_eq!(budget_spent(None, 99.0), None);
        assert_eq!(budget_spent(Some(5.0), 4.99), None);
        assert_eq!(
            budget_spent(Some(5.0), 5.0).as_deref(),
            Some("spent $5.00 of the $5.00 budget")
        );
    }
}
