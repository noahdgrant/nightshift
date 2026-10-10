//! Quality records: one JSON line per review attempt, kept on the `nightshift/quality` branch
//! so a unit's numbers outlive its worktree (docs/DESIGN.md D39).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::artifacts::sha;
use super::blame::{self, Blame};
use super::metrics::{self, Attempt, RunStats, Unit};
use crate::clock;
use crate::review_md::{CycleEntry, Finding, Scope, Severity, Status};

/// The record format; bump it when a field changes meaning.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindingRecord {
    pub id: String,
    pub severity: Severity,
    pub axes: Vec<String>,
    pub axes_from_raised_by: bool,
    pub scope: Scope,
    pub cycle: Option<u32>,
    pub status: Status,
    pub fixed_cycle: Option<u32>,
    /// Relative `path:line`; dropped when the review wrote an absolute path.
    pub location: Option<String>,
    /// For an escape, the commit and PR that introduced its line, blamed when the record was
    /// written: the reviewed commit may be gone by the time the record is read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introduced_by: Option<Blame>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleRecord {
    pub id: String,
    pub axes: Vec<String>,
}

/// One review attempt of one unit. Every record a write produces for a unit shares its
/// `recorded` time; a reader keeps a unit's newest write and ignores the older ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub v: u32,
    pub unit: String,
    pub issue: Option<u64>,
    pub pr: Option<u64>,
    /// How the unit's run ended: `merged`, `done` or `stuck`. `None` for an import whose run
    /// log is gone.
    pub outcome: Option<String>,
    pub recorded: String,
    /// 1-based, oldest first, out of `attempts`.
    pub attempt: usize,
    pub attempts: usize,
    /// Relative to the worktree, e.g. `.ns/<unit>/review.md`.
    pub artifact: String,
    /// The artifact's `status:` when it is one lowercase word, else `None`.
    pub status: Option<String>,
    pub updated: Option<String>,
    /// The local date of `updated` where the record was written, for people reading the
    /// branch. `ns quality` recomputes the day from `updated` in its own `TZ`, as it does for
    /// a live worktree, so the two always agree.
    pub day: Option<String>,
    pub cycles: Option<u32>,
    pub blame_at: Option<String>,
    pub changed_lines: Option<u64>,
    pub changed_lines_source: Option<String>,
    pub stray_statuses: usize,
    pub findings: Vec<FindingRecord>,
    pub first_pass_file: Vec<CycleRecord>,
}

/// What `ns run` knows about a unit beyond its review artifacts.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub unit: String,
    pub issue: Option<u64>,
    pub pr: Option<u64>,
    pub outcome: Option<String>,
}

/// The issue number a unit id starts with (`142-uart-timeout`), if any.
pub fn issue_of(unit: &str) -> Option<u64> {
    let digits: String = unit.chars().take_while(char::is_ascii_digit).collect();
    let rest = &unit[digits.len()..];
    (rest.is_empty() || rest.starts_with('-'))
        .then(|| digits.parse().ok())
        .flatten()
}

/// Whether `s` holds an absolute path or a URL to one: a word, or a part of one after a `:`,
/// starting with `/`, `\\` or `~` and holding a path (`/opt/x`, `a.rs:/srv/x`, `C:\x`,
/// `\\server`, `~/x`, `~user/x`), or a `file:` or `ssh://` URL. `https://` URLs and `n/a`
/// are not paths. Records carry relative paths only.
pub fn absolute(s: &str) -> bool {
    s.split(|c: char| c.is_whitespace() || "`'\"()[]<>,;=".contains(c))
        .any(|w| {
            let lower = w.to_ascii_lowercase();
            lower.starts_with("file:")
                || lower.starts_with("ssh://")
                || w.split(':').any(|part| {
                    let b = part.as_bytes();
                    (b.len() > 1 && b[0] == b'/' && b[1] != b'/')
                        || (b.len() > 1 && b[0] == b'\\')
                        || (b.first() == Some(&b'~') && part.contains('/'))
                })
        })
}

/// A status as one lowercase word (`pass`, `blocked`), else `None`: free text stays out.
fn status_word(s: &Option<String>) -> Option<String> {
    s.as_ref()
        .filter(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
        .cloned()
}

/// Fails when any string in `v` holds an absolute path.
pub fn check_private(v: &Value) -> Result<()> {
    match v {
        Value::String(s) if absolute(s) => bail!("a record holds an absolute path: {s:?}"),
        Value::Array(a) => a.iter().try_for_each(check_private),
        Value::Object(o) => o.values().try_for_each(check_private),
        _ => Ok(()),
    }
}

fn finding(f: &Finding, introduced_by: Option<Blame>) -> FindingRecord {
    FindingRecord {
        id: f.id.clone(),
        severity: f.severity,
        axes: f.axes.clone(),
        axes_from_raised_by: f.axes_from_raised_by,
        scope: f.scope,
        cycle: f.cycle,
        status: f.status,
        fixed_cycle: f.fixed_cycle,
        location: f.location.clone().filter(|l| !absolute(l)),
        introduced_by,
    }
}

/// Each escape's blame at the attempt's reviewed commit, run in `git_at`, by finding index.
/// A blame that fails, or whose subject holds an absolute path, is left out.
fn escape_blames(a: &Attempt, git_at: &Path) -> BTreeMap<usize, Blame> {
    let escapes = metrics::escapes(a);
    a.findings
        .iter()
        .enumerate()
        .filter(|(_, f)| escapes.iter().any(|e| std::ptr::eq(*e, *f)))
        .filter_map(|(i, f)| {
            let b = blame::blame(git_at, a.blame_at.as_deref(), f.location.as_deref()).ok()?;
            (!absolute(&b.summary)).then_some((i, b))
        })
        .collect()
}

/// One record per attempt, oldest first, each checked for absolute paths. Escapes are blamed
/// in `git_at`, the unit's worktree or the repo.
pub fn build(
    meta: &Meta,
    attempts: &[Attempt],
    recorded: i64,
    git_at: &Path,
) -> Result<Vec<Record>> {
    let recorded = clock::iso(recorded);
    attempts
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let mut blames = escape_blames(a, git_at);
            let r = Record {
                v: VERSION,
                unit: meta.unit.clone(),
                issue: meta.issue,
                pr: meta.pr,
                outcome: meta.outcome.clone(),
                recorded: recorded.clone(),
                attempt: i + 1,
                attempts: attempts.len(),
                artifact: a.path.clone(),
                status: status_word(&a.status),
                updated: a.updated.map(clock::iso),
                day: a.updated.map(clock::local_date),
                cycles: a.cycles,
                blame_at: a.blame_at.clone(),
                changed_lines: a.changed_lines,
                changed_lines_source: a.changed_lines_source.map(String::from),
                stray_statuses: a.stray_statuses,
                findings: a
                    .findings
                    .iter()
                    .enumerate()
                    .map(|(i, f)| finding(f, blames.remove(&i)))
                    .collect(),
                first_pass_file: a
                    .first_pass_file
                    .iter()
                    .map(|e| CycleRecord {
                        id: e.id.clone(),
                        axes: e.axes.clone(),
                    })
                    .collect(),
            };
            check_private(&serde_json::to_value(&r)?)?;
            Ok(r)
        })
        .collect()
}

fn source(s: Option<&str>) -> Option<&'static str> {
    match s {
        Some("summary") => Some("summary"),
        Some("git") => Some("git"),
        _ => None,
    }
}

fn attempt(r: &Record) -> Attempt {
    Attempt {
        path: r.artifact.clone(),
        status: r.status.clone(),
        updated: r.updated.as_deref().and_then(clock::parse_iso),
        cycles: r.cycles,
        blame_at: r.blame_at.as_deref().and_then(sha),
        changed_lines: r.changed_lines,
        changed_lines_source: source(r.changed_lines_source.as_deref()),
        findings: r
            .findings
            .iter()
            .map(|f| Finding {
                id: f.id.clone(),
                severity: f.severity,
                title: String::new(),
                location: f.location.clone(),
                axes: f.axes.clone(),
                axes_from_raised_by: f.axes_from_raised_by,
                scope: f.scope,
                cycle: f.cycle,
                status: f.status,
                fixed_cycle: f.fixed_cycle,
            })
            .collect(),
        stray_statuses: r.stray_statuses,
        introduced_by: r
            .findings
            .iter()
            .enumerate()
            .filter_map(|(i, f)| Some((i, f.introduced_by.clone()?)))
            .collect(),
        first_pass_file: r
            .first_pass_file
            .iter()
            .map(|e| CycleEntry {
                id: e.id.clone(),
                axes: e.axes.clone(),
            })
            .collect(),
    }
}

/// Records parsed from JSONL text, and how many lines didn't parse.
pub fn parse(text: &str) -> (Vec<Record>, usize) {
    let mut bad = 0;
    let records = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let r = serde_json::from_str::<Record>(l).ok();
            bad += usize::from(r.is_none());
            r
        })
        .collect();
    (records, bad)
}

/// One unit per unit id, from its newest write: the latest `recorded` time among its records,
/// whatever their order (an outbox flushed late holds older writes), one attempt per index (a
/// repeated line counts once).
pub fn units(records: &[Record]) -> Vec<Unit> {
    let mut newest: BTreeMap<&str, &str> = BTreeMap::new();
    for r in records {
        let n = newest.entry(&r.unit).or_insert(&r.recorded);
        if r.recorded.as_str() > *n {
            *n = &r.recorded;
        }
    }
    let mut picked: BTreeMap<&str, BTreeMap<usize, &Record>> = BTreeMap::new();
    for r in records
        .iter()
        .filter(|r| newest[r.unit.as_str()] == r.recorded)
    {
        picked.entry(&r.unit).or_default().insert(r.attempt, r);
    }
    picked
        .into_iter()
        .map(|(id, by_index)| {
            let attempts: Vec<Attempt> = by_index.values().map(|r| attempt(r)).collect();
            let last = by_index
                .values()
                .last()
                .expect("a picked unit has a record");
            Unit {
                id: id.to_string(),
                worktree: None,
                day: attempts
                    .last()
                    .and_then(|a| a.updated)
                    .map(clock::local_date),
                attempts,
                run: RunStats {
                    outcome: last.outcome.clone(),
                    pr: last.pr,
                    ..RunStats::default()
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::metrics;
    use crate::review_md;
    use crate::testutil::{commit_file, g};
    use std::fs;

    const REVIEW: &str = "\
## Critical
### C1. Drops the record
- Location: `src/a.rs:3`
- Axis: correctness
- Scope: changed
- Cycle: 0
- Status: fixed (cycle 1, abc1234)
### I1. Old helper
- Location: `/srv/someone/repo/src/b.rs:9`
- Raised by: security
- Scope: pre-existing
- Cycle: 0
- Status: deferred: #50
";

    fn attempts() -> Vec<Attempt> {
        let first = Attempt {
            path: ".ns/1-a/history/review-1.md".into(),
            status: Some("fail".into()),
            updated: clock::parse_iso("2026-10-09T02:00:00Z"),
            cycles: None,
            changed_lines: Some(30),
            changed_lines_source: Some("git"),
            findings: review_md::parse("### I1. x\n- Status: open\n"),
            first_pass_file: review_md::cycle_entries("- I1 perf: x\n"),
            ..Attempt::default()
        };
        let last = Attempt {
            path: ".ns/1-a/review.md".into(),
            status: Some("pass".into()),
            updated: clock::parse_iso("2026-10-09T03:00:00Z"),
            cycles: Some(1),
            blame_at: Some("abc1234".into()),
            changed_lines: Some(40),
            changed_lines_source: Some("summary"),
            findings: review_md::parse(REVIEW),
            stray_statuses: 2,
            ..Attempt::default()
        };
        vec![first, last]
    }

    fn meta() -> Meta {
        Meta {
            unit: "1-a".into(),
            issue: Some(1),
            pr: Some(12),
            outcome: Some("merged".into()),
        }
    }

    #[test]
    fn a_record_holds_the_unit_and_each_finding_and_no_title() {
        let rs = build(
            &meta(),
            &attempts(),
            1_791_600_000,
            Path::new("/nonexistent"),
        )
        .unwrap();
        assert_eq!(rs.len(), 2);
        let v = serde_json::to_value(&rs[1]).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["unit"], "1-a");
        assert_eq!((v["issue"].as_u64(), v["pr"].as_u64()), (Some(1), Some(12)));
        assert_eq!(v["outcome"], "merged");
        assert_eq!(v["recorded"], "2026-10-10T02:40:00Z");
        assert_eq!(rs[0].recorded, rs[1].recorded);
        assert_eq!(
            (v["attempt"].as_u64(), v["attempts"].as_u64()),
            (Some(2), Some(2))
        );
        assert_eq!(v["artifact"], ".ns/1-a/review.md");
        assert_eq!(v["status"], "pass");
        assert_eq!(v["updated"], "2026-10-09T03:00:00Z");
        assert_eq!(v["day"], clock::local_date(1_791_514_800));
        assert_eq!(v["cycles"], 1);
        assert_eq!(v["changed_lines"], 40);
        assert_eq!(v["changed_lines_source"], "summary");
        assert_eq!(v["stray_statuses"], 2);
        assert_eq!(
            v["findings"][0],
            serde_json::json!({
                "id": "C1", "severity": "critical", "axes": ["correctness"],
                "axes_from_raised_by": false, "scope": "changed", "cycle": 0,
                "status": "fixed", "fixed_cycle": 1, "location": "src/a.rs:3"
            })
        );
        assert_eq!(v["findings"][1]["scope"], "pre-existing");
        assert_eq!(v["findings"][1]["axes_from_raised_by"], true);
        assert_eq!(rs[0].first_pass_file[0].axes, ["performance"]);
        assert!(!v.to_string().contains("Drops the record"), "{v}");
    }

    #[test]
    fn an_absolute_location_is_dropped_and_any_other_absolute_path_fails_the_record() {
        let rs = build(&meta(), &attempts(), 0, Path::new("/nonexistent")).unwrap();
        assert_eq!(rs[1].findings[1].location, None);
        assert!(!serde_json::to_string(&rs).unwrap().contains("/srv/"));
        let mut a = attempts();
        a[1].path = "/srv/someone/wt/.ns/1-a/review.md".into();
        let e = build(&meta(), &a, 0, Path::new("/nonexistent"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("absolute path"), "{e}");
        let m = Meta {
            unit: "x /srv/y".into(),
            ..meta()
        };
        assert!(build(&m, &attempts(), 0, Path::new("/nonexistent")).is_err());
    }

    #[test]
    fn a_status_that_is_not_one_lowercase_word_is_left_out() {
        let mut a = attempts();
        a[0].status = Some("pass C:\\Users\\x".into());
        a[1].status = Some("Pass".into());
        let rs = build(&meta(), &a, 0, Path::new("/nonexistent")).unwrap();
        assert_eq!(
            (rs[0].status.as_deref(), rs[1].status.as_deref()),
            (None, None)
        );
        a[0].status = Some("needs-human".into());
        let rs = build(&meta(), &a, 0, Path::new("/nonexistent")).unwrap();
        assert_eq!(rs[0].status.as_deref(), Some("needs-human"));
        a[0].status = Some(String::new());
        assert_eq!(
            build(&meta(), &a, 0, Path::new("/nonexistent")).unwrap()[0].status,
            None
        );
    }

    #[test]
    fn absolute_paths_are_spotted_inside_text() {
        for abs in [
            "/opt/x/a.rs:3",
            "see `/etc/passwd`",
            "~/repo/a.rs",
            "C:\\src\\a.rs",
            "d:/src/a.rs",
            "\\\\server\\share",
            "(/tmp/x)",
            "file:///srv/x",
            "FILE:x",
            "ssh://host/srv/x",
            "a.rs:/opt/x:3",
            "~user/repo/a.rs",
            "\\src\\a.rs",
        ] {
            assert!(absolute(abs), "{abs}");
        }
        for rel in [
            "src/a.rs:3",
            "a.rs:3 / b.rs:4",
            "n/a",
            "",
            "https://github.com/o/r/pull/1",
            "fixed (cycle 1, abc)",
            "//comment",
            "~5 lines",
            "x:\\",
        ] {
            assert!(!absolute(rel), "{rel}");
        }
        assert!(check_private(&serde_json::json!({"a": [1, {"b": "src/x"}]})).is_ok());
        assert!(check_private(&serde_json::json!({"a": [1, {"b": "/src/x"}]})).is_err());
    }

    #[test]
    fn issue_of_reads_a_leading_number() {
        assert_eq!(issue_of("142-uart-timeout"), Some(142));
        assert_eq!(issue_of("142"), Some(142));
        assert_eq!(issue_of("uart-142"), None);
        assert_eq!(issue_of("142x-a"), None);
        assert_eq!(issue_of(""), None);
    }

    #[test]
    fn records_read_back_give_the_same_numbers() {
        let live = Unit {
            id: "1-a".into(),
            worktree: Some("/w".into()),
            day: Some(clock::local_date(1_791_514_800)),
            attempts: attempts(),
            run: RunStats::default(),
        };
        let rs = build(&meta(), &live.attempts, 0, Path::new("/nonexistent")).unwrap();
        let text: String = rs
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect();
        let (back, bad) = parse(&text);
        assert_eq!(bad, 0);
        let us = units(&back);
        assert_eq!(us.len(), 1);
        let u = &us[0];
        assert_eq!(
            (u.worktree.as_deref(), u.day.as_deref()),
            (None, live.day.as_deref())
        );
        assert_eq!(
            (u.run.outcome.as_deref(), u.run.pr),
            (Some("merged"), Some(12))
        );
        let s = |u: &Unit| serde_json::to_value(metrics::summarize(&[u])).unwrap();
        let mut want = live.clone();
        for f in &mut want.attempts[1].findings {
            if f.id == "I1" {
                f.location = None;
            }
        }
        assert_eq!(s(u), s(&want));
        assert_eq!(u.last().blame_at.as_deref(), Some("abc1234"));
        assert_eq!(u.first().first_pass_file, want.first().first_pass_file);
        assert_eq!(u.first().changed_lines_source, Some("git"));
    }

    #[test]
    fn a_blame_at_that_is_not_a_sha_never_reaches_git() {
        let mut r = rec("u", "t", 1, "pass");
        for bad in ["--contents=x", "HEAD", "abc"] {
            r.blame_at = Some(bad.into());
            assert_eq!(attempt(&r).blame_at, None, "{bad}");
        }
        r.blame_at = Some("abc1234".into());
        assert_eq!(attempt(&r).blame_at.as_deref(), Some("abc1234"));
    }

    #[test]
    fn an_escape_is_blamed_when_its_record_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        g(d, &["init", "-q", "-b", "main"]);
        commit_file(d, "a.txt", "one\n");
        fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
        g(d, &["commit", "-qam", "feat: add two (#41)"]);
        let head = g(d, &["rev-parse", "HEAD"]);
        let a = Attempt {
            path: "r.md".into(),
            blame_at: Some(head.clone()),
            findings: review_md::parse(
                "### I1. escape\n- Location: `a.txt:2`\n- Scope: pre-existing\n### I2. ours\n- Location: `a.txt:2`\n- Scope: changed\n### I3. gone\n- Location: `gone.txt:1`\n- Scope: pre-existing\n### I4. waved off\n- Location: `a.txt:2`\n- Scope: pre-existing\n- Status: dismissed: no\n",
            ),
            ..Attempt::default()
        };
        let rs = build(&meta(), &[a], 0, d).unwrap();
        let b = rs[0].findings[0].introduced_by.as_ref().unwrap();
        assert_eq!((b.commit.as_str(), b.pr), (head.as_str(), Some(41)));
        for i in 1..4 {
            assert_eq!(rs[0].findings[i].introduced_by, None, "{i}");
        }
        let back = attempt(&rs[0]);
        assert_eq!(back.introduced_by.keys().collect::<Vec<_>>(), [&0]);
        let v = serde_json::to_value(&rs[0]).unwrap();
        assert!(v["findings"][1].get("introduced_by").is_none(), "{v}");
    }

    fn rec(unit: &str, recorded: &str, attempt: usize, status: &str) -> Record {
        let mut r = build(
            &Meta {
                unit: unit.into(),
                ..Meta::default()
            },
            &[Attempt {
                path: "r.md".into(),
                status: Some(status.into()),
                ..Attempt::default()
            }],
            0,
            Path::new("/nonexistent"),
        )
        .unwrap()
        .remove(0);
        r.recorded = recorded.into();
        r.attempt = attempt;
        r
    }

    #[test]
    fn a_unit_comes_from_its_newest_write_and_repeats_count_once() {
        let rs = [
            rec("u", "t1", 1, "fail"),
            rec("u", "t1", 2, "fail"),
            rec("w", "t1", 1, "pass"),
            rec("u", "t2", 1, "pass"),
            rec("u", "t2", 1, "pass"),
        ];
        let us = units(&rs);
        let got: Vec<_> = us
            .iter()
            .map(|u| (u.id.as_str(), u.attempts.len(), u.last().status.as_deref()))
            .collect();
        assert_eq!(got, [("u", 1, Some("pass")), ("w", 1, Some("pass"))]);
    }

    #[test]
    fn an_older_write_later_in_the_file_does_not_win() {
        let rs = [
            rec("u", "2026-10-10T02:00:00Z", 1, "pass"),
            rec("u", "2026-10-09T02:00:00Z", 1, "fail"),
        ];
        assert_eq!(units(&rs)[0].last().status.as_deref(), Some("pass"));
    }

    #[test]
    fn bad_lines_are_counted_not_fatal() {
        let good = serde_json::to_string(&rec("u", "t", 1, "pass")).unwrap();
        let (rs, bad) = parse(&format!("{good}\nnot json\n\n{{\"unit\":\"x\"}}\n"));
        assert_eq!((rs.len(), bad), (1, 2));
    }
}
