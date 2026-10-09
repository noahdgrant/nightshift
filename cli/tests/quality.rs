//! `ns quality` end to end: a temp repo with unit worktrees holding fixture review artifacts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use tempfile::TempDir;

/// UTC-4 with no daylight saving, so the test needs no tz database.
const TZ: &str = "XYZ4";

fn ns() -> Command {
    let mut c = Command::cargo_bin("ns").unwrap();
    c.env_remove("NS_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("TZ", TZ);
    c
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str, msg: &str) -> String {
    fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

fn write(path: PathBuf, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
}

/// A unit worktree `<root>.worktrees/<unit>` on branch `ns/<unit>`, with one commit.
fn unit_worktree(root: &Path, unit: &str) -> (PathBuf, String) {
    let wt = root.with_file_name(format!("myrepo.worktrees/{unit}"));
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("ns/{unit}"),
            wt.to_str().unwrap(),
            "main",
        ],
    );
    let head = commit(&wt, "work.txt", "a\nb\nc\n", "work");
    (wt, head)
}

fn review(fm: &str, body: &str) -> String {
    format!("---\nphase: review\n{fm}---\n\n# Review\n\n{body}")
}

/// Unit `a`: the agreed field format, stopped at the cycle limit with one open Important and a
/// pre-existing finding. Unit `b`: an older format with an archived first attempt and a
/// first-pass cycle file. Unit `c`: no frontmatter. The main checkout's own `.ns/` is ignored.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap().join("myrepo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    commit(&root, "lib.txt", "one\n", "init");
    let base = commit(&root, "lib.txt", "one\ntwo\n", "feat: add two (#41)");

    let (a, _) = unit_worktree(&root, "1-alpha");
    write(
        a.join(".ns/1-alpha/review.md"),
        &review(
            &format!(
                "status: blocked\nupdated: 2026-10-09T02:30:00Z\nbase: main@{}\ncycles: 3\n",
                &base[..7]
            ),
            "Open after 3 fix cycles: I2.

## Summary
Change size: 150 insertions, 50 deletions in 3 files.

## Critical
### C1. Drops the record
- Location: `work.txt:1`
- Axis: correctness
- Scope: changed
- Cycle: 0
- Status: fixed (cycle 1, abc1234)

## Important
### I1. Old helper swallows errors
- Location: `lib.txt:2`
- Axis: correctness, security
- Scope: pre-existing
- Cycle: 0
- Status: deferred: #50
### I2. Untested branch
- **Location:** `work.txt:3`
- **Axis:** tests
- **Scope:** changed
- **Cycle:** 2
- **Status:** open

## Suggestion
### S1. Rename
- Axis: readability
- Scope: changed
- Cycle: 0
- Status: open
",
        ),
    );

    let (b, b_head) = unit_worktree(&root, "2-beta");
    write(
        b.join(".ns/2-beta/history/review-1.md"),
        &review(
            &format!("status: pass\nupdated: 2026-10-10T12:00:00Z\nbase: main@{base}\nhead: {b_head}\ncycles: 2\n"),
            "## Important
### I1. Missing test
- Raised by: tests (claude)
- Status: fixed (cycle 1, 1111111)
### I2. Dense function
- Raised by: readability, architecture
- Status: fixed (cycle 2, 2222222)
### I3. Wrong anyway
- Raised by: spec
- Status: dismissed: the brief says so
",
        ),
    );
    write(
        b.join(".ns/2-beta/review.md"),
        &review(
            "status: pass\nupdated: 2026-10-11T03:00:00Z\ncycles: 0\n",
            "## Summary\nChange size: 40 lines in 1 file.\n\n## Suggestion\n- S1. Naming (readability). Open.\n",
        ),
    );
    write(
        b.join(".ns/2-beta/history/review-1/cycle-1.md"),
        "# Cycle 1\n- I1 tests: add one\n",
    );
    write(b.join(".ns/2-beta/history/review-timeout-3"), "");
    fs::create_dir_all(b.join(".ns/2-beta/history/review-timeout-2-0900")).unwrap();
    write(b.join(".ns/2-beta/build.md"), "---\nphase: build\n---\n");

    let (c, _) = unit_worktree(&root, "3-gamma");
    write(c.join(".ns/3-gamma/review.md"), "### I1. no frontmatter\n");
    write(c.join(".ns/4-delta/brief.md"), "---\nphase: triage\n---\n");

    write(
        root.join(".ns/9-main/review.md"),
        &review(
            "status: pass\ncycles: 0\n",
            "### C1. in main\n- Status: open\n",
        ),
    );
    write(
        root.join(".git/ns/runs.jsonl"),
        &[
            json!({"event":"phase","phase":"review","cost_usd":2.0,"unit":"1-alpha","ts":"2026-10-09T02:00:00Z"}),
            json!({"event":"end","outcome":"stuck","unit":"1-alpha","ts":"2026-10-09T02:31:00Z"}),
            json!({"event":"phase","phase":"review","cost_usd":1.5,"unit":"2-beta","ts":"2026-10-11T02:00:00Z"}),
            json!({"event":"merged","pr":77,"unit":"2-beta","ts":"2026-10-11T04:00:00Z"}),
            json!({"event":"phase","phase":"review","cost_usd":0.5,"unit":"8-gone","ts":"2026-10-11T02:00:00Z"}),
            json!({"event":"start","unit":"7-unreviewed","ts":"2026-10-11T02:00:00Z"}),
        ]
        .iter()
        .map(|v| format!("{v}\n"))
        .collect::<String>(),
    );
    Fixture { _tmp: tmp, root }
}

fn quality(f: &Fixture, args: &[&str]) -> Value {
    let out = ns()
        .current_dir(&f.root)
        .arg("quality")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn reports_every_metric_from_fixture_artifacts() {
    let f = fixture();
    let v = quality(&f, &["--json"]);
    assert_eq!(v["ok"], true);
    assert_eq!(v["units"], 2);
    assert_eq!(
        v["findings"],
        json!({
            "total": 5, "critical": 1, "important": 2, "suggestion": 2,
            "by_status": {"deferred": 1, "fixed": 1, "open": 3},
            "by_axis": {"correctness": 2, "readability": 2, "security": 1, "tests": 1}
        })
    );
    let fp = &v["first_pass"];
    assert_eq!(
        (
            fp["clean"].as_u64(),
            fp["dirty"].as_u64(),
            fp["unknown"].as_u64()
        ),
        (Some(0), Some(2), Some(0))
    );
    assert_eq!(fp["yield"], 0.0);
    // a: C1 in 200 lines (I1 is pre-existing, so an escape, not a first-pass finding). b's first
    // attempt: its cycle-1.md lists one finding; its change size comes from git (3 lines).
    assert_eq!(fp["measured_units"], 2);
    assert_eq!(fp["blocking_findings"], 2);
    assert_eq!(fp["changed_lines"], 203);
    assert_eq!(fp["per_100_lines"], 0.99);
    assert_eq!(
        fp["by_axis"]["correctness"],
        json!({"findings": 1, "per_100_lines": 0.49})
    );
    assert_eq!(
        fp["by_axis"]["security"],
        json!({"findings": 0, "per_100_lines": 0.0})
    );
    assert_eq!(
        fp["by_axis"]["tests"],
        json!({"findings": 1, "per_100_lines": 0.49})
    );
    assert_eq!(
        fp["by_axis"]["spec"],
        json!({"findings": 0, "per_100_lines": 0.0})
    );
    assert_eq!(
        v["cycles_to_clean"],
        json!({"units": 1, "median": 0.0, "max": 0, "not_clean": 1, "unknown": 0})
    );
    assert_eq!(v["leftovers"], 1);
    assert_eq!(v["leftover_findings"][0]["id"], "I2");
    assert_eq!(v["leftover_findings"][0]["unit"], "1-alpha");
    assert_eq!(v["escapes"], 1);
    let e = &v["escape_findings"][0];
    assert_eq!(
        (e["unit"].as_str(), e["id"].as_str()),
        (Some("1-alpha"), Some("I1"))
    );

    let units: Vec<_> = v["per_unit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["unit"].as_str().unwrap())
        .collect();
    assert_eq!(units, ["1-alpha", "2-beta"]);
    let b = &v["per_unit"][1];
    assert_eq!(b["attempts"], 2);
    assert_eq!(b["artifact"], ".ns/2-beta/review.md");
    assert_eq!(b["changed_lines_source"], "git");
    assert_eq!(b["first_pass_blocking"], 1);
    assert_eq!(b["reached_clean"], "clean");
    assert_eq!(b["cycles_to_clean"], 0);
    assert_eq!(v["per_unit"][0]["reached_clean"], "not_clean");

    let unparsed: Vec<_> = v["gaps"]["unparsed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        unparsed,
        ["cycle files with no review artifact", "no frontmatter"]
    );
    assert_eq!(v["gaps"]["findings_without_scope"], 1);
    assert_eq!(v["gaps"]["findings_axis_from_raised_by"], 1);
}

#[test]
fn days_are_local_and_since_filters_by_them() {
    let f = fixture();
    let v = quality(&f, &[]);
    let days: Vec<_> = v["trend"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["day"].as_str().unwrap().to_string(),
                d["units"].as_u64().unwrap(),
            )
        })
        .collect();
    // 02:30Z on the 9th is the 8th at UTC-4; 03:00Z on the 11th is the 10th.
    assert_eq!(
        days,
        [("2026-10-08".to_string(), 1), ("2026-10-10".to_string(), 1)]
    );
    assert_eq!(v["per_unit"][0]["day"], "2026-10-08");

    let v = quality(&f, &["--since", "2026-10-09"]);
    assert_eq!(v["since"], "2026-10-09");
    assert_eq!(v["units"], 1);
    assert_eq!(v["per_unit"][0]["unit"], "2-beta");

    let v = quality(&f, &["--since", "2026-10-11"]);
    assert_eq!(v["units"], 0);
    assert_eq!(v["first_pass"]["yield"], Value::Null);
    assert_eq!(v["trend"], json!([]));
}

#[test]
fn a_repo_with_no_units_reports_zeroes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    let out = ns().current_dir(&root).arg("quality").output().unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["units"], 0);
    assert_eq!(v["cycles_to_clean"]["median"], Value::Null);
}

#[test]
fn an_undated_unit_trends_last_and_since_drops_it() {
    let f = fixture();
    let (wt, _) = unit_worktree(&f.root, "5-undated");
    write(
        wt.join(".ns/5-undated/review.md"),
        &review("status: pass\ncycles: 0\n", ""),
    );
    let v = quality(&f, &[]);
    let last = v["trend"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (last["day"].clone(), last["units"].clone()),
        (Value::Null, json!(1))
    );
    assert_eq!(v["gaps"]["units_without_updated"], 1);
    assert_eq!(quality(&f, &["--since", "2026-01-01"])["units"], 2);
}

#[test]
fn usage_errors_exit_2_with_a_correct_invocation() {
    let f = fixture();
    ns().current_dir(&f.root)
        .args(["quality", "--since", "10/09/2026"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("is not a date"))
        .stderr(predicate::str::contains("ns quality --since 2026-10-01"));
    let tmp = tempfile::tempdir().unwrap();
    ns().current_dir(tmp.path())
        .arg("quality")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not inside a git repository"));
}

#[test]
fn a_deleted_worktree_is_skipped_and_a_duplicate_unit_keeps_the_lower_path() {
    let f = fixture();
    let wts = f.root.with_file_name("myrepo.worktrees");
    fs::remove_dir_all(wts.join("2-beta")).unwrap();
    write(
        wts.join("3-gamma/.ns/1-alpha/review.md"),
        &review(
            "status: pass\ncycles: 0\n",
            "### C9. a copy\n- Status: open\n",
        ),
    );
    let v = quality(&f, &[]);
    assert_eq!(v["units"], 1);
    assert_eq!(v["per_unit"][0]["unit"], "1-alpha");
    assert_eq!(
        v["per_unit"][0]["worktree"],
        wts.join("1-alpha").display().to_string()
    );
    assert_eq!(v["findings"]["critical"], 1);
}
