//! Which change requests `ns` may merge without a person: the ones that touch none of the files
//! the repository wants a human to review.

use anyhow::{Context, Result};
use globset::Glob;

/// Where a change request waits for a person by default: the workflows, the release configuration
/// and what pins the toolchain.
const DEFAULT: [&str; 5] = [
    ".github/**",
    ".nightshift/**",
    "release-please-config.json",
    ".release-please-manifest.json",
    "rust-toolchain.toml",
];

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
        let policy = HumanReview::default_globs();
        assert_eq!(
            policy
                .verdict(&files(&[".github/workflows/ci.yml"]))
                .unwrap(),
            needs(".github/workflows/ci.yml", ".github/**")
        );
    }

    #[test]
    fn a_file_at_the_root_matches_its_own_name() {
        let policy = HumanReview::default_globs();
        assert_eq!(
            policy.verdict(&files(&["rust-toolchain.toml"])).unwrap(),
            needs("rust-toolchain.toml", "rust-toolchain.toml")
        );
    }

    #[test]
    fn a_path_that_matches_nothing_may_be_merged() {
        let policy = HumanReview::default_globs();
        assert_eq!(
            policy
                .verdict(&files(&["cli/src/x.rs", "docs/github.md"]))
                .unwrap(),
            Verdict::Merge
        );
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
}
