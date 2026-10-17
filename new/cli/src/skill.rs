//! The skills the states hand the harness. A state's prompt starts with its skill's text, so the
//! harness always gets the same instructions, whether or not it would have found the skill.

pub const BUILD: &str = include_str!("../../skills/ns-build/SKILL.md");
pub const VERIFY: &str = include_str!("../../skills/ns-verify/SKILL.md");
pub const REVIEW: &str = include_str!("../../skills/ns-review/SKILL.md");

/// A skill's text without its frontmatter.
pub fn body(skill: &str) -> &str {
    skill
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(skill, |(_, body)| body.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frontmatter_is_left_out() {
        assert_eq!(body("---\nname: x\n---\n\n# X\n\ntext\n"), "# X\n\ntext\n");
        assert_eq!(body("no frontmatter"), "no frontmatter");
    }

    #[test]
    fn every_skill_has_a_body() {
        for skill in [BUILD, VERIFY, REVIEW] {
            assert!(body(skill).starts_with("# "), "{}", &skill[..40]);
        }
    }

    #[test]
    fn the_checking_skills_ask_for_the_verdict_lines_the_cli_reads() {
        use crate::states::verdict::{FAIL_LINE, PASS_LINE};
        for skill in [VERIFY, REVIEW] {
            assert!(skill.contains(PASS_LINE) && skill.contains(FAIL_LINE));
        }
    }
}
