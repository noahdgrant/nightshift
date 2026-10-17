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

    /// Commits everything in the workspace at `path` with `message`. Returns whether there was
    /// anything to commit.
    fn workspace_commit(&self, path: &Path, message: &str) -> Result<bool>;

    /// Publishes `branch` from the workspace at `path` to the remote.
    fn branch_push(&self, path: &Path, branch: &str) -> Result<()>;

    /// Removes the workspace at `path` and keeps its branch. Fails when the workspace has
    /// uncommitted changes.
    fn workspace_remove(&self, root: &Path, path: &Path) -> Result<()>;
}

/// The version control `ns` uses. The only place a tool is chosen.
pub fn get() -> Box<dyn Vcs> {
    Box::new(git::Git)
}
