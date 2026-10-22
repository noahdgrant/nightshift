//! The labels `ns` reads and sets on the forge, and making sure the forge has them. `area:` labels
//! are left out: they name the parts of one repository, so only its owner knows them.

use std::path::Path;

use anyhow::Result;

use crate::forge::{self, Forge};
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

/// On a change request that `ns` left for a person to merge, though its checks passed.
pub const NEEDS_HUMAN_MERGE: &str = "needs-human-merge";

const PRIORITY_DESCRIPTIONS: [&str; 4] = [
    "Drop everything",
    "Before new features",
    "Next in line",
    "When there is room",
];

/// The native type that stands for a Conventional Commit type, and whether it replaces the
/// `type:` label. The types without an equivalent are a Task, and keep their label.
fn native_for(kind: &str) -> (&'static str, bool) {
    match kind {
        "feat" => ("Feature", true),
        "fix" => ("Bug", true),
        _ => ("Task", false),
    }
}

/// What `ns` records of a unit's `type:` label where the repository has the native types `native`.
pub struct TypeSplit {
    /// The labels to put on the unit: those given, less the `type:` label a native type replaces.
    pub labels: Vec<String>,
    /// The native type to set, when the repository has the one that fits the unit's type.
    pub native: Option<String>,
}

/// Splits `labels`, which may hold a `type:` label, between that label and a native type. A type
/// name the repository lacks falls back to the label.
pub fn types_split(native: &[String], labels: &[String]) -> TypeSplit {
    let mut split = TypeSplit {
        labels: Vec::new(),
        native: None,
    };
    for label in labels {
        let Some(kind) = label.strip_prefix("type:") else {
            split.labels.push(label.clone());
            continue;
        };
        let (name, replaces) = native_for(kind);
        if !native.iter().any(|n| n == name) {
            split.labels.push(label.clone());
            continue;
        }
        split.native.get_or_insert(name.to_string());
        if !replaces {
            split.labels.push(label.clone());
        }
    }
    split
}

/// Every label `ns` needs where the repository has the native types `native`, with a description
/// of each.
pub fn all(native: &[String]) -> Vec<(String, String)> {
    let status =
        status::all().map(|(name, description)| (name.to_string(), description.to_string()));
    let types = TYPES
        .iter()
        .filter(|(name, _)| {
            !types_split(native, &[format!("type:{name}")])
                .labels
                .is_empty()
        })
        .map(|(name, description)| (format!("type:{name}"), description.to_string()));
    let priorities = auto::PRIORITIES
        .iter()
        .zip(PRIORITY_DESCRIPTIONS)
        .map(|(name, description)| (name.to_string(), description.to_string()));
    let needs_human_merge = (
        NEEDS_HUMAN_MERGE.to_string(),
        "A person merges this: it touches files the repository wants reviewed".to_string(),
    );
    status
        .into_iter()
        .chain(types)
        .chain(priorities)
        .chain([needs_human_merge])
        .collect()
}

/// Creates on the forge the labels it does not have, and leaves the ones it has as they are.
/// Returns the names it created.
pub fn ensure(forge: &dyn Forge, repo: &Path) -> Result<Vec<String>> {
    let have = forge.labels_list(repo)?;
    let mut created = Vec::new();
    for (name, description) in all(&forge::unit_types_get(forge, repo)) {
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
        let names: Vec<String> = all(&[]).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names.len(), 8 + 11 + 4 + 1);
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
    }

    fn native(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn without_native_types_every_label_stays() {
        let split = types_split(&[], &native(&["type:feat", "priority:high"]));
        assert_eq!(split.labels, ["type:feat", "priority:high"]);
        assert_eq!(split.native, None);
    }

    #[test]
    fn feat_and_fix_become_native_types_and_lose_their_label() {
        let have = native(&["Feature", "Bug", "Task"]);
        let split = types_split(&have, &native(&["type:feat", "area:cli"]));
        assert_eq!(split.labels, ["area:cli"]);
        assert_eq!(split.native.as_deref(), Some("Feature"));
        let split = types_split(&have, &native(&["type:fix"]));
        assert!(split.labels.is_empty());
        assert_eq!(split.native.as_deref(), Some("Bug"));
    }

    #[test]
    fn another_type_is_a_task_and_keeps_its_label() {
        let split = types_split(&native(&["Task"]), &native(&["type:refactor"]));
        assert_eq!(split.labels, ["type:refactor"]);
        assert_eq!(split.native.as_deref(), Some("Task"));
    }

    #[test]
    fn a_native_type_the_repository_lacks_falls_back_to_the_label() {
        let split = types_split(&native(&["Bug"]), &native(&["type:feat"]));
        assert_eq!(split.labels, ["type:feat"]);
        assert_eq!(split.native, None);
        let split = types_split(&native(&["Feature"]), &native(&["type:chore"]));
        assert_eq!(split.labels, ["type:chore"]);
        assert_eq!(split.native, None);
    }

    #[test]
    fn with_native_types_feat_and_fix_labels_are_not_needed() {
        let names: Vec<String> = all(&native(&["Feature", "Bug", "Task"]))
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names.len(), 8 + 9 + 4 + 1);
        assert!(!names.contains(&"type:feat".to_string()));
        assert!(!names.contains(&"type:fix".to_string()));
        assert!(names.contains(&"type:refactor".to_string()));
    }

    #[test]
    fn a_label_is_still_needed_where_its_native_type_was_removed() {
        let names: Vec<String> = all(&native(&["Bug"]))
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(names.contains(&"type:feat".to_string()));
        assert!(!names.contains(&"type:fix".to_string()));
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
