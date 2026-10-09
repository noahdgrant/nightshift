//! `ns worktree new|list|remove`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::SfError;
use crate::frontmatter;
use crate::git::{self, Repo, WorktreeEntry};

pub const BRANCH_PREFIX: &str = "ns/";

/// A unit id is `[a-z0-9][a-z0-9-]*`.
pub fn valid_unit_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn check_unit_id(id: &str, example: &str) -> Result<()> {
    if valid_unit_id(id) {
        return Ok(());
    }
    Err(SfError::usage(
        format!("invalid unit id {id:?}: use lowercase letters, digits and dashes, starting with a letter or digit"),
        format!("unit ids look like <issue-number>-<slug> or <slug>, e.g.\n  {example}"),
    )
    .into())
}

#[derive(Serialize)]
pub struct NewOutput {
    pub unit: String,
    pub path: String,
    pub branch: String,
    pub artifacts: String,
    pub setup: Vec<SetupResult>,
}

#[derive(Debug, Serialize)]
pub struct SetupResult {
    pub run: String,
    pub exit: Option<i32>,
}

fn load_setup(root: &Path) -> Result<Vec<String>> {
    Ok(crate::factory::load(&root.join(".nightshift"))?
        .worktree
        .setup)
}

/// Run each setup command with `sh -c` in the worktree; stop at the first failure.
/// Command output goes to stderr so stdout stays JSON.
fn run_setup(
    commands: &[String],
    unit: &str,
    path: &Path,
    root: &Path,
) -> Result<Vec<SetupResult>> {
    let mut results = Vec::new();
    for c in commands {
        let status = Command::new("sh")
            .arg("-c")
            .arg(c)
            .current_dir(path)
            .env("NS_UNIT", unit)
            .env("NS_WORKTREE", path)
            .env("NS_MAIN_ROOT", root)
            .stdin(Stdio::null())
            .stdout(std::io::stderr())
            .stderr(std::io::stderr())
            .status()
            .with_context(|| format!("cannot run setup command {c:?}"))?;
        let ok = status.success();
        results.push(SetupResult {
            run: c.clone(),
            exit: status.code(),
        });
        if !ok {
            break;
        }
    }
    Ok(results)
}

fn setup_failed(results: &[SetupResult], unit: &str) -> Option<ExitCode> {
    let failed = results.iter().find(|r| r.exit != Some(0))?;
    let code = failed
        .exit
        .map(|c| c.to_string())
        .unwrap_or_else(|| "a signal".into());
    eprintln!(
        "error: setup command {:?} exited with {code}; the worktree is left in place\n  fix the cause, then retry:\n  ns worktree setup {unit}",
        failed.run
    );
    Some(ExitCode::from(1))
}

fn repo_from(repo: Option<&Path>) -> Result<Repo> {
    let start = match repo {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("cannot read current directory")?,
    };
    Repo::discover(&start)
}

fn find_unit<'a>(list: &'a [WorktreeEntry], unit: &str) -> Option<&'a WorktreeEntry> {
    let want = format!("refs/heads/{BRANCH_PREFIX}{unit}");
    list.iter()
        .find(|e| e.branch.as_deref() == Some(want.as_str()))
}

/// Append `.ns/` to the common `info/exclude` unless an equivalent line exists.
fn ensure_excluded(common_dir: &Path) -> Result<()> {
    let info = common_dir.join("info");
    fs::create_dir_all(&info).with_context(|| format!("cannot create {}", info.display()))?;
    let exclude = info.join("exclude");
    let current = fs::read_to_string(&exclude).unwrap_or_default();
    let present = current
        .lines()
        .map(str::trim)
        .any(|l| l == ".ns/" || l == ".ns" || l == "/.ns/" || l == "/.ns");
    if present {
        return Ok(());
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(".ns/\n");
    fs::write(&exclude, text).with_context(|| format!("cannot write {}", exclude.display()))
}

pub fn new(
    unit: &str,
    base: Option<&str>,
    repo: Option<&Path>,
    no_setup: bool,
) -> Result<ExitCode> {
    let example = "ns worktree new 142-uart-timeout --base main";
    check_unit_id(unit, example)?;
    let repo = repo_from(repo)?;
    let setup = if no_setup {
        Vec::new()
    } else {
        load_setup(&repo.root)?
    };
    let out = ensure(&repo, unit, base, &setup)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(setup_failed(&out.setup, unit).unwrap_or(ExitCode::SUCCESS))
}

/// Where `ensure` puts a unit's worktree when it creates one.
pub fn planned_path(repo: &Repo, unit: &str) -> PathBuf {
    let parent = repo.root.parent().unwrap_or(&repo.root);
    parent.join(format!("{}.worktrees", repo.name())).join(unit)
}

/// The existing worktree for `unit`, if any.
pub fn existing(repo: &Repo, unit: &str) -> Result<Option<PathBuf>> {
    let list = git::worktrees(&repo.root)?;
    Ok(find_unit(&list, unit).map(|e| e.path.clone()))
}

/// Create or reuse the unit's worktree; run `setup` only when it is newly created.
pub fn ensure(repo: &Repo, unit: &str, base: Option<&str>, setup: &[String]) -> Result<NewOutput> {
    let branch = format!("{BRANCH_PREFIX}{unit}");
    let list = git::worktrees(&repo.root)?;

    let mut created = false;
    let path: PathBuf = if let Some(existing) = find_unit(&list, unit) {
        existing.path.clone()
    } else {
        created = true;
        let path = planned_path(repo, unit);
        if path.exists() {
            return Err(SfError::general(format!(
                "{} exists but is not the worktree for branch {branch}",
                path.display()
            ))
            .hint(
                "move or delete that directory, then re-run:\n  ns worktree new ".to_string()
                    + unit,
            )
            .into());
        }
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).with_context(|| format!("cannot create {}", p.display()))?;
        }
        let path_str = path.to_string_lossy().into_owned();
        let branch_exists = git::ok(
            &repo.root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        );
        if branch_exists {
            git::run(&repo.root, &["worktree", "add", &path_str, &branch])?;
        } else {
            let base = match base {
                Some(b) => {
                    if !git::ok(
                        &repo.root,
                        &[
                            "rev-parse",
                            "--verify",
                            "--quiet",
                            &format!("{b}^{{commit}}"),
                        ],
                    ) {
                        return Err(SfError::usage(
                            format!("base {b:?} does not name a commit in {}", repo.root.display()),
                            format!("ns worktree new {unit} --base main\n  list branches: git branch -a"),
                        )
                        .into());
                    }
                    b.to_string()
                }
                None => repo.default_base()?,
            };
            git::run(
                &repo.root,
                &["worktree", "add", "-b", &branch, &path_str, &base],
            )?;
        }
        path
    };

    let artifacts = path.join(".ns").join(unit);
    fs::create_dir_all(&artifacts)
        .with_context(|| format!("cannot create {}", artifacts.display()))?;
    ensure_excluded(&repo.common_dir)?;

    let results = if created {
        run_setup(setup, unit, &path, &repo.root)?
    } else {
        Vec::new()
    };
    Ok(NewOutput {
        unit: unit.to_string(),
        path: path.to_string_lossy().into_owned(),
        branch,
        artifacts: artifacts.to_string_lossy().into_owned(),
        setup: results,
    })
}

/// `ns worktree setup <unit>`: re-run the `[worktree] setup` commands in an existing worktree.
pub fn setup(unit: &str, repo: Option<&Path>) -> Result<ExitCode> {
    check_unit_id(unit, "ns worktree setup 142-uart-timeout")?;
    let repo = repo_from(repo)?;
    let commands = load_setup(&repo.root)?;
    let list = git::worktrees(&repo.root)?;
    let Some(entry) = find_unit(&list, unit) else {
        return Err(SfError::general(format!("no worktree for unit {unit}"))
            .hint(format!("create it first:\n  ns worktree new {unit}"))
            .into());
    };
    let results = run_setup(&commands, unit, &entry.path, &repo.root)?;
    let out = json!({
        "unit": unit,
        "path": entry.path.to_string_lossy(),
        "setup": results,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(setup_failed(&results, unit).unwrap_or(ExitCode::SUCCESS))
}

const PHASE_ORDER: &[&str] = &[
    "brief", "define", "plan", "build", "verify", "review", "ship",
];

fn phase_rank(phase: &str) -> usize {
    PHASE_ORDER
        .iter()
        .position(|p| *p == phase)
        .map(|i| i + 1)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArtifactStatus {
    pub file: String,
    pub phase: Option<String>,
    pub status: Option<String>,
    pub updated: Option<String>,
}

/// Read every `*.md` artifact's frontmatter and pick the latest phase:
/// newest `updated` first, then the later phase in the pipeline.
pub fn latest_status(artifacts: &Path) -> Option<ArtifactStatus> {
    let entries = fs::read_dir(artifacts).ok()?;
    let mut found: Vec<ArtifactStatus> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&p) else {
            continue;
        };
        let Ok(Some(fm)) = frontmatter::parse(&text) else {
            continue;
        };
        let get = |k: &str| fm.get(k).and_then(yaml_scalar);
        let status = get("status");
        let phase = get("phase");
        if status.is_none() && phase.is_none() {
            continue;
        }
        found.push(ArtifactStatus {
            file: p.file_name().unwrap().to_string_lossy().into_owned(),
            phase,
            status,
            updated: get("updated"),
        });
    }
    found.into_iter().max_by(|a, b| {
        a.updated
            .cmp(&b.updated)
            .then_with(|| {
                phase_rank(a.phase.as_deref().unwrap_or(""))
                    .cmp(&phase_rank(b.phase.as_deref().unwrap_or("")))
            })
            .then_with(|| a.file.cmp(&b.file))
    })
}

pub fn yaml_scalar(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub fn list(repo: Option<&Path>) -> Result<()> {
    let repo = repo_from(repo)?;
    let mut out: Vec<Value> = Vec::new();
    for e in git::worktrees(&repo.root)? {
        let Some(branch) = e
            .branch
            .as_deref()
            .and_then(|b| b.strip_prefix("refs/heads/"))
        else {
            continue;
        };
        let Some(unit) = branch.strip_prefix(BRANCH_PREFIX) else {
            continue;
        };
        let artifacts = e.path.join(".ns").join(unit);
        out.push(json!({
            "unit": unit,
            "path": e.path.to_string_lossy(),
            "branch": branch,
            "artifacts": artifacts.to_string_lossy(),
            "status": latest_status(&artifacts),
        }));
    }
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

pub fn remove(unit: &str, dry_run: bool, force: bool, repo: Option<&Path>) -> Result<()> {
    check_unit_id(unit, "ns worktree remove 142-uart-timeout --dry-run")?;
    let repo = repo_from(repo)?;
    let list = git::worktrees(&repo.root)?;
    let branch = format!("{BRANCH_PREFIX}{unit}");
    let Some(entry) = find_unit(&list, unit) else {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "unit": unit,
                "branch": branch,
                "action": "none",
                "reason": "no worktree for this unit",
                "dry_run": dry_run,
            }))?
        );
        return Ok(());
    };
    if entry.path == repo.root {
        return Err(SfError::general(format!(
            "branch {branch} is checked out in the main worktree; refusing to remove it"
        ))
        .hint("switch the main worktree to another branch first: git switch main")
        .into());
    }
    let path = entry.path.clone();
    let status = git::run(&path, &["status", "--porcelain"])?;
    let dirty: Vec<&str> = status.lines().collect();
    let is_dirty = !dirty.is_empty();
    if is_dirty && !force {
        return Err(SfError::general(format!(
            "worktree {} has uncommitted changes ({} paths); refusing to remove it",
            path.display(),
            dirty.len()
        ))
        .hint(format!(
            "commit or stash the changes, or discard them with:\n  ns worktree remove {unit} --force"
        ))
        .into());
    }
    let path_str = path.to_string_lossy().into_owned();
    let result = json!({
        "unit": unit,
        "path": path_str,
        "branch": branch,
        "action": "remove",
        "dirty": is_dirty,
        "branch_kept": true,
        "dry_run": dry_run,
    });
    if !dry_run {
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&path_str);
        git::run(&repo.root, &args)?;
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_id_validation() {
        for ok in ["142-uart-timeout", "uart", "1", "a-b-c", "0x10"] {
            assert!(valid_unit_id(ok), "{ok}");
        }
        for bad in ["", "-x", "Upper", "a_b", "a/b", "a b", "ü", "a.b"] {
            assert!(!valid_unit_id(bad), "{bad}");
        }
    }

    #[test]
    fn latest_status_prefers_newest_then_later_phase() {
        let dir = tempfile::tempdir().unwrap();
        let w = |name: &str, body: &str| fs::write(dir.path().join(name), body).unwrap();
        w(
            "build.md",
            "---\nunit: u\nphase: build\nstatus: pass\nupdated: 2026-10-08T10:00:00Z\n---\n",
        );
        w(
            "verify.md",
            "---\nunit: u\nphase: verify\nstatus: fail\nupdated: 2026-10-08T11:00:00Z\n---\n",
        );
        w("notes.md", "no frontmatter\n");
        let s = latest_status(dir.path()).unwrap();
        assert_eq!(s.phase.as_deref(), Some("verify"));
        assert_eq!(s.status.as_deref(), Some("fail"));

        w(
            "review.md",
            "---\nphase: review\nstatus: pass\nupdated: 2026-10-08T11:00:00Z\n---\n",
        );
        assert_eq!(
            latest_status(dir.path()).unwrap().phase.as_deref(),
            Some("review")
        );
    }

    #[test]
    fn latest_status_none_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(latest_status(dir.path()).is_none());
        assert!(latest_status(&dir.path().join("missing")).is_none());
    }
}
