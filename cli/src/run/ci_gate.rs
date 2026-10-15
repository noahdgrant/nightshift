//! The CI gate: after a phase, before the merge, and on the merge result (the local merge queue).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::json;

use super::core::merge::{Gated, Red};
use super::currency::set_fields;
use super::merge::fetch_tip;
use super::Ctx;
use crate::factory::artifact_of;
use crate::frontmatter;
use crate::gate;
use crate::git::{self, same_sha};
use crate::rules::BASELINE;

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
        let head = self.head();
        let job = self.gate_job(
            gate_state,
            cmd,
            &self.worktree,
            head.clone(),
            phase,
            attempt,
        );
        let red = gate_state.after(trigger, &job, &|ev| self.log(ev));
        let after_build = matches!(
            trigger,
            gate::Trigger::BuildPassed | gate::Trigger::BuildWroteNothing
        );
        if pruned(&self.worktree) {
            if after_build && matches!(red, Ok(None)) {
                self.commit_pruned(gate_state, phase, &head)?;
            } else {
                if let Err(e) = git::run(&self.worktree, &["checkout", "-q", "--", BASELINE]) {
                    self.log(
                        json!({"event": "baseline_restore_failed", "error": format!("{e:#}")}),
                    );
                }
            }
        }
        red
    }

    /// Commit the rules baseline the green gate after `phase` at `head` pruned, so the PR carries
    /// it, and move `phase`'s artifact from `head` to the commit. The commit only drops entries
    /// the gate's own `ns rules check` matched to nothing, so the gate's verdict and the artifact
    /// hold at it. It skips the hooks, as the full gate just checked the rest of the tree.
    fn commit_pruned(&self, gate_state: &mut gate::Gate, phase: &str, head: &str) -> Result<()> {
        let wt = &self.worktree;
        git::run(
            wt,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "-m",
                PRUNE_SUBJECT,
                "--",
                BASELINE,
            ],
        )?;
        let to = self.head();
        gate_state.carry_green(head, &to);
        move_artifact(&self.artifacts.join(artifact_of(phase)), head, &to)?;
        self.log(json!({"event": "baseline_pruned", "phase": phase, "from": head, "to": to}));
        Ok(())
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
                // A rename lists both of its paths, so a fast gate checks the place a file left too.
                git::run(worktree, &["diff", "--name-only", "--no-renames", &since]).ok()
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

/// The subject of the commit that drops fixed entries from the rules baseline.
const PRUNE_SUBJECT: &str = "chore(rules): drop fixed entries from the rules baseline";

/// Move the artifact at `path` from `from` to `to` when it was written at `from`; leave a stale
/// one, or none, as it is.
fn move_artifact(path: &Path, from: &str, to: &str) -> Result<()> {
    let written_at = fs::read_to_string(path)
        .ok()
        .and_then(|t| frontmatter::field(&t, "sha"));
    if written_at.is_some_and(|sha| same_sha(&sha, from)) {
        set_fields(path, &[("sha", to.to_string())])?;
    }
    Ok(())
}

/// Whether the rules baseline in `wt` differs from HEAD's only by entries `ns rules check
/// --prune` dropped, messages it refreshed, or measured values it lowered (all of which
/// `rules::removal_only` lets through), as opposed to an edit a phase left behind.
fn pruned(wt: &Path) -> bool {
    let dirty = git::run(wt, &["status", "--porcelain", "--", BASELINE]);
    if dirty.map_or(true, |d| d.is_empty()) {
        return false;
    }
    let was = git::run(wt, &["show", &format!("HEAD:{BASELINE}")]);
    let now = fs::read_to_string(wt.join(BASELINE));
    match (was, now) {
        (Ok(was), Ok(now)) => crate::rules::removal_only(&was, &now) == Ok(true),
        _ => false,
    }
}

/// Check out the PR head `merge_head` and run the gate `cmd` there, unless it already went green
/// at that head, and report how it came out.
pub(super) fn gate_before_merge(
    ctx: &Ctx<'_>,
    gate_state: &mut gate::Gate,
    cmd: &str,
    n: u64,
    merge_head: &str,
    run: u32,
) -> Result<Gated> {
    if let Err(e) = check_out(&ctx.worktree, merge_head) {
        return Ok(Gated::Human(format!(
            "cannot check out PR #{n} head {merge_head} for the CI gate: {e:#}; needs a human merge"
        )));
    }
    let red = ctx.gate(gate_state, gate::Trigger::BeforeMerge, cmd, "merge", run);
    Ok(gated(merge_gate_stopped(ctx, red, run)?))
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
/// on that pair, and report how it came out.
pub(super) fn gate_merge_result(
    ctx: &Ctx<'_>,
    gate_state: &mut gate::Gate,
    cmd: &str,
    n: u64,
    pr_head: &str,
    default: &str,
    run: u32,
) -> Result<Gated> {
    let wt = &ctx.worktree;
    let base = match fetch_tip(wt, default) {
        Ok(b) => b,
        Err(e) => {
            return Ok(Gated::Human(format!(
                "cannot fetch {default} for the local merge queue: {e:#}; needs a human merge"
            )))
        }
    };
    if let Err(e) = check_out(wt, pr_head) {
        return Ok(Gated::Human(format!(
            "cannot check out PR #{n} head {pr_head} for the local merge queue: {e:#}; needs a human merge"
        )));
    }
    // Merging a base the PR head already holds leaves the PR head: gate it as the head.
    if git::ok(wt, &["merge-base", "--is-ancestor", &base, pr_head]) {
        let step = gate_before_merge(ctx, gate_state, cmd, n, pr_head, run)?;
        if matches!(step, Gated::Red(_)) {
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
                    return Ok(Gated::Human(format!(
                    "cannot add a worktree for the local merge queue: {e:#}; needs a human merge"
                )))
                }
            };
            match merge_into(&scratch.path, &base) {
                Ok(true) => {}
                Ok(false) => return Ok(Gated::Conflict),
                Err(e) => {
                    return Ok(Gated::Human(format!(
                        "cannot merge PR #{n} into {default} for the local merge queue: {e:#}; needs a human merge"
                    )))
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
    Ok(gated(red))
}

/// The merge step's gate verdict, as `core::merge` takes it.
fn gated(red: Option<gate::Red>) -> Gated {
    match red {
        None => Gated::Green,
        Some(r) => Gated::Red(Red {
            feedback: r.feedback,
            reason: r.reason,
            killed: r.killed,
            ratchets: r.ratchets,
        }),
    }
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

    fn baseline(snippets: &[&str]) -> String {
        let v: Vec<serde_json::Value> = snippets
            .iter()
            .map(|s| json!({"tool": "t", "rule": "r", "file": "f.c", "message": "m", "snippet": s}))
            .collect();
        json!({"version": 1, "violations": v}).to_string()
    }

    #[test]
    fn move_artifact_moves_only_one_written_at_the_old_head() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("build.md");
        let text = |sha: &str| format!("---\nphase: build\nsha: {sha}\nstatus: pass\n---\nbody\n");
        fs::write(&p, text("abc1234")).unwrap();
        move_artifact(&p, "abc1234", "def5678").unwrap();
        let moved = fs::read_to_string(&p).unwrap();
        assert!(
            moved.contains("\nsha: def5678\n") && moved.ends_with("---\nbody\n"),
            "{moved}"
        );
        fs::write(&p, text("0001111")).unwrap();
        move_artifact(&p, "abc1234", "def5678").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), text("0001111"));
        move_artifact(&dir.path().join("none.md"), "abc1234", "def5678").unwrap();
        assert!(!dir.path().join("none.md").exists());
    }

    /// Only a baseline that lost entries since HEAD reads as pruned: never a clean one, one a
    /// phase grew or broke, or one HEAD doesn't track.
    #[test]
    fn pruned_is_a_tracked_baseline_that_only_lost_entries() {
        let (_t, _origin, wt) = cloned();
        assert!(!pruned(&wt));
        let path = wt.join(BASELINE);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, baseline(&["x"])).unwrap();
        assert!(!pruned(&wt), "untracked");
        commit_all(&wt, "baseline");
        assert!(!pruned(&wt), "clean");
        for (text, want) in [
            (baseline(&[]), true),
            (baseline(&["x", "y"]), false),
            (baseline(&["y"]), false),
            ("{".to_string(), false),
        ] {
            fs::write(&path, &text).unwrap();
            assert_eq!(pruned(&wt), want, "{text}");
        }
        fs::remove_file(&path).unwrap();
        assert!(!pruned(&wt), "deleted");
    }

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

    /// A fast gate gets a rename as both of its paths (#667): a file moved from `cli/` to
    /// `contrib/` still makes the gate check `cli/`.
    #[test]
    fn a_fast_gate_lists_both_paths_of_a_rename() {
        let (t, _origin, wt) = cloned();
        fs::create_dir_all(wt.join("cli")).unwrap();
        fs::write(wt.join("cli/x.rs"), "fn x() {}\n").unwrap();
        commit_all(&wt, "add cli/x.rs");
        git::run(&wt, &["switch", "-q", "-c", "ns/9-x"]).unwrap();
        fs::create_dir_all(wt.join("contrib")).unwrap();
        git::run(&wt, &["mv", "cli/x.rs", "contrib/x.rs"]).unwrap();
        commit_all(&wt, "move x.rs");
        let fac = crate::factory::Factory::default();
        let mut ctx = super::super::post_phase::tests::ctx_in(&fac, &wt, t.path());
        ctx.gate.commands = gate::Commands {
            build: Some("full".into()),
            rest: Some("fast".into()),
        };
        let gs = gate::Gate::default();
        let job = ctx.gate_job(&gs, "fast", &wt, String::new(), "verify", 1);
        let files = job.files.expect("a fast gate gets its files");
        let files: Vec<&str> = files.lines().collect();
        assert_eq!(files, ["cli/x.rs", "contrib/x.rs"]);
    }

    fn gate_ctx<'a>(fac: &'a crate::factory::Factory, wt: &Path, common: &Path) -> Ctx<'a> {
        let mut ctx = super::super::post_phase::tests::ctx_in(fac, wt, common);
        ctx.gate.commands = gate::Commands {
            build: Some("full".into()),
            rest: None,
        };
        ctx
    }

    fn baselined_clone() -> (tempfile::TempDir, PathBuf) {
        let (t, _origin, wt) = cloned();
        let path = wt.join(BASELINE);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, baseline(&["x", "y"])).unwrap();
        commit_all(&wt, "baseline");
        (t, wt)
    }

    fn prune_cmd(then: &str) -> String {
        let pruned = baseline(&["y"]).replace('\'', "");
        format!("printf '%s' '{pruned}' > {BASELINE}; {then}")
    }

    #[test]
    fn a_gate_that_errors_restores_the_baseline_it_left_pruned() {
        let (t, wt) = baselined_clone();
        fs::write(wt.join(BASELINE), baseline(&["y"])).unwrap();
        fs::write(t.path().join("ns"), "a file where the log dir goes").unwrap();
        let fac = crate::factory::Factory::default();
        let ctx = gate_ctx(&fac, &wt, t.path());
        let mut gs = gate::Gate::default();
        let r = ctx.gate(&mut gs, gate::Trigger::BuildPassed, "full", "build", 1);
        assert!(r.is_err());
        assert_eq!(git::run(&wt, &["status", "--porcelain"]).unwrap(), "");
        let kept = git::run(&wt, &["show", &format!("HEAD:{BASELINE}")]).unwrap();
        assert_eq!(fs::read_to_string(wt.join(BASELINE)).unwrap(), kept);
    }

    #[test]
    fn a_red_gate_stays_red_when_the_baseline_cannot_be_restored() {
        let (t, wt) = baselined_clone();
        let fac = crate::factory::Factory::default();
        let mut ctx = gate_ctx(&fac, &wt, t.path());
        let mut gs = gate::Gate::default();
        let cmd = prune_cmd("touch .git/index.lock; exit 1");
        ctx.gate.commands.build = Some(cmd.clone());
        let red = ctx
            .gate(&mut gs, gate::Trigger::BuildPassed, &cmd, "build", 1)
            .unwrap();
        assert!(red.is_some());
    }

    #[test]
    fn a_silent_build_after_a_red_gate_commits_the_baseline_the_rerun_pruned() {
        let (t, wt) = baselined_clone();
        let fac = crate::factory::Factory::default();
        let mut ctx = gate_ctx(&fac, &wt, t.path());
        let mut gs = gate::Gate::default();
        ctx.gate.commands.build = Some("exit 1".into());
        let red = ctx
            .gate(&mut gs, gate::Trigger::BuildPassed, "exit 1", "build", 1)
            .unwrap();
        assert!(red.is_some());
        fs::write(wt.join("f.c"), "changed\n").unwrap();
        git::run(&wt, &["add", "f.c"]).unwrap();
        git::run(&wt, &["commit", "-q", "-m", "fix"]).unwrap();
        let cmd = prune_cmd("exit 0");
        ctx.gate.commands.build = Some(cmd.clone());
        let red = ctx
            .gate(&mut gs, gate::Trigger::BuildWroteNothing, &cmd, "build", 2)
            .unwrap();
        assert!(red.is_none());
        assert_eq!(
            git::run(&wt, &["log", "-1", "--format=%s"]).unwrap(),
            PRUNE_SUBJECT
        );
        assert_eq!(git::run(&wt, &["status", "--porcelain"]).unwrap(), "");
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
