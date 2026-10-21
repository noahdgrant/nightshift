use anyhow::{Context as _, Result};

use super::verdict::{self, Verdict};
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
    let skill = skill::load(skill::REVIEW)?;
    let id = ctx.unit.to_string();
    let reviews = reviews_so_far(&dir, &id);
    let previous = failing_review(&dir, &id, reviews);
    let outcome = check::run(
        ctx,
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
    artifact::write(&dir, &id, &review_file(reviews + 1), &text)?;
    Ok(outcome)
}

/// The text of review `n`, when it ended in a failing verdict.
fn failing_review(dir: &std::path::Path, unit: &str, n: usize) -> Option<String> {
    if n == 0 {
        return None;
    }
    artifact::read(dir, unit, &review_file(n))
        .ok()
        .filter(|text| verdict::parse(text) == Some(Verdict::Fail))
}

fn review_file(n: usize) -> String {
    format!("review-{n}.md")
}

/// How many reviews the unit has had: the highest `n` with a `review-<n>.md`.
fn reviews_so_far(dir: &std::path::Path, unit: &str) -> usize {
    let mut n = 0;
    while artifact::read(dir, unit, &review_file(n + 1)).is_ok() {
        n += 1;
    }
    n
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_failing_last_review_is_carried_forward() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, unit) = (tmp.path(), "7");
        assert_eq!(reviews_so_far(dir, unit), 0);
        assert_eq!(failing_review(dir, unit, 0), None);
        artifact::write(dir, unit, "review-1.md", "bad\nVERDICT: FAIL\n").unwrap();
        assert_eq!(reviews_so_far(dir, unit), 1);
        assert_eq!(
            failing_review(dir, unit, 1).as_deref(),
            Some("bad\nVERDICT: FAIL\n")
        );
        artifact::write(dir, unit, "review-2.md", "ok\nVERDICT: PASS\n").unwrap();
        assert_eq!(reviews_so_far(dir, unit), 2);
        assert_eq!(failing_review(dir, unit, 2), None);
    }
}
