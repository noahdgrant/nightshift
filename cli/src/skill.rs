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

/// The coding standards that hold in any language, which a repository's own standards point to.
/// It is committed, at this path from the repository root, and `ns sync` keeps it current.
pub const STANDARDS_PATH: &str = ".nightshift/coding-standards.md";
pub const STANDARDS: &str = include_str!("../../standards/ns-coding-standards.md");

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
    fn the_verify_skill_reads_the_how_to_run_page() {
        assert!(load(VERIFY).unwrap().contains("docs/agents/how-to-run.md"));
    }

    #[test]
    fn the_review_skill_reads_the_review_page_and_never_leaves_out_a_critical_finding() {
        let text = load(REVIEW).unwrap();
        assert!(text.contains("docs/agents/review.md"));
        assert!(text.contains("except a Critical finding"));
    }

    #[test]
    fn the_frontmatter_is_left_out() {
        assert_eq!(body("---\nname: x\n---\n\n# X\n\ntext\n"), "# X\n\ntext\n");
        assert_eq!(body("no frontmatter"), "no frontmatter");
    }

    #[test]
    fn the_checking_skills_ask_for_the_verdict_lines_the_cli_reads() {
        use crate::states::verdict::{FAIL_LINE, INCONCLUSIVE_LINE, PASS_LINE};
        for name in [VERIFY, REVIEW] {
            let file = format!("{}/../skills/{name}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
            let skill = std::fs::read_to_string(file).unwrap();
            assert!(
                skill.contains(PASS_LINE) && skill.contains(FAIL_LINE),
                "{name}"
            );
            assert_eq!(skill.contains(INCONCLUSIVE_LINE), name == VERIFY, "{name}");
        }
    }

    #[test]
    fn the_verify_skill_keeps_a_copys_build_output_out_of_the_workspace() {
        let skill = load(VERIFY).unwrap();
        assert!(skill.contains("keep the build's output outside the workspace too"));
        assert!(skill.contains("Never point a build's output directory"));
        assert!(skill.contains("`CARGO_TARGET_DIR`"));
        assert!(skill.contains("breaks the workspace's next build"));
    }

    #[test]
    fn the_verify_skill_reruns_the_earlier_failed_requirements_and_trusts_no_earlier_pass() {
        let file = format!("{}/../skills/{VERIFY}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
        let skill = std::fs::read_to_string(file).unwrap();
        assert!(skill.contains("Findings from the previous verification"));
        assert!(skill.contains("re-run each requirement that was FAIL or INCONCLUSIVE"));
        assert!(skill.contains("An earlier PASS is not carried forward"));
    }

    #[test]
    fn the_review_skill_names_the_three_statuses() {
        let file = format!("{}/../skills/{REVIEW}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
        let skill = std::fs::read_to_string(file).unwrap();
        for status in ["**Fixed**", "**Still open**", "**Withdrawn**"] {
            assert!(skill.contains(status), "{status}");
        }
    }

    #[test]
    fn the_service_design_skill_is_short_and_reference_only() {
        let text = embedded()
            .iter()
            .find(|(n, _)| *n == "ns-service-design")
            .unwrap()
            .1;
        assert!(text.lines().count() <= 60);
        assert!(!file_for_session("ns-service-design", text).contains("disable-model-invocation"));
        assert!(text.contains(".nightshift/coding-standards.md"));
    }

    #[test]
    fn the_ci_design_skill_is_short_may_start_on_its_own_and_states_each_rule() {
        let text = embedded()
            .iter()
            .find(|(n, _)| *n == "ns-ci-design")
            .unwrap()
            .1;
        assert!(text.lines().count() <= 60);
        assert!(!file_for_session("ns-ci-design", text).contains("disable-model-invocation"));
        for needed in [
            "`.nightshift/check`",
            "Conventional Commit",
            "`Closes #<unit>`",
            "no `needs`",
            "full commit SHA",
            "`ubuntu-latest`",
            "--locked",
            "`timeout-minutes`",
            "`pull_request_target`",
        ] {
            assert!(text.contains(needed), "{needed}");
        }
    }

    #[test]
    fn build_and_review_name_the_ci_design_skill() {
        for name in [BUILD, REVIEW] {
            let file = format!("{}/../skills/{name}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
            let skill = std::fs::read_to_string(file).unwrap();
            assert!(skill.contains("`ns-ci-design`"), "{name}");
        }
    }

    #[test]
    fn the_architecture_design_skill_is_short_may_start_on_its_own_and_states_each_rule() {
        let text = embedded()
            .iter()
            .find(|(n, _)| *n == "ns-architecture-design")
            .unwrap()
            .1;
        assert!(text.lines().count() <= 60);
        assert!(
            !file_for_session("ns-architecture-design", text).contains("disable-model-invocation")
        );
        for needed in [
            "## Trodden path",
            "## Industry-standard tools",
            "## Testable seams",
            "## Harness-friendly layout",
            "One way per need",
            "**Assumptions**",
            "A new dependency",
            "A **seam**",
            "`.nightshift/check`",
            "Fix at the highest layer",
        ] {
            assert!(text.contains(needed), "{needed}");
        }
    }

    #[test]
    fn design_build_and_review_name_the_architecture_design_skill() {
        for name in [DESIGN_DOC, BUILD, REVIEW] {
            let file = format!("{}/../skills/{name}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
            let skill = std::fs::read_to_string(file).unwrap();
            assert!(skill.contains("`ns-architecture-design`"), "{name}");
        }
    }

    #[test]
    fn the_cli_design_skill_is_short_may_start_on_its_own_and_states_each_rule() {
        let text = embedded()
            .iter()
            .find(|(n, _)| *n == "ns-cli-design")
            .unwrap()
            .1;
        assert!(text.lines().count() <= 60);
        assert!(!file_for_session("ns-cli-design", text).contains("disable-model-invocation"));
        for needed in [
            "## Architecture",
            "## Shape: noun verb",
            "## Output",
            "## Exit codes",
            "## Usage",
            "## Changing a command",
            "`tool noun verb [object] [flags]`",
            "stdout is the answer",
            "A flag changes how, never what.",
            "**A flag is never required.**",
            "`NO_COLOR`",
            "`--dry-run`",
        ] {
            assert!(text.contains(needed), "{needed}");
        }
    }

    #[test]
    fn build_and_review_name_the_cli_design_skill() {
        for name in [BUILD, REVIEW] {
            let file = format!("{}/../skills/{name}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
            let skill = std::fs::read_to_string(file).unwrap();
            assert!(skill.contains("`ns-cli-design`"), "{name}");
        }
    }

    #[test]
    fn the_whiteboard_defence_skill_is_short_may_start_on_its_own_and_answers_from_the_record() {
        let text = embedded()
            .iter()
            .find(|(n, _)| *n == "ns-whiteboard-defence")
            .unwrap()
            .1;
        assert!(text.lines().count() <= 60);
        assert!(text.contains("Use when the user asks why"));
        assert!(
            !file_for_session("ns-whiteboard-defence", text).contains("disable-model-invocation")
        );
        for needed in [
            "git blame",
            "git log -S",
            "ns log <unit>",
            "`recorded`",
            "`inferred`",
            "the record does not say",
        ] {
            assert!(text.contains(needed), "{needed}");
        }
    }

    #[test]
    fn build_and_review_name_the_whiteboard_defence_skill() {
        for name in [BUILD, REVIEW] {
            let file = format!("{}/../skills/{name}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
            let skill = std::fs::read_to_string(file).unwrap();
            assert!(skill.contains("`ns-whiteboard-defence`"), "{name}");
        }
    }

    #[test]
    fn design_build_and_review_name_the_service_design_skill() {
        for name in [DESIGN_DOC, BUILD, REVIEW] {
            let file = format!("{}/../skills/{name}/SKILL.md", env!("CARGO_MANIFEST_DIR"));
            let skill = std::fs::read_to_string(file).unwrap();
            assert!(skill.contains("`ns-service-design`"), "{name}");
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

    /// The `ns` subcommands and `--flags` that `text` names in backticks and the real `ns` does
    /// not have.
    fn unknown_ns_names(text: &str) -> Vec<String> {
        use clap::CommandFactory;
        let cli = crate::Cli::command();
        let flags_of = |sub: &str| -> Vec<String> {
            cli.find_subcommand(sub)
                .map(|c| {
                    c.get_arguments()
                        .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut unknown = Vec::new();
        for span in text.split('`').skip(1).step_by(2) {
            let words: Vec<&str> = span.split_whitespace().collect();
            let flags = words.iter().filter(|w| w.starts_with("--"));
            let subs: Vec<&str> = match words.as_slice() {
                ["ns", sub, ..] => {
                    if cli.find_subcommand(sub).is_none() {
                        unknown.push(format!("ns {sub}"));
                        continue;
                    }
                    vec![*sub]
                }
                _ => vec!["run", "log"],
            };
            for flag in flags {
                if !subs.iter().any(|s| flags_of(s).contains(&flag.to_string())) {
                    unknown.push((*flag).to_string());
                }
            }
        }
        unknown
    }

    #[test]
    fn the_supervise_skill_is_short_may_start_on_its_own_and_states_each_rule() {
        let text = embedded()
            .iter()
            .find(|(n, _)| *n == "ns-supervise")
            .unwrap()
            .1;
        assert!(text.lines().count() <= 60);
        assert!(!file_for_session("ns-supervise", text).contains("disable-model-invocation"));
        let description = text
            .lines()
            .find(|l| l.starts_with("description:"))
            .unwrap();
        assert!(description.contains("driving `ns`"));
        for needed in [
            "main checkout",
            "`NS_DEV",
            "`PATH`",
            "--fresh",
            "`ns run <unit>`",
            "`ns/<unit>`",
            "--gates stop",
            "`.ns/<unit>/`",
            "`ns log <unit>`",
            "`needs-human-merge`",
            "`failed`",
            "`went_wrong`",
            "`inconclusive`",
            "`base_moved`",
            "{\"waiting\":",
            "{\"stopped\":\"usage_limit\"}",
            "{\"blocked_by\":",
            "{\"from\",\"to\"}",
            "--ignore-blockers",
            "`status:waiting-for-human`",
            "is merged only when the person says to",
        ] {
            assert!(text.contains(needed), "{needed}");
        }
    }

    #[test]
    fn the_supervise_skill_names_only_ns_commands_and_flags_that_exist() {
        let text = embedded()
            .iter()
            .find(|(n, _)| *n == "ns-supervise")
            .unwrap()
            .1;
        assert_eq!(unknown_ns_names(text), Vec::<String>::new());
    }

    #[test]
    fn the_checker_finds_a_made_up_command_and_a_made_up_flag() {
        assert_eq!(unknown_ns_names("`ns frobnicate`"), ["ns frobnicate"]);
        assert_eq!(unknown_ns_names("`ns run 3 --bogus`"), ["--bogus"]);
        assert_eq!(
            unknown_ns_names("`ns run 3 --fresh` `--last`"),
            Vec::<String>::new()
        );
    }
}
