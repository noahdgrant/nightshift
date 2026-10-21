use anyhow::{Context as _, Result};

use super::rounds::Rounds;
use super::{check, Context};
use crate::artifact;
use crate::skill;
use crate::state::{Outcome, State};

/// Has the harness check the build against the unit's description by running it. See
/// [`check::run`] for how it ends. Every verification is kept as `verify-<n>.md`, and one after a
/// failing verification is shown that one.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let dir = ctx
        .workspace
        .as_ref()
        .context("verify ran before setup made a workspace")?
        .path
        .clone();
    let id = ctx.unit.to_string();
    let unit = ctx.forge.unit_get(&dir, ctx.unit)?;
    let build = artifact::read(&dir, &id, "build.md")?;
    let skill = skill::load(skill::VERIFY)?;
    let verifications = Rounds::new(&dir, &id, "verify");
    let previous = verifications.last_failing();
    let outcome = check::run(
        ctx,
        State::Verify,
        true,
        prompt(&skill, &unit.title, &unit.body, &build, previous.as_deref()),
    )?;
    let text = artifact::read(&dir, &id, "evidence.md")?;
    verifications.keep(&text)?;
    Ok(outcome)
}

fn prompt(skill: &str, title: &str, body: &str, build: &str, previous: Option<&str>) -> String {
    let previous = previous
        .map(|text| format!("\n# Findings from the previous verification\n\n{text}"))
        .unwrap_or_default();
    format!(
        "{}\n\n# The unit\n\n## {title}\n\n{body}\n\n# The build summary\n\n{build}\n{previous}",
        skill
    )
}
