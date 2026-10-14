//! The merge step: bring the PR up to date with its base and wait for CI, then take the merge
//! lock and squash-merge it if the base has not moved since (`merge.policy = "auto"`).

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use super::ci_gate::{gate_before_merge, gate_merge_result};
use super::currency::{known_pr, State};
use super::locks::{Bound, MERGE_LOCK};
use super::{finish, gh, gh_json, holder_pid, remote_default, Ctx, Finish, Lock, Outcome, Shared};
use crate::clock::Clock;
use crate::eval::trial::run_process;
use crate::factory::LocalGate;
use crate::gate;
use crate::git::{self, same_sha};
use crate::markers;

/// Take the merge lock for `unit`, waiting while another unit merges, each wait logged by
/// `log`. A wait that reaches `until` ends the run as `budget`.
fn take_merge_lock(
    common: &Path,
    unit: &str,
    clock: &Clock,
    until: Option<i64>,
    log: impl Fn(Value),
) -> Result<Result<Lock, Finish>> {
    let me = json!({"pid": std::process::id(), "unit": unit});
    let deadline = until.map(|u| (clock, u));
    let taken = Lock::wait(common, MERGE_LOCK, &me, deadline, |by| {
        eprintln!(
            "ns run: {unit} merge waits for the merge lock (held by pid {}, unit {})",
            holder_pid(by),
            by["unit"].as_str().unwrap_or("?")
        );
        log(json!({"event": "lock_wait", "phase": "merge", "lock": "merge", "held_by": by}));
    })?;
    Ok(taken.map_err(|by| {
        let reason = format!(
            "gave up waiting for the merge lock (held by pid {}, unit {}) at --until",
            holder_pid(&by),
            by["unit"].as_str().unwrap_or("?")
        );
        eprintln!("ns run: {unit} merge {reason}");
        log(json!({
            "event": "lock_wait_timeout",
            "phase": "merge",
            "lock": "merge",
            "held_by": by,
            "bound": Bound::Until.label(),
        }));
        finish(Outcome::Budget, reason, Some("merge"))
    }))
}

const REGISTER_BACKOFF_START: i64 = 5;
const REGISTER_BACKOFF_MAX: i64 = 30;

enum Registered {
    Yes,
    No,
    QueryFailed(String),
}

fn check_count(wt: &Path, sha: &str, kind: &str) -> Result<u64> {
    let out = gh(
        wt,
        &[
            "api",
            &format!("repos/{{owner}}/{{repo}}/commits/{sha}/{kind}"),
            "--jq",
            ".total_count",
        ],
    )?;
    let out = out.trim();
    out.parse()
        .map_err(|_| anyhow!("unexpected output from the {kind} query: {out:?}"))
}

/// GitHub may not have registered any check run or status right after a push or update-branch.
fn checks_registered(
    wt: &Path,
    sha: &str,
    timeout_minutes: u64,
    clock: &Clock,
) -> Result<Registered> {
    let deadline = clock.now() + (timeout_minutes * 60) as i64;
    let mut wait = REGISTER_BACKOFF_START;
    let mut polls = 0;
    loop {
        clock.check_polls(polls)?;
        polls += 1;
        let counts = ["check-runs", "status"].map(|kind| check_count(wt, sha, kind));
        if counts.iter().any(|c| matches!(c, Ok(n) if *n > 0)) {
            return Ok(Registered::Yes);
        }
        let now = clock.now();
        if now >= deadline {
            return Ok(match counts.into_iter().find_map(|c| c.err()) {
                Some(e) => Registered::QueryFailed(e.to_string()),
                None => Registered::No,
            });
        }
        clock.sleep_until((now + wait).min(deadline))?;
        wait = (wait * 2).min(REGISTER_BACKOFF_MAX);
    }
}

/// The PR's merge state once GitHub has worked it out. Right after its base moves, as when the
/// unit before it merged, GitHub reports `UNKNOWN` while it recomputes, so poll with the register
/// backoff until it reports anything else, or `timeout_minutes` pass, and return the last read.
fn merge_state(wt: &Path, pr: &str, timeout_minutes: u64, clock: &Clock) -> Result<Value> {
    let deadline = clock.now() + (timeout_minutes * 60) as i64;
    let mut wait = REGISTER_BACKOFF_START;
    let mut polls = 0;
    loop {
        clock.check_polls(polls)?;
        polls += 1;
        let state = gh_json(wt, &["pr", "view", pr, "--json", "mergeStateStatus"])?
            ["mergeStateStatus"]
            .take();
        let now = clock.now();
        if state != "UNKNOWN" || now >= deadline {
            return Ok(state);
        }
        clock.sleep_until((now + wait).min(deadline))?;
        wait = (wait * 2).min(REGISTER_BACKOFF_MAX);
    }
}

enum UpdatedHead {
    Moved(String),
    Dirty,
    /// The head stayed put; carries the last merge state GitHub reported.
    Unchanged(Value),
}

/// GitHub may still report the old head right after update-branch. A view with no head keeps the
/// old one.
fn head_after_update(
    wt: &Path,
    pr: &str,
    old: &str,
    timeout_minutes: u64,
    clock: &Clock,
) -> Result<UpdatedHead> {
    let deadline = clock.now() + (timeout_minutes * 60) as i64;
    let mut wait = REGISTER_BACKOFF_START;
    let mut polls = 0;
    loop {
        clock.check_polls(polls)?;
        polls += 1;
        let v = gh_json(
            wt,
            &["pr", "view", pr, "--json", "headRefOid,mergeStateStatus"],
        )?;
        if v["mergeStateStatus"].as_str() == Some("DIRTY") {
            return Ok(UpdatedHead::Dirty);
        }
        match v["headRefOid"].as_str() {
            Some(h) if h == old => {}
            h => return Ok(UpdatedHead::Moved(h.unwrap_or(old).to_string())),
        }
        let now = clock.now();
        if now >= deadline {
            return Ok(UpdatedHead::Unchanged(v["mergeStateStatus"].clone()));
        }
        clock.sleep_until((now + wait).min(deadline))?;
        wait = (wait * 2).min(REGISTER_BACKOFF_MAX);
    }
}

pub(super) enum MergeStep {
    Finish(Finish),
    Rebuild(gate::Red),
    /// Back to ship, only to bring in the base, with this `{feedback}`.
    Reship(String),
    /// Back to build only to rebase, with its feedback and why.
    Rebase(String, String),
}

/// Back to build with `feedback` because CI on the PR failed.
fn ci_failed(feedback: String) -> MergeStep {
    MergeStep::Rebuild(gate::Red {
        feedback,
        reason: "CI failed on the PR".into(),
        killed: None,
        ratchets: None,
    })
}

/// Back to build to rebase PR #`n` onto `default`, which it conflicts with, or to ship under
/// `[gate] local = "build-only"`.
pub(super) fn conflicts(ctx: &Ctx<'_>, n: u64, default: &str) -> MergeStep {
    ctx.log(json!({"event": "conflict", "pr": n}));
    let feedback = format!(
        "PR #{n} conflicts with {default}: rebase onto {default} and resolve conflicts, then force-push with --force-with-lease"
    );
    if ctx.fac.gate.local == LocalGate::BuildOnly {
        MergeStep::Reship(feedback)
    } else {
        MergeStep::Rebase(feedback, format!("PR #{n} conflicts with {default}"))
    }
}

pub(super) fn human(reason: impl Into<String>) -> MergeStep {
    let mut f = finish(Outcome::Done, reason, Some("merge"));
    f.needs_human = true;
    MergeStep::Finish(f)
}

/// How many times the default branch may move between a merge step's CI wait and its merge
/// before the merge step holds the merge lock through the next update and CI wait, as it did
/// before merges were optimistic, so no other unit's merge can move it again.
pub(super) const BASE_MOVE_CAP: u32 = 5;

/// How many moves end the unit stuck even with the lock held: only a merge from outside `ns`
/// can move the default branch then.
pub(super) const BASE_MOVE_HARD_CAP: u32 = 2 * BASE_MOVE_CAP;

/// A PR the merge step got ready without the merge lock: CI passed at `head`, and `base` is the
/// tip of `origin/<default>` read before the update and CI wait, `None` when it couldn't be read.
struct Ready {
    head: String,
    base: Option<String>,
}

/// `merge.policy = "auto"`: squash-merge this unit's PR when CI is green, review.md passed and is
/// current at the PR head, the CI gate passed at that head, and no file or marked region that
/// needs human review changed. The only place ns merges anything. `run` numbers this merge step
/// in the invocation, for its gate log.
///
/// Optimistic: the update, gate and CI wait run without the merge lock, which is taken only for
/// the final merge. A default branch that moved since CI started sends the PR round again, at
/// most [`BASE_MOVE_CAP`] times.
pub(super) fn merge_step(
    ctx: &Ctx<'_>,
    state: &State,
    shared: &Shared,
    gate_state: &mut gate::Gate,
    run: u32,
) -> Result<MergeStep> {
    let (n, head, pr_head) = match open_pr(ctx, state)? {
        Ok(pr) => pr,
        Err(step) => return Ok(step),
    };
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let default = remote_default(wt)
        .map(|d| d.0)
        .unwrap_or_else(|| "main".into());
    let mut current = pr_head;
    let mut moves = 0;
    let mut held = None;
    let take = || {
        take_merge_lock(&ctx.common, &ctx.unit, &shared.clock, shared.until, |ev| {
            ctx.log(ev)
        })
    };
    loop {
        if moves >= BASE_MOVE_CAP && held.is_none() {
            // Past the cap: hold the lock through the update and CI wait, so no merge by another
            // unit can move the base again.
            held = match take()? {
                Ok(lock) => Some(lock),
                Err(f) => return Ok(MergeStep::Finish(f)),
            };
            ctx.log(json!({"event": "merge_lock_held_through_ci", "pr": n, "moves": moves}));
        }
        let ready = match get_ready(ctx, n, &current, &default, shared, gate_state, run)? {
            Ok(r) => r,
            Err(step) => return Ok(step),
        };
        current.clone_from(&ready.head);

        // One merge at a time per repo, held only for the final check and the merge, unless
        // the base kept moving.
        let _merging = match &held {
            Some(_) => None,
            None => match take()? {
                Ok(lock) => Some(lock),
                Err(f) => return Ok(MergeStep::Finish(f)),
            },
        };
        let base_now = fetch_tip(wt, &default).ok();
        if base_now.is_some() && base_now != ready.base {
            moves += 1;
            ctx.log(json!({
                "event": "merge_base_moved",
                "pr": n,
                "from": ready.base,
                "to": base_now,
                "attempt": moves,
            }));
            if moves > BASE_MOVE_HARD_CAP {
                return Ok(MergeStep::Finish(finish(
                    Outcome::Stuck,
                    format!(
                        "the default branch moved {BASE_MOVE_HARD_CAP} times during this PR's CI, the last {} with the merge lock held (cap {BASE_MOVE_HARD_CAP})",
                        BASE_MOVE_HARD_CAP - BASE_MOVE_CAP
                    ),
                    Some("merge"),
                )));
            }
            continue;
        }
        // A view that fails or names no head leaves the check to --match-head-commit.
        let view = gh_json(wt, &["pr", "view", &ns, "--json", "headRefOid"]).unwrap_or_default();
        if let Some(now) = view["headRefOid"].as_str() {
            if !same_sha(now, &ready.head) {
                return Ok(human(format!(
                    "PR #{n} head moved from {} to {now} after CI passed on it; needs a human merge",
                    ready.head
                )));
            }
        }
        return final_merge(ctx, n, &head, &ready.head, &default, shared);
    }
}

/// Bring PR #`n`, at `pr_head`, up to date with `default` when it is behind, gate it when the
/// config calls for a gate here, and wait for its CI, all without the merge lock: the head CI
/// passed on and the base read first, or how the merge step ends.
fn get_ready(
    ctx: &Ctx<'_>,
    n: u64,
    pr_head: &str,
    default: &str,
    shared: &Shared,
    gate_state: &mut gate::Gate,
    run: u32,
) -> Result<Result<Ready, MergeStep>> {
    let ns = n.to_string();
    let wt = &ctx.worktree;
    // Read before the merge state, so a merge that lands during the update counts as a move.
    let base = fetch_tip(wt, default).ok();

    // Strict required checks: a PR behind its base gets the base merged in, then fresh checks,
    // unless the local merge queue gates the merge result instead. Under build-only GitHub's
    // checks on the PR head with the base merged in are the merge queue, and no local gate runs.
    let remote = ctx.fac.gate.local == LocalGate::BuildOnly;
    let queued = !remote && ctx.fac.merge.local_queue && ctx.gate.commands.rest.is_some();
    let conflict = || conflicts(ctx, n, default);
    let register_minutes = ctx.fac.merge.ci_register_timeout;
    let mut merge_head = pr_head.to_string();
    let behind = match merge_state(wt, &ns, register_minutes, &shared.clock)?.as_str() {
        Some("DIRTY") => return Ok(Err(conflict())),
        Some("BEHIND") => !queued,
        _ if remote => match fetch_tip(wt, default) {
            Ok(tip) => !git::ok(wt, &["merge-base", "--is-ancestor", &tip, pr_head]),
            Err(e) => {
                return Ok(Err(human(format!(
                    "cannot fetch {default} to update PR #{n}: {e:#}; needs a human merge"
                ))))
            }
        },
        _ => false,
    };
    if behind {
        match update_branch(ctx, n, pr_head, default, &shared.clock, &conflict)? {
            Ok(h) => merge_head = h,
            Err(step) => return Ok(Err(step)),
        }
    }

    if let Some(cmd) = ctx.gate.commands.rest.as_ref().filter(|_| !remote) {
        let step = if queued {
            gate_merge_result(ctx, gate_state, cmd, n, pr_head, default, run)?
        } else {
            gate_before_merge(ctx, gate_state, cmd, n, &merge_head, run)?
        };
        if let Some(step) = step {
            return Ok(Err(step));
        }
    }

    if let Some(step) = wait_for_ci(ctx, n, &merge_head, &shared.clock)? {
        return Ok(Err(step));
    }
    Ok(Ok(Ready {
        head: merge_head,
        base,
    }))
}

/// Under the merge lock: check for files and regions that need human review, then squash-merge
/// PR #`n` at `merge_head` and delete its branch. `head` is the worktree's HEAD, for the log.
fn final_merge(
    ctx: &Ctx<'_>,
    n: u64,
    head: &str,
    merge_head: &str,
    default: &str,
    shared: &Shared,
) -> Result<MergeStep> {
    let ns = n.to_string();
    let wt = &ctx.worktree;
    if let Some(step) = human_review(ctx, n, default)? {
        return Ok(step);
    }
    // No --delete-branch: gh would check out the default branch in the unit's worktree first.
    let merged = gh(
        wt,
        &[
            "pr",
            "merge",
            &ns,
            "--squash",
            "--match-head-commit",
            merge_head,
        ],
    );
    let state_now = gh_json(wt, &["pr", "view", &ns, "--json", "state"])
        .ok()
        .and_then(|v| v["state"].as_str().map(String::from));
    if state_now.as_deref() == Some("MERGED") {
        ctx.log(json!({"event": "merged", "pr": n, "sha": head, "spent_usd": shared.spent()}));
        let branch = format!("{}{}", crate::worktree::BRANCH_PREFIX, ctx.unit);
        let path = format!("repos/{{owner}}/{{repo}}/git/refs/heads/{branch}");
        if let Err(e) = gh(wt, &["api", &path, "-X", "DELETE"]) {
            ctx.log(json!({"event": "branch_not_deleted", "pr": n, "branch": branch, "error": format!("{e:#}")}));
        }
        return Ok(MergeStep::Finish(finish(
            Outcome::Merged,
            format!("squash-merged PR #{n}"),
            Some("merge"),
        )));
    }
    Ok(MergeStep::Finish(finish(
        Outcome::Stuck,
        format!(
            "gh pr merge #{n} did not merge it: {}",
            merged.err().map(|e| format!("{e:#}")).unwrap_or_default()
        ),
        Some("merge"),
    )))
}

/// The open PR `pr.md` names, if the merge step may go on: its number, HEAD, and the PR head.
fn open_pr(ctx: &Ctx<'_>, state: &State) -> Result<Result<(u64, String, String), MergeStep>> {
    if !state.arts.contains_key("ship") {
        return Ok(Err(human("no pr.md")));
    }
    let Some(n) = known_pr(&ctx.artifacts) else {
        return Ok(Err(human("pr.md has no pr: number; needs a human merge")));
    };
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let view = gh_json(wt, &["pr", "view", &ns, "--json", "state,headRefOid"])?;
    match view["state"].as_str() {
        Some("OPEN") => {}
        Some("MERGED") => {
            return Ok(Err(MergeStep::Finish(finish(
                Outcome::Stuck,
                format!("PR merged by run: #{n} was merged before ns run's merge step"),
                Some("merge"),
            ))))
        }
        other => {
            return Ok(Err(MergeStep::Finish(finish(
                Outcome::Stuck,
                format!("PR #{n} is {}", other.unwrap_or("unknown")),
                Some("merge"),
            ))))
        }
    }
    let head = git::run(wt, &["rev-parse", "HEAD"]).unwrap_or_default();
    let pr_head = view["headRefOid"].as_str().unwrap_or("");
    if !state.current(pr_head) {
        return Ok(Err(human(format!(
            "PR head {pr_head} is not current with HEAD {head}; needs a human merge"
        ))));
    }
    if !state.arts.get("review").is_some_and(|r| state.passes(r)) {
        return Ok(Err(human(
            "review.md is not pass and current; needs a human merge",
        )));
    }
    Ok(Ok((n, head, pr_head.to_string())))
}

/// Wait, bounded, for CI on the PR at `merge_head`: `None` when every check passed or skipped,
/// else how the merge step ends.
fn wait_for_ci(
    ctx: &Ctx<'_>,
    n: u64,
    merge_head: &str,
    clock: &Clock,
) -> Result<Option<MergeStep>> {
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let register_minutes = ctx.fac.merge.ci_register_timeout;
    match checks_registered(wt, merge_head, register_minutes, clock)? {
        Registered::Yes => {}
        Registered::No => {
            return Ok(Some(human(format!(
                "no CI checks registered within {register_minutes} min on PR #{n}; needs a human merge"
            ))));
        }
        Registered::QueryFailed(err) => {
            return Ok(Some(human(format!(
                "could not query CI checks on PR #{n}: {err}; needs a human merge"
            ))));
        }
    }

    // Wait for CI, bounded.
    let mut cmd = Command::new("gh");
    crate::git::scrub(&mut cmd);
    cmd.args(["pr", "checks", &ns, "--watch"])
        .current_dir(wt)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mins = ctx.fac.merge.ci_timeout_minutes;
    let (_, timed_out, _) = run_process(cmd, None, Duration::from_secs(mins * 60))
        .context("cannot run gh pr checks")?;
    if timed_out {
        return Ok(Some(MergeStep::Finish(finish(
            Outcome::Stuck,
            format!("CI on PR #{n} did not finish within {mins} min"),
            Some("merge"),
        ))));
    }
    let checks = gh_json(
        wt,
        &["pr", "checks", &ns, "--json", "name,state,bucket,link"],
    )
    .unwrap_or(Value::Array(Vec::new()));
    let checks = checks.as_array().cloned().unwrap_or_default();
    if checks.is_empty() {
        return Ok(Some(human(format!(
            "no CI checks reported on PR #{n}; needs a human merge"
        ))));
    }
    let failed: Vec<&Value> = checks
        .iter()
        .filter(|c| !matches!(c["bucket"].as_str(), Some("pass") | Some("skipping")))
        .collect();
    if !failed.is_empty() {
        let names: Vec<String> = failed
            .iter()
            .map(|c| {
                format!(
                    "{} ({})",
                    c["name"].as_str().unwrap_or("?"),
                    c["bucket"].as_str().or(c["state"].as_str()).unwrap_or("?")
                )
            })
            .collect();
        let fb = ci_feedback(wt, n, &names, &failed);
        ctx.log(json!({"event": "ci_failed", "pr": n, "checks": names, "where": "github"}));
        return Ok(Some(ci_failed(fb)));
    }
    ctx.log(json!({"event": "ci_passed", "pr": n, "sha": merge_head, "where": "github"}));
    Ok(None)
}

/// The `{feedback}` for `failed` checks: their `names` and the tail of each one's failed log.
fn ci_feedback(wt: &Path, n: u64, names: &[String], failed: &[&Value]) -> String {
    let mut fb = format!("CI failed on PR #{n}: {}\n", names.join(", "));
    let run_re = regex::Regex::new(r"/actions/runs/(\d+)").unwrap();
    for c in failed {
        if let Some(id) = c["link"].as_str().and_then(|l| run_re.captures(l)) {
            if let Ok(log) = gh(wt, &["run", "view", &id[1], "--log-failed"]) {
                let start = log.len().saturating_sub(3000);
                let start = (start..log.len())
                    .find(|i| log.is_char_boundary(*i))
                    .unwrap_or(0);
                fb.push_str(&format!(
                    "\nLog tail ({}):\n{}\n",
                    c["name"].as_str().unwrap_or("?"),
                    &log[start..]
                ));
            }
        }
    }
    fb
}

/// Whether the PR changes a file or marked region that needs human review: `None` when it
/// doesn't, else how the merge step ends.
fn human_review(ctx: &Ctx<'_>, n: u64, default: &str) -> Result<Option<MergeStep>> {
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let files: Vec<String> = gh(wt, &["pr", "diff", &ns, "--name-only"])?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let hits = ctx.fac.merge.human_review_hits(&files);
    if !hits.is_empty() {
        ctx.log(json!({"event": "human_review", "pr": n, "paths": hits}));
        return Ok(Some(human(format!(
            "changes files that need human review; needs a human merge ({})",
            hits.join(", ")
        ))));
    }
    match marked_regions(wt, default) {
        Ok(regions) if regions.is_empty() => {}
        Ok(regions) => {
            ctx.log(json!({"event": "human_review", "pr": n, "regions": regions}));
            return Ok(Some(human(format!(
                "changes code in a human-review region; needs a human merge ({})",
                regions.join(", ")
            ))));
        }
        Err(e) => {
            return Ok(Some(human(format!(
                "cannot check human-review regions: {e:#}; needs a human merge"
            ))))
        }
    }
    Ok(None)
}

/// `gh pr update-branch`, or a merge of `default` with git when GitHub refuses it for the
/// `workflow` scope: the new PR head, or how the merge step ends.
fn update_branch(
    ctx: &Ctx<'_>,
    n: u64,
    pr_head: &str,
    default: &str,
    clock: &Clock,
    conflict: &dyn Fn() -> MergeStep,
) -> Result<Result<String, MergeStep>> {
    let ns = n.to_string();
    let wt = &ctx.worktree;
    let register_minutes = ctx.fac.merge.ci_register_timeout;
    let mut via = "api";
    if let Err(e) = gh(wt, &["pr", "update-branch", &ns]) {
        let error = format!("{e:#}");
        let scope = workflow_refusal(&error);
        let mut ev = json!({"event": "update_branch_failed", "pr": n, "error": error});
        if scope {
            ev["workflow_scope"] = json!(true);
        }
        ctx.log(ev);
        if !scope {
            return Ok(Err(conflict()));
        }
        match merge_default_with_git(wt, default, pr_head) {
            Ok(true) => via = "git",
            Ok(false) => return Ok(Err(conflict())),
            Err(e) => {
                return Ok(Err(human(format!(
                        "PR #{n}: GitHub refused update-branch without the `workflow` scope, and merging {default} with git failed: {e:#}; needs a human merge"
                    ))));
            }
        }
    }
    let head = match head_after_update(wt, &ns, pr_head, register_minutes, clock)? {
        UpdatedHead::Moved(h) => h,
        UpdatedHead::Dirty => return Ok(Err(conflict())),
        UpdatedHead::Unchanged(merge_state) => {
            let error = format!(
                    "PR #{n} head {pr_head} did not change after update-branch within {register_minutes} min"
                );
            ctx.log(json!({"event": "update_branch_failed", "pr": n, "error": error, "merge_state": merge_state}));
            return Ok(Err(human(format!("{error}; needs a human merge"))));
        }
    };
    ctx.log(json!({"event": "update_branch", "pr": n, "via": via}));
    Ok(Ok(head))
}

/// The tip of `origin/<branch>`, freshly fetched.
pub(super) fn fetch_tip(wt: &Path, branch: &str) -> Result<String> {
    let out = git::net(wt, &["fetch", "-q", "origin", branch])
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git fetch origin {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    git::run(wt, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"])
}

/// Whether a failed `gh pr update-branch` was GitHub refusing to write a workflow file: the
/// token lacks the `workflow` scope (an OAuth or personal token) or the `workflows` permission
/// (a GitHub App).
fn workflow_refusal(error: &str) -> bool {
    error.contains("refusing to allow") && error.contains("workflow")
}

/// Merge the fresh tip of `origin/<default>` into the worktree's branch and push it over
/// `pr_head` with a lease, as update-branch would have. `Ok(false)` is a merge conflict,
/// aborted; any other failure is an error. HEAD must be `pr_head`, since the lease would
/// otherwise replace a PR head the merge was not built on. A failed push resets the branch to
/// the HEAD it started from, so the worktree still matches the PR. The merge runs
/// with `core.hooksPath=/dev/null`, since `--no-verify` leaves `post-merge` running; the push
/// uses `--no-verify`, as `pre-push` is the only hook it runs. Both skip the checkout's hooks
/// because the forge's CI checks the merge result; the merge does carry unit code.
fn merge_default_with_git(wt: &Path, default: &str, pr_head: &str) -> Result<bool> {
    let net = |args: &[&str]| -> Result<()> {
        let out = git::net(wt, args).output().context("failed to run git")?;
        if !out.status.success() {
            return Err(anyhow!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    };
    let start = git::run(wt, &["rev-parse", "HEAD"])?;
    if !same_sha(&start, pr_head) {
        return Err(anyhow!(
            "HEAD {start} is not the PR head {pr_head}, so a merge of {default} would replace it"
        ));
    }
    net(&["fetch", "-q", "origin", default])?;
    let branch = git::run(wt, &["symbolic-ref", "--short", "HEAD"])?;
    let theirs = format!("origin/{default}");
    let merge = git::command()
        .arg("-C")
        .arg(wt)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "merge",
            "-q",
            "--no-edit",
            "FETCH_HEAD",
            "-m",
        ])
        .arg(format!("Merge {theirs} into {branch}"))
        .output()
        .context("failed to run git")?;
    if !merge.status.success() {
        let merging = git::ok(wt, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
        let unmerged = git::run(wt, &["diff", "--name-only", "--diff-filter=U"])
            .is_ok_and(|paths| !paths.is_empty());
        let aborted = !merging || git::run(wt, &["merge", "--abort"]).is_ok();
        if unmerged && aborted {
            return Ok(false);
        }
        return Err(anyhow!(
            "git merge failed: {}",
            String::from_utf8_lossy(&merge.stderr).trim()
        ));
    }
    let lease = format!("--force-with-lease=refs/heads/{branch}:{pr_head}");
    let spec = format!("HEAD:refs/heads/{branch}");
    if let Err(e) = net(&["push", "-q", "--no-verify", &lease, "origin", &spec]) {
        return Err(match git::run(wt, &["reset", "-q", "--keep", &start]) {
            Ok(_) => e,
            Err(reset) => anyhow!("{e:#}; resetting to {start} failed too: {reset:#}"),
        });
    }
    Ok(true)
}

/// Human-review regions that HEAD's changes since its merge base with `default` touch, counting
/// fences at the freshly fetched tip of `default`. A failed fetch is an error, never a stale tip.
pub(super) fn marked_regions(wt: &Path, default: &str) -> Result<Vec<String>> {
    git::run(wt, &["fetch", "-q", "origin", default])?;
    let tip = git::run(wt, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"])?;
    let base = git::run(wt, &["merge-base", "HEAD", &tip])?;
    markers::touched_between(wt, &base, "HEAD", Some(&tip))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::locks::tests::hold;
    use super::super::tests::{cloned, commit_all};
    use super::*;

    #[test]
    fn workflow_refusal_needs_both_the_refusal_and_a_workflow() {
        assert!(workflow_refusal(
            "gh pr update-branch 12 failed: GraphQL: refusing to allow an OAuth App to create or update workflow `.github/workflows/ci.yml` without `workflow` scope"
        ));
        assert!(workflow_refusal(
            "refusing to allow a GitHub App to create or update workflow `.github/workflows/ci.yml` without `workflows` permission"
        ));
        assert!(!workflow_refusal("refusing to allow a merge: conflict"));
        assert!(!workflow_refusal("workflow run failed: conflict"));
    }

    #[test]
    fn marked_regions_errors_when_the_merge_base_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert!(marked_regions(dir.path(), "main").is_err());
        git::run(dir.path(), &["init", "-q"]).unwrap();
        assert!(marked_regions(dir.path(), "main").is_err());
    }

    /// The update-branch merge runs with no hook (`--no-verify` would leave `post-merge`) and its
    /// push skips `pre-push`, because the forge's CI checks the merge result, and the merge does
    /// carry unit code.
    #[test]
    fn merging_the_default_branch_runs_none_of_the_checkouts_hooks() {
        let (t, _origin, wt) = cloned();
        let g = |dir: &Path, args: &[&str]| git::run(dir, args).unwrap();
        g(&wt, &["switch", "-q", "-c", "ns/9-x"]);
        fs::write(wt.join("f.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "unit");
        g(&wt, &["push", "-q", "origin", "ns/9-x"]);
        let other = t.path().join("other");
        g(t.path(), &["clone", "-q", "origin.git", "other"]);
        for args in [
            &["config", "user.email", "t@example.com"][..],
            &["config", "user.name", "t"],
            &["config", "commit.gpgsign", "false"],
        ] {
            g(&other, args);
        }
        fs::write(other.join("g.c"), "a\nx = 3\nb\n").unwrap();
        commit_all(&other, "main moves");
        g(&other, &["push", "-q", "origin", "HEAD:main"]);
        for name in ["pre-push", "pre-merge-commit", "commit-msg", "post-merge"] {
            let hook = wt.join(".git/hooks").join(name);
            crate::testutil::write_exe(&hook, format!("#!/bin/sh\ntouch ran-{name}\nexit 1\n"));
        }
        let head = g(&wt, &["rev-parse", "HEAD"]);
        assert!(merge_default_with_git(&wt, "main", &head).unwrap());
        for name in ["pre-push", "pre-merge-commit", "commit-msg", "post-merge"] {
            assert!(!wt.join(format!("ran-{name}")).exists(), "{name} ran");
        }
    }

    #[test]
    fn marked_regions_sees_a_fence_that_landed_after_the_pr_branched() {
        let (_t, _origin, wt) = cloned();
        git::run(&wt, &["checkout", "-q", "-b", "pr"]).unwrap();
        fs::write(wt.join("f.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "pr edits f.c");
        // Spelled out at run time so this file holds no real markers.
        let (start, end) = ("ns:human-review start", "ns:human-review end");
        git::run(&wt, &["checkout", "-q", "-b", "fence", "main"]).unwrap();
        fs::write(
            wt.join("f.c"),
            format!("a\n// {start}\nx = 1\n// {end}\nb\n"),
        )
        .unwrap();
        commit_all(&wt, "fence f.c");
        // With no fetch refspec origin/main stays at the branch point; only FETCH_HEAD sees the fence.
        git::run(&wt, &["config", "--unset", "remote.origin.fetch"]).unwrap();
        git::run(&wt, &["push", "-q", "origin", "fence:main"]).unwrap();
        git::run(&wt, &["checkout", "-q", "pr"]).unwrap();
        assert_eq!(marked_regions(&wt, "main").unwrap(), ["f.c:1-5"]);
        fs::write(wt.join("f.c"), "a\nx = 1\nb\n").unwrap();
        fs::write(wt.join("g.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "pr edits only g.c");
        assert!(marked_regions(&wt, "main").unwrap().is_empty());
    }

    #[test]
    fn marked_regions_ignores_a_fence_the_pr_merged_in_from_the_default_branch() {
        let (_t, _origin, wt) = cloned();
        git::run(&wt, &["checkout", "-q", "-b", "pr"]).unwrap();
        fs::write(wt.join("g.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "pr edits g.c");
        let (start, end) = ("ns:human-review start", "ns:human-review end");
        git::run(&wt, &["checkout", "-q", "-b", "fence", "main"]).unwrap();
        fs::write(
            wt.join("f.c"),
            format!("a\n// {start}\nx = 1\n// {end}\nb\n"),
        )
        .unwrap();
        commit_all(&wt, "fence f.c");
        git::run(&wt, &["config", "--unset", "remote.origin.fetch"]).unwrap();
        git::run(&wt, &["push", "-q", "origin", "fence:main"]).unwrap();
        git::run(&wt, &["checkout", "-q", "pr"]).unwrap();
        git::run(&wt, &["merge", "-q", "--no-edit", "fence"]).unwrap();
        assert!(marked_regions(&wt, "main").unwrap().is_empty());
    }

    #[test]
    fn marked_regions_errors_when_the_fetch_fails() {
        let (_t, origin, wt) = cloned();
        assert!(marked_regions(&wt, "main").unwrap().is_empty());
        fs::remove_dir_all(origin).unwrap();
        let err = marked_regions(&wt, "main").unwrap_err();
        assert!(
            format!("{err:#}").starts_with("git fetch -q origin main failed"),
            "{err:#}"
        );
    }

    #[test]
    fn marked_regions_errors_when_head_shares_no_history_with_the_tip() {
        let (_t, _origin, wt) = cloned();
        git::run(&wt, &["checkout", "-q", "--orphan", "unrelated"]).unwrap();
        commit_all(&wt, "unrelated");
        assert!(marked_regions(&wt, "main").is_err());
    }

    #[test]
    fn a_merge_lock_wait_at_until_ends_the_run_as_budget() {
        let dir = tempfile::tempdir().unwrap();
        let _other = hold(dir.path(), "ns-merge");
        let events = std::cell::RefCell::new(Vec::new());
        let clock = Clock::pinned(1000);
        let f = take_merge_lock(dir.path(), "u", &clock, Some(1000), |ev| {
            events.borrow_mut().push(ev)
        })
        .unwrap()
        .err()
        .unwrap();
        assert_eq!(f.outcome, Outcome::Budget);
        assert_eq!(f.phase.as_deref(), Some("merge"));
        assert_eq!(
            f.reason,
            "gave up waiting for the merge lock (held by pid 42, unit other) at --until"
        );
        let events = events.into_inner();
        let by = json!({"pid": 42, "unit": "other"});
        assert_eq!(
            events,
            [
                json!({"event": "lock_wait", "phase": "merge", "lock": "merge", "held_by": by}),
                json!({"event": "lock_wait_timeout", "phase": "merge", "lock": "merge", "held_by": by, "bound": "until"}),
            ]
        );
    }

    #[test]
    fn a_merge_lock_before_until_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let clock = Clock::pinned(1000);
        let taken = take_merge_lock(dir.path(), "u", &clock, Some(1001), |ev| {
            events.borrow_mut().push(ev)
        })
        .unwrap();
        assert!(taken.is_ok());
        assert!(events.into_inner().is_empty());
    }
}
