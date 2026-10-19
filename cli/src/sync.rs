//! Puts the skills that came with this `ns` where each of the repository's harnesses finds them,
//! for a session the user takes part in. A copy is a function of the version, so it is kept out
//! of the repository's changes and rewritten whenever it differs.

use std::path::Path;

use anyhow::{Context, Result};

use crate::harness::Kind;
use crate::{harness, skill, vcs};

#[derive(Debug, PartialEq, Eq)]
pub struct Synced {
    pub harness: Kind,
    pub dir: String,
    /// Skills written because they were missing or differed.
    pub written: usize,
    /// Skills `ns` once wrote that this version no longer has.
    pub removed: usize,
}

pub fn run(root: &Path, kinds: &[Kind]) -> Result<Vec<Synced>> {
    kinds.iter().map(|kind| one(root, *kind)).collect()
}

/// What a harness runs when a session starts. A machine without `ns` goes on without it.
pub const HOOK: &str = "command -v ns >/dev/null 2>&1 && ns sync >/dev/null || true";

fn one(root: &Path, kind: Kind) -> Result<Synced> {
    let dir = harness::of(kind).skills_dir();
    vcs::get().ignore_add(root, &format!("/{dir}/ns-*/"))?;
    let base = root.join(dir);
    let mut written = 0;
    for (name, text) in skill::embedded() {
        let file = base.join(name).join("SKILL.md");
        let wanted = skill::file_for_session(name, text);
        if std::fs::read_to_string(&file).ok().as_deref() == Some(wanted.as_str()) {
            continue;
        }
        std::fs::create_dir_all(file.parent().context("a skill has no directory")?)?;
        std::fs::write(&file, wanted)
            .with_context(|| format!("cannot write {}", file.display()))?;
        written += 1;
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(&base)? {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with("ns-") && !skill::embedded().iter().any(|(n, _)| *n == name) {
            std::fs::remove_dir_all(&path)?;
            removed += 1;
        }
    }
    Ok(Synced {
        harness: kind,
        dir: dir.to_string(),
        written,
        removed,
    })
}
