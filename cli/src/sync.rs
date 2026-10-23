//! Puts the skills that came with this `ns` where each of the repository's harnesses finds them,
//! for a session the user takes part in. A copy is a function of the version, so it is kept out
//! of the repository's changes and rewritten whenever it differs.
//!
//! With `NS_DEV` set, in a repository that has a `skills` directory of its own (nightshift's), a
//! skill is a link to that directory's copy instead, so an edit to a skill shows in a session
//! without a rebuild.

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
    vcs::get().ignore_add(root, &format!("/{dir}/ns-*"))?;
    let base = root.join(dir);
    let source = root.join("skills");
    let linked = std::env::var_os("NS_DEV").is_some() && source.is_dir();
    let mut written = 0;
    for (name, text) in skill::embedded() {
        let at = base.join(name);
        let is_link = at.symlink_metadata().is_ok_and(|m| m.is_symlink());
        if linked && source.join(name).is_dir() {
            if is_link && std::fs::read_link(&at)? == Path::new("../../skills").join(name) {
                continue;
            }
            std::fs::create_dir_all(&base)?;
            remove(&at)?;
            std::os::unix::fs::symlink(Path::new("../../skills").join(name), &at)
                .with_context(|| format!("cannot link {}", at.display()))?;
            written += 1;
            continue;
        }
        // A link left by a development run must not be written through into the source.
        if is_link {
            remove(&at)?;
        }
        let file = at.join("SKILL.md");
        let wanted = skill::file_for_session(name, text);
        if std::fs::read_to_string(&file).ok().as_deref() == Some(wanted.as_str()) {
            continue;
        }
        std::fs::create_dir_all(file.parent().context("a skill has no directory")?)?;
        std::fs::write(&file, wanted)
            .with_context(|| format!("cannot write {}", file.display()))?;
        written += 1;
    }
    let standards = base.join(skill::STANDARDS_FILE);
    if std::fs::read_to_string(&standards).ok().as_deref() != Some(skill::STANDARDS) {
        std::fs::write(&standards, skill::STANDARDS)
            .with_context(|| format!("cannot write {}", standards.display()))?;
        written += 1;
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(&base)? {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with("ns-")
            && name != skill::STANDARDS_FILE
            && !skill::embedded().iter().any(|(n, _)| *n == name)
        {
            remove(&path)?;
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

/// Removes a skill directory, or the link that stands for it, without touching what it links to.
fn remove(path: &Path) -> Result<()> {
    if path.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
        std::fs::remove_file(path)?;
    } else if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}
