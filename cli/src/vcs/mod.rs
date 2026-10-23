//! Version control behind one trait. Everything specific to a tool lives in its own file here.

mod git;

use std::path::{Path, PathBuf};

use anyhow::Result;

pub trait Vcs {
    /// The root of the main checkout of the repository `from` is in, even when `from` is
    /// another workspace of it.
    fn root_find(&self, from: &Path) -> Result<PathBuf>;

    /// The top of the checkout `from` is in, which is a workspace's own directory inside one.
    fn workspace_find(&self, from: &Path) -> Result<PathBuf>;

    /// Adds a workspace at `path` on `branch` for the repository at `root`. A branch that
    /// exists is checked out; otherwise it is created from `base`, or from the current
    /// commit when `base` is `None`.
    fn workspace_create(
        &self,
        root: &Path,
        path: &Path,
        branch: &str,
        base: Option<&str>,
    ) -> Result<()>;

    /// A fingerprint of everything in the workspace at `path` that a commit would hold: tracked
    /// files, untracked files that are not ignored, and their contents. Two snapshots are equal
    /// exactly when nothing changed between them.
    fn workspace_snapshot(&self, path: &Path) -> Result<String>;

    /// A directory shared by all of the repository's workspaces where `ns` keeps what it
    /// remembers.
    fn state_dir_find(&self, root: &Path) -> Result<PathBuf>;

    /// Keeps files matching `pattern` out of the repository's changes, in every workspace of it,
    /// without a committed ignore file. `pattern` is anchored at the workspace's top.
    fn ignore_add(&self, root: &Path, pattern: &str) -> Result<()>;

    /// The commit the workspace's branch started from.
    fn workspace_base_find(&self, path: &Path) -> Result<String>;

    /// What the workspace's branch changed since `base`, as a patch.
    fn workspace_diff(&self, path: &Path, base: &str) -> Result<String>;

    /// The files the workspace's branch changed since `base`, relative to the top of the
    /// repository. A file that moved is listed under both names.
    fn workspace_files_changed(&self, path: &Path, base: &str) -> Result<Vec<String>>;

    /// How many commits the workspace's branch holds past `base`.
    fn workspace_commits_count(&self, path: &Path, base: &str) -> Result<usize>;

    /// The changes in the workspace at `path` that are not committed, one per entry, and empty
    /// when everything is committed. Files `ns` keeps there are not counted.
    fn workspace_changes_list(&self, path: &Path) -> Result<Vec<String>>;

    /// Publishes `branch` from the workspace at `path` to the remote.
    fn branch_push(&self, path: &Path, branch: &str) -> Result<()>;

    /// The commit the workspace's branch is at.
    fn workspace_head_find(&self, path: &Path) -> Result<String>;

    /// Fetches the base branch (the branch the main checkout at `root` is on) from the remote and
    /// returns the commit it is at there. `None` when the remote has no such branch.
    fn base_fetch(&self, root: &Path) -> Result<Option<String>>;

    /// Whether `commit` is part of the workspace's branch.
    fn workspace_contains(&self, path: &Path, commit: &str) -> Result<bool>;

    /// Rebases the workspace's branch onto `onto`. A conflict aborts the rebase.
    fn workspace_rebase(&self, path: &Path, onto: &str) -> Result<Rebase>;

    /// Records the base the workspace's branch is judged against: `workspace_diff` and the
    /// commit count start from it. It may be a commit the branch has yet to be rebased onto.
    fn workspace_base_set(&self, path: &Path, base: &str) -> Result<()>;

    /// Moves the recorded base to the commit where the workspace's branch leaves the base
    /// branch, as the remote-tracking ref of the main checkout at `root` last saw it. Reads
    /// only local refs. The base stays put when the tracking ref is missing, or when the new
    /// commit would not be past the recorded base (the branch has not been rebased onto it).
    fn workspace_base_anchor(&self, root: &Path, path: &Path) -> Result<()>;

    /// Deletes `branch` from the repository at `root`, and what `ns` remembers of where it
    /// started. The branch must not be checked out in any workspace.
    fn branch_delete(&self, root: &Path, branch: &str) -> Result<()>;

    /// Brings the main checkout at `root` up to date with what its branch tracks on the remote,
    /// by fast-forward only. Does nothing when the branch tracks nothing.
    fn root_update(&self, root: &Path) -> Result<()>;

    /// Removes the workspace at `path` even when it holds changes that are not committed.
    fn workspace_discard(&self, root: &Path, path: &Path) -> Result<()>;

    /// Removes the workspace at `path` and keeps its branch. Fails when the workspace has
    /// uncommitted changes.
    fn workspace_remove(&self, root: &Path, path: &Path) -> Result<()>;
}

/// How a rebase of the workspace's branch ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Rebase {
    /// The branch now sits on the new commit.
    Done,
    /// The branch could not be moved. The rebase was aborted, so the workspace is as it was.
    /// These are the files that conflicted.
    Conflict(Vec<String>),
}

/// The version control `ns` uses. The only place a tool is chosen.
pub fn get() -> Box<dyn Vcs> {
    Box::new(git::Git::new())
}
