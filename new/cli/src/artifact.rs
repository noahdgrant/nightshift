//! Files a state leaves for the next one, under `.ns/<unit>/` in the unit's workspace.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const DIR: &str = ".ns";

/// Writes `text` to `name` in the unit's artifact directory and returns its path.
pub fn write(workspace: &Path, unit: &str, name: &str, text: &str) -> Result<PathBuf> {
    let dir = workspace.join(DIR).join(unit);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(name);
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

/// Removes `name` from the unit's artifact directory, if it is there.
pub fn remove(workspace: &Path, unit: &str, name: &str) -> Result<()> {
    match std::fs::remove_file(workspace.join(DIR).join(unit).join(name)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Reads `name` from the unit's artifact directory.
pub fn read(workspace: &Path, unit: &str, name: &str) -> Result<String> {
    let path = workspace.join(DIR).join(unit).join(name);
    std::fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))
}
