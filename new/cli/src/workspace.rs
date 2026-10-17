//! A unit's workspace: `<repo>.workspaces/<unit>` on the branch `ns/<unit>`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::vcs::Vcs;

const BRANCH_PREFIX: &str = "ns/";

pub struct Workspace {
    pub path: PathBuf,
    pub branch: String,
}

/// A unit id is `[a-z0-9][a-z0-9-]*`.
pub fn check_unit_id(id: &str) -> Result<()> {
    let mut chars = id.chars();
    let ok = matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        bail!("invalid unit id {id:?}: use lowercase letters, digits and dashes, starting with a letter or digit");
    }
    Ok(())
}

/// Creates the unit's workspace from `base`, or returns it when it exists.
pub fn ensure(vcs: &dyn Vcs, from: &Path, unit: &str, base: Option<&str>) -> Result<Workspace> {
    check_unit_id(unit)?;
    let root = vcs.root_find(from)?;
    let wt = locate(&root, unit)?;
    if wt.path.exists() {
        return Ok(wt);
    }
    vcs.workspace_create(&root, &wt.path, &wt.branch, base)?;
    Ok(wt)
}

/// Removes the unit's workspace and keeps its branch. A workspace with uncommitted changes is
/// refused. Returns whether there was a workspace to remove.
pub fn remove(vcs: &dyn Vcs, from: &Path, unit: &str) -> Result<bool> {
    check_unit_id(unit)?;
    let root = vcs.root_find(from)?;
    let wt = locate(&root, unit)?;
    if !wt.path.exists() {
        return Ok(false);
    }
    vcs.workspace_remove(&root, &wt.path)?;
    Ok(true)
}

fn locate(root: &Path, unit: &str) -> Result<Workspace> {
    let name = root
        .file_name()
        .context("repository has no directory name")?;
    let mut dir = name.to_os_string();
    dir.push(".workspaces");
    let parent = root
        .parent()
        .context("repository has no parent directory")?;
    Ok(Workspace {
        path: parent.join(dir).join(unit),
        branch: format!("{BRANCH_PREFIX}{unit}"),
    })
}
