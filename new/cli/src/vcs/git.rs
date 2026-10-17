use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use super::Vcs;

pub struct Git;

impl Vcs for Git {
    fn root_find(&self, from: &Path) -> Result<PathBuf> {
        let list = run(from, &["worktree", "list", "--porcelain"])?;
        let first = list
            .lines()
            .find_map(|l| l.strip_prefix("worktree "))
            .context("git lists no worktree")?;
        Ok(PathBuf::from(first))
    }

    fn workspace_create(
        &self,
        root: &Path,
        path: &Path,
        branch: &str,
        base: Option<&str>,
    ) -> Result<()> {
        let path = path.to_string_lossy();
        exclude(root, ".ns/")?;
        if succeeds(root, &["rev-parse", "--verify", "--quiet", branch]) {
            run(root, &["worktree", "add", &path, branch])?;
        } else {
            run(
                root,
                &[
                    "worktree",
                    "add",
                    "-b",
                    branch,
                    &path,
                    base.unwrap_or("HEAD"),
                ],
            )?;
        }
        Ok(())
    }

    fn workspace_snapshot(&self, path: &Path) -> Result<String> {
        // A scratch index, so the workspace's own index is left alone.
        let git_dir = run(path, &["rev-parse", "--absolute-git-dir"])?;
        let index = Path::new(&git_dir).join("ns-snapshot-index");
        let _ = std::fs::remove_file(&index);
        let tree = (|| {
            run_with(path, &["read-tree", "HEAD"], &index)?;
            run_with(path, &["add", "--all"], &index)?;
            run_with(path, &["write-tree"], &index)
        })();
        let _ = std::fs::remove_file(&index);
        tree
    }

    fn workspace_changes_list(&self, path: &Path) -> Result<Vec<String>> {
        let status = run(path, &["status", "--porcelain"])?;
        Ok(status.lines().map(|l| l.trim().to_string()).collect())
    }

    fn branch_push(&self, path: &Path, branch: &str) -> Result<()> {
        run(path, &["push", "--set-upstream", "origin", branch])?;
        Ok(())
    }

    fn workspace_remove(&self, root: &Path, path: &Path) -> Result<()> {
        run(root, &["worktree", "remove", &path.to_string_lossy()])?;
        Ok(())
    }
}

/// Adds `pattern` to the repository's `info/exclude`, shared by all its worktrees, unless it is
/// there. Files `ns` leaves in a workspace then don't count as changes.
fn exclude(root: &Path, pattern: &str) -> Result<()> {
    let common = run(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let file = Path::new(&common).join("info").join("exclude");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    if text.lines().any(|l| l == pattern) {
        return Ok(());
    }
    std::fs::create_dir_all(file.parent().context("no info directory")?)?;
    let sep = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::write(&file, format!("{text}{sep}{pattern}\n"))
        .with_context(|| format!("cannot write {}", file.display()))
}

/// Runs `git args` in `dir` and returns its trimmed stdout.
fn run(dir: &Path, args: &[&str]) -> Result<String> {
    run_with_env(dir, args, None)
}

/// Like [`run`], with git's index file set to `index`.
fn run_with(dir: &Path, args: &[&str], index: &Path) -> Result<String> {
    run_with_env(dir, args, Some(index))
}

fn run_with_env(dir: &Path, args: &[&str], index: Option<&Path>) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd.output().context("cannot run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether `git args` in `dir` exits 0.
fn succeeds(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .is_ok_and(|o| o.status.success())
}
