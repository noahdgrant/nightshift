//! The skills the states hand the harness. A state's prompt starts with its skill's text, so the
//! harness always gets the same instructions, whether or not it would have found the skill.
//!
//! Skills are files, read when a state runs, so they can be changed without a new `ns`. A skill
//! is looked for by name in the repository's `.nightshift/skills/`, then in `ns`'s own skills
//! directory, which `ns install` fills.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

pub const TRIAGE: &str = "ns-triage";
pub const BUILD: &str = "ns-build";
pub const VERIFY: &str = "ns-verify";
pub const REVIEW: &str = "ns-review";
pub const GRILLING: &str = "ns-grilling";
pub const REQUIREMENTS: &str = "ns-requirements";
pub const DESIGN_DOC: &str = "ns-design-doc";
pub const TROUBLESHOOT: &str = "ns-troubleshoot";

/// Where `ns` keeps what belongs to the user: `$NS_CONFIG_DIR`, else where the operating system
/// keeps such files.
pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("NS_CONFIG_DIR") {
        return Ok(dir.into());
    }
    crate::os::get().config_dir_find()
}

/// The directory `ns install` links skills into.
pub fn installed_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("skills"))
}

/// The text of skill `name`, without its frontmatter. `root` is the repository's main checkout.
pub fn load(root: &Path, name: &str) -> Result<String> {
    let mut looked = vec![root.join(".nightshift").join("skills")];
    looked.push(installed_dir()?);
    for dir in &looked {
        let file = dir.join(name).join("SKILL.md");
        match std::fs::read_to_string(&file) {
            Ok(text) => return Ok(body(&text).to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
        }
    }
    bail!(
        "skill {name} is not installed; run `ns install` with the directory that holds it (looked in {})",
        looked
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(" and ")
    )
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
