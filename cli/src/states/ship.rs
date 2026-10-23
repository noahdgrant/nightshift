use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};
use serde_json::json;

use super::Context;
use crate::artifact;
use crate::checks;
use crate::forge::{Checks, DISCLOSURE};
use crate::labels;
use crate::lock;
use crate::manifest;
use crate::merge::Verdict;
use crate::runlog::RunLog;
use crate::state::Outcome;
use crate::vcs::{Rebase, Vcs};

/// The lock that lets one unit at a time merge into the base branch.
const MERGE_LOCK: &str = "merge.lock";

pub struct Options {
    /// Merge the change request once its checks pass.
    pub merge: bool,
    /// How often to look at the checks.
    pub poll: Duration,
    /// How long to wait for the checks before the state goes wrong.
    pub timeout: Duration,
    /// How long to wait for the first check to be reported. A repository with no checks has
    /// none to wait for after that.
    pub start: Duration,
    /// Bring the branch up to date with the base branch before it is pushed, and merge under a
    /// lock only a head that was tested on the base branch's newest commit.
    pub queue: bool,
    /// How many times the base branch may move while one change request is merged before the
    /// unit is stuck.
    pub max_base_moves: u32,
}

/// Publishes the unit's branch, opens a change request for it, waits for its checks and, when
/// asked, merges it. With `queue`, the branch is first rebased onto the base branch when that has
/// moved and the repository's checks run on the result. A unit that is to be merged takes the
/// merge lock before that and holds it until it is merged or leaves `ship`, so the units that
/// merge go one at a time, each onto a base branch that holds the one before. The lock does not
/// keep out a merge from outside `ns`: the base branch is fetched again before the merge, and
/// the unit goes round again if it moved. The address of the change request is kept as `ship.md`.
/// The work is already committed: `build` is the only state that commits.
///
/// Failed checks find the build wrong: the evidence names them, ends in a failing verdict, and the
/// unit goes back to `build`.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let workspace = ctx
        .workspace
        .as_ref()
        .context("ship ran before setup made a workspace")?;
    let (dir, branch) = (workspace.path.clone(), workspace.branch.clone());
    let id = ctx.unit.to_string();
    let root = ctx.vcs.root_find(&ctx.from)?;
    // Whether the unit is to be merged, and so is queued behind the others that are.
    let human = if ctx.ship.merge {
        let base = ctx.vcs.workspace_base_find(&dir)?;
        let files = ctx.vcs.workspace_files_changed(&dir, &base)?;
        match manifest::human_review_read(&root)?.verdict(&files)? {
            Verdict::NeedsHuman { file, glob } => Some((file, glob)),
            _ => None,
        }
    } else {
        None
    };
    // Held from before the rebase until this function returns, whichever way it does.
    let _lock = if ctx.ship.queue && ctx.ship.merge && human.is_none() {
        Some(lock::lock_wait(
            &ctx.vcs.state_dir_find(&root)?,
            MERGE_LOCK,
        )?)
    } else {
        None
    };
    let mut moves = 0;
    loop {
        if ctx.ship.queue {
            if let Some(tip) = ctx.vcs.base_fetch(&root)? {
                if refresh(ctx, &root, &dir, &id, &tip)? == Refresh::Failed {
                    return Ok(Outcome::Fail);
                }
            }
        }
        let number = publish(ctx, &dir, &branch, &id)?;
        if let Some(outcome) = checks_wait(ctx, &dir, number, &id)? {
            return Ok(outcome);
        }
        if !ctx.ship.merge {
            return Ok(Outcome::Pass);
        }
        if let Some((file, glob)) = &human {
            ctx.forge
                .change_request_label_add(&dir, number, labels::NEEDS_HUMAN_MERGE)?;
            println!(
                "{}",
                json!({
                    "unit": id,
                    "state": "ship",
                    "not_merged": format!("{file} matches {glob} in merge.human_review, so a person merges it"),
                })
            );
            return Ok(Outcome::Pass);
        }
        if ctx.ship.queue && base_moved(ctx, &root, &dir)? {
            moves += 1;
            if moves > ctx.ship.max_base_moves {
                println!(
                    "{}",
                    json!({
                        "unit": id,
                        "state": "ship",
                        "not_merged": format!("the base branch kept moving: it moved {moves} times while the change request was being merged"),
                    })
                );
                return Ok(Outcome::BaseMoved);
            }
            continue;
        }
        let head = ctx.vcs.workspace_head_find(&dir)?;
        ctx.forge.change_request_merge(&dir, number, &head)?;
        return Ok(Outcome::Pass);
    }
}

/// What bringing a branch up to date with the base branch did.
#[derive(Debug, PartialEq, Eq)]
pub enum Refresh {
    /// The branch already held the base branch's newest commit, so nothing ran.
    Current,
    /// The branch was rebased onto the base branch and the repository's checks passed on it.
    Refreshed,
    /// The rebase conflicted or the checks failed, and the evidence says so: the unit must go
    /// back to `build`.
    Failed,
}

/// Brings the branch up to date with the base branch, whose newest commit is `tip`, when the
/// branch lacks it, and runs the repository's checks on the result. Neither `verify` nor `review`
/// runs again after a clean rebase, for the unit's own change is as it was.
fn refresh(ctx: &Context, root: &Path, dir: &Path, id: &str, tip: &str) -> Result<Refresh> {
    if ctx.vcs.workspace_contains(dir, tip)? {
        // A run stopped between the rebase and the move of the recorded base leaves it stale.
        ctx.vcs.workspace_base_set(dir, tip)?;
        return Ok(Refresh::Current);
    }
    if let Rebase::Conflict(files) = ctx.vcs.workspace_rebase(dir, tip)? {
        // The build resolves the conflict by rebasing onto `tip`, so it is judged against it.
        ctx.vcs.workspace_base_set(dir, tip)?;
        let evidence = format!(
            "The base branch moved and the unit's commit conflicts with it in: {}. Rebase onto \
             the base branch and resolve them.\n{}\n",
            files.join(", "),
            super::verdict::FAIL_LINE
        );
        artifact::write(dir, id, "evidence.md", &evidence)?;
        return Ok(Refresh::Failed);
    }
    ctx.vcs.workspace_base_set(dir, tip)?;
    let report = checks::run(
        ctx.vcs.as_ref(),
        ctx.os.as_ref(),
        root,
        dir,
        false,
        ctx.checks_timeout,
    )?;
    if let Some(report) = report.filter(|r| !r.passed) {
        let evidence = format!(
            "The base branch moved, and the checks failed on the unit rebased onto it:\n{}\nFix them.\n{}\n",
            checks::tail(&report.output, 40),
            super::verdict::FAIL_LINE
        );
        artifact::write(dir, id, "evidence.md", &evidence)?;
        return Ok(Refresh::Failed);
    }
    Ok(Refresh::Refreshed)
}

/// The base branch's newest commit when the open change request of the unit in `dir` lacks it and
/// no refresh has already tested it, and `None` when the change request is current.
pub fn base_behind(
    vcs: &dyn Vcs,
    log: &RunLog,
    root: &Path,
    dir: &Path,
    unit: &str,
) -> Result<Option<String>> {
    let Some(tip) = vcs.base_fetch(root)? else {
        return Ok(None);
    };
    if log.base_tested(unit)?.as_deref() == Some(tip.as_str())
        || vcs.workspace_contains(dir, &tip)?
    {
        return Ok(None);
    }
    Ok(Some(tip))
}

/// Brings the change request of a unit that is done, and still open, up to date with the base
/// branch, whose newest commit is `tip`: the refresh `run` makes, then the push and the wait for
/// the change request's checks. `verify` and `review` do not run again.
pub fn open_refresh(ctx: &Context, tip: &str) -> Result<Refresh> {
    let workspace = ctx
        .workspace
        .as_ref()
        .context("a refresh ran before there was a workspace")?;
    let (dir, branch) = (workspace.path.clone(), workspace.branch.clone());
    let id = ctx.unit.to_string();
    let root = ctx.vcs.root_find(&ctx.from)?;
    match refresh(ctx, &root, &dir, &id, tip)? {
        Refresh::Refreshed => {}
        other => return Ok(other),
    }
    let number = publish(ctx, &dir, &branch, &id)?;
    Ok(match checks_wait(ctx, &dir, number, &id)? {
        Some(_) => Refresh::Failed,
        None => Refresh::Refreshed,
    })
}

/// Whether the base branch has a commit the branch about to be merged lacks.
fn base_moved(ctx: &Context, root: &Path, dir: &Path) -> Result<bool> {
    match ctx.vcs.base_fetch(root)? {
        Some(tip) => Ok(!ctx.vcs.workspace_contains(dir, &tip)?),
        None => Ok(false),
    }
}

/// Pushes the branch and returns the number of its change request, opened if it has none.
fn publish(ctx: &Context, dir: &Path, branch: &str, id: &str) -> Result<u64> {
    let unit = ctx.forge.unit_get(dir, ctx.unit)?;
    let build = artifact::read(dir, id, "build.md")?;
    ctx.vcs.branch_push(dir, branch)?;
    let change_request = match ctx.forge.change_request_get(dir, branch)? {
        Some(existing) => existing,
        None => {
            let body = format!("{DISCLOSURE}\n\n{}", build.trim_end());
            // `status:` labels say where the unit is, so only the unit carries them.
            let labels: Vec<String> = unit
                .labels
                .iter()
                .filter(|l| {
                    ["type:", "area:", "priority:"]
                        .iter()
                        .any(|p| l.starts_with(p))
                })
                .cloned()
                .collect();
            ctx.forge
                .change_request_create(dir, branch, ctx.unit, &unit.title, &body, &labels)?
        }
    };
    // The assignee is for people to read; a unit does not depend on it.
    if let Err(e) = ctx
        .forge
        .change_request_assignee_add(dir, change_request.number)
    {
        eprintln!("ns: cannot assign the change request: {e:#}");
    }
    artifact::write(dir, id, "ship.md", &format!("{}\n", change_request.url))?;
    Ok(change_request.number)
}

/// Waits for the checks on change request `number`. `Some` when they failed and the unit must go
/// back to `build`.
fn checks_wait(ctx: &Context, dir: &Path, number: u64, id: &str) -> Result<Option<Outcome>> {
    let started = Instant::now();
    loop {
        match ctx.forge.change_request_checks_get(dir, number)? {
            Checks::Passed => return Ok(None),
            Checks::Failed(names) => {
                let evidence = format!(
                    "The checks on the change request failed: {}. Fix them.\n{}\n",
                    names.join(", "),
                    super::verdict::FAIL_LINE
                );
                artifact::write(dir, id, "evidence.md", &evidence)?;
                return Ok(Some(Outcome::Fail));
            }
            Checks::NoneReported if started.elapsed() >= ctx.ship.start => return Ok(None),
            Checks::NoneReported | Checks::Pending if started.elapsed() >= ctx.ship.timeout => {
                bail!("the checks on the change request did not finish in time")
            }
            Checks::NoneReported | Checks::Pending => std::thread::sleep(ctx.ship.poll),
        }
    }
}
