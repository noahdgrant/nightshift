//! The skills and settings `ns run` pins for a unit's phases (docs/FACTORY.md, "Pinned skills").
//! The skills are a snapshot of the unit's base commit, written once into `.ns/<unit>/skills/`
//! and linked as the worktree's `.claude/skills` and `.agents/skills`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::error::SfError;
use crate::eval::trial::copy_dir;
use crate::git;
use crate::install;
use crate::shell::shell_quote;

/// The directory under the worktree that holds each unit's artifacts and its skill snapshot.
pub const NS_DIR: &str = ".ns";
/// The snapshot's directory name inside a unit's `.ns/<unit>/`.
pub const SKILLS_DIR: &str = "skills";

/// Where the harness looks for project skills, relative to the worktree.
pub const LINKS: &[&str] = &[CLAUDE_LINK, ".agents/skills"];

/// The path claude reads project skills from.
const CLAUDE_LINK: &str = ".claude/skills";

/// The factory's phase settings (docs/FACTORY.md, "Phase settings"), versioned in
/// `.nightshift/` so a change to them needs a human merge. `{ns}` stands for the `ns` binary.
const PHASE_SETTINGS: &str = include_str!("../../.nightshift/phase-settings.json");

/// The directories under the git common dir that hold a unit's settings, and the file's name.
pub const COMMON_NS: &str = "ns";
pub const COMMON_UNITS: &str = "units";
/// The variable that names the settings file to a phase.
pub const SETTINGS_VAR: &str = "NS_PHASE_SETTINGS";
pub const SETTINGS_FILE: &str = "phase-settings.json";
/// The directory under a unit's that holds each phase's `TMPDIR`.
pub const TMP_DIR: &str = "tmp";

/// The settings file a claude phase of `unit` runs with. It lives under the git common dir,
/// outside every worktree, so a phase cannot reach it by editing its own worktree.
pub fn settings_path(common_dir: &Path, unit: &str) -> PathBuf {
    unit_dir(common_dir, unit).join(SETTINGS_FILE)
}

/// The directory that holds `unit`'s phase `TMPDIR`s, one `<phase>-<attempt>` each.
pub fn tmp_path(common_dir: &Path, unit: &str) -> PathBuf {
    unit_dir(common_dir, unit).join(TMP_DIR)
}

fn unit_dir(common_dir: &Path, unit: &str) -> PathBuf {
    common_dir.join(COMMON_NS).join(COMMON_UNITS).join(unit)
}

fn skills_path(worktree: &Path, unit: &str) -> PathBuf {
    worktree.join(NS_DIR).join(unit).join(SKILLS_DIR)
}

/// The settings a claude phase runs with: the factory's denies and one `PreToolUse` hook that
/// runs `ns guard-hooks`. Claude Code lets a call through when a hook exits with anything but
/// 2, so a missing or crashing `ns` must exit 2 to block it.
pub fn settings_json(ns: &Path) -> String {
    let ns = shell_quote(&ns.to_string_lossy());
    let mut v: Value = serde_json::from_str(PHASE_SETTINGS).expect("phase-settings.json is JSON");
    for entry in v["hooks"]["PreToolUse"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        for hook in entry["hooks"].as_array_mut().into_iter().flatten() {
            if let Some(c) = hook["command"].as_str() {
                hook["command"] = json!(c.replace("{ns}", &ns));
            }
        }
    }
    serde_json::to_string_pretty(&v).expect("a Value serializes")
}

/// How much of a harness's skills `ns run` pins (docs/FACTORY.md, "Pinned skills and phase
/// settings"). Every harness's worktree gets the `.claude/skills` and `.agents/skills` links;
/// the mode is what the harness is known to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The harness sees only the unit's snapshot and the factory's settings: claude.
    Snapshot,
    /// The harness reads the snapshot as the project's skills (`.agents/skills`) and also the
    /// user's installed skills: codex. A repo that tracks `.agents/skills` keeps the live
    /// directory there, which the harness reads instead.
    Partial,
    /// No claim: the harness may not read either link, or its lookup is not confirmed.
    None,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Snapshot => "snapshot",
            Mode::Partial => "partial",
            Mode::None => "none",
        }
    }
}

/// The pinning mode of the harness named `harness` (as `ns doctor` lists them).
pub fn mode(harness: &str) -> Mode {
    match harness {
        "claude" => Mode::Snapshot,
        "codex" => Mode::Partial,
        _ => Mode::None,
    }
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

/// The arguments that give a claude phase of `unit` only the project's settings sources, the
/// factory's phase settings file, and the unit's skill snapshot as an added directory.
pub fn settings_args(settings: &Path, worktree: &Path, unit: &str) -> [String; 6] {
    [
        "--setting-sources".into(),
        "project,local".into(),
        "--settings".into(),
        settings.to_string_lossy().into_owned(),
        "--add-dir".into(),
        skills_path(worktree, unit).to_string_lossy().into_owned(),
    ]
}

/// Give the claude command `argv` of `who` (a phase, or `ns ask`) the pin: the arguments of
/// `settings_args`. A command that already carries a flag `ns run` owns is a usage error, and
/// is left as it was.
pub fn apply(
    argv: &mut Vec<String>,
    settings: &Path,
    worktree: &Path,
    unit: &str,
    who: &str,
) -> std::result::Result<(), SfError> {
    if let Some(flag) = owned_flag(argv) {
        return Err(SfError::usage(
            format!("{who}: the harness command already passes {flag}"),
            format!("ns gives each claude phase the factory's phase settings and only the project's own settings sources; remove {flag} from the harness command and put project settings in the repo's .claude/settings.json"),
        ));
    }
    argv.extend(settings_args(settings, worktree, unit));
    Ok(())
}

/// Write the settings file for the `ns` binary at `ns`. `ns run` writes it again before every
/// phase, so a file left stale by an upgrade or a crash is never read.
pub fn write_settings(common_dir: &Path, unit: &str, ns: &Path) -> Result<()> {
    let settings = settings_path(common_dir, unit);
    if let Some(dir) = settings.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let tmp = settings.with_extension("json.tmp");
    fs::write(&tmp, settings_json(ns))
        .and_then(|()| fs::rename(&tmp, &settings))
        .with_context(|| format!("cannot write {}", settings.display()))
}

/// Pin `unit`'s skills and settings in `worktree`: write the snapshot of `base`'s skills unless
/// a good one exists, write the settings (under `common_dir`) for the `ns` binary at `ns`, and
/// link the snapshot. A `.claude/skills` that is not ours is an error, since claude reads it and
/// would run unpinned. Returns the run-log events: `skills_pinned` when it wrote a snapshot,
/// and `skills_unpinned` for each other link path it left alone.
pub fn pin(
    worktree: &Path,
    common_dir: &Path,
    unit: &str,
    base: &str,
    ns: &Path,
) -> Result<Vec<Value>> {
    let skills = skills_path(worktree, unit);
    let mut events = Vec::new();
    if !has_ns_skills(&skills) {
        if skills.is_dir() {
            fs::remove_dir_all(&skills)
                .with_context(|| format!("cannot remove {}", skills.display()))?;
        }
        events.push(snapshot(worktree, base, &skills)?);
    }
    write_settings(common_dir, unit, ns)?;
    let target = Path::new("..").join(NS_DIR).join(unit).join(SKILLS_DIR);
    for link in LINKS {
        let Some(why) = link_to(&worktree.join(link), &target)? else {
            continue;
        };
        if *link == CLAUDE_LINK {
            return Err(SfError::usage(
                format!("{link} {why}, so claude phases would not run the pinned skills"),
                format!(
                    "remove or untrack {link} in this repo, or move its skills to .agents/skills"
                ),
            )
            .into());
        }
        eprintln!("ns run: warning: {link} {why}; phases may not see the pinned skills");
        events.push(json!({"event": "skills_unpinned", "path": link, "why": why}));
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
    let sha = git::run(worktree, &["merge-base", "--", base, "HEAD"])?;
    let tmp = into.with_extension("tmp");
    let built = build_snapshot(worktree, base, &sha, &tmp);
    if built.is_err() {
        let _ = fs::remove_dir_all(&tmp);
    }
    let (source, count) = built?;
    fs::rename(&tmp, into).with_context(|| format!("cannot write {}", into.display()))?;
    Ok(json!({
        "event": "skills_pinned",
        "sha": sha,
        "source": source,
        "skills": count,
    }))
}

/// Fill `tmp` with the skills of `sha`. A skill in both `skills/` and `.agents/skills/` (the
/// repo's own tracked copy) takes the `.agents/skills` file, which is checked out last.
fn build_snapshot(
    worktree: &Path,
    base: &str,
    sha: &str,
    tmp: &Path,
) -> Result<(&'static str, usize)> {
    if tmp.exists() {
        fs::remove_dir_all(tmp).with_context(|| format!("cannot remove {}", tmp.display()))?;
    }
    fs::create_dir_all(tmp).with_context(|| format!("cannot create {}", tmp.display()))?;
    checkout_tree(worktree, &format!("{sha}:skills"), tmp)?;
    let mut source = "base";
    if !has_ns_skills(tmp) {
        source = "installed";
        copy_installed_skills(tmp)?;
    }
    checkout_tree(worktree, &format!("{sha}:.agents/skills"), tmp)?;
    if !has_ns_skills(tmp) {
        anyhow::bail!(
            "no ns-* skills to pin for {base}: its skills/ tree has none and none are installed in {}; run `ns install`",
            install::Harness::Claude.dir().display()
        );
    }
    Ok((source, count_skills(tmp)))
}

/// Copy the installed `ns-*` skills into `into`; none installed copies nothing.
fn copy_installed_skills(into: &Path) -> Result<()> {
    let installed = install::Harness::Claude.dir();
    if !installed.is_dir() {
        return Ok(());
    }
    let dirs = install::skill_dirs(&installed).with_context(|| {
        format!(
            "cannot read the installed skills in {}",
            installed.display()
        )
    })?;
    for dir in dirs {
        let name = dir.file_name().context("skill dir has no name")?;
        copy_dir(&dir, &into.join(name), &|_| false)?;
    }
    Ok(())
}

/// Whether `dir` holds an `ns-*` skill directory: the one test of a good snapshot.
fn has_ns_skills(dir: &Path) -> bool {
    install::skill_dirs(dir).is_ok_and(|d| !d.is_empty())
}

/// Check out the tree `treeish` into `into` through a scratch index, so neither the worktree
/// nor its index changes. A `treeish` that names no tree is skipped, not an error.
fn checkout_tree(worktree: &Path, treeish: &str, into: &Path) -> Result<()> {
    if git::run(worktree, &["cat-file", "-t", treeish])
        .ok()
        .as_deref()
        != Some("tree")
    {
        return Ok(());
    }
    let scratch = tempfile::tempdir().context("cannot create a scratch dir")?;
    let prefix = format!("--prefix={}/", into.display());
    for args in [
        vec!["read-tree", treeish],
        vec!["checkout-index", "--all", "-f", &prefix],
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
    Ok(())
}

/// Point the symlink `link` at `target`. A symlink into some unit's snapshot is replaced; any
/// other existing path (a tracked `.claude/skills` or `.agents/skills`, or a symlink git
/// tracks) is left alone and the returned text says why. A tracked `.agents/skills` is the live,
/// editable copy of the repo's skills, so a harness that reads only `.agents/skills` runs
/// unpinned there; the snapshot holds the base commit's copy of it.
fn link_to(link: &Path, target: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(link) {
        Ok(m) if m.is_symlink() => {
            if !points_into_a_snapshot(&fs::read_link(link)?) {
                return Ok(Some("is a symlink that is not the unit's snapshot".into()));
            }
            fs::remove_file(link)?;
        }
        Ok(_) if link.ends_with(".agents/skills") => {
            return Ok(Some(
                "is a tracked directory, the live copy in the worktree, not the snapshot".into(),
            ))
        }
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
pub(crate) fn points_into_a_snapshot(target: &Path) -> bool {
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
    fn the_snapshot_is_the_third_component_from_the_end_under_ns() {
        for (target, want) in [
            ("../.ns/u1/skills", true),
            ("/abs/.ns/u/skills", true),
            (".ns/u1/skills", true),
            ("../.ns/u1/other", false),
            ("../x/u1/skills", false),
            ("../.ns/skills", false),
            ("/.ns/skills", false),
            ("skills", false),
            ("../tracked", false),
        ] {
            assert_eq!(points_into_a_snapshot(Path::new(target)), want, "{target}");
        }
    }

    #[test]
    fn modes_name_the_harnesses_that_read_the_snapshot() {
        assert_eq!(mode("claude"), Mode::Snapshot);
        assert_eq!(mode("codex"), Mode::Partial);
        assert_eq!(mode("cursor-agent"), Mode::None);
        assert_eq!(mode("gemini"), Mode::None);
        assert_eq!(mode("opencode"), Mode::None);
    }

    #[test]
    fn settings_args_name_the_file_and_the_project_sources() {
        assert_eq!(
            settings_args(&settings_path(Path::new("/c"), "u"), Path::new("/wt"), "u"),
            [
                "--setting-sources",
                "project,local",
                "--settings",
                "/c/ns/units/u/phase-settings.json",
                "--add-dir",
                "/wt/.ns/u/skills"
            ]
        );
    }

    #[test]
    fn apply_appends_the_pin_and_refuses_a_command_that_carries_an_owned_flag() {
        let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (settings, wt) = (Path::new("/c/s.json"), Path::new("/wt"));
        let mut ok = argv(&["claude", "-p"]);
        apply(&mut ok, settings, wt, "u", "phase build").unwrap();
        let mut want = argv(&["claude", "-p"]);
        want.extend(settings_args(settings, wt, "u"));
        assert_eq!(ok, want);
        for flag in ["--settings=x", "--setting-sources"] {
            let mut bad = argv(&["claude", flag]);
            let err = apply(&mut bad, settings, wt, "u", "phase build")
                .unwrap_err()
                .to_string();
            assert!(err.starts_with("phase build: "), "{err}");
            assert!(err.contains(flag.split('=').next().unwrap()), "{err}");
            assert_eq!(bad, argv(&["claude", flag]), "left untouched");
        }
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
        // The repo's own tracked .agents/skills stays the live, editable copy: reported, kept.
        let agents = dir.path().join(".agents/skills");
        fs::create_dir_all(&agents).unwrap();
        let why = link_to(&agents, &target).unwrap().expect("a reason");
        assert!(why.contains("tracked"), "{why}");
        assert!(!fs::symlink_metadata(&agents).unwrap().is_symlink());
    }
}
