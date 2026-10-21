use anyhow::{Context as _, Result};

use super::rounds::Rounds;
use super::{check, Context};
use crate::artifact;
use crate::skill;
use crate::state::{Outcome, State};

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
    let skill = skill::load(skill::REVIEW)?;
    let id = ctx.unit.to_string();
    let reviews = Rounds::new(&dir, &id, "review");
    let previous = reviews.last_failing();
    let outcome = check::run(
        ctx,
        State::Review,
        false,
        prompt(
            &skill,
            &unit.title,
            &unit.body,
            &build,
            &diff,
            previous.as_deref(),
        ),
    )?;
    let text = artifact::read(&dir, &id, "evidence.md")?;
    reviews.keep(&text)?;
    Ok(outcome)
}

fn prompt(
    skill: &str,
    title: &str,
    body: &str,
    build: &str,
    diff: &str,
    previous: Option<&str>,
) -> String {
    let previous = previous
        .map(|text| format!("# Findings from the previous review\n\n{text}\n\n"))
        .unwrap_or_default();
    format!(
        "{}\n\n# The unit\n\n## {title}\n\n{body}\n\n# The build summary\n\n{build}\n\n\
         {previous}# The change\n\n{diff}\n",
        skill
    )
}
