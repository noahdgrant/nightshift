use anyhow::{Context as _, Result};

use super::{check, Context};
use crate::artifact;
use crate::skill;
use crate::state::Outcome;

/// Has the harness review the unit's changes. It reads them and changes nothing. See
/// [`check::run`] for how it ends; a failing review sends the unit back to `build`.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let dir = ctx
        .workspace
        .as_ref()
        .context("review ran before setup made a workspace")?
        .path
        .clone();
    let unit = ctx.forge.unit_get(&dir, ctx.unit)?;
    let build = artifact::read(&dir, &ctx.unit.to_string(), "build.md")?;
    let base = ctx.vcs.workspace_base_find(&dir)?;
    let diff = ctx.vcs.workspace_diff(&dir, &base)?;
    let skill = skill::load(&ctx.vcs.root_find(&ctx.from)?, skill::REVIEW)?;
    check::run(
        ctx,
        false,
        prompt(&skill, &unit.title, &unit.body, &build, &diff),
    )
}

fn prompt(skill: &str, title: &str, body: &str, build: &str, diff: &str) -> String {
    format!(
        "{}\n\n# The unit\n\n## {title}\n\n{body}\n\n# The build summary\n\n{build}\n\n\
         # The change\n\n{diff}\n",
        skill
    )
}
