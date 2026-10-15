//! Cleanup: a unit's worktree, local `ns/<unit>` branch and the `TMPDIR`s its failed phases
//! kept go once its change merged and its quality record is saved (docs/factory/unit-lifecycle.md,
//! Cleanup). `ns run` cleans after its own merge, `ns watch` at start and between units, and
//! `ns clean` on demand.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::clock::Clock;
use crate::factory;
use crate::git::{self, Repo};
use crate::quality::{self, Meta};
use crate::run::{self, currency::known_pr, holder_pid, run_lock, Lock, WORKTREE_LOCK};
use crate::watch::forge::{self, Forge, IssueState, Pr, PrStatus};
use crate::worktree::{self, BRANCH_PREFIX};

/// A unit worktree whose issue is closed and whose PR merged.
#[derive(Debug, Clone)]
pub struct Merged {
    pub unit: String,
    pub path: PathBuf,
    pub issue: Option<u64>,
    pub pr: u64,
    /// The merged PR's head, which must hold everything on the local branch.
    pub pr_head: PrHead,
    /// The default branch, when the worktree was found on it rather than on its own branch.
    pub on_default: Option<String>,
}

impl Merged {
    fn branch(&self) -> String {
        format!("{BRANCH_PREFIX}{}", self.unit)
    }

    fn json(&self) -> Value {
        let mut v = json!({
            "unit": self.unit,
            "path": self.path.to_string_lossy(),
            "branch": self.branch(),
            "issue": self.issue,
            "pr": self.pr,
        });
        if let Some(d) = &self.on_default {
            v["detached"] = json!(d);
        }
        v
    }
}

/// A merged PR's head commit, empty when unknown, and the remote ref it is fetched from when it
/// isn't here. One value, so the forge that names the commit names the ref too.
#[derive(Debug, Clone, Default)]
pub struct PrHead {
    pub sha: String,
    pub fetch_ref: String,
}

fn kept(unit: &str, path: &Path, reason: impl Into<String>) -> Value {
    json!({"unit": unit, "path": path.to_string_lossy(), "reason": reason.into()})
}

/// What a sweep found: the units that can go and, with a reason each, those that stay.
#[derive(Debug, Default)]
pub struct Plan {
    pub merged: Vec<Merged>,
    pub kept: Vec<Value>,
}

/// Every `ns/<unit>` worktree, and every unit worktree left on the default branch, sorted into
/// merged (issue closed, and the PR from `pr.md` merged, closes it and comes from the unit's
/// branch) and kept. A unit whose run lock a live `ns run` holds is kept too. Nothing changes.
pub fn plan(repo: &Repo) -> Result<Plan> {
    let mut plan = Plan::default();
    let default = repo.default_base().ok();
    let queue = factory::load(&factory::root(None, &repo.root))
        .map(|f| f.queue)
        .unwrap_or_default();
    for e in worktree::entries(repo)? {
        let Some(branch) = e
            .branch
            .as_deref()
            .and_then(|b| b.strip_prefix("refs/heads/"))
        else {
            continue;
        };
        let (unit, on_default) = match branch.strip_prefix(BRANCH_PREFIX) {
            Some(unit) => (unit, None),
            None if default.as_deref() == Some(branch) => match unit_at(repo, &e.path) {
                Some(unit) => (unit, Some(branch.to_string())),
                None => continue,
            },
            None => continue,
        };
        match merged(&queue, unit, &e.path, on_default) {
            Ok(m) => match held(repo, unit).or_else(|| local_reason(repo, &m)) {
                None => plan.merged.push(m),
                Some(r) => plan.kept.push(kept(unit, &e.path, r)),
            },
            Err(r) => plan.kept.push(kept(unit, &e.path, r)),
        }
    }
    Ok(plan)
}

/// The unit whose worktree `path` is, from where `ns worktree new` puts it.
fn unit_at<'a>(repo: &Repo, path: &'a Path) -> Option<&'a str> {
    let unit = path.file_name()?.to_str()?;
    (worktree::planned_path(repo, unit) == path).then_some(unit)
}

/// The unit as [`Merged`], or why it is not.
fn merged(
    queue: &factory::Queue,
    unit: &str,
    path: &Path,
    on_default: Option<String>,
) -> Result<Merged, String> {
    let Some(issue) = quality::issue_of(unit) else {
        return Err("the unit id names no issue".into());
    };
    let forge = open(queue, path)?;
    let state = forge
        .state(issue)
        .map_err(|e| format!("cannot read issue #{issue}: {e:#}"))?;
    if state == IssueState::Open {
        return Err(format!("issue #{issue} is open"));
    }
    let Some(pr) = known_pr(&path.join(".ns").join(unit)) else {
        return Err(format!("no PR in .ns/{unit}/pr.md"));
    };
    let v = pr_view(forge.as_ref(), pr)?;
    if v.status != PrStatus::Merged {
        return Err(format!("PR #{pr} is {}", v.status.name()));
    }
    if !crate::watch::closed_by(&v.body).contains(&issue) {
        return Err(format!("PR #{pr} does not close #{issue}"));
    }
    let branch = format!("{BRANCH_PREFIX}{unit}");
    let head_ref = v.head_branch;
    if head_ref != branch {
        return Err(format!(
            "PR #{pr} is from branch {head_ref:?}, not {branch}"
        ));
    }
    Ok(Merged {
        unit: unit.to_string(),
        path: path.to_path_buf(),
        issue: Some(issue),
        pr,
        pr_head: PrHead {
            sha: v.head_sha,
            fetch_ref: forge.pr_ref(pr),
        },
        on_default,
    })
}

/// The forge, run from the unit's worktree as the merge step runs it.
fn open(queue: &factory::Queue, path: &Path) -> Result<Box<dyn Forge>, String> {
    forge::open(queue, path).map_err(|e| format!("cannot open the forge: {e:#}"))
}

fn pr_view(forge: &dyn Forge, pr: u64) -> Result<Pr, String> {
    forge
        .pr(pr)
        .map_err(|e| format!("cannot read PR #{pr}: {e:#}"))
}

/// Why the worktree must stay, from what is on disk: the main checkout, the current directory,
/// changes to tracked files, or commits the merged PR doesn't hold. `None` when it can go.
fn local_reason(repo: &Repo, m: &Merged) -> Option<String> {
    if m.path == repo.root {
        return Some("the main checkout is on its branch".into());
    }
    if std::env::current_dir().is_ok_and(|d| d.starts_with(&m.path)) {
        return Some("the current directory is inside it".into());
    }
    match git::run(&m.path, &["status", "--porcelain", "--untracked-files=no"]) {
        Err(e) => return Some(format!("cannot read its status: {e:#}")),
        Ok(s) if !s.trim().is_empty() => {
            return Some(format!(
                "uncommitted changes to tracked files ({} paths)",
                s.lines().count()
            ))
        }
        Ok(_) => {}
    }
    if let Some(d) = &m.on_default {
        // Its commits are on the default branch, which stays.
        let on = git::run(&m.path, &["symbolic-ref", "--quiet", "HEAD"]).ok();
        return (on != Some(format!("refs/heads/{d}"))).then(|| format!("its HEAD is not on {d}"));
    }
    // Commits made on a detached HEAD or another branch would be lost with the worktree.
    let branch_ref = format!("refs/heads/{}", m.branch());
    let tip = match git::run(&m.path, &["symbolic-ref", "--quiet", "HEAD"]) {
        Ok(h) if h == branch_ref => git::run(&m.path, &["rev-parse", "HEAD"]).unwrap_or_default(),
        _ => return Some(format!("its HEAD is not on {}", m.branch())),
    };
    let base = repo.default_base().unwrap_or_else(|_| "main".into());
    if !holds(&m.path, &base, &tip, &m.pr_head) {
        let head = if m.pr_head.sha.is_empty() {
            "unknown"
        } else {
            &m.pr_head.sha
        };
        return Some(format!(
            "{} at {tip} has changes merged PR #{} (head {head}) does not hold",
            m.branch(),
            m.pr
        ));
    }
    None
}

fn is_commit(dir: &Path, sha: &str) -> bool {
    !sha.starts_with('-') && git::ok(dir, &["cat-file", "-e", &format!("{sha}^{{commit}}")])
}

/// Whether the merged PR head holds everything at `tip`: it is `tip`, descends from it
/// (`update-branch` merged the base in), or carries the same change against `base`, whitespace
/// included (a rebase). A head missing here is fetched from its ref first.
fn holds(dir: &Path, base: &str, tip: &str, head: &PrHead) -> bool {
    let pr_head = head.sha.as_str();
    if pr_head.is_empty() {
        return false;
    }
    if !is_commit(dir, pr_head) {
        let fetch = ["fetch", "--quiet", "--no-tags", "origin", &head.fetch_ref];
        let _ = git::net(dir, &fetch).output();
        if !is_commit(dir, pr_head) {
            return false;
        }
    }
    if git::ok(dir, &["merge-base", "--is-ancestor", tip, pr_head]) {
        return true;
    }
    let id = git::exact_diff_id(dir, base, tip);
    id.is_some() && id == git::exact_diff_id(dir, base, pr_head)
}

/// Remove the `TMPDIR`s its failed phases kept, then the worktree and its branch. Errors name what
/// is left.
fn remove(repo: &Repo, m: &Merged) -> Result<(), String> {
    let path = m.path.to_string_lossy().into_owned();
    if m.on_default.is_some() {
        // Detach first, so the default branch is free even if the removal fails.
        git::run(&m.path, &["checkout", "--quiet", "--detach"])
            .map_err(|e| format!("worktree not detached: {e:#}"))?;
    }
    // The TMPDIRs go first: once the worktree or branch is gone the unit is no longer found, and
    // a later step failing would leave them for good.
    let tmp = repo.unit_tmp(&m.unit);
    let tmp_kept = remove_unit_tmp(&tmp).err();
    // --force: untracked and ignored files (build output, .ns/) go with it; tracked changes were
    // checked just before.
    git::run(&repo.root, &["worktree", "remove", "--force", &path])
        .map_err(|e| format!("worktree not removed: {e:#}"))?;
    if m.on_default.is_none() {
        delete_branch(repo, m)?;
    }
    match tmp_kept {
        Some(e) => Err(format!("worktree removed, {} kept: {e}", tmp.display())),
        None => Ok(()),
    }
}

/// Remove a unit's phase `TMPDIR`s; a directory already gone is removed.
fn remove_unit_tmp(tmp: &Path) -> Result<(), std::io::Error> {
    match std::fs::remove_dir_all(tmp) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Delete the unit's branch under the worktree lock: `git branch -D` edits `.git/config`, as a
/// `git worktree add` does.
fn delete_branch(repo: &Repo, m: &Merged) -> Result<(), String> {
    let kept = |e: anyhow::Error| format!("worktree removed, branch {} kept: {e:#}", m.branch());
    let me = json!({"pid": std::process::id(), "unit": m.unit});
    let _config = Lock::wait(&repo.common_dir, WORKTREE_LOCK, &me, None, |by| {
        eprintln!(
            "ns: {} cleanup waits for the worktree lock (held by pid {}, unit {})",
            m.unit,
            holder_pid(by),
            by["unit"].as_str().unwrap_or("?")
        )
    })
    .map_err(kept)?;
    git::run(&repo.root, &["branch", "-D", &m.branch()]).map_err(kept)?;
    Ok(())
}

/// What one removal attempt came to.
pub enum Outcome {
    Removed,
    Kept(String),
    Failed(String),
}

impl Outcome {
    fn json(&self, m: &Merged) -> Value {
        let mut v = m.json();
        match self {
            Outcome::Removed => v["removed"] = json!(true),
            Outcome::Kept(r) => {
                v["removed"] = json!(false);
                v["reason"] = json!(r);
            }
            Outcome::Failed(r) => {
                v["removed"] = json!(false);
                v["reason"] = json!(r);
                v["error"] = json!(true);
            }
        }
        v
    }
}

/// Check again and remove. The caller holds the unit's run lock, so no `ns run` starts on it.
fn checked_remove(repo: &Repo, m: &Merged) -> Outcome {
    if let Some(r) = local_reason(repo, m) {
        return Outcome::Kept(r);
    }
    match remove(repo, m) {
        Ok(()) => Outcome::Removed,
        Err(e) => Outcome::Failed(e),
    }
}

/// Why a unit stays while `holder` holds its run lock: a live `ns run`, or another cleanup.
fn holder_reason(holder: &Value) -> String {
    match holder["by"].as_str() {
        Some(by) => format!("ns {by} (pid {}) is cleaning it up", holder_pid(holder)),
        None => format!(
            "a live ns run (pid {}) holds its run lock",
            holder_pid(holder)
        ),
    }
}

/// Why `unit` stays while its run lock is held, or `None` when it is free.
fn held(repo: &Repo, unit: &str) -> Option<String> {
    let holders = Lock::run_holders(&repo.common_dir).ok()?;
    let h = holders.iter().find(|h| h["unit"].as_str() == Some(unit))?;
    Some(holder_reason(h))
}

/// Take `unit`'s run lock for cleanup `by`, without waiting: the lock, or why the unit stays.
fn take_run_lock(repo: &Repo, unit: &str, issue: Option<u64>, by: &str) -> Result<Lock, String> {
    let me = json!({"pid": std::process::id(), "unit": unit, "issue": issue, "by": by});
    let clock = Clock::from_env();
    let now = clock.now();
    match Lock::wait(
        &repo.common_dir,
        &run_lock(unit),
        &me,
        Some((&clock, now)),
        |_| {},
    ) {
        Ok(Ok(lock)) => Ok(lock),
        Ok(Err(holder)) => Err(holder_reason(&holder)),
        Err(e) => Err(format!("cannot take its run lock: {e:#}")),
    }
}

fn log(repo: &Repo, by: &str, m: &Merged, o: &Outcome) {
    let mut ev = o.json(m);
    ev["event"] = json!("cleanup");
    ev["by"] = json!(by);
    run::log_event(&repo.common_dir, ev);
}

/// After `ns run`'s merge step merged `pr`: remove the unit's worktree and branch once its
/// quality record is saved (`record_saved`) and the worktree holds nothing the PR didn't.
/// Returns the run-log event's body.
pub fn after_merge(
    repo: &Repo,
    queue: &factory::Queue,
    unit: &str,
    path: &Path,
    issue: Option<u64>,
    pr: u64,
    record_saved: bool,
) -> Value {
    let mut m = Merged {
        unit: unit.to_string(),
        path: path.to_path_buf(),
        issue,
        pr,
        pr_head: PrHead::default(),
        on_default: None,
    };
    let o = if !record_saved {
        Outcome::Kept("its quality record is not saved".into())
    } else {
        // The merge step saw it MERGED; only its head is needed.
        let read = open(queue, path).and_then(|f| {
            Ok(PrHead {
                sha: pr_view(f.as_ref(), pr)?.head_sha,
                fetch_ref: f.pr_ref(pr),
            })
        });
        match read {
            Err(e) => Outcome::Kept(e),
            Ok(head) => {
                m.pr_head = head;
                checked_remove(repo, &m)
            }
        }
    };
    report(repo, "run", &m, &o);
    o.json(&m)
}

fn report(repo: &Repo, by: &str, m: &Merged, o: &Outcome) {
    match o {
        Outcome::Removed => match &m.on_default {
            Some(d) => eprintln!(
                "ns {by}: detached {} from {d} and removed it (PR #{} merged)",
                m.path.display(),
                m.pr
            ),
            None => eprintln!(
                "ns {by}: removed {} and branch {} (PR #{} merged)",
                m.path.display(),
                m.branch(),
                m.pr
            ),
        },
        Outcome::Kept(r) | Outcome::Failed(r) => {
            eprintln!("ns {by}: kept {}: {r}", m.path.display())
        }
    }
    log(repo, by, m, o);
}

/// The `base:` the unit's brief names, when it has one.
fn brief_base(worktree: &Path, unit: &str) -> Option<String> {
    let text = fs::read_to_string(worktree.join(".ns").join(unit).join("brief.md")).ok()?;
    text.lines()
        .skip(1)
        .take_while(|l| l.trim_end() != "---")
        .find_map(|l| l.strip_prefix("base:"))
        .map(|b| b.trim().trim_matches('"').to_string())
        .filter(|b| !b.is_empty())
}

/// Save the records of every merged unit in one write, then remove each whose record is saved.
/// Returns each unit's result.
fn sweep(repo: &Repo, by: &str, merged: &[Merged]) -> (Value, Vec<(Merged, Outcome)>) {
    // Each unit's run lock is taken before it is recorded and held until it is removed, so a
    // unit a run holds is neither recorded nor removed.
    let mut done = Vec::new();
    let mut locks = Vec::new();
    let mut ours = Vec::new();
    for m in merged {
        match take_run_lock(repo, &m.unit, m.issue, by) {
            Ok(lock) => {
                locks.push(lock);
                ours.push(m.clone());
            }
            Err(r) => {
                let o = Outcome::Kept(r);
                report(repo, by, m, &o);
                done.push((m.clone(), o));
            }
        }
    }
    let merged = ours;
    if merged.is_empty() {
        return (Value::Null, done);
    }
    // This write replaces the record `ns run` wrote when the unit ended, so it detects against
    // the base the unit's brief names, as `ns run` did. The conflicts' events were logged then;
    // only a failed check is logged here, which the record cannot carry.
    let default = repo.default_base().unwrap_or_else(|_| "main".into());
    let metas: Vec<Meta> = merged
        .iter()
        .map(|m| {
            let base = brief_base(&m.path, &m.unit).unwrap_or_else(|| default.clone());
            let found = quality::conflicts::detect(&m.path, &base, "HEAD");
            for ev in found.events {
                if ev["event"] == "conflict_check_failed" {
                    run::log_event(&repo.common_dir, ev);
                }
            }
            Meta {
                unit: m.unit.clone(),
                issue: m.issue,
                pr: Some(m.pr),
                outcome: Some("merged".into()),
                conflicts: found.conflicts,
            }
        })
        .collect();
    let units: Vec<(&Path, &Meta)> = merged
        .iter()
        .zip(&metas)
        .map(|(m, meta)| (m.path.as_path(), meta))
        .collect();
    let (ev, unsaved) = quality::record_units(
        repo,
        &quality::Where::of(&factory::root(None, &repo.root)),
        &units,
    );
    run::log_event(&repo.common_dir, ev.clone());
    for ((m, unsaved), lock) in merged.iter().zip(unsaved).zip(locks) {
        let o = match unsaved {
            Some(e) => Outcome::Kept(format!("its quality record is not saved: {e}")),
            None => checked_remove(repo, m),
        };
        drop(lock);
        report(repo, by, m, &o);
        done.push((m.clone(), o));
    }
    (ev, done)
}

/// `ns watch`'s cleanup at start and between units: remove what merged since, and never end
/// the night over it. A unit in `tried` was tried tonight and is left for the next night, so
/// one that stays is recorded once. Returns the units removed.
/// `busy` names the issues of units running or paused tonight: their worktrees stay, whatever
/// their PR, and they are tried again once they end.
pub fn between_units(
    repo: &Repo,
    tried: &mut BTreeSet<String>,
    busy: &BTreeSet<u64>,
) -> Vec<String> {
    let plan = match plan(repo) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ns watch: cleanup skipped: {e:#}");
            return Vec::new();
        }
    };
    let merged: Vec<Merged> = plan
        .merged
        .into_iter()
        .filter(|m| !m.issue.is_some_and(|i| busy.contains(&i)))
        .filter(|m| tried.insert(m.unit.clone()))
        .collect();
    sweep(repo, "watch", &merged)
        .1
        .into_iter()
        .filter(|(_, o)| matches!(o, Outcome::Removed))
        .map(|(m, _)| m.unit)
        .collect()
}

/// Delete the build output of each worktree in `kept`, under its unit's run lock, and add what
/// went to its entry as `build_output`. A unit a live run holds keeps its build output.
fn remove_build_output(repo: &Repo, kept: &mut [Value]) {
    for k in kept {
        let (Some(unit), Some(path)) = (k["unit"].as_str(), k["path"].as_str()) else {
            continue;
        };
        let (unit, path) = (unit.to_string(), PathBuf::from(path));
        let Ok(_lock) = take_run_lock(repo, &unit, quality::issue_of(&unit), "clean") else {
            continue;
        };
        if let Some(removed) = crate::build_output::remove(repo, &path) {
            k["build_output"] = removed.clone();
            let mut ev = removed;
            ev["event"] = json!("build_output_removed");
            ev["unit"] = json!(unit);
            ev["by"] = json!("clean");
            run::log_event(&repo.common_dir, ev);
        }
    }
}

/// `ns clean [--dry-run]`.
pub fn cli(dry_run: bool) -> Result<ExitCode> {
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let mut plan = plan(&repo)?;
    let mut out = json!({"ok": true, "command": "clean", "dry_run": dry_run});
    let mut failed = false;
    if dry_run {
        out["planned"] = json!(plan.merged.iter().map(Merged::json).collect::<Vec<_>>());
        out["status"] = json!(if plan.merged.is_empty() {
            "nothing-to-clean"
        } else {
            "planned"
        });
    } else {
        let (records, done) = sweep(&repo, "clean", &plan.merged);
        let mut removed = Vec::new();
        for (m, o) in &done {
            match o {
                Outcome::Removed => removed.push(m.json()),
                Outcome::Kept(_) => plan.kept.push(o.json(m)),
                Outcome::Failed(_) => {
                    failed = true;
                    plan.kept.push(o.json(m));
                }
            }
        }
        out["status"] = json!(if removed.is_empty() {
            "nothing-to-clean"
        } else {
            "cleaned"
        });
        out["removed"] = json!(removed);
        out["records"] = records;
        remove_build_output(&repo, &mut plan.kept);
    }
    if failed {
        out["ok"] = json!(false);
    }
    out["kept"] = json!(plan.kept);
    println!("{}", serde_json::to_string_pretty(&out)?);
    if failed {
        eprintln!(
            "error: some worktrees could not be removed (see \"kept\" entries with \"error\")"
        );
        eprintln!("  check:  git worktree list");
        eprintln!("  then:   ns clean");
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{commit_file, g, rebased_unit};

    #[test]
    fn a_briefs_base_is_read_from_its_frontmatter() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(".ns").join("u");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("brief.md"),
            "---\nunit: u\nbase: develop\n---\nbase: no\n",
        )
        .unwrap();
        assert_eq!(brief_base(t.path(), "u").as_deref(), Some("develop"));
        fs::write(dir.join("brief.md"), "---\nunit: u\n---\n").unwrap();
        assert_eq!(brief_base(t.path(), "u"), None);
        assert_eq!(brief_base(t.path(), "missing"), None);
    }

    /// Head `sha` of PR #1, fetched from its GitHub ref.
    fn at(sha: &str) -> PrHead {
        PrHead {
            sha: sha.into(),
            fetch_ref: "refs/pull/1/head".into(),
        }
    }

    /// A repo whose main checkout is on `other`, and unit `3-done`'s worktree left on `main`.
    fn left_on_main() -> (tempfile::TempDir, Repo, Merged) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("r");
        std::fs::create_dir(&root).unwrap();
        g(&root, &["init", "-q", "-b", "main"]);
        commit_file(&root, "README", "hi\n");
        g(&root, &["checkout", "-qb", "other"]);
        let repo = Repo::discover(&root).unwrap();
        let path = worktree::planned_path(&repo, "3-done");
        g(
            &root,
            &["worktree", "add", "-q", path.to_str().unwrap(), "main"],
        );
        let m = Merged {
            unit: "3-done".into(),
            path,
            issue: Some(3),
            pr: 30,
            pr_head: PrHead::default(),
            on_default: Some("main".into()),
        };
        (tmp, repo, m)
    }

    #[test]
    fn a_worktree_found_on_the_default_branch_stays_once_it_moves_off_it() {
        let (_tmp, repo, m) = left_on_main();
        assert_eq!(local_reason(&repo, &m), None);
        g(&m.path, &["checkout", "-q", "--detach"]);
        assert_eq!(
            local_reason(&repo, &m).as_deref(),
            Some("its HEAD is not on main")
        );
    }

    #[test]
    fn a_malformed_factory_file_does_not_stop_the_plan() {
        let (_tmp, repo, m) = left_on_main();
        g(&m.path, &["checkout", "-q", "-b", "ns/3-done"]);
        let dir = repo.root.join(".nightshift");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nightshift.toml"), "[queue\nsource = ").unwrap();
        assert!(factory::load(&dir).is_err());
        let plan = plan(&repo).expect("a malformed factory file must not abort the plan");
        assert!(plan.merged.is_empty());
        assert_eq!(plan.kept.len(), 1, "{:?}", plan.kept);
        assert!(plan.kept[0].to_string().contains("3-done"));
    }

    #[test]
    fn a_worktree_on_the_default_branch_that_cannot_be_detached_stays() {
        let (_tmp, repo, m) = left_on_main();
        let git_dir = g(&m.path, &["rev-parse", "--absolute-git-dir"]);
        std::fs::write(Path::new(&git_dir).join("index.lock"), "").unwrap();
        let e = remove(&repo, &m).unwrap_err();
        assert!(e.starts_with("worktree not detached: "), "{e}");
        assert!(m.path.is_dir());
        assert_eq!(g(&m.path, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    }

    #[test]
    fn a_merged_head_holds_the_same_commit_a_descendant_or_a_rebase_of_it() {
        let r = rebased_unit();
        let d = &r.dir;
        // Same commit.
        assert!(holds(d, "main", &r.rebased, &at(&r.rebased)));
        // A rebase: the same change against main.
        assert!(holds(d, "main", &r.rebased, &at(&r.reviewed)));
        assert!(holds(d, "main", &r.reviewed, &at(&r.rebased)));
        // A descendant: update-branch merged main into the PR.
        g(d, &["checkout", "-qb", "pr", &r.rebased]);
        let later = commit_file(d, "MERGE", "m\n");
        assert!(holds(d, "main", &r.rebased, &at(&later)));
        // The tip has a change the PR doesn't.
        g(d, &["checkout", "-q", "unit"]);
        let extra = commit_file(d, "extra.txt", "more\n");
        assert!(!holds(d, "main", &extra, &at(&r.rebased)));
        assert!(!holds(d, "main", &extra, &at(&later)));
        // Two commits with no change against main to compare share no change.
        g(d, &["checkout", "-q", "--orphan", "lone"]);
        let lone = commit_file(d, "lone.txt", "x\n");
        let on_main = g(d, &["rev-parse", "main"]);
        assert!(!holds(d, "main", &lone, &at(&on_main)));
        // Nothing known.
        assert!(!holds(d, "main", &extra, &at("")));
        assert!(!holds(d, "main", "", &at(&extra)));
    }

    #[test]
    fn a_change_that_differs_only_in_whitespace_is_not_held() {
        let r = rebased_unit();
        let d = &r.dir;
        g(d, &["checkout", "-qb", "merged", "main"]);
        let merged = commit_file(d, "f.py", "if y:\n    g()\nh()\n");
        g(d, &["checkout", "-qb", "local", "main"]);
        let local = commit_file(d, "f.py", "if y:\n    g()\n    h()\n");
        // Currency ignores whitespace; cleanup must not.
        assert_eq!(
            git::diff_id(d, "main", &local),
            git::diff_id(d, "main", &merged)
        );
        assert!(!holds(d, "main", &local, &at(&merged)));
    }

    #[test]
    fn a_pr_head_that_is_not_here_and_cannot_be_fetched_holds_nothing() {
        let r = rebased_unit();
        let missing = "0123456789abcdef0123456789abcdef01234567";
        assert!(!holds(&r.dir, "main", &r.rebased, &at(missing)));
        assert!(!holds(&r.dir, "main", &r.rebased, &at("--all")));
    }

    #[test]
    fn a_pr_head_missing_here_is_fetched_from_its_pull_ref() {
        let r = rebased_unit();
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        g(
            tmp.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        g(&r.dir, &["checkout", "-qb", "pr", &r.rebased]);
        let merged_in = commit_file(&r.dir, "MERGE", "m\n");
        g(
            &r.dir,
            &[
                "push",
                "-q",
                remote.to_str().unwrap(),
                "pr:refs/pull/5/head",
            ],
        );
        g(&r.dir, &["checkout", "-q", "unit"]);
        g(&r.dir, &["branch", "-D", "pr"]);
        g(&r.dir, &["reflog", "expire", "--expire=now", "--all"]);
        g(&r.dir, &["gc", "-q", "--prune=now"]);
        assert!(!is_commit(&r.dir, &merged_in));
        g(
            &r.dir,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        let head = PrHead {
            sha: merged_in,
            fetch_ref: "refs/pull/5/head".into(),
        };
        assert!(holds(&r.dir, "main", &r.rebased, &head));
    }

    const AFTER_MERGE_MARK: &str = "NS_TEST_AFTER_MERGE_FETCH_PROBE";

    /// The cleanup after ns run's merge fetches a merged head made on the forge from the ref the
    /// forge names for it.
    #[test]
    fn the_cleanup_after_a_merge_fetches_a_head_missing_here_from_the_forges_ref() {
        crate::testutil::probe(
            "clean::tests::after_merge_fetch_probe",
            AFTER_MERGE_MARK,
            "1",
        );
    }

    #[test]
    fn after_merge_fetch_probe() {
        if std::env::var_os(AFTER_MERGE_MARK).is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (root, remote) = (base.join("r"), base.join("remote.git"));
        std::fs::create_dir(&root).unwrap();
        g(&root, &["init", "-q", "-b", "main"]);
        commit_file(&root, "README", "hi\n");
        g(&base, &["init", "-q", "--bare", remote.to_str().unwrap()]);
        g(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        let repo = Repo::discover(&root).unwrap();
        let path = worktree::planned_path(&repo, "3-done");
        let wt = path.to_str().unwrap();
        g(
            &root,
            &["worktree", "add", "-q", "-b", "ns/3-done", wt, "main"],
        );
        commit_file(&path, "work.txt", "one\n");
        // The forge merged main into the PR: a head on the remote's PR ref only.
        let merged = commit_file(&path, "MERGE", "m\n");
        g(&path, &["push", "-q", "origin", "HEAD:refs/pull/30/head"]);
        g(&path, &["reset", "-q", "--hard", "HEAD~1"]);
        g(&root, &["reflog", "expire", "--expire=now", "--all"]);
        g(&root, &["gc", "-q", "--prune=now"]);
        assert!(!is_commit(&root, &merged));
        let pr = format!(
            r#"{{"state":"MERGED","headRefOid":"{merged}","headRefName":"ns/3-done","body":"Closes #3"}}"#
        );
        let _gh = crate::testutil::fake_on_path("gh", &format!("echo '{pr}'"));
        let queue = factory::Queue::default();
        let v = after_merge(&repo, &queue, "3-done", &path, Some(3), 30, true);
        assert_eq!(v["removed"], true, "{v}");
        assert!(!path.exists());
    }
}
