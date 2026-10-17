//! Version control behind one trait. Everything specific to a tool lives in its own file here.

mod git;

use std::path::{Path, PathBuf};

use anyhow::Result;

pub trait Vcs {
    /// The root of the main checkout of the repository `from` is in, even when `from` is
    /// another workspace of it.
    fn root_find(&self, from: &Path) -> Result<PathBuf>;

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

    /// The commit the workspace's branch started from.
    fn workspace_base_find(&self, path: &Path) -> Result<String>;

    /// What the workspace's branch changed since `base`, as a patch.
    fn workspace_diff(&self, path: &Path, base: &str) -> Result<String>;

    /// How many commits the workspace's branch holds past `base`.
    fn workspace_commits_count(&self, path: &Path, base: &str) -> Result<usize>;

    /// The changes in the workspace at `path` that are not committed, one per entry, and empty
    /// when everything is committed. Files `ns` keeps there are not counted.
    fn workspace_changes_list(&self, path: &Path) -> Result<Vec<String>>;

    /// Publishes `branch` from the workspace at `path` to the remote.
    fn branch_push(&self, path: &Path, branch: &str) -> Result<()>;

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

/// The version control `ns` uses. The only place a tool is chosen.
pub fn get() -> Box<dyn Vcs> {
    Box::new(git::Git::new())
}
