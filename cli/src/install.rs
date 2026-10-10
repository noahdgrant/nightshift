//! `ns install`: symlink `skills/ns-*` into harness skill directories.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::error::SfError;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// New symlink.
    Link,
    /// Already points at the source.
    Unchanged,
    /// Replaced a stale ns-owned symlink.
    Relink,
    /// Removed a dangling ns-owned symlink to a skill that no longer exists in the source.
    Prune,
    /// Something else is in the way; left untouched.
    Conflict,
}

#[derive(Debug, Serialize)]
pub struct Step {
    pub skill: String,
    pub link: String,
    pub action: Action,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub source: String,
    pub targets: Vec<String>,
    pub dry_run: bool,
    pub conflicts: usize,
    pub actions: Vec<Step>,
}

pub fn default_targets() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    vec![home.join(".agents/skills"), home.join(".claude/skills")]
}

pub fn skill_dirs(source: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(source)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("SKILL.md").is_file() && is_sf_name(p))
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn is_sf_name(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("ns-"))
}

/// A symlink is ours if it is named `ns-*` and points at a directory of the same name
/// (another checkout of the same skill) or at nothing at all.
fn owned(link: &Path, dest: &Path) -> bool {
    is_sf_name(link) && (dest.file_name() == link.file_name() || !link.exists())
}

/// `./skills` when run from a checkout, else the checkout `ns` was built from.
pub fn default_source() -> PathBuf {
    let local = PathBuf::from("skills");
    if local.is_dir() {
        return local;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills")
}

pub fn run(source: &Path, targets: &[PathBuf], dry_run: bool) -> Result<Report> {
    if !source.is_dir() {
        return Err(SfError::usage(
            format!("skills source {} does not exist", source.display()),
            "ns install --source ~/src/nightshift/skills --dry-run",
        )
        .into());
    }
    let source = source
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", source.display()))?;
    let skills = skill_dirs(&source)?;
    if skills.is_empty() {
        return Err(SfError::usage(
            format!("no ns-*/SKILL.md skills in {}", source.display()),
            "ns install --source ~/src/nightshift/skills --dry-run",
        )
        .into());
    }

    let mut actions = Vec::new();
    for target in targets {
        if !dry_run {
            fs::create_dir_all(target)
                .with_context(|| format!("cannot create {}", target.display()))?;
        }
        for skill in &skills {
            let name = skill.file_name().unwrap().to_string_lossy().into_owned();
            let link = target.join(&name);
            let mut step = Step {
                skill: name.clone(),
                link: link.to_string_lossy().into_owned(),
                action: Action::Link,
                previous: None,
                reason: None,
            };
            match fs::symlink_metadata(&link) {
                Err(_) => {
                    if !dry_run {
                        symlink(skill, &link)?;
                    }
                }
                Ok(m) if m.file_type().is_symlink() => {
                    let dest = fs::read_link(&link)?;
                    if dest == *skill {
                        step.action = Action::Unchanged;
                    } else if owned(&link, &dest) {
                        step.action = Action::Relink;
                        step.previous = Some(dest.to_string_lossy().into_owned());
                        if !dry_run {
                            fs::remove_file(&link)?;
                            symlink(skill, &link)?;
                        }
                    } else {
                        step.action = Action::Conflict;
                        step.previous = Some(dest.to_string_lossy().into_owned());
                        step.reason = Some("symlink to an unrelated path".into());
                    }
                }
                Ok(_) => {
                    step.action = Action::Conflict;
                    step.reason = Some("a real file or directory is in the way".into());
                }
            }
            actions.push(step);
        }
        // Prune dangling links into the source for skills that were deleted.
        if let Ok(entries) = fs::read_dir(target) {
            for e in entries.flatten() {
                let link = e.path();
                let Ok(dest) = fs::read_link(&link) else {
                    continue;
                };
                if is_sf_name(&link) && dest.starts_with(&source) && !link.exists() {
                    if !dry_run {
                        fs::remove_file(&link)?;
                    }
                    actions.push(Step {
                        skill: link.file_name().unwrap().to_string_lossy().into_owned(),
                        link: link.to_string_lossy().into_owned(),
                        action: Action::Prune,
                        previous: Some(dest.to_string_lossy().into_owned()),
                        reason: None,
                    });
                }
            }
        }
    }
    let conflicts = actions
        .iter()
        .filter(|a| a.action == Action::Conflict)
        .count();
    Ok(Report {
        source: source.to_string_lossy().into_owned(),
        targets: targets
            .iter()
            .map(|t| t.to_string_lossy().into_owned())
            .collect(),
        dry_run,
        conflicts,
        actions,
    })
}

#[cfg(unix)]
fn symlink(src: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, link)
        .with_context(|| format!("cannot link {} -> {}", link.display(), src.display()))
}

#[cfg(windows)]
fn symlink(src: &Path, link: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(src, link)
        .with_context(|| format!("cannot link {} -> {}", link.display(), src.display()))
}
