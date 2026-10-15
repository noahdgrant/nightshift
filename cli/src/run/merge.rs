//! The merge step's shell: `core::merge` decides, and this module performs what it asks (the
//! `gh` and git calls, the gate, the merge lock and the sleeps) and reports what came back
//! (`merge.policy = "auto"`).

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use super::ci_gate::{gate_before_merge, gate_merge_result};
use super::core::merge::{
    self as core_merge, Ask, Check, MergeConfig, MergeEnd, MergeFact, MergeState,
};
use super::currency::{known_pr, learn_sha, State};
use super::locks::{Bound, MERGE_LOCK};
use super::{finish, gh, gh_json, holder_pid, remote_default, Ctx, Finish, Lock, Outcome, Shared};
use crate::clock::Clock;
use crate::factory::LocalGate;
use crate::gate;
use crate::git::{self, same_sha};
use crate::markers;
use crate::proc::Proc;

mod hold;
mod overlap;

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

pub(super) enum MergeStep {
    Finish(Finish),
    Rebuild(gate::Red),
    /// Back to ship, only to bring in the base, with this `{feedback}`.
    Reship(String),
    /// Back to build only to rebase, with its feedback and why.
    Rebase(String, String),
}

/// What the merge step's actions run with.
struct Io<'a, 'c> {
    ctx: &'a Ctx<'c>,
    state: &'a State,
    shared: &'a Shared,
    gate_state: &'a mut gate::Gate,
    run: u32,
    /// The merge lock, while the step holds it.
    lock: Option<Lock>,
}

/// `merge.policy = "auto"`: squash-merge this unit's PR when CI is green, review.md passed and is
/// current at the PR head, the CI gate passed at that head, and no file or marked region that
/// needs human review changed. The only place ns merges anything. `run` numbers this merge step
/// in the invocation, for its gate log. `core::merge` decides each step; this loop feeds it what
/// each action found, with the time.
pub(super) fn merge_step(
    ctx: &Ctx<'_>,
    state: &State,
    shared: &Shared,
    gate_state: &mut gate::Gate,
    run: u32,
) -> Result<MergeStep> {
    let cfg = MergeConfig {
        build_only: ctx.fac.gate.local == LocalGate::BuildOnly,
        local_queue: ctx.fac.merge.local_queue,
        gate: ctx.gate.commands.rest.is_some(),
        register_minutes: ctx.fac.merge.ci_register_timeout,
        ci_minutes: ctx.fac.merge.ci_timeout_minutes,
        fixes_main: hold::fixes_main(&ctx.unit),
        branch: format!("{}{}", crate::worktree::BRANCH_PREFIX, ctx.unit),
    };
    let mut m = MergeState::new(cfg);
    let mut io = Io {
        ctx,
        state,
        shared,
        gate_state,
        run,
        lock: None,
    };
    let mut fact = MergeFact::Start;
    loop {
        let act = core_merge::next(&mut m, fact, shared.clock.now());
        for ev in act.log {
            ctx.log(ev);
        }
        shared.clock.check_polls(act.poll)?;
        fact = match act.ask {
            Ask::End(end) => return Ok(ended(end)),
            ask => io.perform(&m, ask)?,
        };
    }
}

/// The merge step's end, as the driver takes it.
fn ended(end: MergeEnd) -> MergeStep {
    match end {
        MergeEnd::Finish {
            outcome,
            reason,
            needs_human,
        } => {
            let mut f = finish(outcome, reason, Some("merge"));
            f.needs_human = needs_human;
            MergeStep::Finish(f)
        }
        MergeEnd::Rebuild(r) => MergeStep::Rebuild(gate::Red {
            feedback: r.feedback,
            reason: r.reason,
            killed: r.killed,
            ratchets: r.ratchets,
        }),
        MergeEnd::Reship(feedback) => MergeStep::Reship(feedback),
        MergeEnd::Rebase { feedback, why } => MergeStep::Rebase(feedback, why),
    }
}

/// An error as the merge step's reasons quote it.
fn says(e: &anyhow::Error) -> String {
    format!("{e:#}")
}

impl Io<'_, '_> {
    /// Perform `ask` for the step at `m`, and report what came of it.
    fn perform(&mut self, m: &MergeState, ask: Ask) -> Result<MergeFact> {
        let (ctx, wt) = (self.ctx, &self.ctx.worktree);
        let ns = m.pr().to_string();
        let default = m.default_branch();
        Ok(match ask {
            Ask::ReadPr => MergeFact::Pr {
                ship: self.state.arts.contains_key("ship"),
                number: known_pr(&ctx.artifacts),
            },
            Ask::ViewPr => {
                let view = gh_json(wt, &["pr", "view", &ns, "--json", "state,headRefOid"])?;
                MergeFact::View {
                    state: view["state"].as_str().map(String::from),
                    head: view["headRefOid"].as_str().unwrap_or("").to_string(),
                }
            }
            Ask::Currency { pr_head } => {
                let head = git::run(wt, &["rev-parse", "HEAD"]).unwrap_or_default();
                let mut state = self.state.clone();
                learn_sha(&mut state, wt, &ctx.base, &pr_head);
                MergeFact::Currency {
                    head,
                    current: state.current(&pr_head),
                    review_passes: state.arts.get("review").is_some_and(|r| state.passes(r)),
                }
            }
            Ask::ReadDefault => MergeFact::Default(remote_default(wt).map(|d| d.0)),
            Ask::FetchTip => MergeFact::Tip(fetch_tip(wt, default).map_err(|e| says(&e))),
            Ask::MergeState => {
                let v = gh_json(wt, &["pr", "view", &ns, "--json", "mergeStateStatus"])?;
                MergeFact::MergeState(v["mergeStateStatus"].as_str().map(String::from))
            }
            Ask::IsAncestor { tip, head } => {
                MergeFact::Ancestor(git::ok(wt, &["merge-base", "--is-ancestor", &tip, &head]))
            }
            Ask::Overlap { head, tip } => MergeFact::Overlap(overlap::read(wt, &head, &tip)),
            Ask::UpdateBranch => MergeFact::Updated(
                gh(wt, &["pr", "update-branch", &ns])
                    .map(drop)
                    .map_err(|e| says(&e)),
            ),
            Ask::MergeWithGit { pr_head } => MergeFact::GitMerged(
                merge_default_with_git(wt, default, &pr_head).map_err(|e| says(&e)),
            ),
            Ask::ViewHead => {
                let v = gh_json(
                    wt,
                    &["pr", "view", &ns, "--json", "headRefOid,mergeStateStatus"],
                )?;
                MergeFact::HeadView {
                    head: v["headRefOid"].as_str().map(String::from),
                    merge_state: v["mergeStateStatus"].clone(),
                }
            }
            ask => return self.perform_ci(m, ask),
        })
    }

    /// The gate, CI and lock actions.
    fn perform_ci(&mut self, m: &MergeState, ask: Ask) -> Result<MergeFact> {
        let (ctx, wt, clock) = (self.ctx, &self.ctx.worktree, &self.shared.clock);
        let (n, ns) = (m.pr(), m.pr().to_string());
        Ok(match ask {
            Ask::Gate { queued, head } => {
                let cmd = ctx.gate.commands.rest.as_deref().unwrap_or_default();
                let (gs, run) = (&mut *self.gate_state, self.run);
                MergeFact::Gated(if queued {
                    gate_merge_result(ctx, gs, cmd, n, &head, m.default_branch(), run)?
                } else {
                    gate_before_merge(ctx, gs, cmd, n, &head, run)?
                })
            }
            Ask::CountChecks { sha } => MergeFact::CheckCounts(
                ["check-runs", "status"]
                    .map(|kind| check_count(wt, &sha, kind).map_err(|e| e.to_string())),
            ),
            Ask::WatchChecks { minutes } => {
                let ended = Proc::forge("gh")
                    .args(["pr", "checks", &ns, "--watch"])
                    .current_dir(wt)
                    .timeout(Duration::from_secs(minutes * 60))
                    .run()
                    .context("cannot run gh pr checks")?;
                MergeFact::Watched {
                    timed_out: ended.timed_out(),
                }
            }
            Ask::ReadChecks => MergeFact::Checks(read_checks(wt, &ns)),
            Ask::FailedLogs(ids) => MergeFact::FailedLogs(
                ids.iter()
                    .map(|id| gh(wt, &["run", "view", id, "--log-failed"]).ok())
                    .collect(),
            ),
            Ask::MainChecks => MergeFact::MainCi(hold::main_head(ctx)),
            Ask::TakeLock => {
                let (shared, log) = (self.shared, |ev| ctx.log(ev));
                match take_merge_lock(&ctx.common, &ctx.unit, clock, shared.until, log)? {
                    Ok(lock) => {
                        self.lock = Some(lock);
                        MergeFact::Locked(None)
                    }
                    Err(f) => MergeFact::Locked(Some(f.reason)),
                }
            }
            Ask::Unlock => {
                self.lock = None;
                MergeFact::Done
            }
            Ask::Sleep(t) => {
                clock.sleep_until(t)?;
                MergeFact::Done
            }
            ask => return self.perform_merge(m, ask),
        })
    }

    /// The actions under the lock: the last head check, the human-review checks, the merge.
    fn perform_merge(&mut self, m: &MergeState, ask: Ask) -> Result<MergeFact> {
        let (ctx, wt) = (self.ctx, &self.ctx.worktree);
        let ns = m.pr().to_string();
        Ok(match ask {
            Ask::ViewHeadRef => {
                // A view that fails or names no head leaves the check to --match-head-commit.
                let view =
                    gh_json(wt, &["pr", "view", &ns, "--json", "headRefOid"]).unwrap_or_default();
                MergeFact::HeadRef(view["headRefOid"].as_str().map(String::from))
            }
            Ask::ReviewHits => {
                let files: Vec<String> = gh(wt, &["pr", "diff", &ns, "--name-only"])?
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                MergeFact::ReviewHits(ctx.fac.merge.human_review_hits(&files))
            }
            Ask::Regions => {
                MergeFact::Regions(marked_regions(wt, m.default_branch()).map_err(|e| says(&e)))
            }
            Ask::Merge { head } => {
                // No --delete-branch: gh would check out the default branch in the unit's
                // worktree first.
                let merged = gh(
                    wt,
                    &["pr", "merge", &ns, "--squash", "--match-head-commit", &head],
                );
                let state = gh_json(wt, &["pr", "view", &ns, "--json", "state"])
                    .ok()
                    .and_then(|v| v["state"].as_str().map(String::from));
                MergeFact::Merged {
                    state,
                    error: merged.err().map(|e| says(&e)),
                    spent_usd: self.shared.spent(),
                }
            }
            Ask::DeleteBranch => {
                let branch = format!("{}{}", crate::worktree::BRANCH_PREFIX, ctx.unit);
                let path = format!("repos/{{owner}}/{{repo}}/git/refs/heads/{branch}");
                MergeFact::BranchDeleted(
                    gh(wt, &["api", &path, "-X", "DELETE"])
                        .map(drop)
                        .map_err(|e| says(&e)),
                )
            }
            ask => anyhow::bail!("ns run: the merge step asked for {ask:?} out of turn"),
        })
    }
}

/// The PR's checks once CI ended: none when they can't be read.
fn read_checks(wt: &Path, ns: &str) -> Vec<Check> {
    let checks = gh_json(
        wt,
        &["pr", "checks", ns, "--json", "name,state,bucket,link"],
    )
    .unwrap_or(Value::Array(Vec::new()));
    let field = |c: &Value, k: &str| c[k].as_str().map(String::from);
    checks
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| Check {
                    name: field(c, "name"),
                    state: field(c, "state"),
                    bucket: field(c, "bucket"),
                    link: field(c, "link"),
                })
                .collect()
        })
        .unwrap_or_default()
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
