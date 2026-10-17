use anyhow::{Context as _, Result};

use super::{check, Context};
use crate::artifact;
use crate::skill;
use crate::state::Outcome;

/// Has the harness check the build against the unit's description by running it. See
/// [`check::run`] for how it ends.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let dir = ctx
        .workspace
        .as_ref()
        .context("verify ran before setup made a workspace")?
        .path
        .clone();
    let unit = ctx.forge.unit_get(&dir, ctx.unit)?;
    let build = artifact::read(&dir, &ctx.unit.to_string(), "build.md")?;
    check::run(ctx, true, prompt(&unit.title, &unit.body, &build))
}

fn prompt(title: &str, body: &str, build: &str) -> String {
    format!(
        "{}\n\n# The unit\n\n## {title}\n\n{body}\n\n# The build summary\n\n{build}\n",
        skill::body(skill::VERIFY)
    )
}
