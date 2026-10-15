//! The merge conflicts a unit's branch resolved: each merge commit since the base, merged again
//! with `git merge-tree`, whoever resolved it (#615).

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::git;

/// A merge commit on a unit's branch whose parents conflicted, and the paths that did,
/// relative to the repo root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub merge: String,
    pub paths: Vec<String>,
}

/// The conflicts found, and one run-log event per conflicting merge, skipped merge or git
/// failure.
#[derive(Debug, Default)]
pub struct Detected {
    pub conflicts: Vec<Conflict>,
    pub events: Vec<Value>,
}

/// The conflicts of the merge commits in `base..head`, oldest first, from git in `dir`. A merge
/// of more than two parents is skipped, and a git failure records nothing for that merge (or
/// for the branch, when listing its merges fails): each is an event, never an error.
pub fn detect(dir: &Path, base: &str, head: &str) -> Detected {
    let mut d = Detected::default();
    let range = format!("{base}..{head}");
    let merges = match git::run(
        dir,
        &[
            "rev-list",
            "--merges",
            "--reverse",
            "--parents",
            "--end-of-options",
            &range,
        ],
    ) {
        Ok(m) => m,
        Err(e) => {
            d.events.push(json!({
                "event": "conflict_check_failed",
                "range": range,
                "error": format!("{e:#}"),
            }));
            return d;
        }
    };
    for line in merges.lines() {
        let shas: Vec<&str> = line.split_whitespace().collect();
        let (merge, parents) = (shas[0], &shas[1..]);
        if parents.len() != 2 {
            d.events.push(json!({
                "event": "conflict_skipped",
                "merge": merge,
                "reason": format!("an octopus merge of {} parents", parents.len()),
            }));
            continue;
        }
        match conflicted(dir, parents[0], parents[1]) {
            Ok(paths) if paths.is_empty() => {}
            Ok(paths) => {
                d.events.push(json!({
                    "event": "conflict_resolved",
                    "merge": merge,
                    "paths": paths,
                }));
                d.conflicts.push(Conflict {
                    merge: merge.to_string(),
                    paths,
                });
            }
            Err(e) => d.events.push(json!({
                "event": "conflict_check_failed",
                "merge": merge,
                "error": e,
            })),
        }
    }
    d
}

/// The paths that conflict when `a` and `b` merge, from `git merge-tree`: exit 0 is a clean
/// merge, 1 a conflicted one, anything else a failure.
fn conflicted(dir: &Path, a: &str, b: &str) -> Result<Vec<String>, String> {
    let out = git::command()
        .arg("-C")
        .arg(dir)
        .args(["merge-tree", "--write-tree", "--name-only", "-z", a, b])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(Vec::new()),
        Some(1) => Ok(String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .skip(1)
            .take_while(|p| !p.is_empty())
            .map(String::from)
            .collect()),
        _ => Err(format!(
            "git merge-tree {a} {b} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{commit_file, g};

    /// A repo on `main` with `a` and `b` committed, and a branch `x` off it.
    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        g(&root, &["init", "-q", "-b", "main"]);
        commit_file(&root, "a", "a\n");
        commit_file(&root, "b", "b\n");
        g(&root, &["branch", "x"]);
        (tmp, root)
    }

    /// Commit `files` with `text` on `branch`.
    fn change(root: &Path, branch: &str, files: &[&str], text: &str) {
        g(root, &["checkout", "-q", branch]);
        for f in files {
            commit_file(root, f, text);
        }
    }

    /// Merge `main` into `x` keeping `x`'s side, so the merge succeeds whatever conflicts.
    fn merge_main(root: &Path) -> String {
        g(root, &["checkout", "-q", "x"]);
        g(
            root,
            &["merge", "-q", "-s", "ours", "-m", "merge main", "main"],
        );
        g(root, &["rev-parse", "HEAD"])
    }

    #[test]
    fn a_conflicting_merge_records_both_its_paths() {
        let (_t, root) = repo();
        change(&root, "x", &["a", "b"], "x\n");
        change(&root, "main", &["a", "b"], "m\n");
        let merge = merge_main(&root);
        let d = detect(&root, "main", "x");
        assert_eq!(
            d.conflicts,
            [Conflict {
                merge: merge.clone(),
                paths: vec!["a".into(), "b".into()],
            }]
        );
        assert_eq!(
            d.events,
            [json!({"event": "conflict_resolved", "merge": merge, "paths": ["a", "b"]})]
        );
    }

    #[test]
    fn a_clean_merge_and_a_branch_with_no_merges_record_nothing() {
        let (_t, root) = repo();
        change(&root, "x", &["a"], "x\n");
        let d = detect(&root, "main", "x");
        assert!(d.conflicts.is_empty() && d.events.is_empty(), "{d:?}");
        change(&root, "main", &["b"], "m\n");
        merge_main(&root);
        let d = detect(&root, "main", "x");
        assert!(d.conflicts.is_empty() && d.events.is_empty(), "{d:?}");
    }

    #[test]
    fn an_octopus_merge_is_skipped_and_logged() {
        let (_t, root) = repo();
        change(&root, "x", &["a"], "x\n");
        g(&root, &["branch", "y", "main"]);
        change(&root, "main", &["a"], "m\n");
        change(&root, "y", &["a"], "y\n");
        let tree = g(&root, &["rev-parse", "x^{tree}"]);
        let octo = g(
            &root,
            &[
                "commit-tree",
                &tree,
                "-p",
                "x",
                "-p",
                "main",
                "-p",
                "y",
                "-m",
                "octopus",
            ],
        );
        let d = detect(&root, "main", &octo);
        assert!(d.conflicts.is_empty(), "{d:?}");
        assert_eq!(
            d.events,
            [json!({
                "event": "conflict_skipped",
                "merge": octo,
                "reason": "an octopus merge of 3 parents",
            })]
        );
    }

    #[test]
    fn a_git_failure_is_an_event_and_never_an_error() {
        let (_t, root) = repo();
        let d = detect(&root, "main", "no-such-branch");
        assert!(d.conflicts.is_empty());
        assert_eq!(d.events.len(), 1, "{:?}", d.events);
        assert_eq!(d.events[0]["event"], "conflict_check_failed");
        assert_eq!(d.events[0]["range"], "main..no-such-branch");
    }

    #[test]
    fn a_merge_whose_objects_are_gone_is_an_event_and_the_others_still_count() {
        let (_t, root) = repo();
        change(&root, "x", &["a"], "x\n");
        change(&root, "main", &["a"], "m\n");
        let first = merge_main(&root);
        change(&root, "x", &["b"], "only x\n");
        change(&root, "main", &["b"], "only main\n");
        let second = merge_main(&root);
        let blob = g(&root, &["rev-parse", &format!("{second}^1:b")]);
        let loose = root.join(".git/objects").join(&blob[..2]).join(&blob[2..]);
        std::fs::remove_file(loose).unwrap();
        let d = detect(&root, "main", "x");
        assert_eq!(d.conflicts.len(), 1, "{d:?}");
        assert_eq!(d.conflicts[0].merge, first);
        assert_eq!(d.events[1]["event"], "conflict_check_failed");
        assert_eq!(d.events[1]["merge"], second);
        assert!(
            d.events[1]["error"]
                .as_str()
                .unwrap()
                .contains("merge-tree"),
            "{d:?}"
        );
    }
}
