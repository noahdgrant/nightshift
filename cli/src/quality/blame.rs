//! Attribute an escape to the commit and PR that introduced the line it is about.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::git;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Blame {
    pub commit: String,
    pub summary: String,
    pub pr: Option<u64>,
}

/// The commit that last changed the finding's line in the reviewed code (`at`), and the PR
/// named in its subject.
pub fn blame(wt: &Path, at: Option<&str>, location: Option<&str>) -> Result<Blame, String> {
    let at = at.ok_or("no sha, head or base in the review frontmatter")?;
    let (path, line) = location
        .and_then(path_line)
        .ok_or("location has no path:line")?;
    let range = format!("{line},{line}");
    let out = git::run(wt, &["blame", "-L", &range, "--porcelain", at, "--", &path])
        .map_err(|e| e.to_string())?;
    let commit = out
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let summary = out
        .lines()
        .find_map(|l| l.strip_prefix("summary "))
        .unwrap_or_default()
        .to_string();
    Ok(Blame {
        commit,
        pr: pr_in(&summary),
        summary,
    })
}

/// The first `path:line` in a location such as `` `a.py:42`, `b.py` ``.
fn path_line(location: &str) -> Option<(String, u32)> {
    static R: OnceLock<Regex> = OnceLock::new();
    let c = R
        .get_or_init(|| Regex::new(r"([^\s:`,;()]+):(\d+)").unwrap())
        .captures(location)?;
    Some((c[1].to_string(), c[2].parse().ok()?))
}

/// The PR a squash-merge subject names: the last `(#<n>)`, so a revert names the revert's PR.
fn pr_in(subject: &str) -> Option<u64> {
    static PR: OnceLock<Regex> = OnceLock::new();
    PR.get_or_init(|| Regex::new(r"\(#(\d+)\)").unwrap())
        .captures_iter(subject)
        .last()
        .and_then(|c| c[1].parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{commit_file, g};

    #[test]
    fn path_line_takes_the_first_located_path() {
        assert_eq!(
            path_line("`cli/src/a.rs:42`, `b.rs:7`"),
            Some(("cli/src/a.rs".into(), 42))
        );
        assert_eq!(path_line("cli/src/a.rs (gate)"), None);
        assert_eq!(path_line("README.md:27-30"), Some(("README.md".into(), 27)));
    }

    #[test]
    fn pr_in_takes_the_last_reference() {
        assert_eq!(pr_in("Revert \"feat: x (#3)\" (#41)"), Some(41));
        assert_eq!(pr_in("fix #7 without parens"), None);
    }

    #[test]
    fn blame_names_the_commit_and_pr_that_introduced_a_line() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        g(d, &["init", "-q", "-b", "main"]);
        commit_file(d, "a.txt", "one\n");
        std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
        g(d, &["commit", "-qam", "feat: add two (#41)"]);
        let base = g(d, &["rev-parse", "HEAD"]);
        let b = blame(d, Some(&base), Some("`a.txt:2`")).unwrap();
        assert_eq!(b.commit, base);
        assert_eq!(b.summary, "feat: add two (#41)");
        assert_eq!(b.pr, Some(41));
        let b = blame(d, Some(&base), Some("a.txt:1")).unwrap();
        assert_eq!((b.summary.as_str(), b.pr), ("a.txt", None));
        assert_eq!(
            blame(d, None, Some("a.txt:1")).unwrap_err(),
            "no sha, head or base in the review frontmatter"
        );
        assert_eq!(
            blame(d, Some(&base), Some("a.txt")).unwrap_err(),
            "location has no path:line"
        );
        assert_eq!(
            blame(d, Some(&base), None).unwrap_err(),
            "location has no path:line"
        );
        assert!(blame(d, Some(&base), Some("a.txt:9"))
            .unwrap_err()
            .contains("git blame"));
        assert!(blame(d, Some(&base), Some("gone.txt:1"))
            .unwrap_err()
            .contains("git blame"));
    }
}
