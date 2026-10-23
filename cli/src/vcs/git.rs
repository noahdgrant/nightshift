use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::{Rebase, Vcs};
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

    fn workspace_files_changed(&self, path: &Path, base: &str) -> Result<Vec<String>> {
        let names = run(
            path,
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                &format!("{base}..HEAD"),
            ],
        )?;
        Ok(names
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(String::from)
            .collect())
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

    fn workspace_head_find(&self, path: &Path) -> Result<String> {
        run(path, &["rev-parse", "HEAD"])
    }

    fn base_fetch(&self, root: &Path) -> Result<Option<String>> {
        let branch = run(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        // A detached checkout is on no branch.
        if branch == "HEAD" {
            return Ok(None);
        }
        let heads = self.remote_run(root, &["ls-remote", "--heads", "origin", &branch])?;
        if heads.is_empty() {
            return Ok(None);
        }
        let tracking = format!("refs/remotes/origin/{branch}");
        self.remote_run(
            root,
            &[
                "fetch",
                "origin",
                &format!("+refs/heads/{branch}:{tracking}"),
            ],
        )?;
        Ok(Some(run(root, &["rev-parse", &tracking])?))
    }

    fn workspace_contains(&self, path: &Path, commit: &str) -> Result<bool> {
        Ok(succeeds(
            path,
            &["merge-base", "--is-ancestor", commit, "HEAD"],
        ))
    }

    fn workspace_rebase(&self, path: &Path, onto: &str) -> Result<Rebase> {
        let failed = match run(path, &["rebase", onto]) {
            Ok(_) => return Ok(Rebase::Done),
            Err(e) => e,
        };
        let conflicted = run(path, &["diff", "--name-only", "-z", "--diff-filter=U"])?;
        let files: Vec<String> = conflicted
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(String::from)
            .collect();
        run(path, &["rebase", "--abort"])?;
        if files.is_empty() {
            return Err(failed);
        }
        Ok(Rebase::Conflict(files))
    }

    fn workspace_base_set(&self, path: &Path, base: &str) -> Result<()> {
        let branch = run(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        run(path, &["update-ref", &base_ref(&branch), base])?;
        Ok(())
    }

    fn workspace_base_anchor(&self, root: &Path, path: &Path) -> Result<()> {
        let base_branch = run(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let tracking = format!("refs/remotes/origin/{base_branch}");
        // A detached checkout is on no branch; a missing ref means no remote was ever fetched.
        if base_branch == "HEAD"
            || !succeeds(path, &["rev-parse", "--verify", "--quiet", &tracking])
        {
            return Ok(());
        }
        let branch = run(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let recorded = run(path, &["rev-parse", &base_ref(&branch)])?;
        let Ok(anchor) = run(path, &["merge-base", "HEAD", &tracking]) else {
            return Ok(());
        };
        if anchor != recorded
            && succeeds(path, &["merge-base", "--is-ancestor", &recorded, &anchor])
        {
            run(path, &["update-ref", &base_ref(&branch), &anchor])?;
        }
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

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    }

    #[test]
    fn a_changed_file_with_a_non_ascii_name_is_listed_as_it_is_named() {
        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path();
        git(dir, &["init", "-q"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "base"]);
        let base = run(dir, &["rev-parse", "HEAD"]).unwrap();
        std::fs::create_dir(dir.join(".github")).unwrap();
        std::fs::write(dir.join(".github/\u{e9}.yml"), "x").unwrap();
        std::fs::write(dir.join("a.rs"), "x").unwrap();
        git(dir, &["add", "--all"]);
        git(dir, &["commit", "-q", "-m", "change"]);

        let mut files = Git::new().workspace_files_changed(dir, &base).unwrap();
        files.sort();

        assert_eq!(files, vec![".github/\u{e9}.yml", "a.rs"]);
    }

    fn commit_file(dir: &Path, name: &str, text: &str) -> String {
        std::fs::write(dir.join(name), text).unwrap();
        git(dir, &["add", "--all"]);
        git(dir, &["commit", "-q", "-m", name]);
        run(dir, &["rev-parse", "HEAD"]).unwrap()
    }

    /// A repository on `main` with a bare `origin`, and a workspace `ns/1` branched from the
    /// first commit, which holds `a.txt`.
    fn repo_with_workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let origin = tmp.path().join("origin.git");
        std::fs::create_dir(&root).unwrap();
        git(
            tmp.path(),
            &["init", "-q", "--bare", "-b", "main", "origin.git"],
        );
        git(&root, &["init", "-q", "-b", "main"]);
        git(
            &root,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git(&root, &["config", "user.name", "t"]);
        git(&root, &["config", "user.email", "t@t"]);
        commit_file(&root, "a.txt", "a\n");
        let ws = tmp.path().join("ws");
        Git::new()
            .workspace_create(&root, &ws, "ns/1", None)
            .unwrap();
        (tmp, root, ws)
    }

    #[test]
    fn the_base_branch_is_fetched_and_its_tip_returned() {
        let (_tmp, root, _ws) = repo_with_workspace();
        git(&root, &["push", "-q", "origin", "main"]);
        let tip = commit_file(&root, "b.txt", "b\n");
        git(&root, &["push", "-q", "origin", "main"]);
        // A clone made later sees the remote's tip, not its own.
        let other = root.parent().unwrap().join("other");
        git(
            root.parent().unwrap(),
            &[
                "clone",
                "-q",
                &root.parent().unwrap().join("origin.git").to_string_lossy(),
                "other",
            ],
        );
        let newer = commit_file(&other, "c.txt", "c\n");
        git(&other, &["push", "-q", "origin", "main"]);

        assert_eq!(Git::new().base_fetch(&root).unwrap(), Some(newer));
        assert_ne!(
            tip,
            run(&root, &["rev-parse", "refs/remotes/origin/main"]).unwrap()
        );
    }

    #[test]
    fn a_remote_without_the_base_branch_has_no_tip() {
        let (_tmp, root, _ws) = repo_with_workspace();
        assert_eq!(Git::new().base_fetch(&root).unwrap(), None);
    }

    #[test]
    fn a_checkout_on_no_branch_has_no_base_to_fetch() {
        let (_tmp, root, _ws) = repo_with_workspace();
        git(&root, &["checkout", "-q", "--detach"]);
        assert_eq!(Git::new().base_fetch(&root).unwrap(), None);
    }

    /// A workspace rebased onto a base that moved by one commit, its own commit on top, and the
    /// commit it was branched from.
    fn workspace_on_a_moved_base(root: &Path, ws: &Path) -> (String, String) {
        let first = run(ws, &["rev-parse", "HEAD"]).unwrap();
        git(root, &["push", "-q", "origin", "main"]);
        let moved = commit_file(root, "b.txt", "b\n");
        git(root, &["push", "-q", "origin", "main"]);
        git(ws, &["fetch", "-q", "origin", "main"]);
        commit_file(ws, "w.txt", "w\n");
        git(ws, &["rebase", "-q", "origin/main"]);
        (first, moved)
    }

    #[test]
    fn the_recorded_base_moves_to_where_a_rebased_workspace_now_sits() {
        let (_tmp, root, ws) = repo_with_workspace();
        let vcs = Git::new();
        let (first, moved) = workspace_on_a_moved_base(&root, &ws);
        assert_eq!(vcs.workspace_base_find(&ws).unwrap(), first);
        vcs.workspace_base_anchor(&root, &ws).unwrap();
        assert_eq!(vcs.workspace_base_find(&ws).unwrap(), moved);
        assert_eq!(vcs.workspace_commits_count(&ws, &moved).unwrap(), 1);
    }

    #[test]
    fn the_recorded_base_stays_when_the_main_checkout_is_on_no_branch() {
        let (_tmp, root, ws) = repo_with_workspace();
        let vcs = Git::new();
        let (first, _) = workspace_on_a_moved_base(&root, &ws);
        git(&root, &["checkout", "-q", "--detach"]);
        vcs.workspace_base_anchor(&root, &ws).unwrap();
        assert_eq!(vcs.workspace_base_find(&ws).unwrap(), first);
    }

    #[test]
    fn the_recorded_base_stays_when_the_remote_was_never_fetched() {
        let (_tmp, root, ws) = repo_with_workspace();
        let vcs = Git::new();
        let first = vcs.workspace_base_find(&ws).unwrap();
        commit_file(&ws, "w.txt", "w\n");
        vcs.workspace_base_anchor(&root, &ws).unwrap();
        assert_eq!(vcs.workspace_base_find(&ws).unwrap(), first);
    }

    #[test]
    fn a_workspace_contains_the_commits_it_is_built_on_and_not_newer_ones() {
        let (_tmp, root, ws) = repo_with_workspace();
        let first = run(&ws, &["rev-parse", "HEAD"]).unwrap();
        let newer = commit_file(&root, "b.txt", "b\n");
        let vcs = Git::new();
        assert!(vcs.workspace_contains(&ws, &first).unwrap());
        assert!(!vcs.workspace_contains(&ws, &newer).unwrap());
    }

    #[test]
    fn a_rebase_moves_the_branch_onto_the_commit_and_the_recorded_base_follows() {
        let (_tmp, root, ws) = repo_with_workspace();
        let vcs = Git::new();
        let old = vcs.workspace_base_find(&ws).unwrap();
        commit_file(&ws, "mine.txt", "mine\n");
        let newer = commit_file(&root, "b.txt", "b\n");

        assert_eq!(vcs.workspace_rebase(&ws, &newer).unwrap(), Rebase::Done);
        assert!(vcs.workspace_contains(&ws, &newer).unwrap());
        // Until the base is moved, the diff holds the other unit's file as well.
        assert_eq!(vcs.workspace_files_changed(&ws, &old).unwrap().len(), 2);
        vcs.workspace_base_set(&ws, &newer).unwrap();
        assert_eq!(vcs.workspace_base_find(&ws).unwrap(), newer);
        assert_eq!(
            vcs.workspace_files_changed(&ws, &newer).unwrap(),
            ["mine.txt"]
        );
        assert_eq!(
            vcs.workspace_head_find(&ws).unwrap(),
            run(&ws, &["rev-parse", "HEAD"]).unwrap()
        );
    }

    #[test]
    fn a_conflicting_rebase_is_aborted_and_names_the_files() {
        let (_tmp, root, ws) = repo_with_workspace();
        let vcs = Git::new();
        let mine = commit_file(&ws, "a.txt", "mine\n");
        let newer = commit_file(&root, "a.txt", "theirs\n");

        assert_eq!(
            vcs.workspace_rebase(&ws, &newer).unwrap(),
            Rebase::Conflict(vec!["a.txt".into()])
        );
        assert_eq!(vcs.workspace_head_find(&ws).unwrap(), mine);
        assert!(vcs.workspace_changes_list(&ws).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "mine\n");
    }

    #[test]
    fn a_rebase_that_fails_for_another_reason_is_an_error() {
        let (_tmp, _root, ws) = repo_with_workspace();
        let err = Git::new()
            .workspace_rebase(&ws, "0000000000000000000000000000000000000000")
            .unwrap_err();
        assert!(err.to_string().contains("rebase"), "{err}");
    }
}
