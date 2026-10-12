//! The skills and settings `ns run` pins for a unit's phases (docs/FACTORY.md, "Pinned skills").
//! The skills are a snapshot of the unit's base commit, written once into `.ns/<unit>/skills/`
//! and linked as the worktree's `.claude/skills` and `.agents/skills`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::eval::trial::copy_dir;
use crate::git;
use crate::install;

/// Where the harness looks for project skills, relative to the worktree.
pub const LINKS: &[&str] = &[".claude/skills", ".agents/skills"];

/// The skill trees a snapshot takes from the base commit, in order.
const TREES: &[&str] = &["skills", ".agents/skills"];

/// The settings file a claude phase of `unit` runs with.
pub fn settings_path(worktree: &Path, unit: &str) -> PathBuf {
    worktree.join(".ns").join(unit).join("phase-settings.json")
}

fn skills_path(worktree: &Path, unit: &str) -> PathBuf {
    worktree.join(".ns").join(unit).join("skills")
}

/// Pin `unit`'s skills and settings in `worktree`: write the snapshot of `base`'s skills unless
/// one exists, write the settings for the `ns` binary at `ns`, and link the snapshot. Returns
/// the `skills_pinned` event when it wrote a snapshot.
pub fn pin(worktree: &Path, unit: &str, base: &str, ns: &Path) -> Result<Option<Value>> {
    let skills = skills_path(worktree, unit);
    let event = if skills.is_dir() {
        None
    } else {
        Some(snapshot(worktree, base, &skills)?)
    };
    let settings = settings_path(worktree, unit);
    fs::write(&settings, crate::hook_guard::settings(ns))
        .with_context(|| format!("cannot write {}", settings.display()))?;
    let target = Path::new("..").join(".ns").join(unit).join("skills");
    for link in LINKS {
        link_to(&worktree.join(link), &target)?;
    }
    Ok(event)
}

/// Write the skills of the commit `worktree` forked from `base` into `into`, as real files.
/// With no `skills/` tree there (a repo other than nightshift), the installed `ns-*` skills are
/// copied instead.
fn snapshot(worktree: &Path, base: &str, into: &Path) -> Result<Value> {
    let sha = git::run(worktree, &["merge-base", base, "HEAD"])?;
    let tmp = into.with_extension("tmp");
    if tmp.exists() {
        fs::remove_dir_all(&tmp).with_context(|| format!("cannot remove {}", tmp.display()))?;
    }
    fs::create_dir_all(&tmp).with_context(|| format!("cannot create {}", tmp.display()))?;
    let mut source = "base";
    for tree in TREES {
        let found = checkout_tree(worktree, &format!("{sha}:{tree}"), &tmp)?;
        if *tree == "skills" && !found {
            source = "installed";
            for dir in install::skill_dirs(&install::Harness::Claude.dir()).unwrap_or_default() {
                let name = dir.file_name().context("skill dir has no name")?;
                copy_dir(&dir, &tmp.join(name), &|_| false)?;
            }
        }
    }
    fs::rename(&tmp, into).with_context(|| format!("cannot write {}", into.display()))?;
    let count = fs::read_dir(into)?
        .flatten()
        .filter(|e| e.path().is_dir())
        .count();
    Ok(json!({
        "event": "skills_pinned",
        "sha": sha,
        "source": source,
        "skills": count,
    }))
}

/// Check out the tree `treeish` into `into` through a scratch index, so neither the worktree
/// nor its index changes. False when there is no such tree.
fn checkout_tree(worktree: &Path, treeish: &str, into: &Path) -> Result<bool> {
    if git::run(worktree, &["cat-file", "-t", treeish])
        .ok()
        .as_deref()
        != Some("tree")
    {
        return Ok(false);
    }
    let scratch = tempfile::tempdir().context("cannot create a scratch dir")?;
    let prefix = format!("--prefix={}/", into.display());
    for args in [
        vec!["read-tree", treeish],
        vec!["checkout-index", "--all", &prefix],
    ] {
        let out = git::command()
            .arg("-C")
            .arg(worktree)
            .args(&args)
            .env("GIT_INDEX_FILE", scratch.path().join("index"))
            .output()
            .context("failed to run git; is it installed and on PATH?")?;
        if !out.status.success() {
            anyhow::bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }
    Ok(true)
}

/// Point the symlink `link` at `target`, unless something other than a symlink is there (a
/// tracked `.agents/skills`), which is left alone.
fn link_to(link: &Path, target: &Path) -> Result<()> {
    match fs::symlink_metadata(link) {
        Ok(m) if !m.is_symlink() => return Ok(()),
        Ok(_) => fs::remove_file(link)?,
        Err(_) => {}
    }
    if let Some(dir) = link.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("cannot link {}", link.display()))
}
