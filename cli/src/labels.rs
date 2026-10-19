//! The labels `ns` reads and sets on the forge, and making sure the forge has them. `area:` labels
//! are left out: they name the parts of one repository, so only its owner knows them.

use std::path::Path;

use anyhow::Result;

use crate::forge::Forge;
use crate::{auto, status};

/// The kinds of change a unit can be, which are the Conventional Commit types.
const TYPES: [(&str, &str); 11] = [
    ("feat", "A new capability"),
    ("fix", "A bug fix"),
    ("docs", "Documentation only"),
    ("refactor", "A change that neither fixes nor adds"),
    ("test", "Tests only"),
    ("perf", "A faster way to do the same"),
    ("ci", "Continuous integration"),
    ("build", "The build or its dependencies"),
    ("chore", "Upkeep that is none of the above"),
    ("style", "Formatting only"),
    ("revert", "Undoes an earlier change"),
];

const PRIORITY_DESCRIPTIONS: [&str; 4] = [
    "Drop everything",
    "Before new features",
    "Next in line",
    "When there is room",
];

/// Every label `ns` needs, with a description of each.
pub fn all() -> Vec<(String, String)> {
    let status =
        status::all().map(|(name, description)| (name.to_string(), description.to_string()));
    let types = TYPES.map(|(name, description)| (format!("type:{name}"), description.to_string()));
    let priorities = auto::PRIORITIES
        .iter()
        .zip(PRIORITY_DESCRIPTIONS)
        .map(|(name, description)| (name.to_string(), description.to_string()));
    status.into_iter().chain(types).chain(priorities).collect()
}

/// Creates on the forge the labels it does not have, and leaves the ones it has as they are.
/// Returns the names it created.
pub fn ensure(forge: &dyn Forge, repo: &Path) -> Result<Vec<String>> {
    let have = forge.labels_list(repo)?;
    let mut created = Vec::new();
    for (name, description) in all() {
        if !have.contains(&name) {
            forge.label_create(repo, &name, &description)?;
            created.push(name);
        }
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_label_is_there_once() {
        let names: Vec<String> = all().into_iter().map(|(name, _)| name).collect();
        assert_eq!(names.len(), 8 + 11 + 4);
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
    }

    #[test]
    fn the_types_are_the_ones_the_pr_title_check_allows() {
        let allowed = [
            "build", "chore", "ci", "docs", "feat", "fix", "perf", "refactor", "style", "test",
            "revert",
        ];
        let mut types: Vec<&str> = TYPES.iter().map(|(name, _)| *name).collect();
        types.sort();
        let mut allowed = allowed.to_vec();
        allowed.sort();
        assert_eq!(types, allowed);
    }
}
