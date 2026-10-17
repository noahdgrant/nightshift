use anyhow::{Context as _, Result};

use super::verdict::{FAIL_LINE, PASS_LINE};
use super::{check, Context};
use crate::artifact;
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
    check::run(ctx, false, prompt(&unit.title, &unit.body, &build, &diff))
}

fn prompt(title: &str, body: &str, build: &str, diff: &str) -> String {
    format!(
        "You are reviewing a change. Find what must be fixed before it merges: bugs, missing or \
         weak tests, and anything that does not do what the unit asks. Leave out style and \
         preferences. For each finding, give the file, what is wrong and why it matters. End with \
         a last line that is exactly `{PASS_LINE}` when nothing must be fixed, or `{FAIL_LINE}` \
         when something must.\n\n# {title}\n\n{body}\n\n# What the build says it did\n\n{build}\n\n\
         # The change\n\n{diff}\n"
    )
}
