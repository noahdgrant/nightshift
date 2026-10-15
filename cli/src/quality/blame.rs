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
/// named in its subject. Tries each `path:line` in the location in order, so a time
/// (`3:14`) or a `host:port` before the real path doesn't stop the search.
pub fn blame(wt: &Path, at: Option<&str>, location: Option<&str>) -> Result<Blame, String> {
    let at = at.ok_or("no sha, head or base in the review frontmatter")?;
    let mut first_err = None;
    for (path, line) in location.map(path_lines).unwrap_or_default() {
        let range = format!("{line},{line}");
        match git::run(wt, &["blame", "-L", &range, "--porcelain", at, "--", &path]) {
            Ok(out) => return Ok(parse(&out)),
            Err(e) => {
                first_err.get_or_insert(e.to_string());
            }
        }
    }
    Err(first_err.unwrap_or_else(|| "location has no path:line".into()))
}

fn parse(porcelain: &str) -> Blame {
    let commit = porcelain
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let summary = porcelain
        .lines()
        .find_map(|l| l.strip_prefix("summary "))
        .unwrap_or_default()
        .to_string();
    Blame {
        commit,
        pr: pr_in(&summary),
        summary,
    }
}

/// Every `path:line` in a location such as `` `a.py:42`, `b.py` ``, in order. A line number
/// too large for `u32` skips that match.
fn path_lines(location: &str) -> Vec<(String, u32)> {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"([^\s:`,;()]+):(\d+)").unwrap())
        .captures_iter(location)
        .filter_map(|c| Some((c[1].to_string(), c[2].parse().ok()?)))
        .collect()
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
    fn path_lines_takes_every_located_path_in_order() {
        assert_eq!(
            path_lines("`cli/src/a.rs:42`, `b.rs:7`"),
            [("cli/src/a.rs".into(), 42), ("b.rs".into(), 7)]
        );
        assert_eq!(path_lines("cli/src/a.rs (gate)"), []);
        assert_eq!(path_lines("README.md:27-30"), [("README.md".into(), 27)]);
        assert_eq!(path_lines("a.rs:99999999999 b.rs:3"), [("b.rs".into(), 3)]);
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

    #[test]
    fn blame_tries_each_path_line_until_one_blames() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        g(d, &["init", "-q", "-b", "main"]);
        commit_file(d, "a.txt", "one\n");
        std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
        g(d, &["commit", "-qam", "feat: add two (#41)"]);
        let base = g(d, &["rev-parse", "HEAD"]);
        for location in [
            "Took 3:14, see a.txt:2",
            "localhost:8080 a.txt:2",
            "a.txt:0 then a.txt:2",
            "a.txt:99999999999 then a.txt:2",
        ] {
            let b = blame(d, Some(&base), Some(location)).unwrap();
            assert_eq!(
                (b.commit.as_str(), b.pr),
                (base.as_str(), Some(41)),
                "{location}"
            );
        }
        let err = blame(d, Some(&base), Some("Took 3:14, see a.txt:9")).unwrap_err();
        assert!(err.contains("git blame -L 14,14"), "{err}");
        assert_eq!(
            blame(d, Some(&base), Some("a.txt:99999999999")).unwrap_err(),
            "location has no path:line"
        );
    }
}
