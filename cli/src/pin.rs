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

/// The flags a claude phase's command must not carry itself: `ns run` owns them.
const OWNED_FLAGS: &[&str] = &["--settings", "--setting-sources"];

/// The first of the flags `ns run` owns that `argv` already carries.
pub fn owned_flag(argv: &[String]) -> Option<&'static str> {
    OWNED_FLAGS.iter().copied().find(|f| {
        argv.iter()
            .any(|a| a == f || a.starts_with(&format!("{f}=")))
    })
}

/// The arguments that give a claude phase only the project's settings sources and the factory's
/// phase settings file `settings`.
pub fn settings_args(settings: &Path) -> [String; 4] {
    [
        "--setting-sources".into(),
        "project,local".into(),
        "--settings".into(),
        settings.to_string_lossy().into_owned(),
    ]
}

/// Write the settings file for the `ns` binary at `ns`. A phase can edit or delete it, so
/// `ns run` writes it again before every phase.
pub fn write_settings(worktree: &Path, unit: &str, ns: &Path) -> Result<()> {
    let settings = settings_path(worktree, unit);
    if let Some(dir) = settings.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    fs::write(&settings, crate::hook_guard::settings(ns))
        .with_context(|| format!("cannot write {}", settings.display()))
}

/// Pin `unit`'s skills and settings in `worktree`: write the snapshot of `base`'s skills unless
/// a good one exists, write the settings for the `ns` binary at `ns`, and link the snapshot.
/// Returns the run-log events: `skills_pinned` when it wrote a snapshot, and `skills_unpinned`
/// for each link path it left alone.
pub fn pin(worktree: &Path, unit: &str, base: &str, ns: &Path) -> Result<Vec<Value>> {
    let skills = skills_path(worktree, unit);
    let mut events = Vec::new();
    if skills.is_dir() && count_skills(&skills) == 0 {
        fs::remove_dir_all(&skills)
            .with_context(|| format!("cannot remove {}", skills.display()))?;
    }
    if !skills.is_dir() {
        events.push(snapshot(worktree, base, &skills)?);
    }
    write_settings(worktree, unit, ns)?;
    let target = Path::new("..").join(".ns").join(unit).join("skills");
    for link in LINKS {
        if let Some(why) = link_to(&worktree.join(link), &target)? {
            eprintln!("ns run: warning: {link} {why}; phases may not see the pinned skills");
            events.push(json!({"event": "skills_unpinned", "path": link, "why": why}));
        }
    }
    Ok(events)
}

fn count_skills(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|d| d.flatten().filter(|e| e.path().is_dir()).count())
        .unwrap_or(0)
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
        checkout_tree(worktree, &format!("{sha}:{tree}"), &tmp)?;
        if *tree == "skills" && !has_ns_skills(&tmp) {
            source = "installed";
            let installed = install::Harness::Claude.dir();
            let dirs = if installed.is_dir() {
                install::skill_dirs(&installed).with_context(|| {
                    format!(
                        "cannot read the installed skills in {}",
                        installed.display()
                    )
                })?
            } else {
                Vec::new()
            };
            for dir in dirs {
                let name = dir.file_name().context("skill dir has no name")?;
                copy_dir(&dir, &tmp.join(name), &|_| false)?;
            }
        }
    }
    let count = count_skills(&tmp);
    if !has_ns_skills(&tmp) {
        let _ = fs::remove_dir_all(&tmp);
        anyhow::bail!(
            "no ns-* skills to pin for {base}: its skills/ tree has none and none are installed in {}; run `ns install`",
            install::Harness::Claude.dir().display()
        );
    }
    fs::rename(&tmp, into).with_context(|| format!("cannot write {}", into.display()))?;
    Ok(json!({
        "event": "skills_pinned",
        "sha": sha,
        "source": source,
        "skills": count,
    }))
}

/// Whether `dir` holds an `ns-*` skill directory.
fn has_ns_skills(dir: &Path) -> bool {
    install::skill_dirs(dir).is_ok_and(|d| !d.is_empty())
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

/// Point the symlink `link` at `target`. A symlink into some unit's snapshot is replaced; any
/// other existing path (a tracked `.claude/skills` or a symlink git tracks) is left alone and
/// the returned text says why. A tracked `.agents/skills` directory is the base commit's own
/// copy of the repo's skills, which the snapshot holds too, so it is no warning.
fn link_to(link: &Path, target: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(link) {
        Ok(m) if m.is_symlink() => {
            if !points_into_a_snapshot(&fs::read_link(link)?) {
                return Ok(Some("is a symlink that is not the unit's snapshot".into()));
            }
            fs::remove_file(link)?;
        }
        Ok(_) if link.ends_with(".agents/skills") => return Ok(None),
        Ok(_) => return Ok(Some("is a real directory in the worktree".into())),
        Err(_) => {}
    }
    if let Some(dir) = link.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("cannot link {}", link.display()))?;
    Ok(None)
}

/// `target` ends in `.ns/<unit>/skills`.
fn points_into_a_snapshot(target: &Path) -> bool {
    let parts: Vec<_> = target.components().rev().take(3).collect();
    matches!(parts.as_slice(), [skills, _, ns]
        if skills.as_os_str() == "skills" && ns.as_os_str() == ".ns")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(dir: &Path) -> (PathBuf, PathBuf) {
        (
            dir.join(".claude/skills"),
            Path::new("../.ns/u/skills").into(),
        )
    }

    #[test]
    fn settings_args_name_the_file_and_the_project_sources() {
        assert_eq!(
            settings_args(Path::new("/wt/s.json")),
            [
                "--setting-sources",
                "project,local",
                "--settings",
                "/wt/s.json"
            ]
        );
    }

    #[test]
    fn owned_flag_finds_either_flag_in_either_form() {
        let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            owned_flag(&argv(&["claude", "--settings=x"])),
            Some("--settings")
        );
        assert_eq!(
            owned_flag(&argv(&["claude", "--setting-sources", "user"])),
            Some("--setting-sources")
        );
        assert_eq!(owned_flag(&argv(&["claude", "--settings-x"])), None);
    }

    #[test]
    fn a_stale_snapshot_symlink_is_pointed_at_the_unit() {
        let dir = tempfile::tempdir().unwrap();
        let (l, target) = link(dir.path());
        fs::create_dir_all(l.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../.ns/old/skills", &l).unwrap();
        assert_eq!(link_to(&l, &target).unwrap(), None);
        assert_eq!(fs::read_link(&l).unwrap(), target);
    }

    #[test]
    fn a_symlink_elsewhere_is_left_alone_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (l, target) = link(dir.path());
        fs::create_dir_all(l.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../tracked", &l).unwrap();
        let why = link_to(&l, &target).unwrap().expect("a reason");
        assert!(why.contains("symlink"), "{why}");
        assert_eq!(fs::read_link(&l).unwrap(), Path::new("../tracked"));
    }

    #[test]
    fn a_real_claude_skills_dir_is_left_alone_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (l, target) = link(dir.path());
        fs::create_dir_all(&l).unwrap();
        let why = link_to(&l, &target).unwrap().expect("a reason");
        assert!(why.contains("real directory"), "{why}");
        assert!(!fs::symlink_metadata(&l).unwrap().is_symlink());
        // The repo's own tracked .agents/skills is the base copy: no report.
        let agents = dir.path().join(".agents/skills");
        fs::create_dir_all(&agents).unwrap();
        assert_eq!(link_to(&agents, &target).unwrap(), None);
    }
}
