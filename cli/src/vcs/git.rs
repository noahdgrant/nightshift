use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::Vcs;
use crate::retry;

/// What git says when a call to the remote failed for a reason that can pass.
const TRANSIENT: [&str; 10] = [
    "could not resolve host",
    "temporary failure in name resolution",
    "network is unreachable",
    "connection",
    "timed out",
    "remote end hung up",
    "early eof",
    "ssh: connect to host",
    "returned error: 5",
    "returned error: 429",
];

pub struct Git {
    /// How long to wait before the second attempt of a call to the remote.
    retry_delay: Duration,
}

impl Git {
    pub fn new() -> Git {
        Git {
            retry_delay: retry::DELAY,
        }
    }

    /// Runs `git args` in `dir`, trying again when the remote cannot be reached. For calls that
    /// leave the same result when made twice.
    fn remote_run(&self, dir: &Path, args: &[&str]) -> Result<String> {
        retry::retry(
            self.retry_delay,
            || run(dir, args),
            |r| retry::error_says(r, &TRANSIENT),
        )
    }
}

impl Vcs for Git {
    fn root_find(&self, from: &Path) -> Result<PathBuf> {
        let list = run(from, &["worktree", "list", "--porcelain"])?;
        let first = list
            .lines()
            .find_map(|l| l.strip_prefix("worktree "))
            .context("git lists no worktree")?;
        Ok(PathBuf::from(first))
    }

    fn workspace_find(&self, from: &Path) -> Result<PathBuf> {
        Ok(PathBuf::from(run(from, &["rev-parse", "--show-toplevel"])?))
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
            let base = run(root, &["rev-parse", base.unwrap_or("HEAD")])?;
            run(root, &["worktree", "add", "-b", branch, &path, &base])?;
            // Where the branch started, for counting its commits later.
            run(root, &["update-ref", &base_ref(branch), &base])?;
        }
        Ok(())
    }

    fn state_dir_find(&self, root: &Path) -> Result<PathBuf> {
        let common = run(
            root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        Ok(Path::new(&common).join("ns"))
    }

    fn ignore_add(&self, root: &Path, pattern: &str) -> Result<()> {
        exclude(root, pattern)
    }

    fn workspace_base_find(&self, path: &Path) -> Result<String> {
        let branch = run(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        run(path, &["rev-parse", &base_ref(&branch)])
    }

    fn workspace_diff(&self, path: &Path, base: &str) -> Result<String> {
        run(path, &["diff", &format!("{base}..HEAD")])
    }

    fn workspace_commits_count(&self, path: &Path, base: &str) -> Result<usize> {
        let count = run(path, &["rev-list", "--count", &format!("{base}..HEAD")])?;
        count.parse().context("git printed a bad commit count")
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
        // The branch may have been rewritten since it was last pushed: `build` squashes, and
        // `--fresh` starts over. The lease keeps a push from overwriting work it has not seen.
        self.remote_run(
            path,
            &[
                "push",
                "--force-with-lease",
                "--set-upstream",
                "origin",
                branch,
            ],
        )?;
        Ok(())
    }

    fn branch_delete(&self, root: &Path, branch: &str) -> Result<()> {
        if succeeds(root, &["rev-parse", "--verify", "--quiet", branch]) {
            // The branch was merged on the forge, as a new commit, so git sees it as unmerged.
            run(root, &["branch", "--delete", "--force", branch])?;
        }
        run(root, &["update-ref", "-d", &base_ref(branch)])?;
        Ok(())
    }

    fn root_update(&self, root: &Path) -> Result<()> {
        self.remote_run(root, &["fetch", "origin"])?;
        if succeeds(root, &["rev-parse", "--abbrev-ref", "@{upstream}"]) {
            run(root, &["merge", "--ff-only", "@{upstream}"])?;
        }
        Ok(())
    }

    fn workspace_discard(&self, root: &Path, path: &Path) -> Result<()> {
        run(
            root,
            &["worktree", "remove", "--force", &path.to_string_lossy()],
        )?;
        Ok(())
    }

    fn workspace_remove(&self, root: &Path, path: &Path) -> Result<()> {
        run(root, &["worktree", "remove", &path.to_string_lossy()])?;
        Ok(())
    }
}

/// The ref that remembers the commit `branch` started from.
fn base_ref(branch: &str) -> String {
    format!("refs/ns/base/{branch}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_remote_that_cannot_be_reached_is_worth_another_attempt() {
        for (said, repeated) in [
            (
                "fatal: unable to access 'https://x/': Could not resolve host: x",
                true,
            ),
            ("ssh: connect to host x port 22: Connection timed out", true),
            ("fatal: the remote end hung up unexpectedly", true),
            (
                "error: RPC failed; HTTP 503 curl 22 The requested URL returned error: 503",
                true,
            ),
            ("! [rejected] x -> x (stale info)", false),
            ("fatal: Authentication failed for 'https://x/'", false),
            ("ERROR: Permission to x denied", false),
        ] {
            assert_eq!(crate::retry::says(said, &TRANSIENT), repeated, "{said}");
        }
    }
}
