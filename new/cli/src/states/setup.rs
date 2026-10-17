use anyhow::Result;

use super::Context;
use crate::state::Outcome;
use crate::workspace;

/// Creates the unit's workspace.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    ctx.workspace = Some(workspace::ensure(
        ctx.vcs.as_ref(),
        &ctx.from,
        &ctx.unit.to_string(),
        None,
    )?);
    Ok(Outcome::Pass)
}
