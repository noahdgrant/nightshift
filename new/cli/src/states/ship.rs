use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};

use super::Context;
use crate::artifact;
use crate::forge::Checks;
use crate::state::Outcome;

/// Every post `ns` makes on the forge starts with this line.
const DISCLOSURE: &str = "🤖 *Posted by nightshift, an AI agent.*";

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
            ctx.forge
                .change_request_create(&dir, &branch, ctx.unit, &unit.title, &body)?
        }
    };
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
        ctx.forge
            .change_request_merge(&dir, change_request.number)?;
    }
    Ok(Outcome::Pass)
}
