//! The CI gate: after a phase, before the merge, and on the merge result (the local merge queue).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::json;

use super::merge::{conflicts, fetch_tip, human, MergeStep};
use super::Ctx;
use crate::gate;
use crate::git::{self, same_sha};

impl Ctx<'_> {
    /// Run the gate `cmd` at HEAD after `phase` when `trigger` calls for it. A stop during the
    /// run is an error with the stop set (`crate::stop::requested`).
    pub(super) fn gate(
        &self,
        gate_state: &mut gate::Gate,
        trigger: gate::Trigger,
        cmd: &str,
        phase: &str,
        attempt: u32,
    ) -> Result<Option<gate::Red>> {
        let job = self.gate_job(gate_state, cmd, &self.worktree, self.head(), phase, attempt);
        gate_state.after(trigger, &job, &|ev| self.log(ev))
    }

    /// The gate `cmd` run in `worktree`, which holds `head`. A fast gate gets the paths that
    /// differ in `worktree` from where the full gate last went green, else from the base.
    pub(super) fn gate_job<'j>(
        &'j self,
        gate_state: &gate::Gate,
        cmd: &'j str,
        worktree: &'j Path,
        head: String,
        phase: &'j str,
        attempt: u32,
    ) -> gate::Job<'j> {
        let full = self.gate.commands.full(cmd);
        let files = (!full)
            .then(|| {
                let since = gate_state
                    .full_green()
                    .map(String::from)
                    .or_else(|| git::run(worktree, &["merge-base", &self.base, "HEAD"]).ok())?;
                git::run(worktree, &["diff", "--name-only", &since]).ok()
            })
            .flatten();
        gate::Job {
            cmd,
            full,
            files,
            worktree,
            log_dir: self.common.join("ns").join("transcripts").join(&self.unit),
            unit: &self.unit,
            timeout: gate::timeout(self.fac.phase("build").timeout_minutes),
            head,
            phase,
            attempt,
            cap: self.env.cap,
            ratchets: gate::Ratchets {
                factory: &self.root,
                night: self.gate.night.as_deref(),
            },
        }
    }
}

/// Check out the PR head `merge_head` and run the gate `cmd` there, unless it already went green
/// at that head: `None` to go on to the merge, else how the merge step ends.
pub(super) fn gate_before_merge(
    ctx: &Ctx<'_>,
    gate_state: &mut gate::Gate,
    cmd: &str,
    n: u64,
    merge_head: &str,
    run: u32,
) -> Result<Option<MergeStep>> {
    if let Err(e) = check_out(&ctx.worktree, merge_head) {
        return Ok(Some(human(format!(
            "cannot check out PR #{n} head {merge_head} for the CI gate: {e:#}; needs a human merge"
        ))));
    }
    let red = ctx.gate(gate_state, gate::Trigger::BeforeMerge, cmd, "merge", run);
    Ok(merge_gate_stopped(ctx, red, run)?.map(MergeStep::Rebuild))
}

/// `red`, or the stop that interrupted the merge step's gate, logged.
fn merge_gate_stopped(
    ctx: &Ctx<'_>,
    red: Result<Option<gate::Red>>,
    run: u32,
) -> Result<Option<gate::Red>> {
    if let Some(sig) = red.as_ref().err().and(crate::stop::requested()) {
        ctx.log(json!({
            "event": "gate",
            "phase": "merge",
            "attempt": run,
            "interrupted": crate::stop::name(sig),
        }));
        return Err(crate::stop::error(sig));
    }
    red
}

/// The local merge queue: merge the PR head `pr_head` into the fresh tip of `origin/<default>`
/// in a scratch worktree, no push, and run the gate `cmd` on the result, unless it already ran
/// on that pair. `None` to go on to the merge, else how the merge step ends.
pub(super) fn gate_merge_result(
    ctx: &Ctx<'_>,
    gate_state: &mut gate::Gate,
    cmd: &str,
    n: u64,
    pr_head: &str,
    default: &str,
    run: u32,
) -> Result<Option<MergeStep>> {
    let wt = &ctx.worktree;
    let base = match fetch_tip(wt, default) {
        Ok(b) => b,
        Err(e) => {
            return Ok(Some(human(format!(
                "cannot fetch {default} for the local merge queue: {e:#}; needs a human merge"
            ))))
        }
    };
    if let Err(e) = check_out(wt, pr_head) {
        return Ok(Some(human(format!(
            "cannot check out PR #{n} head {pr_head} for the local merge queue: {e:#}; needs a human merge"
        ))));
    }
    // Merging a base the PR head already holds leaves the PR head: gate it as the head.
    if git::ok(wt, &["merge-base", "--is-ancestor", &base, pr_head]) {
        let step = gate_before_merge(ctx, gate_state, cmd, n, pr_head, run)?;
        if matches!(step, Some(MergeStep::Rebuild(_))) {
            log_merge_gate_failed(ctx, n, pr_head, &base, false);
        }
        return Ok(step);
    }
    let reused = gate_state.merge_verdict(pr_head, &base);
    let was_reused = reused.is_some();
    let red = match reused {
        Some(gate::Reused::Green) => None,
        Some(gate::Reused::Red(red)) => Some(red),
        None => {
            let scratch = match Scratch::add(wt, &ctx.common, &ctx.unit, pr_head) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(Some(human(format!(
                    "cannot add a worktree for the local merge queue: {e:#}; needs a human merge"
                ))))
                }
            };
            match merge_into(&scratch.path, &base) {
                Ok(true) => {}
                Ok(false) => return Ok(Some(conflicts(ctx, n, default))),
                Err(e) => {
                    return Ok(Some(human(format!(
                        "cannot merge PR #{n} into {default} for the local merge queue: {e:#}; needs a human merge"
                    ))))
                }
            }
            let job = ctx.gate_job(
                gate_state,
                cmd,
                &scratch.path,
                pr_head.to_string(),
                "merge",
                run,
            );
            let red = gate_state.on_merge_result(&job, default, &base, &|ev| ctx.log(ev));
            merge_gate_stopped(ctx, red, run)?
        }
    };
    if red.is_some() {
        log_merge_gate_failed(ctx, n, pr_head, &base, was_reused);
    }
    Ok(red.map(MergeStep::Rebuild))
}

fn log_merge_gate_failed(ctx: &Ctx<'_>, n: u64, pr_head: &str, base: &str, reused: bool) {
    ctx.log(json!({
        "event": "merge_gate_failed",
        "pr": n,
        "sha": pr_head,
        "base": base,
        "reused": reused,
    }));
}

/// A detached worktree under `<common>/ns/merge-queue/<unit>` for the local merge queue,
/// removed when dropped, so a failure or a stop leaves none behind.
struct Scratch {
    repo: PathBuf,
    path: PathBuf,
}

impl Scratch {
    /// Add it at `sha`, first removing one a killed run left at that path. No hook runs.
    fn add(repo: &Path, common: &Path, unit: &str, sha: &str) -> Result<Scratch> {
        let dir = common.join("ns").join("merge-queue");
        let s = Scratch {
            repo: repo.to_path_buf(),
            path: dir.join(unit),
        };
        s.remove();
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = s.path.to_string_lossy();
        git::run(
            repo,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "worktree",
                "add",
                "-q",
                "--detach",
                &path,
                sha,
            ],
        )?;
        Ok(s)
    }

    fn remove(&self) {
        let _ = fs::remove_dir_all(&self.path);
        let _ = git::run(&self.repo, &["worktree", "prune"]);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Merge `base` into the detached HEAD at `dir` without committing, so no merge hook runs:
/// `Ok(false)` is a conflict. The identity is a placeholder git asks for anyway.
fn merge_into(dir: &Path, base: &str) -> Result<bool> {
    let out = git::command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=ns",
            "-c",
            "user.email=ns@localhost",
            "merge",
            "-q",
            "--no-ff",
            "--no-commit",
            base,
        ])
        .output()
        .context("failed to run git")?;
    if out.status.success() {
        return Ok(true);
    }
    if git::run(dir, &["diff", "--name-only", "--diff-filter=U"]).is_ok_and(|p| !p.is_empty()) {
        return Ok(false);
    }
    Err(anyhow!(
        "git merge {base} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// Point the worktree's branch at exactly `sha`, fetched from origin when it is not here: the PR
/// head the merge goes ahead with, which after a rebase need not descend from HEAD.
fn check_out(wt: &Path, sha: &str) -> Result<()> {
    if git::run(wt, &["rev-parse", "HEAD"]).is_ok_and(|h| same_sha(&h, sha)) {
        return Ok(());
    }
    let here = git::run(wt, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok();
    if !here {
        let branch = git::run(wt, &["symbolic-ref", "--short", "HEAD"])?;
        let out = git::net(wt, &["fetch", "-q", "origin", &branch])
            .output()
            .context("failed to run git")?;
        if !out.status.success() {
            return Err(anyhow!(
                "git fetch origin {branch} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    git::run(wt, &["reset", "-q", "--hard", &format!("{sha}^{{commit}}")])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{cloned, commit_all};
    use super::*;

    /// The local merge queue's scratch worktree is `ns` plumbing that commits nothing: its
    /// checkout and its merge run no hook, and the merge needs no identity of the user's. One a killed run left behind is replaced, and a drop
    /// removes it from disk and from `git worktree list`.
    #[test]
    fn the_merge_queue_scratch_worktree_runs_no_hook_and_is_removed_on_drop() {
        let (t, _origin, wt) = cloned();
        let g = |dir: &Path, args: &[&str]| git::run(dir, args).unwrap();
        g(&wt, &["switch", "-q", "-c", "ns/9-x"]);
        fs::write(wt.join("f.c"), "a\nx = 2\nb\n").unwrap();
        commit_all(&wt, "unit");
        let head = g(&wt, &["rev-parse", "HEAD"]);
        g(&wt, &["switch", "-q", "main"]);
        fs::write(wt.join("g.c"), "a\nx = 3\nb\n").unwrap();
        commit_all(&wt, "main moves");
        let base = g(&wt, &["rev-parse", "HEAD"]);
        let hooks = [
            "post-checkout",
            "pre-merge-commit",
            "commit-msg",
            "post-merge",
        ];
        for name in hooks {
            let hook = wt.join(".git/hooks").join(name);
            let ran = t.path().join(format!("ran-{name}"));
            crate::testutil::write_exe(
                &hook,
                format!("#!/bin/sh\ntouch '{}'\nexit 1\n", ran.display()),
            );
        }
        let common = t.path().join("common");
        std::mem::forget(Scratch::add(&wt, &common, "9-x", &head).unwrap());
        let s = Scratch::add(&wt, &common, "9-x", &head).unwrap();
        g(&wt, &["config", "user.name", ""]);
        g(&wt, &["config", "user.email", ""]);
        assert!(merge_into(&s.path, &base).unwrap());
        assert_eq!(g(&s.path, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            fs::read_to_string(s.path.join("f.c")).unwrap(),
            "a\nx = 2\nb\n"
        );
        assert_eq!(
            fs::read_to_string(s.path.join("g.c")).unwrap(),
            "a\nx = 3\nb\n"
        );
        let path = s.path.clone();
        drop(s);
        assert!(!path.exists());
        let list = g(&wt, &["worktree", "list", "--porcelain"]);
        assert!(!list.contains("9-x"), "{list}");
        for name in hooks {
            assert!(!t.path().join(format!("ran-{name}")).exists(), "{name} ran");
        }
    }

    /// A killed run can leave the scratch path as a plain directory git doesn't know, or as a
    /// registered worktree whose directory is gone: either way the next one is added there.
    #[test]
    fn a_scratch_worktree_replaces_whatever_a_killed_run_left_at_its_path() {
        let (t, _origin, wt) = cloned();
        let head = git::run(&wt, &["rev-parse", "HEAD"]).unwrap();
        let common = t.path().join("common");
        let path = common.join("ns/merge-queue/9-x");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("junk"), "left behind").unwrap();
        drop(Scratch::add(&wt, &common, "9-x", &head).unwrap());
        let s = Scratch::add(&wt, &common, "9-x", &head).unwrap();
        std::mem::forget(s);
        fs::remove_dir_all(&path).unwrap();
        let s = Scratch::add(&wt, &common, "9-x", &head).unwrap();
        assert!(s.path.join("f.c").is_file());
    }
}
