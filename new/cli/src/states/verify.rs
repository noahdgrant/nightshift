use anyhow::{Context as _, Result};

use super::verdict::{FAIL_LINE, PASS_LINE};
use super::{check, Context};
use crate::artifact;
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
        "Check that the work described below is done, by running it the way a user would, not \
         by reading the code. Do not change any source file. For each requirement, say what you \
         ran and what you saw. End with a last line that is exactly `{PASS_LINE}` or \
         `{FAIL_LINE}`.\n\n# {title}\n\n{body}\n\n# What the build says it did\n\n{build}\n"
    )
}
