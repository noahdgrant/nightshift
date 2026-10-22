use anyhow::{Context as _, Result};

use std::path::Path;

use super::rounds::Rounds;
use super::verdict::{self, Verdict, INCONCLUSIVE_LINE};
use super::{check, Context};
use crate::artifact;
use crate::skill;
use crate::state::{Outcome, State};

/// Has the harness check the build against the unit's description by running it. See
/// [`check::run`] for how it ends. Every verification is kept as `verify-<n>.md`, and one after a
/// failing or inconclusive verification is shown that one. A verification that is inconclusive
/// right after an inconclusive one leaves the unit unprovable, for a human.
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
    let previous = verifications.last_unmet();
    let outcome = check::run(
        ctx,
        State::Verify,
        true,
        prompt(&skill, &unit.title, &unit.body, &build, previous.as_deref()),
    )?;
    let text = artifact::read(&dir, &id, "evidence.md")?;
    verifications.keep(&text)?;
    let again = previous
        .as_deref()
        .is_some_and(|p| verdict::parse(p) == Some(Verdict::Inconclusive));
    Ok(match outcome {
        Outcome::Inconclusive if again => Outcome::Unprovable,
        other => other,
    })
}

/// What a human needs to unstick a unit verify could not prove: the sections of the last
/// verification that are inconclusive, each with what would make it checkable. The whole
/// verification when none can be told apart.
pub fn unproven_note(dir: &Path, unit: &str) -> Result<String> {
    let verifications = Rounds::new(dir, unit, "verify");
    let n = verifications.so_far();
    let text = artifact::read(dir, unit, &format!("verify-{n}.md"))?;
    let mut sections: Vec<Vec<&str>> = Vec::new();
    for line in text.lines().filter(|l| l.trim() != INCONCLUSIVE_LINE) {
        if line.starts_with('#') || sections.is_empty() {
            sections.push(Vec::new());
        }
        sections
            .last_mut()
            .expect("a section was just pushed")
            .push(line);
    }
    let unproven: Vec<String> = sections
        .iter()
        .map(|s| s.join("\n"))
        .filter(|s| s.contains("INCONCLUSIVE"))
        .collect();
    let body = if unproven.is_empty() {
        text.trim().to_string()
    } else {
        unproven.join("\n\n")
    };
    Ok(format!(
        "Verification could not prove these requirements, twice, so this unit waits for a human. \
         Each says what would make it checkable.\n\n{body}\n"
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_keeps_only_the_unproven_sections() {
        let tmp = tempfile::tempdir().unwrap();
        Rounds::new(tmp.path(), "7", "verify")
            .keep("### R1\nPASS\n\n### R2\nINCONCLUSIVE: needs staging\nVERDICT: INCONCLUSIVE\n")
            .unwrap();
        let note = unproven_note(tmp.path(), "7").unwrap();
        assert!(
            note.contains("### R2\nINCONCLUSIVE: needs staging"),
            "{note}"
        );
        assert!(!note.contains("R1"), "{note}");
        assert!(!note.contains("VERDICT"), "{note}");
    }

    #[test]
    fn the_note_holds_the_whole_verification_when_no_section_is_marked() {
        let tmp = tempfile::tempdir().unwrap();
        Rounds::new(tmp.path(), "7", "verify")
            .keep("could not run it\nVERDICT: INCONCLUSIVE\n")
            .unwrap();
        let note = unproven_note(tmp.path(), "7").unwrap();
        assert!(note.contains("could not run it"), "{note}");
    }
}
