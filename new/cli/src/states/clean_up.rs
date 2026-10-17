use anyhow::{bail, Result};

use super::Context;
use crate::forge::ChangeState;
use crate::state::Outcome;
use crate::workspace;

/// Once the unit's change request has merged, removes its workspace and its branch. A change
/// request still open is the human's to merge, so its workspace stays.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let root = ctx.vcs.root_find(&ctx.from)?;
    let unit = ctx.unit.to_string();
    let wanted = workspace::locate(&root, &unit)?;
    match ctx.forge.change_request_get(&root, &wanted.branch)? {
        None => bail!("the unit has no change request to clean up after"),
        Some(cr) => match cr.state {
            ChangeState::Open => return Ok(Outcome::Pass),
            ChangeState::Closed => bail!("the change request was closed without merging"),
            ChangeState::Merged => {}
        },
    }
    workspace::remove(ctx.vcs.as_ref(), &ctx.from, &unit)?;
    ctx.workspace = None;
    ctx.vcs.branch_delete(&root, &wanted.branch)?;
    Ok(Outcome::Pass)
}
