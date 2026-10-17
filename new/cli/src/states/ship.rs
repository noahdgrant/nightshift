use anyhow::{Context as _, Result};

use super::Context;
use crate::artifact;
use crate::state::Outcome;

/// Every post `ns` makes on the forge starts with this line.
const DISCLOSURE: &str = "🤖 *Posted by nightshift, an AI agent.*";

/// Commits the unit's work, publishes its branch and opens a change request for it, whose
/// address it keeps as `ship.md`. A change request that is already open is kept.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let workspace = ctx
        .workspace
        .as_ref()
        .context("ship ran before setup made a workspace")?;
    let (dir, branch) = (workspace.path.clone(), workspace.branch.clone());
    let id = ctx.unit.to_string();
    let unit = ctx.forge.unit_get(&dir, ctx.unit)?;
    let build = artifact::read(&dir, &id, "build.md")?;
    ctx.vcs.workspace_commit(&dir, &unit.title)?;
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
    Ok(Outcome::Pass)
}
