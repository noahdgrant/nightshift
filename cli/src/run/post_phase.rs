//! What follows a phase, gathered as facts for `core::step`: the hard rules (the default branch
//! didn't move because of this run, no phase merged a PR), and the gate with the phase its
//! failure points at. `core::guard` and `core::gate` decide; this module reads git and the forge
//! for them and logs what they return.

use anyhow::Result;
use serde_json::json;

use super::core::gate::Suspect;
use super::core::guard::{self, Unread};
use super::core::step::{GateCall, GateRan, GateRed, GuardFacts};
use super::core::{self, Ledger};
use super::currency::known_pr;
use super::drive::Io;
use super::phase_run::{set_aside, Undo};
use super::{remote_sha, Ctx};
use crate::gate;
use crate::git;
use crate::watch::forge::{self, PrStatus};
use crate::worktree;

/// Run the gate `call` names after `phase`'s attempt `attempt`, and report how it came out. A
/// stop during the gate sets aside what the phase wrote (`undo`) and is an error.
pub(super) fn run_gate(
    io: &mut Io<'_, '_>,
    ledger: &Ledger,
    call: &GateCall,
    (phase, attempt): (&str, u32),
    undo: &Undo,
) -> Result<GateRan> {
    let ctx = io.ctx;
    let Some(cmd) = ctx.gate.commands.for_trigger(call.trigger) else {
        return Ok(GateRan::NoCommand);
    };
    let red = ctx.gate(&mut io.gate_state, call.trigger, cmd, phase, attempt);
    if let Some(sig) = red.as_ref().err().and(crate::stop::requested()) {
        let mut ev = json!({"event": "gate", "phase": phase, "attempt": attempt});
        if undo.written {
            return Err(set_aside(ctx, phase, &undo.moves, sig, ev)?);
        }
        // The phase wrote nothing and its moves are already undone: nothing to set aside.
        ev["interrupted"] = json!(crate::stop::name(sig));
        ctx.log(ev);
        return Err(crate::stop::error(sig));
    }
    let Some(red) = red? else {
        return Ok(GateRan::Green);
    };
    let moved_since = call.rebased_onto.as_ref().is_some_and(|(branch, onto)| {
        remote_sha(&ctx.worktree, branch).is_some_and(|now| now != *onto)
    });
    let culprit = culprit(ctx, ledger, &red.feedback);
    if let Some((i, who, paths)) = &culprit {
        ctx.log(json!({"event": "gate_fault", "phase": phase, "attempt": attempt, "culprit": who, "move": i, "paths": paths}));
    }
    Ok(GateRan::Red(GateRed {
        stuck_reason: red.stuck_reason(),
        feedback: red.feedback,
        reason: red.reason,
        moved_since,
        culprit: culprit.map(|(i, _, _)| i),
    }))
}

/// The files git tracks in the worktree. When `git ls-files` fails, logs it and returns none: a
/// failing path then matches by equality or by suffix, with no other file to make it ambiguous.
fn tracked_files(ctx: &Ctx<'_>) -> Vec<String> {
    match git::run(&ctx.worktree, &["ls-files"]) {
        Ok(t) => t.lines().map(str::to_string).collect(),
        Err(e) => {
            ctx.log(json!({"event": "ls_files_failed", "error": format!("{e:#}")}));
            Vec::new()
        }
    }
}

/// The phase whose commits a red gate's output points at, with the paths it matched, as
/// `core::gate::culprit` decides over the files each suspect changed. `None` when the output
/// names no file, or none any phase changed (the base, or the environment).
pub(super) fn culprit(
    ctx: &Ctx<'_>,
    ledger: &Ledger,
    output: &str,
) -> Option<(usize, String, Vec<String>)> {
    let failing = gate::failing_paths(output);
    if failing.is_empty() {
        return None;
    }
    let tracked = tracked_files(ctx);
    let suspects: Vec<Suspect> = ledger
        .suspects()
        .map(|(index, m)| Suspect {
            index,
            phase: m.phase.clone(),
            changed: if m.phase == "ship" {
                git::interdiff_files(&ctx.worktree, &ctx.base, &m.from, &m.to)
            } else {
                git::run(&ctx.worktree, &["diff", "--name-only", &m.from, &m.to])
                    .ok()
                    .map(|t| t.lines().map(str::to_string).collect())
            },
        })
        .collect();
    core::gate::culprit(&failing, &suspects, &ctx.worktree, &tracked)
}

/// Hard rules after every phase: the default branch didn't move because of this run, and no PR
/// was merged by a phase. Gathers what `core::guard` decides on, and logs what it returns.
pub(super) fn guards(
    ctx: &Ctx<'_>,
    baseline: &mut Option<(String, String)>,
    phase: &str,
) -> Result<GuardFacts> {
    if let Some((branch, sha)) = baseline.clone() {
        if let Some(now) = remote_sha(&ctx.worktree, &branch) {
            if now != sha {
                let reachable = git::ok(
                    &ctx.worktree,
                    &["merge-base", "--is-ancestor", &now, "HEAD"],
                );
                let attributed = reachable.then(|| merged_by_others(ctx, &sha, &now));
                let (ev, stop) = guard::branch_moved(phase, (&branch, &sha, &now), attributed);
                ctx.log(ev);
                if stop.is_some() {
                    return Ok(GuardFacts { breach: stop });
                }
                *baseline = Some((branch, now));
            }
        }
    }
    if let Some(n) = known_pr(&ctx.artifacts) {
        let forge = forge::open(&ctx.fac.queue, &ctx.worktree)?;
        if forge.pr(n)?.status == PrStatus::Merged {
            let (ev, reason) = guard::pr_merged(phase, n);
            ctx.log(ev);
            return Ok(GuardFacts {
                breach: Some(reason),
            });
        }
    }
    Ok(GuardFacts::default())
}

/// The PRs that made the default branch's move from `from` to `now`, a commit the unit's HEAD
/// holds, or why the move counts as this unit's: the commits it added, read here, and the
/// forge's PRs for each, read as `core::guard::moved_by_others` asks.
fn merged_by_others(ctx: &Ctx<'_>, from: &str, now: &str) -> std::result::Result<Vec<u64>, String> {
    let commits = git::run(
        &ctx.worktree,
        &[
            "rev-list",
            "--first-parent",
            &format!("--max-count={}", guard::MOVE_CHECK_MAX + 1),
            &format!("{from}..{now}"),
        ],
    )
    .map(|t| t.split_whitespace().map(String::from).collect())
    .map_err(|e| format!("{e:#}"));
    let own_branch = format!("{}{}", worktree::BRANCH_PREFIX, ctx.unit);
    let mut forge = None;
    guard::moved_by_others(commits, known_pr(&ctx.artifacts), &own_branch, |c| {
        if forge.is_none() {
            let f = forge::open(&ctx.fac.queue, &ctx.worktree)
                .map_err(|e| Unread::NoForge(format!("{e:#}")))?;
            forge = Some(f);
        }
        let f = forge.as_ref().expect("opened above");
        f.commit_prs(c)
            .map_err(|e| Unread::Commit(format!("{e:#}")))
    })
}

#[cfg(test)]
pub(super) mod tests;
