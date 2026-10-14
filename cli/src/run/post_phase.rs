//! What follows a phase, gathered as facts for `core::step`: the hard rules (the default branch
//! didn't move because of this run, no phase merged a PR), and the gate with the phase its
//! failure points at. Phase 2 of #562 moves the decisions still made here (who moved the default
//! branch, which move a red gate blames) into `core::guard` and `core::gate`.

use anyhow::Result;
use serde_json::json;

use super::core::step::{GateCall, GateRan, GateRed, GuardFacts};
use super::core::Ledger;
use super::currency::known_pr;
use super::drive::Io;
use super::phase_run::{set_aside, Undo};
use super::{remote_sha, Ctx};
use crate::gate;
use crate::git;
use crate::watch::forge::{self, CommitPr, PrStatus};
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

/// The phase whose commits a red gate's output points at, with the paths it matched: the newest
/// phase run that moved HEAD and changed a file the output names. `None` when the output names
/// no file, or none any phase changed (the base, or the environment).
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
            .filter(|f| {
                changed
                    .iter()
                    .any(|c| gate::same_file(c, f, &ctx.worktree, &tracked))
            })
            .cloned()
            .collect();
        if !hit.is_empty() {
            return Some((i, m.phase.clone(), hit));
        }
    }
    None
}

/// Hard rules after every phase: the default branch didn't move because of this run, and no PR
/// was merged by a phase.
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
                let mut prs = vec![];
                if reachable {
                    match merged_by_others(ctx, &sha, &now) {
                        Ok(p) => prs = p,
                        Err(why) => {
                            ctx.log(json!({"event": "breach", "rule": "default branch moved", "phase": phase, "from": sha, "to": now, "why": why}));
                            return Ok(breach(format!("default branch moved: {branch} went from {sha} to {now}, which this unit holds and may have pushed: {why}")));
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
            return Ok(breach(format!("PR merged by run: #{n} was merged during the {phase} phase, outside ns run's merge step")));
        }
    }
    Ok(GuardFacts::default())
}

fn breach(reason: String) -> GuardFacts {
    GuardFacts {
        breach: Some(reason),
    }
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
pub(super) mod tests;
