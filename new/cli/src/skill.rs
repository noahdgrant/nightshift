//! The skills the states hand the harness. A state's prompt starts with its skill's text, so the
//! harness always gets the same instructions, whether or not it would have found the skill.
//!
//! The skills are in the binary, so every install runs the skills that came with it. A repository
//! may replace one with a file of its own, `.nightshift/skills/<name>/SKILL.md`, read when a
//! state runs.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

include!(concat!(env!("OUT_DIR"), "/skills.rs"));

pub const TRIAGE: &str = "ns-triage";
pub const BUILD: &str = "ns-build";
pub const VERIFY: &str = "ns-verify";
pub const REVIEW: &str = "ns-review";
pub const GRILLING: &str = "ns-grilling";
pub const REQUIREMENTS: &str = "ns-requirements";
pub const DESIGN_DOC: &str = "ns-design-doc";
pub const TROUBLESHOOT: &str = "ns-troubleshoot";

/// Where a repository keeps its own copy of the skills.
pub fn dir_find(root: &Path) -> PathBuf {
    root.join(".nightshift").join("skills")
}

/// Every skill that came with this `ns`, as its name and its file's text.
pub fn embedded() -> &'static [(&'static str, &'static str)] {
    EMBEDDED
}

/// The text of skill `name`, without its frontmatter. `root` is the repository's main checkout.
pub fn load(root: &Path, name: &str) -> Result<String> {
    let file = dir_find(root).join(name).join("SKILL.md");
    match std::fs::read_to_string(&file) {
        Ok(text) => return Ok(body(&text).to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
    }
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
    fn a_skill_is_the_embedded_one_unless_the_repository_has_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load(tmp.path(), BUILD).unwrap().starts_with("# Build"));
        let own = tmp.path().join(".nightshift/skills/ns-build");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("SKILL.md"), "---\nname: ns-build\n---\nMINE\n").unwrap();
        assert_eq!(load(tmp.path(), BUILD).unwrap(), "MINE\n");
    }

    #[test]
    fn a_skill_nobody_wrote_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(tmp.path(), "ns-nothing").unwrap_err();
        assert!(err.to_string().contains("no skill named ns-nothing"));
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
