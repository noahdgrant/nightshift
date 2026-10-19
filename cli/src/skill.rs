//! The skills the states hand the harness. A state's prompt starts with its skill's text, so the
//! harness always gets the same instructions, whether or not it would have found the skill.
//!
//! The skills are in the binary, so every install runs the skills that came with it.

use anyhow::{bail, Result};

include!(concat!(env!("OUT_DIR"), "/skills.rs"));

pub const TRIAGE: &str = "ns-triage";
pub const BUILD: &str = "ns-build";
pub const TDD: &str = "ns-tdd";
pub const VERIFY: &str = "ns-verify";
pub const REVIEW: &str = "ns-review";
pub const GRILLING: &str = "ns-grilling";
pub const REQUIREMENTS: &str = "ns-requirements";
pub const DESIGN_DOC: &str = "ns-design-doc";
pub const TROUBLESHOOT: &str = "ns-troubleshoot";

/// The skills whose output the CLI parses, such as a route or a verdict line. In a session they
/// run only when the user types their name.
const STATE_ONLY: [&str; 3] = [TRIAGE, VERIFY, REVIEW];

/// The file of skill `name` as a harness's session should find it: the embedded file, and for a
/// state skill the frontmatter line that keeps the harness from starting it on its own.
pub fn file_for_session(name: &str, text: &str) -> String {
    if !STATE_ONLY.contains(&name) {
        return text.to_string();
    }
    match text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
    {
        Some((front, body)) => format!("---\n{front}\ndisable-model-invocation: true\n---\n{body}"),
        None => text.to_string(),
    }
}

/// Every skill that came with this `ns`, as its name and its file's text.
pub fn embedded() -> &'static [(&'static str, &'static str)] {
    EMBEDDED
}

/// The text of skill `name`, without its frontmatter.
pub fn load(name: &str) -> Result<String> {
    match embedded().iter().find(|(embedded, _)| *embedded == name) {
        Some((_, text)) => Ok(body(text).to_string()),
        None => bail!("there is no skill named {name}"),
    }
}

/// A skill's text without its frontmatter.
fn body(skill: &str) -> &str {
    skill
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(skill, |(_, body)| body.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_skill_in_the_skills_directory_is_embedded() {
        let dir = format!("{}/../skills", env!("CARGO_MANIFEST_DIR"));
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        let embedded: Vec<&str> = embedded().iter().map(|(name, _)| *name).collect();
        assert_eq!(embedded, on_disk);
    }

    #[test]
    fn a_skill_is_the_embedded_one() {
        assert!(load(BUILD).unwrap().starts_with("# Build"));
    }

    #[test]
    fn a_skill_nobody_wrote_is_an_error() {
        let err = load("ns-nothing").unwrap_err();
        assert!(err.to_string().contains("no skill named ns-nothing"));
    }

    #[test]
    fn only_a_state_skill_is_kept_from_starting_on_its_own_in_a_session() {
        let text = "---\nname: x\ndescription: y\n---\n# X\n";
        assert_eq!(file_for_session("ns-grilling", text), text);
        assert_eq!(
            file_for_session(TRIAGE, text),
            "---\nname: x\ndescription: y\ndisable-model-invocation: true\n---\n# X\n"
        );
    }

    #[test]
    fn every_state_skill_is_embedded_and_has_frontmatter_to_add_to() {
        for name in STATE_ONLY {
            let text = embedded().iter().find(|(n, _)| *n == name).unwrap().1;
            assert!(file_for_session(name, text).contains("\ndisable-model-invocation: true\n"));
        }
    }

    #[test]
    fn the_frontmatter_is_left_out() {
        assert_eq!(body("---\nname: x\n---\n\n# X\n\ntext\n"), "# X\n\ntext\n");
        assert_eq!(body("no frontmatter"), "no frontmatter");
    }

    #[test]
    fn the_checking_skills_ask_for_the_verdict_lines_the_cli_reads() {
        use crate::states::verdict::{FAIL_LINE, PASS_LINE};
        for name in [VERIFY, REVIEW] {
            let file = format!("{}/../skills/{name}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
            let skill = std::fs::read_to_string(file).unwrap();
            assert!(
                skill.contains(PASS_LINE) && skill.contains(FAIL_LINE),
                "{name}"
            );
        }
    }

    #[test]
    fn the_triage_skill_names_every_route_and_asks_for_the_lines_the_cli_reads() {
        let file = format!("{}/../skills/{TRIAGE}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
        let skill = std::fs::read_to_string(file).unwrap();
        assert!(skill.contains("\nLABELS: ") && skill.contains("\nROUTE: "));
        for route in crate::triage::Route::ALL {
            assert!(
                skill.contains(&format!("**{}**", route.name())),
                "{route:?}"
            );
        }
    }
}
