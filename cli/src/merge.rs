//! Which change requests `ns` may merge without a person: the ones that touch none of the files
//! the repository wants a human to review.

use anyhow::{Context, Result};
use globset::Glob;

use crate::markers;
use crate::vcs::Hunk;

/// Where a change request waits for a person by default: everywhere, until the repository names
/// the files it wants reviewed.
const DEFAULT: [&str; 1] = ["**"];

/// The globs of the files whose change a person must merge.
#[derive(Debug, PartialEq, Eq)]
pub struct HumanReview(Vec<String>);

/// Whether a change request may be merged without a person.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Merge,
    /// A person merges it, because `file` matches `glob`.
    NeedsHuman {
        file: String,
        glob: String,
    },
}

impl HumanReview {
    pub fn new(globs: Vec<String>) -> HumanReview {
        HumanReview(globs)
    }

    /// The globs of a repository that names none.
    pub fn default_globs() -> HumanReview {
        HumanReview(DEFAULT.map(String::from).to_vec())
    }

    /// The first of `files` that matches a glob, with the first glob it matches. Paths are
    /// relative to the repository's top.
    pub fn verdict(&self, files: &[String]) -> Result<Verdict> {
        let globs = self
            .0
            .iter()
            .map(|g| {
                let matcher = Glob::new(g)
                    .with_context(|| format!("{g:?} in `merge.human_review` is not a glob"))?
                    .compile_matcher();
                Ok((g, matcher))
            })
            .collect::<Result<Vec<_>>>()?;
        for file in files {
            if let Some((glob, _)) = globs.iter().find(|(_, m)| m.is_match(file)) {
                return Ok(Verdict::NeedsHuman {
                    file: file.clone(),
                    glob: (*glob).clone(),
                });
            }
        }
        Ok(Verdict::Merge)
    }
}

/// Why a change to `file` must be merged by a person, when it touches a region a person must
/// review or a marker `ns` cannot read. `old` and `new` are the file before and after the change,
/// `None` where it did not exist, and `hunks` the lines the change touched. Both versions are
/// checked: lines a change deletes sit in the old version's regions, and a marker the change adds
/// or edits sits in the new one's. A marker line is part of its region, so changing one is
/// touching it.
pub fn region_hold(
    file: &str,
    old: Option<&str>,
    new: Option<&str>,
    hunks: &[Hunk],
) -> Option<String> {
    let mut read = Vec::new();
    for (version, text) in [("old", old), ("new", new)] {
        let Some(text) = text else { continue };
        match markers::regions_read(text) {
            Ok(regions) => read.push((version, regions)),
            Err(e) => {
                return Some(format!(
                    "{file} has a marker `ns` cannot read in its {version} version (line {}: {e}), so a person merges it",
                    e.line()
                ))
            }
        }
    }
    for (version, regions) in read {
        for region in regions {
            let touched = hunks.iter().any(|h| {
                let lines = if version == "old" { &h.old } else { &h.new };
                !lines.is_empty() && lines.start <= region.end && region.start < lines.end
            });
            if touched {
                let reason = if region.reason.is_empty() {
                    String::new()
                } else {
                    format!(": {}", region.reason)
                };
                return Some(format!(
                    "{file} changes lines {}-{} of its {version} version, a human-review region{reason}, so a person merges it",
                    region.start, region.end
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn needs(file: &str, glob: &str) -> Verdict {
        Verdict::NeedsHuman {
            file: file.into(),
            glob: glob.into(),
        }
    }

    #[test]
    fn a_double_star_crosses_directories() {
        let policy = HumanReview::new(files(&[".github/**"]));
        assert_eq!(
            policy
                .verdict(&files(&[".github/workflows/ci.yml"]))
                .unwrap(),
            needs(".github/workflows/ci.yml", ".github/**")
        );
    }

    #[test]
    fn a_file_at_the_root_matches_its_own_name() {
        let policy = HumanReview::new(files(&["rust-toolchain.toml"]));
        assert_eq!(
            policy.verdict(&files(&["rust-toolchain.toml"])).unwrap(),
            needs("rust-toolchain.toml", "rust-toolchain.toml")
        );
    }

    #[test]
    fn a_path_that_matches_nothing_may_be_merged() {
        let policy = HumanReview::new(files(&[".github/**"]));
        assert_eq!(
            policy
                .verdict(&files(&["cli/src/x.rs", "docs/github.md"]))
                .unwrap(),
            Verdict::Merge
        );
    }

    #[test]
    fn the_default_holds_every_path() {
        let policy = HumanReview::default_globs();
        for file in ["cli/src/x.rs", "README.md", ".github/workflows/ci.yml"] {
            assert_eq!(policy.verdict(&files(&[file])).unwrap(), needs(file, "**"));
        }
    }

    #[test]
    fn the_first_file_that_matches_is_named_with_the_first_glob_it_matches() {
        let policy = HumanReview::new(files(&["b/**", "**/*.md", "b/x.md"]));
        assert_eq!(
            policy.verdict(&files(&["a.rs", "b/x.md", "c.md"])).unwrap(),
            needs("b/x.md", "b/**")
        );
    }

    #[test]
    fn no_globs_let_everything_merge() {
        let policy = HumanReview::new(Vec::new());
        assert_eq!(
            policy.verdict(&files(&[".github/ci.yml"])).unwrap(),
            Verdict::Merge
        );
    }

    #[test]
    fn a_glob_that_is_not_one_is_an_error() {
        let policy = HumanReview::new(files(&["[a"]));
        let err = policy.verdict(&files(&["a"])).unwrap_err();
        assert!(err.to_string().contains("not a glob"), "{err}");
    }

    /// `text` with `@start` and `@end` written as the markers.
    fn marked(text: &str) -> String {
        text.replace("@start", markers::START)
            .replace("@end", markers::END)
    }

    fn hunk(old: std::ops::Range<usize>, new: std::ops::Range<usize>) -> Hunk {
        Hunk { old, new }
    }

    const FILE: &str = "a\n@start: the limit\nb\n@end\nc\n";

    #[test]
    fn a_change_outside_every_region_is_not_held() {
        let text = marked(FILE);
        assert_eq!(
            region_hold("f", Some(&text), Some(&text), &[hunk(5..6, 5..6)]),
            None
        );
    }

    #[test]
    fn a_change_inside_a_region_is_held_with_its_lines_and_reason() {
        let text = marked(FILE);
        assert_eq!(
            region_hold("f", Some(&text), Some(&text), &[hunk(3..4, 3..4)]),
            Some("f changes lines 2-4 of its old version, a human-review region: the limit, so a person merges it".to_string())
        );
    }

    #[test]
    fn a_region_without_a_reason_is_named_by_its_lines() {
        let text = marked("@start\nb\n@end\n");
        assert_eq!(
            region_hold("f", Some(&text), Some(&text), &[hunk(2..3, 2..3)]),
            Some("f changes lines 1-3 of its old version, a human-review region, so a person merges it".to_string())
        );
    }

    #[test]
    fn lines_deleted_from_a_region_of_the_old_version_hold() {
        let old = marked(FILE);
        let new = "a\nc\n";
        let why = region_hold("f", Some(&old), Some(new), &[hunk(2..5, 2..2)]).unwrap();
        assert!(why.contains("lines 2-4 of its old version"), "{why}");
    }

    #[test]
    fn lines_added_inside_a_region_of_the_new_version_hold() {
        let old = marked("a\n@start\n@end\n");
        let new = marked("a\n@start\nx\n@end\n");
        let why = region_hold("f", Some(&old), Some(&new), &[hunk(3..3, 3..4)]).unwrap();
        assert!(why.contains("lines 2-4 of its new version"), "{why}");
    }

    #[test]
    fn a_marker_that_moves_holds() {
        let old = marked(FILE);
        let new = marked("a\nb\n@start: the limit\n@end\nc\n");
        let why = region_hold(
            "f",
            Some(&old),
            Some(&new),
            &[hunk(2..3, 2..2), hunk(3..3, 3..4)],
        );
        assert!(why.is_some());
    }

    #[test]
    fn a_marker_that_is_edited_holds() {
        let old = marked(FILE);
        let new = marked(&FILE.replace("the limit", "a new reason"));
        let why = region_hold("f", Some(&old), Some(&new), &[hunk(2..3, 2..3)]).unwrap();
        assert!(why.contains("lines 2-4"), "{why}");
    }

    #[test]
    fn a_marker_that_cannot_be_read_holds_in_either_version() {
        let good = marked(FILE);
        let bad = marked("a\n@start\nb\n");
        let why = region_hold("f", Some(&good), Some(&bad), &[hunk(4..5, 4..4)]).unwrap();
        assert_eq!(
            why,
            "f has a marker `ns` cannot read in its new version (line 2: `ns:human-review start` has no `ns:human-review end`), so a person merges it"
        );
        let why = region_hold("f", Some(&bad), Some(&good), &[hunk(5..5, 5..6)]).unwrap();
        assert!(why.contains("in its old version"), "{why}");
    }

    #[test]
    fn a_file_without_markers_is_not_held() {
        assert_eq!(
            region_hold("f", Some("a\n"), Some("b\n"), &[hunk(1..2, 1..2)]),
            None
        );
        assert_eq!(
            region_hold("f", None, Some("b\n"), &[hunk(0..0, 1..2)]),
            None
        );
    }
}
