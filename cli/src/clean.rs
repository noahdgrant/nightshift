//! Cleanup: a unit's worktree and local `ns/<unit>` branch go once its change merged and its
//! quality record is saved (docs/FACTORY.md, Cleanup). `ns run` cleans after its own merge,
//! `ns watch` at start and between units, and `ns clean` on demand.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::clock::Clock;
use crate::git::{self, Repo};
use crate::quality::{self, Meta};
use crate::run::{self, currency::known_pr, gh_json, holder_pid, run_lock, Lock, WORKTREE_LOCK};
use crate::worktree::BRANCH_PREFIX;

/// A unit worktree whose issue is closed and whose PR merged.
#[derive(Debug, Clone)]
pub struct Merged {
    pub unit: String,
    pub path: PathBuf,
    pub issue: Option<u64>,
    pub pr: u64,
    /// The merged PR's head commit, which must hold everything on the local branch.
    pub pr_head: String,
}

impl Merged {
    fn branch(&self) -> String {
        format!("{BRANCH_PREFIX}{}", self.unit)
    }

    fn json(&self) -> Value {
        json!({
            "unit": self.unit,
            "path": self.path.to_string_lossy(),
            "branch": self.branch(),
            "issue": self.issue,
            "pr": self.pr,
        })
    }
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

/// Every `ns/<unit>` worktree, sorted into merged (issue closed, and the PR from `pr.md`
/// merged, closes it and comes from the unit's branch) and kept. A unit whose run lock a live
/// `ns run` holds is kept too. Nothing changes.
pub fn plan(repo: &Repo) -> Result<Plan> {
    let mut plan = Plan::default();
    for e in git::worktrees(&repo.root)? {
        let Some(unit) = e
            .branch
            .as_deref()
            .and_then(|b| b.strip_prefix("refs/heads/"))
            .and_then(|b| b.strip_prefix(BRANCH_PREFIX))
        else {
            continue;
        };
        match merged(repo, unit, &e.path) {
            Ok(m) => match held(repo, unit).or_else(|| local_reason(repo, &m)) {
                None => plan.merged.push(m),
                Some(r) => plan.kept.push(kept(unit, &e.path, r)),
            },
            Err(r) => plan.kept.push(kept(unit, &e.path, r)),
        }
    }
    Ok(plan)
}

/// The unit as [`Merged`], or why it is not.
fn merged(repo: &Repo, unit: &str, path: &Path) -> Result<Merged, String> {
    let Some(issue) = quality::issue_of(unit) else {
        return Err("the unit id names no issue".into());
    };
    let v = gh_json(
        &repo.root,
        &["issue", "view", &issue.to_string(), "--json", "state"],
    )
    .map_err(|e| format!("cannot read issue #{issue}: {e:#}"))?;
    if v["state"].as_str() != Some("CLOSED") {
        return Err(format!("issue #{issue} is open"));
    }
    let Some(pr) = known_pr(&path.join(".ns").join(unit)) else {
        return Err(format!("no PR in .ns/{unit}/pr.md"));
    };
    let v = pr_view(path, pr, "state,headRefOid,headRefName,body")?;
    if v["state"].as_str() != Some("MERGED") {
        return Err(format!(
            "PR #{pr} is {}",
            v["state"].as_str().unwrap_or("unknown")
        ));
    }
    if !crate::watch::closed_by(v["body"].as_str().unwrap_or("")).contains(&issue) {
        return Err(format!("PR #{pr} does not close #{issue}"));
    }
    let branch = format!("{BRANCH_PREFIX}{unit}");
    let head_ref = v["headRefName"].as_str().unwrap_or("");
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
        pr_head: v["headRefOid"].as_str().unwrap_or("").to_string(),
    })
}

/// `gh pr view` from the unit's worktree, as the merge step runs it.
fn pr_view(path: &Path, pr: u64, fields: &str) -> Result<Value, String> {
    gh_json(path, &["pr", "view", &pr.to_string(), "--json", fields])
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
    // Commits made on a detached HEAD or another branch would be lost with the worktree.
    let branch_ref = format!("refs/heads/{}", m.branch());
    let tip = match git::run(&m.path, &["symbolic-ref", "--quiet", "HEAD"]) {
        Ok(h) if h == branch_ref => git::run(&m.path, &["rev-parse", "HEAD"]).unwrap_or_default(),
        _ => return Some(format!("its HEAD is not on {}", m.branch())),
    };
    let base = repo.default_base().unwrap_or_else(|_| "main".into());
    if !holds(&m.path, &base, &tip, &m.pr_head, m.pr) {
        let head = if m.pr_head.is_empty() {
            "unknown"
        } else {
            &m.pr_head
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

/// Whether the merged PR head `pr_head` holds everything at `tip`: it is `tip`, descends from it
/// (`update-branch` merged the base in), or carries the same change against `base`, whitespace
/// included (a rebase). A head missing here is fetched from `refs/pull/<pr>/head` first.
fn holds(dir: &Path, base: &str, tip: &str, pr_head: &str, pr: u64) -> bool {
    if pr_head.is_empty() {
        return false;
    }
    if !is_commit(dir, pr_head) {
        let pull = format!("refs/pull/{pr}/head");
        let _ = git::net(dir, &["fetch", "--quiet", "--no-tags", "origin", &pull])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
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

/// Remove the worktree, then the branch, under the worktree lock: `git branch -D` edits
/// `.git/config`, as a `git worktree add` does. Errors name what is left.
fn remove(repo: &Repo, m: &Merged) -> Result<(), String> {
    let path = m.path.to_string_lossy().into_owned();
    // --force: untracked and ignored files (build output, .ns/) go with it; tracked changes were
    // checked just before.
    git::run(&repo.root, &["worktree", "remove", "--force", &path])
        .map_err(|e| format!("worktree not removed: {e:#}"))?;
    let kept = |e: anyhow::Error| format!("worktree removed, branch {} kept: {e:#}", m.branch());
    let me = json!({"pid": std::process::id(), "unit": m.unit});
    let _config = Lock::wait(&repo.common_dir, WORKTREE_LOCK, me, None, |by| {
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

/// Take the unit's run lock for cleanup `by`, without waiting: the lock, or why the unit stays.
fn take_run_lock(repo: &Repo, m: &Merged, by: &str) -> Result<Lock, String> {
    let me = json!({"pid": std::process::id(), "unit": m.unit, "issue": m.issue, "by": by});
    let clock = Clock::from_env();
    let now = clock.now();
    match Lock::wait(
        &repo.common_dir,
        &run_lock(&m.unit),
        me,
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
        pr_head: String::new(),
    };
    let o = if !record_saved {
        Outcome::Kept("its quality record is not saved".into())
    } else {
        // The merge step saw it MERGED; only its head is needed.
        match pr_view(path, pr, "headRefOid") {
            Err(e) => Outcome::Kept(e),
            Ok(v) => {
                m.pr_head = v["headRefOid"].as_str().unwrap_or("").to_string();
                checked_remove(repo, &m)
            }
        }
    };
    report(repo, "run", &m, &o);
    o.json(&m)
}

fn report(repo: &Repo, by: &str, m: &Merged, o: &Outcome) {
    match o {
        Outcome::Removed => eprintln!(
            "ns {by}: removed {} and branch {} (PR #{} merged)",
            m.path.display(),
            m.branch(),
            m.pr
        ),
        Outcome::Kept(r) | Outcome::Failed(r) => {
            eprintln!("ns {by}: kept {}: {r}", m.path.display())
        }
    }
    log(repo, by, m, o);
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
        match take_run_lock(repo, m, by) {
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
    let metas: Vec<Meta> = merged
        .iter()
        .map(|m| Meta {
            unit: m.unit.clone(),
            issue: m.issue,
            pr: Some(m.pr),
            outcome: Some("merged".into()),
        })
        .collect();
    let units: Vec<(&Path, &Meta)> = merged
        .iter()
        .zip(&metas)
        .map(|(m, meta)| (m.path.as_path(), meta))
        .collect();
    let (ev, unsaved) = quality::record_units(repo, &units);
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
    fn a_merged_head_holds_the_same_commit_a_descendant_or_a_rebase_of_it() {
        let r = rebased_unit();
        let d = &r.dir;
        // Same commit.
        assert!(holds(d, "main", &r.rebased, &r.rebased, 1));
        // A rebase: the same change against main.
        assert!(holds(d, "main", &r.rebased, &r.reviewed, 1));
        assert!(holds(d, "main", &r.reviewed, &r.rebased, 1));
        // A descendant: update-branch merged main into the PR.
        g(d, &["checkout", "-qb", "pr", &r.rebased]);
        let later = commit_file(d, "MERGE", "m\n");
        assert!(holds(d, "main", &r.rebased, &later, 1));
        // The tip has a change the PR doesn't.
        g(d, &["checkout", "-q", "unit"]);
        let extra = commit_file(d, "extra.txt", "more\n");
        assert!(!holds(d, "main", &extra, &r.rebased, 1));
        assert!(!holds(d, "main", &extra, &later, 1));
        // Two commits with no change against main to compare share no change.
        g(d, &["checkout", "-q", "--orphan", "lone"]);
        let lone = commit_file(d, "lone.txt", "x\n");
        let on_main = g(d, &["rev-parse", "main"]);
        assert!(!holds(d, "main", &lone, &on_main, 1));
        // Nothing known.
        assert!(!holds(d, "main", &extra, "", 1));
        assert!(!holds(d, "main", "", &extra, 1));
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
        assert!(!holds(d, "main", &local, &merged, 1));
    }

    #[test]
    fn a_pr_head_that_is_not_here_and_cannot_be_fetched_holds_nothing() {
        let r = rebased_unit();
        let missing = "0123456789abcdef0123456789abcdef01234567";
        assert!(!holds(&r.dir, "main", &r.rebased, missing, 1));
        assert!(!holds(&r.dir, "main", &r.rebased, "--all", 1));
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
        assert!(holds(&r.dir, "main", &r.rebased, &merged_in, 5));
    }
}
