//! Find each unit's review artifacts in the linked worktrees and read them into `Attempt`s.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use super::metrics::{Attempt, RunStats, Unit};
use crate::clock;
use crate::frontmatter;
use crate::git::{self, Repo};
use crate::review_md::{self, CycleEntry};

/// An artifact that couldn't be read, reported under `gaps` instead of dropped.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Unparsed {
    pub path: String,
    pub reason: String,
}

fn unparsed(wt: &Path, path: &Path, reason: impl Into<String>) -> Unparsed {
    Unparsed {
        path: path.strip_prefix(wt).unwrap_or(path).display().to_string(),
        reason: reason.into(),
    }
}

/// A hex sha (7 to 40 digits), or `None`. Artifact values are model-written, so nothing else
/// reaches a git command line.
pub(super) fn sha(v: &str) -> Option<String> {
    let ok = (7..=40).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| v.to_string())
}

/// `cycle-<n>.md` files in `dir` as `(n, path)`, lowest first.
fn cycle_files(dir: &Path) -> Vec<(u32, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(u32, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name
                .strip_prefix("cycle-")?
                .strip_suffix(".md")?
                .parse()
                .ok()?;
            Some((n, e.path()))
        })
        .collect();
    files.sort();
    files
}

/// The findings the first pass's cycle file lists, when the lowest one on disk is `cycle-0`
/// or `cycle-1`.
fn first_pass_file(cycle_dir: Option<&Path>) -> Vec<CycleEntry> {
    cycle_dir
        .and_then(|d| cycle_files(d).into_iter().next())
        .filter(|(n, _)| *n <= 1)
        .and_then(|(_, p)| fs::read_to_string(p).ok())
        .map(|t| review_md::cycle_entries(&t))
        .unwrap_or_default()
}

/// Changed lines between `base` and `head`, measured as `ns-contract` does: from the merge
/// base, without `.ns/`.
fn shortstat(wt: &Path, base: &str, head: &str) -> Option<u64> {
    let range = format!("{base}...{head}");
    let out = git::run(wt, &["diff", "--shortstat", &range, "--", ".", ":!.ns"]).ok()?;
    let counted = out.split(',').filter_map(|part| {
        let mut words = part.split_whitespace();
        let n: u64 = words.next()?.parse().ok()?;
        let kind = words.next()?;
        (kind.starts_with("insertion") || kind.starts_with("deletion")).then_some(n)
    });
    Some(counted.sum())
}

fn read_attempt(
    wt: &Path,
    path: &Path,
    cycle_dir: Option<&Path>,
    git_at: &Path,
    gaps: &mut Vec<Unparsed>,
) -> Option<Attempt> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            gaps.push(unparsed(wt, path, format!("unreadable: {e}")));
            return None;
        }
    };
    let Some((yaml, body)) = frontmatter::split(&text) else {
        gaps.push(unparsed(wt, path, "no frontmatter"));
        return None;
    };
    let field = |k: &str| frontmatter::raw_field(yaml, k);
    let base = field("base").and_then(|b| sha(b.rsplit('@').next().unwrap_or(&b)));
    let head = field("head").and_then(|h| sha(&h));
    let blame_at = field("sha")
        .and_then(|s| sha(&s))
        .or_else(|| head.clone())
        .or_else(|| base.clone());
    let (changed_lines, changed_lines_source) = match review_md::change_size(body) {
        Some(n) => (Some(n), Some("summary")),
        None => match base
            .as_deref()
            .zip(head.as_deref())
            .and_then(|(b, h)| shortstat(git_at, b, h))
        {
            Some(n) => (Some(n), Some("git")),
            None => (None, None),
        },
    };
    Some(Attempt {
        path: path.strip_prefix(wt).unwrap_or(path).display().to_string(),
        status: field("status"),
        updated: field("updated").and_then(|u| clock::parse_iso(&u)),
        cycles: field("cycles").and_then(|c| c.parse().ok()),
        blame_at,
        changed_lines,
        changed_lines_source,
        findings: review_md::parse(body),
        stray_statuses: review_md::stray_statuses(body),
        first_pass_file: first_pass_file(cycle_dir),
        introduced_by: Default::default(),
    })
}

/// A unit's review attempts, oldest first: archived `history/review*.md` by `updated:` (undated
/// first), then `review.md`. `review/` holds the newest attempt's cycle files; an archived
/// attempt's are in `history/<stem>/`. A change size missing from the Summary is measured in
/// `git_at`: the worktree, or the repo an archived worktree came from.
pub fn read_attempts(
    wt: &Path,
    dir: &Path,
    git_at: &Path,
    gaps: &mut Vec<Unparsed>,
) -> Vec<Attempt> {
    let hist = dir.join("history");
    let mut names: Vec<String> = fs::read_dir(&hist)
        .map(|es| {
            es.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("review"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut attempts = Vec::new();
    for name in &names {
        let path = hist.join(name);
        if let Some(stem) = name.strip_suffix(".md").filter(|_| path.is_file()) {
            let cycles = hist.join(stem);
            let cycles = cycles.is_dir().then_some(cycles);
            attempts.extend(read_attempt(wt, &path, cycles.as_deref(), git_at, gaps));
        } else if path.is_dir() && !hist.join(format!("{name}.md")).is_file() {
            gaps.push(unparsed(wt, &path, "cycle files with no review artifact"));
        }
    }
    attempts.sort_by_key(|a| a.updated.map_or((false, 0), |t| (true, t)));
    let current = dir.join("review");
    let current = current.is_dir().then_some(current);
    let review = dir.join("review.md");
    if review.exists() {
        attempts.extend(read_attempt(wt, &review, current.as_deref(), git_at, gaps));
    } else if let Some(last) = attempts.last_mut() {
        if last.first_pass_file.is_empty() {
            last.first_pass_file = first_pass_file(current.as_deref());
        }
    } else if let Some(c) = current {
        gaps.push(unparsed(wt, &c, "cycle files with no review artifact"));
    }
    attempts
}

/// Every unit with a review artifact in a linked worktree, by unit id. The main checkout is
/// skipped; a unit id seen in two worktrees keeps the one with the lower path.
pub fn discover(repo: &Repo, gaps: &mut Vec<Unparsed>) -> Result<Vec<Unit>> {
    let mut units: BTreeMap<String, Unit> = BTreeMap::new();
    let mut linked: Vec<PathBuf> = git::worktrees(&repo.root)?
        .into_iter()
        .skip(1)
        .map(|w| w.path)
        .collect();
    linked.sort();
    for wt in linked {
        let Ok(entries) = fs::read_dir(wt.join(".ns")) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            let id = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if units.contains_key(&id) {
                continue;
            }
            let attempts = read_attempts(&wt, &dir, &wt, gaps);
            if attempts.is_empty() {
                continue;
            }
            let day = attempts
                .last()
                .and_then(|a| a.updated)
                .map(clock::local_date);
            units.insert(
                id.clone(),
                Unit {
                    id,
                    worktree: Some(wt.display().to_string()),
                    attempts,
                    day,
                    run: RunStats::default(),
                },
            );
        }
    }
    Ok(units.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{commit_file, g};

    fn ids(entries: &[CycleEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn sha_accepts_only_hex_of_sha_length() {
        assert_eq!(sha("abc1234").as_deref(), Some("abc1234"));
        assert_eq!(
            sha(&"a".repeat(40)).as_deref(),
            Some("a".repeat(40).as_str())
        );
        for bad in ["abc123", "HEAD", "--all", "abc123z", &"a".repeat(41)] {
            assert_eq!(sha(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_first_pass_file_is_cycle_0_or_1_only() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        assert!(first_pass_file(None).is_empty());
        assert!(first_pass_file(Some(d)).is_empty());
        fs::write(d.join("cycle-3.md"), "## I1. late\n").unwrap();
        fs::write(d.join("cycle-2.md"), "## I6. second fix cycle\n").unwrap();
        fs::write(d.join("cycle-x.md"), "## I8. junk\n").unwrap();
        assert!(first_pass_file(Some(d)).is_empty());
        fs::write(d.join("cycle-1.md"), "## I2. a\n- I3 perf: b\n").unwrap();
        assert_eq!(ids(&first_pass_file(Some(d))), ["I2", "I3"]);
        fs::write(d.join("cycle-0.md"), "I4 Important (spec): c\n").unwrap();
        assert_eq!(ids(&first_pass_file(Some(d))), ["I4"]);
        assert_eq!(
            cycle_files(d).iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    fn art(updated: &str, body: &str) -> String {
        format!("---\nstatus: pass\n{updated}cycles: 1\n---\n{body}")
    }

    #[test]
    fn attempts_are_ordered_and_take_the_right_cycle_files() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path();
        let dir = wt.join(".ns/u");
        let hist = dir.join("history");
        fs::create_dir_all(hist.join("review-2")).unwrap();
        fs::create_dir_all(dir.join("review")).unwrap();
        fs::write(
            hist.join("review-1.md"),
            art("updated: 2026-10-09T05:00:00Z\n", ""),
        )
        .unwrap();
        fs::write(
            hist.join("review-2.md"),
            art("updated: 2026-10-09T01:00:00Z\n", ""),
        )
        .unwrap();
        fs::write(hist.join("review-blocked-1.md"), art("", "")).unwrap();
        fs::write(hist.join("review-2/cycle-1.md"), "- I7 a\n").unwrap();
        fs::write(hist.join("evidence-1.md"), art("", "")).unwrap();
        fs::write(dir.join("review/cycle-0.md"), "- I9 b\n").unwrap();
        let mut gaps = Vec::new();
        let a = read_attempts(wt, &dir, wt, &mut gaps);
        let paths: Vec<_> = a.iter().map(|a| a.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".ns/u/history/review-blocked-1.md",
                ".ns/u/history/review-2.md",
                ".ns/u/history/review-1.md"
            ]
        );
        assert_eq!(ids(&a[1].first_pass_file), ["I7"]);
        assert_eq!(ids(&a[2].first_pass_file), ["I9"]);
        assert!(gaps.is_empty());

        fs::write(
            dir.join("review.md"),
            art("updated: 2026-10-01T00:00:00Z\n", ""),
        )
        .unwrap();
        let a = read_attempts(wt, &dir, wt, &mut gaps);
        assert_eq!(a.len(), 4);
        assert_eq!(a[3].path, ".ns/u/review.md");
        assert_eq!(ids(&a[3].first_pass_file), ["I9"]);
        assert!(a[2].first_pass_file.is_empty());

        fs::write(dir.join("review.md"), "no frontmatter\n").unwrap();
        let a = read_attempts(wt, &dir, wt, &mut gaps);
        assert_eq!(a.len(), 3);
        assert!(a[2].first_pass_file.is_empty());
        assert_eq!(
            gaps,
            [Unparsed {
                path: ".ns/u/review.md".into(),
                reason: "no frontmatter".into()
            }]
        );
    }

    #[test]
    fn an_archived_attempt_keeps_its_own_cycle_files_over_review_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".ns/u");
        fs::create_dir_all(dir.join("history/review-1")).unwrap();
        fs::create_dir_all(dir.join("review")).unwrap();
        fs::write(dir.join("history/review-1.md"), art("", "")).unwrap();
        fs::write(dir.join("history/review-1/cycle-1.md"), "- I5 a\n").unwrap();
        fs::write(dir.join("review/cycle-0.md"), "- I9 b\n").unwrap();
        let a = read_attempts(tmp.path(), &dir, tmp.path(), &mut Vec::new());
        assert_eq!(ids(&a[0].first_pass_file), ["I5"]);
    }

    #[test]
    fn cycle_files_with_no_artifact_are_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".ns/u");
        fs::create_dir_all(dir.join("review")).unwrap();
        fs::write(dir.join("review/cycle-1.md"), "- I1 a\n").unwrap();
        let mut gaps = Vec::new();
        assert!(read_attempts(tmp.path(), &dir, tmp.path(), &mut gaps).is_empty());
        assert_eq!(
            gaps,
            [Unparsed {
                path: ".ns/u/review".into(),
                reason: "cycle files with no review artifact".into()
            }]
        );
        fs::remove_dir_all(dir.join("review")).unwrap();
        let mut gaps = Vec::new();
        assert!(read_attempts(tmp.path(), &dir, tmp.path(), &mut gaps).is_empty());
        assert!(gaps.is_empty());
    }

    #[test]
    fn an_attempt_reads_its_frontmatter_and_change_size() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("review.md");
        fs::write(
            &p,
            "---\nstatus: blocked\nupdated: 2026-10-09T00:00:00Z\nbase: origin/main@0e05787\nhead: HEAD\ncycles: 3\n---\n## Summary\nChange size: 12 lines.\n### I1. x\n### C3-1. not a finding\n- Status: open\n",
        )
        .unwrap();
        let mut gaps = Vec::new();
        let a = read_attempt(tmp.path(), &p, None, tmp.path(), &mut gaps).unwrap();
        assert_eq!(a.path, "review.md");
        assert_eq!(a.status.as_deref(), Some("blocked"));
        assert_eq!(a.updated, Some(1_791_504_000));
        assert_eq!(a.blame_at.as_deref(), Some("0e05787"));
        assert_eq!(a.cycles, Some(3));
        assert_eq!(
            (a.changed_lines, a.changed_lines_source),
            (Some(12), Some("summary"))
        );
        assert_eq!((a.findings.len(), a.stray_statuses), (1, 1));
        fs::write(&p, "---\nbase: main@HEAD\nhead: abcdef0\n---\n").unwrap();
        let a = read_attempt(tmp.path(), &p, None, tmp.path(), &mut gaps).unwrap();
        assert_eq!(
            (
                a.blame_at.as_deref(),
                a.changed_lines,
                a.changed_lines_source
            ),
            (Some("abcdef0"), None, None)
        );
        fs::write(
            &p,
            "---\nsha: 1234567\nbase: main@0e05787\nhead: abcdef0\n---\n",
        )
        .unwrap();
        let a = read_attempt(tmp.path(), &p, None, tmp.path(), &mut gaps).unwrap();
        assert_eq!(a.blame_at.as_deref(), Some("1234567"));
        fs::write(&p, "---\nsha: HEAD\nbase: main@HEAD\n---\n").unwrap();
        let a = read_attempt(tmp.path(), &p, None, tmp.path(), &mut gaps).unwrap();
        assert_eq!(a.blame_at, None);
        assert_eq!((a.status, a.updated, a.cycles), (None, None, None));
        assert!(read_attempt(
            tmp.path(),
            &tmp.path().join("gone.md"),
            None,
            tmp.path(),
            &mut gaps
        )
        .is_none());
        assert!(gaps[0].reason.starts_with("unreadable: "), "{gaps:?}");
    }

    #[test]
    fn shortstat_sums_insertions_and_deletions_outside_ns() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        g(d, &["init", "-q", "-b", "main"]);
        let base = commit_file(d, "a.txt", "one\ntwo\n");
        let head = commit_file(d, "a.txt", "one\n2\n3\n");
        assert_eq!(shortstat(d, &base, &head), Some(3));
        assert_eq!(shortstat(d, &base, &base), Some(0));
        let only_add = commit_file(d, "b.txt", "x\n");
        assert_eq!(shortstat(d, &head, &only_add), Some(1));
        fs::create_dir(d.join(".ns")).unwrap();
        let ns = commit_file(d, ".ns/c.txt", "y\nz\n");
        assert_eq!(shortstat(d, &only_add, &ns), Some(0));
        assert_eq!(shortstat(d, &base, "0000000"), None);
    }
}
