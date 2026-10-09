//! Keep installed skills that live in the watched repo at `origin/<default>` (docs/FACTORY.md).
//!
//! A unit runs in a fresh worktree, so skills checked into the repo are current. A skill
//! installed as a symlink into the main checkout is not: `ns watch` merges skill changes to
//! `origin/<default>` all night, but the checkout never moves.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git;
use crate::install::{self, Action};

#[derive(Debug, PartialEq)]
pub enum Sync {
    /// No installed skill resolves into the checkout; it was left alone.
    NotInstalled,
    /// The checkout already sits at `base`.
    Current,
    /// Fast-forwarded from one commit to another.
    Updated { from: String, to: String },
    /// Left alone, so the installed skills may be stale.
    Stale { reason: String },
}

/// Installed skills whose symlinks resolve into a checkout, and the dirs they sit in.
#[derive(Debug, Default)]
pub struct Installed {
    pub names: Vec<String>,
    sources: BTreeSet<PathBuf>,
}

/// The skills in `targets` that are symlinks to a skill dir (one with a `SKILL.md`) in `root`.
pub fn installed_into(targets: &[PathBuf], root: &Path) -> Installed {
    let mut found = Installed::default();
    let Ok(root) = root.canonicalize() else {
        return found;
    };
    let links = targets
        .iter()
        .filter_map(|t| fs::read_dir(t).ok())
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_symlink()));
    for e in links {
        let Ok(dest) = e.path().canonicalize() else {
            continue;
        };
        if dest.starts_with(&root) && dest.join("SKILL.md").is_file() {
            found
                .names
                .push(e.file_name().to_string_lossy().into_owned());
            found.sources.extend(dest.parent().map(Path::to_path_buf));
        }
    }
    found.names.sort();
    found.names.dedup();
    found
}

/// Fast-forward `root` to `base` (`origin/<branch>`, already fetched) when skills are
/// installed from it and the checkout is on `<branch>` with no changes to tracked files.
/// Never resets, stashes or switches branches, and skips the checkout's git hooks.
pub fn sync(root: &Path, base: &str, installed: &Installed) -> Sync {
    if installed.names.is_empty() {
        return Sync::NotInstalled;
    }
    let stale = |reason: String| Sync::Stale { reason };
    let (Ok(from), Ok(to)) = (
        git::run(root, &["rev-parse", "HEAD"]),
        git::run(root, &["rev-parse", base]),
    ) else {
        return stale(format!("cannot resolve HEAD or {base}"));
    };
    if from == to {
        return Sync::Current;
    }
    let branch = base.strip_prefix("origin/").unwrap_or(base);
    match git::run(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(b) if b == branch => {}
        Ok(b) => return stale(format!("the checkout is on {b}, not {branch}")),
        Err(_) => return stale("the checkout has a detached HEAD".into()),
    }
    match git::run(root, &["status", "--porcelain", "--untracked-files=no"]) {
        Ok(s) if s.is_empty() => {}
        Ok(_) => return stale("the checkout has local changes".into()),
        Err(e) => return stale(format!("cannot read the checkout's status: {e}")),
    }
    if !git::ok(root, &["merge-base", "--is-ancestor", "HEAD", base]) {
        return stale(format!("{branch} has diverged from {base}"));
    }
    let merge = [
        "-c",
        "core.hooksPath=/dev/null",
        "merge",
        "--ff-only",
        "--quiet",
        base,
    ];
    match git::run(root, &merge) {
        Ok(_) => Sync::Updated { from, to },
        Err(e) => stale(format!("cannot fast-forward to {base}: {e}")),
    }
}

/// After a fast-forward, rerun `ns install` for each source dir of `ns-*` skills, so skills
/// added or removed on `origin/<default>` are linked or pruned. Returns what changed.
pub fn relink(installed: &Installed, targets: &[PathBuf]) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for src in &installed.sources {
        if install::skill_dirs(src).map_or(true, |d| d.is_empty()) {
            continue;
        }
        for step in install::run(src, targets, false)?.actions {
            let verb = match step.action {
                Action::Link => "linked",
                Action::Relink => "relinked",
                Action::Prune => "pruned",
                Action::Unchanged | Action::Conflict => continue,
            };
            changed.push(format!("{verb} {}", step.link));
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{commit_file, g};

    struct Setup {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        targets: Vec<PathBuf>,
    }

    const SKILL: &str = "skills/ns-a/SKILL.md";

    /// A clone `root` of an origin that has one commit `root` hasn't seen, with
    /// `skills/ns-a` installed into one target dir.
    fn setup() -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let origin = base.join("origin");
        fs::create_dir(&origin).unwrap();
        g(&origin, &["init", "-q", "-b", "main"]);
        fs::create_dir_all(origin.join("skills/ns-a")).unwrap();
        commit_file(&origin, SKILL, "v1\n");
        let root = base.join("root");
        g(&base, &["clone", "-q", "origin", "root"]);
        commit_file(&origin, SKILL, "v2\n");
        g(&root, &["fetch", "-q", "origin"]);
        let target = base.join("target");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(root.join("skills/ns-a"), target.join("ns-a")).unwrap();
        Setup {
            _tmp: tmp,
            root,
            targets: vec![target],
        }
    }

    fn head(dir: &Path) -> String {
        g(dir, &["rev-parse", "HEAD"])
    }

    fn sync_all(s: &Setup) -> Sync {
        sync(&s.root, "origin/main", &installed_into(&s.targets, &s.root))
    }

    #[test]
    fn finds_only_skill_dirs_inside_the_repo() {
        let s = setup();
        let elsewhere = s._tmp.path().join("elsewhere/ns-b");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("SKILL.md"), "x\n").unwrap();
        std::os::unix::fs::symlink(&elsewhere, s.targets[0].join("ns-b")).unwrap();
        std::os::unix::fs::symlink(s.root.join("skills"), s.targets[0].join("notes")).unwrap();
        fs::create_dir(s.targets[0].join("real")).unwrap();
        assert_eq!(installed_into(&s.targets, &s.root).names, ["ns-a"]);
    }

    #[test]
    fn leaves_the_checkout_alone_when_no_skill_is_installed_from_it() {
        let s = setup();
        let before = head(&s.root);
        let none = installed_into(&[], &s.root);
        assert_eq!(sync(&s.root, "origin/main", &none), Sync::NotInstalled);
        assert_eq!(head(&s.root), before);
    }

    #[test]
    fn fast_forwards_a_clean_checkout_on_the_default_branch() {
        let s = setup();
        let from = head(&s.root);
        let to = g(&s.root, &["rev-parse", "origin/main"]);
        assert_eq!(
            sync_all(&s),
            Sync::Updated {
                from,
                to: to.clone()
            }
        );
        assert_eq!(head(&s.root), to);
        assert_eq!(fs::read_to_string(s.root.join(SKILL)).unwrap(), "v2\n");
        assert_eq!(sync_all(&s), Sync::Current);
    }

    #[test]
    fn the_fast_forward_skips_the_checkouts_hooks() {
        let s = setup();
        let hook = s.root.join(".git/hooks/post-merge");
        fs::write(&hook, "#!/bin/sh\ntouch ran\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(sync_all(&s), Sync::Updated { .. }));
        assert!(!s.root.join("ran").exists());
    }

    #[test]
    fn relink_links_skills_added_and_prunes_skills_removed_upstream() {
        let s = setup();
        let origin = s._tmp.path().canonicalize().unwrap().join("origin");
        fs::create_dir(origin.join("skills/ns-new")).unwrap();
        commit_file(&origin, "skills/ns-new/SKILL.md", "new\n");
        g(&s.root, &["fetch", "-q", "origin"]);
        let installed = installed_into(&s.targets, &s.root);
        assert!(matches!(
            sync(&s.root, "origin/main", &installed),
            Sync::Updated { .. }
        ));
        let changed = relink(&installed, &s.targets).unwrap();
        assert_eq!(changed.len(), 1, "{changed:?}");
        assert!(changed[0].starts_with("linked ") && changed[0].ends_with("ns-new"));
        assert!(s.targets[0].join("ns-new/SKILL.md").is_file());

        g(&origin, &["rm", "-rq", "skills/ns-new"]);
        g(&origin, &["commit", "-qm", "drop ns-new"]);
        g(&s.root, &["fetch", "-q", "origin"]);
        let installed = installed_into(&s.targets, &s.root);
        assert!(matches!(
            sync(&s.root, "origin/main", &installed),
            Sync::Updated { .. }
        ));
        let changed = relink(&installed, &s.targets).unwrap();
        assert!(
            changed
                .iter()
                .any(|c| c.starts_with("pruned ") && c.ends_with("ns-new")),
            "{changed:?}"
        );
        assert!(fs::symlink_metadata(s.targets[0].join("ns-new")).is_err());
    }

    fn assert_stale(s: &Setup, want: &str) {
        let before = head(&s.root);
        match sync_all(s) {
            Sync::Stale { reason } => assert!(reason.contains(want), "{reason}"),
            other => panic!("want Stale, got {other:?}"),
        }
        assert_eq!(head(&s.root), before);
    }

    #[test]
    fn a_dirty_tree_is_left_alone() {
        let s = setup();
        fs::write(s.root.join(SKILL), "edited\n").unwrap();
        assert_stale(&s, "local changes");
        assert_eq!(fs::read_to_string(s.root.join(SKILL)).unwrap(), "edited\n");
    }

    #[test]
    fn another_branch_is_left_alone() {
        let s = setup();
        g(&s.root, &["checkout", "-qb", "topic"]);
        assert_stale(&s, "on topic, not main");
    }

    #[test]
    fn a_detached_head_is_left_alone() {
        let s = setup();
        g(&s.root, &["checkout", "-q", "--detach"]);
        assert_stale(&s, "detached HEAD");
    }

    #[test]
    fn diverged_history_is_left_alone() {
        let s = setup();
        commit_file(&s.root, "local.txt", "mine\n");
        assert_stale(&s, "diverged");
    }
}
