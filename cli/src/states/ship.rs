use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};
use serde_json::json;

use super::Context;
use crate::artifact;
use crate::forge::{Checks, DISCLOSURE};
use crate::labels;
use crate::manifest;
use crate::merge::Verdict;
use crate::state::Outcome;

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
}

/// Publishes the unit's branch, opens a change request for it, waits for its checks and, when
/// asked, merges it. The address of the change request is kept as `ship.md`. The work is already
/// committed: `build` is the only state that commits.
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
    let unit = ctx.forge.unit_get(&dir, ctx.unit)?;
    let build = artifact::read(&dir, &id, "build.md")?;
    ctx.vcs.branch_push(&dir, &branch)?;
    let change_request = match ctx.forge.change_request_get(&dir, &branch)? {
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
                .change_request_create(&dir, &branch, ctx.unit, &unit.title, &body, &labels)?
        }
    };
    // The assignee is for people to read; a unit does not depend on it.
    if let Err(e) = ctx
        .forge
        .change_request_assignee_add(&dir, change_request.number)
    {
        eprintln!("ns: cannot assign the change request: {e:#}");
    }
    artifact::write(&dir, &id, "ship.md", &format!("{}\n", change_request.url))?;

    let started = Instant::now();
    loop {
        match ctx
            .forge
            .change_request_checks_get(&dir, change_request.number)?
        {
            Checks::Passed => break,
            Checks::Failed(names) => {
                let evidence = format!(
                    "The checks on the change request failed: {}. Fix them.\n{}\n",
                    names.join(", "),
                    super::verdict::FAIL_LINE
                );
                artifact::write(&dir, &id, "evidence.md", &evidence)?;
                return Ok(Outcome::Fail);
            }
            Checks::NoneReported if started.elapsed() >= ctx.ship.start => break,
            Checks::NoneReported | Checks::Pending if started.elapsed() >= ctx.ship.timeout => {
                bail!("the checks on the change request did not finish in time")
            }
            Checks::NoneReported | Checks::Pending => std::thread::sleep(ctx.ship.poll),
        }
    }
    if ctx.ship.merge {
        let root = ctx.vcs.root_find(&ctx.from)?;
        let base = ctx.vcs.workspace_base_find(&dir)?;
        let files = ctx.vcs.workspace_files_changed(&dir, &base)?;
        match manifest::human_review_read(&root)?.verdict(&files)? {
            Verdict::Merge => ctx
                .forge
                .change_request_merge(&dir, change_request.number)?,
            Verdict::NeedsHuman { file, glob } => {
                ctx.forge.change_request_label_add(
                    &dir,
                    change_request.number,
                    labels::NEEDS_HUMAN_MERGE,
                )?;
                println!(
                    "{}",
                    json!({
                        "unit": id,
                        "state": "ship",
                        "not_merged": format!("{file} matches {glob} in merge.human_review, so a person merges it"),
                    })
                );
            }
        }
    }
    Ok(Outcome::Pass)
}
