//! `ns quality` end to end: a temp repo with unit worktrees holding fixture review artifacts.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use common::Ns;
use predicates::prelude::*;
use serde_json::{json, Value};
use tempfile::TempDir;

/// UTC-4 with no daylight saving, so the test needs no tz database.
const TZ: &str = "XYZ4";

fn ns() -> Ns {
    let mut c = common::ns();
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
    base: String,
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
/// pre-existing finding on a line the unit moved down by adding one above it. Unit `b`: an older format with an archived first attempt and a
/// first-pass cycle file. Unit `c`: no frontmatter. The main checkout's own `.ns/` is ignored.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap().join("myrepo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    commit(&root, "lib.txt", "one\n", "init");
    let base = commit(&root, "lib.txt", "one\ntwo\n", "feat: add two (#41)");

    let (a, _) = unit_worktree(&root, "1-alpha");
    let a_head = commit(&a, "lib.txt", "zero\none\ntwo\n", "unit adds a line");
    write(
        a.join(".ns/1-alpha/review.md"),
        &review(
            &format!(
                "status: blocked\nsha: {}\nupdated: 2026-10-09T02:30:00Z\nbase: main@{}\ncycles: 3\n",
                &a_head[..7],
                &base[..7]
            ),
            "Open after at least 2 fix attempts: I2.

## Summary
Change size: 150 insertions, 50 deletions in 3 files.

## Critical
### C1. Drops the record
- Location: `work.txt:1`
- Axis: correctness
- Scope: changed
- Cycle: 0
- Attempts: 2
- Status: fixed (cycle 1, abc1234)

## Important
### I1. Old helper swallows errors
- Location: `lib.txt:3`
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
            // 08:00 on the 9th at UTC-4: on the --since day itself.
            json!({"event":"phase","phase":"review","cost_usd":0.25,"unit":"8-gone","ts":"2026-10-09T12:00:00Z"}),
        ]
        .iter()
        .map(|v| format!("{v}\n"))
        .collect::<String>(),
    );
    Fixture {
        _tmp: tmp,
        root,
        base,
    }
}

fn quality(f: &Fixture, args: &[&str]) -> Value {
    let out = ns().current_dir(&f.root).arg("quality").args(args).output();
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
            "total": 5, "critical": 1, "important": 2, "suggestion": 2, "unknown_severity": 0,
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
    assert_eq!(
        e["introduced_by"],
        json!({"commit": f.base, "summary": "feat: add two (#41)", "pr": 41})
    );
    assert_eq!(e["blame_error"], Value::Null);

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
    let a = &v["per_unit"][0];
    assert_eq!(a["reached_clean"], "not_clean");
    assert_eq!(a["cycles_to_clean"], Value::Null);
    assert_eq!(
        (&a["critical"], &a["important"], &a["suggestion"]),
        (&json!(1), &json!(2), &json!(1))
    );
    assert_eq!((&a["leftovers"], &a["escapes"]), (&json!(1), &json!(1)));
    assert_eq!(a["first_pass"], "dirty");
    assert_eq!(a["first_pass_blocking"], 1);
    assert_eq!(
        b["run"],
        json!({"review_runs": 1, "review_cost_usd": 1.5, "outcome": null, "pr": 77})
    );
    assert_eq!(v["per_unit"][0]["run"]["outcome"], "stuck");

    assert_eq!(v["run_log"]["review_runs"], 4);
    assert_eq!(v["run_log"]["units_without_artifacts"], json!(["8-gone"]));
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

/// a's C1 (pass 0, fixed in cycle 1 on the second attempt), I1 (pass 0, deferred) and I2 (pass
/// 2, open). b's first attempt predates `Cycle:`, so its three Importants are left out.
fn fixture_finding_flow() -> Value {
    json!({
        "findings": 3,
        "without_cycle": 3,
        "raised_per_pass": {
            "critical": {"passes": [1, 0, 0, 0, 0, 0], "later": 0},
            "important": {"passes": [1, 0, 1, 0, 0, 0], "later": 0}
        },
        "fix_rate": {
            "critical": {
                "findings": 1, "fixed_first_attempt": 0, "fixed_second_attempt": 1,
                "fixed_later_attempt": 0, "open": 0, "fixed_attempts_unknown": 0,
                "first_attempt_rate": 0.0, "second_attempt_rate": 1.0, "open_rate": 0.0
            },
            "important": {
                "findings": 1, "fixed_first_attempt": 0, "fixed_second_attempt": 0,
                "fixed_later_attempt": 0, "open": 1, "fixed_attempts_unknown": 0,
                "first_attempt_rate": 0.0, "second_attempt_rate": 0.0, "open_rate": 1.0
            }
        },
        "cycles_to_fix": {"findings": 1, "median": 1.0, "max": 1, "next_cycle": 1, "left_out": 0},
        "dismiss_rate": {"count": 0, "findings": 3, "rate": 0.0},
        "late_finding_rate": {"count": 1, "findings": 3, "rate": 0.333}
    })
}

#[test]
fn reports_how_findings_arrive_and_get_fixed() {
    let f = fixture();
    let v = quality(&f, &[]);
    assert_eq!(v["finding_flow"], fixture_finding_flow());
    assert_eq!(
        v["cycles_to_clean"],
        json!({"units": 1, "median": 0.0, "max": 0, "not_clean": 1, "unknown": 0})
    );
    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => m.iter().for_each(|(k, v)| {
                out.push(k.clone());
                keys(v, out)
            }),
            Value::Array(a) => a.iter().for_each(|v| keys(v, out)),
            _ => {}
        }
    }
    let mut all = Vec::new();
    keys(&v, &mut all);
    assert!(
        !all.iter().any(|k| k.contains("wall") || k.contains("time")),
        "no wall-time key: {all:?}"
    );
}

#[test]
fn human_output_shows_each_finding_metric() {
    let f = fixture();
    let out = ns()
        .current_dir(&f.root)
        .args(["quality", "--human"])
        .output();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    for line in [
        "Units: 2",
        "First-pass yield: 0.000 (0 clean, 2 dirty, 0 unknown)",
        "Cycles to clean: median 0.0, max 0 (1 not clean, 0 unknown)",
        "Leftovers: 1. Escapes: 1.",
        "Findings: 3 Critical and Important over every review attempt; 3 without a cycle left out",
        "Raised per pass    0    1    2    3    4    5  later",
        "  critical         1    0    0    0    0    0      0",
        "  important        1    0    1    0    0    0      0",
        "Fix rate           findings  attempt 1  attempt 2 (of the rest)  open  attempts unknown",
        "  critical                1      0.000                    1.000 0.000                 0",
        "  important               1      0.000                    0.000 1.000                 0",
        "Cycles to fix: median 1.0, max 1, over 1 fixed (1 in the next cycle; 0 left out)",
        "Dismiss rate: 0.000 (0 of 3)",
        "Late-finding rate: 0.333 (1 of 3)",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    ns().current_dir(&f.root)
        .args(["quality", "--human", "--json"])
        .assert()
        .code(2);
    ns().current_dir(&f.root)
        .args(["quality", "--human", "--bugs"])
        .assert()
        .code(2);
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

    // --since is UTC: 02:30Z on the 9th is kept from the 9th though its local day is the 8th.
    let v = quality(&f, &["--since", "2026-10-09"]);
    assert_eq!(v["since"], "2026-10-09T00:00:00Z");
    assert_eq!(v["units"], 2);
    assert_eq!(v["run_log"]["review_runs"], 4);

    let v = quality(&f, &["--since", "2026-10-09T02:30:01Z"]);
    assert_eq!(v["units"], 1);
    assert_eq!(v["per_unit"][0]["unit"], "2-beta");
    assert_eq!(v["run_log"]["review_runs"], 3);

    let v = quality(&f, &["--since", "2026-10-11T03:00:01Z"]);
    assert_eq!(v["units"], 0);
    assert_eq!(v["first_pass"]["yield"], Value::Null);
    assert_eq!(v["trend"], json!([]));
}

fn quality_tz(f: &Fixture, tz: &str, args: &[&str]) -> Value {
    let out = ns()
        .current_dir(&f.root)
        .env("TZ", tz)
        .arg("quality")
        .args(args)
        .output();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Adds unit `6-late`, reviewed at 02:43Z on the 10th: the evening of the 9th in New York.
fn late_unit(f: &Fixture) {
    let (wt, _) = unit_worktree(&f.root, "6-late");
    write(
        wt.join(".ns/6-late/review.md"),
        &review(
            "status: pass
updated: 2026-10-10T02:43:00Z
cycles: 0
",
            "",
        ),
    );
    let log = f.root.join(".git/ns/runs.jsonl");
    let mut text = fs::read_to_string(&log).unwrap();
    text.push_str(
        &json!({"event":"phase","phase":"review","cost_usd":0.75,"unit":"6-late","ts":"2026-10-10T02:43:00Z"})
            .to_string(),
    );
    fs::write(log, text + "\n").unwrap();
}

fn selected(v: &Value) -> (Vec<String>, Value, Value) {
    let units = v["per_unit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["unit"].as_str().unwrap().to_string())
        .collect();
    (
        units,
        v["run_log"]["events"].clone(),
        v["run_log"]["review_runs"].clone(),
    )
}

#[test]
fn since_filters_a_unit_by_its_last_attempt() {
    let f = fixture();
    for since in ["2026-10-10T12:00:01Z", "2026-10-11T03:00:00Z"] {
        let v = quality(&f, &["--since", since]);
        assert_eq!(v["units"], 1, "{since}");
        assert_eq!(v["per_unit"][0]["unit"], "2-beta", "{since}");
    }
    assert_eq!(
        quality(&f, &["--since", "2026-10-11T03:00:01Z"])["units"],
        0
    );
}

#[test]
fn since_limits_the_finding_flow_and_the_human_text_to_the_units_in_range() {
    let f = fixture();
    let since = ["--since", "2026-10-10T12:00:01Z"];
    let v = quality(&f, &since);
    assert_eq!(v["finding_flow"]["findings"], 0);
    assert_eq!(v["finding_flow"]["without_cycle"], 3);
    let out = ns()
        .current_dir(&f.root)
        .args(["quality", "--human"])
        .args(since)
        .output();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    for line in [
        "Units: 1",
        "Findings: 0 Critical and Important over every review attempt; 3 without a cycle left out",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
}

#[test]
fn since_a_date_does_not_depend_on_tz() {
    let f = fixture();
    late_unit(&f);
    // New York's rules spelled out, so the test needs no tz database.
    let ny = quality_tz(&f, "EST5EDT,M3.2.0,M11.1.0", &["--since", "2026-10-10"]);
    let utc = quality_tz(&f, "UTC0", &["--since", "2026-10-10"]);
    assert_eq!(selected(&ny), selected(&utc));
    assert_eq!(
        selected(&ny),
        (
            vec!["2-beta".to_string(), "6-late".to_string()],
            json!(5),
            json!(3)
        )
    );
}

#[test]
fn since_a_date_is_midnight_utc() {
    let f = fixture();
    late_unit(&f);
    assert_eq!(
        quality(&f, &["--since", "2026-10-10"]),
        quality(&f, &["--since", "2026-10-10T00:00:00Z"])
    );
    let v = quality(&f, &["--since", "2026-10-10T02:43:01Z"]);
    assert_eq!(selected(&v).0, ["2-beta"]);
}

#[test]
fn until_a_date_keeps_that_whole_utc_day() {
    let f = fixture();
    let v = quality(&f, &["--until", "2026-10-09"]);
    assert_eq!(v["until"], "2026-10-09T23:59:59Z");
    assert_eq!(v["since"], Value::Null);
    // 1-alpha's runs at 02:00Z and 02:31Z and 8-gone's at 12:00Z, all on the 9th.
    assert_eq!(
        selected(&v),
        (vec!["1-alpha".to_string()], json!(3), json!(2))
    );
    assert_eq!(
        quality(&f, &["--until", "2026-10-09"]),
        quality(&f, &["--until", "2026-10-09T23:59:59Z"])
    );
    assert_eq!(quality(&f, &["--until", "2026-10-08"])["units"], 0);
}

#[test]
fn until_an_instant_is_inclusive_and_filters_a_unit_by_its_last_attempt() {
    let f = fixture();
    assert_eq!(
        selected(&quality(&f, &["--until", "2026-10-09T02:30:00Z"])).0,
        ["1-alpha"]
    );
    assert_eq!(
        quality(&f, &["--until", "2026-10-09T02:29:59Z"])["units"],
        0
    );
    // 2-beta's first attempt is 12:00Z on the 10th, its last 03:00Z on the 11th.
    assert_eq!(
        selected(&quality(&f, &["--until", "2026-10-10T12:00:00Z"])).0,
        ["1-alpha"]
    );
    assert_eq!(
        quality(&f, &["--until", "2026-10-11T03:00:00Z"])["units"],
        2
    );
}

#[test]
fn since_and_until_select_a_closed_range() {
    let f = fixture();
    let v = quality(&f, &["--since", "2026-10-09", "--until", "2026-10-09"]);
    assert_eq!(
        (v["since"].clone(), v["until"].clone()),
        (json!("2026-10-09T00:00:00Z"), json!("2026-10-09T23:59:59Z"))
    );
    assert_eq!(
        selected(&v),
        (vec!["1-alpha".to_string()], json!(3), json!(2))
    );
    let at = "2026-10-09T02:30:00Z";
    let v = quality(&f, &["--since", at, "--until", at]);
    assert_eq!(
        selected(&v),
        (vec!["1-alpha".to_string()], json!(0), json!(0))
    );
    assert_eq!(
        quality(
            &f,
            &[
                "--since",
                "2026-10-09T02:30:01Z",
                "--until",
                "2026-10-11T02:59:59Z"
            ]
        )["units"],
        0
    );
}

#[test]
fn until_a_day_and_since_the_next_split_with_no_overlap_or_gap() {
    let f = fixture();
    late_unit(&f);
    let all = selected(&quality(&f, &[]));
    for tz in ["EST5EDT,M3.2.0,M11.1.0", "UTC0"] {
        let before = selected(&quality_tz(&f, tz, &["--until", "2026-10-09"]));
        let after = selected(&quality_tz(&f, tz, &["--since", "2026-10-10"]));
        assert_eq!(before.0, ["1-alpha"], "{tz}");
        assert_eq!([before.0, after.0].concat(), all.0, "{tz}");
        let n = |v: &Value| v.as_u64().unwrap();
        assert_eq!(n(&before.1) + n(&after.1), n(&all.1), "{tz}");
        assert_eq!(n(&before.2) + n(&after.2), n(&all.2), "{tz}");
    }
}

#[test]
fn a_blame_that_cannot_run_is_reported_not_fatal() {
    let f = fixture();
    let p = f
        .root
        .with_file_name("myrepo.worktrees/1-alpha/.ns/1-alpha/review.md");
    let text = fs::read_to_string(&p)
        .unwrap()
        .replace("lib.txt:3", "gone.txt:3");
    fs::write(&p, text).unwrap();
    let v = quality(&f, &[]);
    let e = &v["escape_findings"][0];
    assert_eq!(e["introduced_by"], Value::Null);
    assert!(
        e["blame_error"].as_str().unwrap().contains("git blame"),
        "{e}"
    );
}

#[test]
fn a_repo_with_no_units_reports_zeroes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    let out = ns().current_dir(&root).arg("quality").output();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["units"], 0);
    assert_eq!(v["run_log"]["present"], false);
    assert_eq!(v["cycles_to_clean"]["median"], Value::Null);
    assert_eq!(v["finding_flow"]["findings"], 0);
    assert_eq!(v["finding_flow"]["dismiss_rate"]["rate"], Value::Null);
    assert_eq!(v["finding_flow"]["cycles_to_fix"]["median"], Value::Null);
    let out = ns()
        .current_dir(&root)
        .args(["quality", "--human"])
        .output();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Dismiss rate: n/a (0 of 0)"), "{text}");
    assert!(
        text.contains("Cycles to clean: median n/a, max n/a (0 not clean, 0 unknown)"),
        "{text}"
    );
    assert!(text.contains("Cycles to fix: n/a, over 0 fixed"), "{text}");
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
    assert_eq!(quality(&f, &["--until", "2099-01-01"])["units"], 2);
}

#[test]
fn usage_errors_exit_2_with_a_correct_invocation() {
    let f = fixture();
    ns().current_dir(&f.root)
        .args(["quality", "--since", "10/09/2026"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "YYYY-MM-DDTHH:MM:SSZ or YYYY-MM-DD",
        ))
        .stderr(predicate::str::contains(
            "ns quality --since 2026-10-01T00:00:00Z",
        ));
    ns().current_dir(&f.root)
        .args(["quality", "--since", "garbage"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "YYYY-MM-DDTHH:MM:SSZ or YYYY-MM-DD",
        ));
    ns().current_dir(&f.root)
        .args(["quality", "--until", "2026-10-9"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "--until \"2026-10-9\" is not a UTC time: use YYYY-MM-DDTHH:MM:SSZ or YYYY-MM-DD (23:59:59 UTC)",
        ))
        .stderr(predicate::str::contains(
            "ns quality --until 2026-10-01T23:59:59Z",
        ));
    ns().current_dir(&f.root)
        .args(["quality", "--since", "2026-10-10", "--until", "2026-10-09"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "--until 2026-10-09T23:59:59Z is before --since 2026-10-10T00:00:00Z",
        ))
        .stderr(predicate::str::contains(
            "ns quality --since 2026-10-01 --until 2026-10-09",
        ));
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

/// A bare `remote.git` beside the repo, as its `origin`, with `main` pushed.
fn with_origin(f: &Fixture) -> PathBuf {
    let remote = f.root.with_file_name("remote.git");
    git(
        f.root.parent().unwrap(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    git(
        &f.root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&f.root, &["push", "-q", "origin", "main"]);
    remote
}

fn import(f: &Fixture, dir: &Path, args: &[&str], code: i32) -> Value {
    let out = ns()
        .current_dir(&f.root)
        .args(["quality", "import"])
        .arg(dir)
        .args(args)
        .output();
    assert_eq!(
        out.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn sources(v: &Value) -> Vec<(String, String)> {
    v["per_unit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["unit"].as_str().unwrap().to_string(),
                u["source"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn branch_text(remote: &Path) -> String {
    git(
        remote,
        &["show", "refs/heads/nightshift/quality:records.jsonl"],
    )
}

fn branch_records(remote: &Path) -> Vec<Value> {
    branch_text(remote)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn imported_records_keep_the_numbers_after_the_worktrees_are_removed() {
    let f = fixture();
    let remote = with_origin(&f);
    let live = quality(&f, &[]);
    assert_eq!(
        sources(&live),
        pairs(&[("1-alpha", "worktree"), ("2-beta", "worktree")])
    );
    assert_eq!(live["records"]["fetched"], true);
    assert_eq!(live["records"]["units"], 0);

    let live_since = quality(&f, &["--since", "2026-10-09T02:30:01Z"]);
    let wts = f.root.with_file_name("myrepo.worktrees");
    fs::create_dir(wts.join("Not_A_Unit")).unwrap();
    fs::write(wts.join("notes.txt"), "x\n").unwrap();
    let v = import(&f, &wts, &[], 0);
    assert_eq!(v["status"], "pushed");
    assert_eq!(v["imported"], json!(["1-alpha", "2-beta"]));
    assert_eq!(v["records"], 3);
    let skipped: Vec<_> = v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["unit"].as_str().unwrap())
        .collect();
    assert_eq!(skipped, ["3-gamma", "Not_A_Unit", "notes.txt"]);
    assert_eq!(v["skipped"][1]["reason"], "not a unit id");
    let records = branch_records(&remote);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["issue"], 1);
    assert_eq!(records[0]["outcome"], "stuck");
    assert_eq!(records[2]["pr"], 77);
    assert_eq!(records[0]["findings"][1]["id"], "I1");
    assert_eq!(records[0]["findings"][1]["introduced_by"]["pr"], 41);
    assert!(!branch_text(&remote).contains(f.root.parent().unwrap().to_str().unwrap()));

    let again = import(&f, &wts, &[], 0);
    assert_eq!(again["status"], "already-done");
    assert_eq!(again["already_recorded"], json!(["1-alpha", "2-beta"]));
    assert_eq!(branch_records(&remote).len(), 3);

    for u in ["1-alpha", "2-beta", "3-gamma"] {
        let wt = wts.join(u);
        git(
            &f.root,
            &["worktree", "remove", "--force", wt.to_str().unwrap()],
        );
        git(&f.root, &["branch", "-q", "-D", &format!("ns/{u}")]);
    }
    // The reviewed commits are gone, as after a squash merge and a gc: escapes keep the blame
    // their records took.
    git(&f.root, &["reflog", "expire", "--expire=now", "--all"]);
    git(&f.root, &["gc", "-q", "--prune=now"]);
    let after = quality(&f, &[]);
    for k in [
        "units",
        "findings",
        "first_pass",
        "finding_flow",
        "cycles_to_clean",
        "leftovers",
        "escapes",
        "trend",
        "run_log",
    ] {
        assert_eq!(live[k], after[k], "{k}");
    }
    for (k, field) in [
        ("leftover_findings", "id"),
        ("escape_findings", "id"),
        ("escape_findings", "introduced_by"),
    ] {
        let pick = |v: &Value| -> Vec<Value> {
            v[k].as_array()
                .unwrap()
                .iter()
                .map(|x| x[field].clone())
                .collect()
        };
        assert_eq!(pick(&live), pick(&after), "{k}.{field}");
    }
    let gaps = |v: &Value| {
        let mut g = v["gaps"].clone();
        g.as_object_mut().unwrap().remove("unparsed");
        g
    };
    assert_eq!(gaps(&live), gaps(&after));
    assert_eq!(
        sources(&after),
        pairs(&[("1-alpha", "record"), ("2-beta", "record")])
    );
    assert_eq!(after["per_unit"][0]["worktree"], Value::Null);
    assert_eq!(after["records"]["units"], 2);
    assert_eq!(after["records"]["on_branch"], 3);
    let since = quality(&f, &["--since", "2026-10-09T02:30:01Z"]);
    assert_eq!(since["units"], 1);
    assert_eq!(live_since["first_pass"], since["first_pass"]);
    assert_eq!(live_since["trend"], since["trend"]);

    // A fetch that fails reads the records as last fetched.
    let gone = f.root.with_file_name("gone.git");
    git(
        &f.root,
        &["remote", "set-url", "origin", gone.to_str().unwrap()],
    );
    let offline = quality(&f, &[]);
    assert_eq!(offline["records"]["fetched"], false);
    assert!(offline["records"]["fetch_error"].is_string());
    assert_eq!(offline["records"]["on_branch"], 3);
    assert_eq!(offline["first_pass"], live["first_pass"]);
}

#[test]
fn human_output_says_when_the_records_fetch_failed_and_what_the_gaps_are() {
    let f = fixture();
    let gone = f.root.with_file_name("gone-human.git");
    git(
        &f.root,
        &["remote", "add", "origin", gone.to_str().unwrap()],
    );
    let out = ns()
        .current_dir(&f.root)
        .args(["quality", "--human"])
        .output();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("Records: fetch failed"),
        "missing the fetch failure in:\n{text}"
    );
    assert!(
        text.contains("Gaps: 2 unparsed artifacts, 4 findings missing a field"),
        "{text}"
    );
}

#[test]
fn an_import_with_nothing_to_import_fails_and_says_what_it_expected() {
    let f = fixture();
    let remote = with_origin(&f);
    let empty = f.root.with_file_name("empty");
    fs::create_dir_all(empty.join("1-alpha/.ns")).unwrap();
    for args in [&[][..], &["--dry-run"][..]] {
        let out = ns()
            .current_dir(&f.root)
            .args(["quality", "import"])
            .arg(&empty)
            .args(args)
            .output();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            (v["ok"].as_bool(), v["status"].as_str()),
            (Some(false), Some("nothing-to-import"))
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("<unit>/.ns/<unit>/review.md"), "{err}");
        assert!(err.contains("--dry-run"), "{err}");
        assert!(err.contains("skipped:  1 (see"), "{err}");
        assert_eq!(v["skipped"][0]["reason"], "no .ns/1-alpha/ inside");
    }
    assert_eq!(git(&remote, &["for-each-ref", "refs/heads/nightshift"]), "");
}

#[test]
fn a_record_wins_over_its_worktree_and_a_unit_without_one_falls_back() {
    let f = fixture();
    with_origin(&f);
    let wts = f.root.with_file_name("myrepo.worktrees");
    let archive = f.root.with_file_name("archive");
    write(
        archive.join("1-alpha/.ns/1-alpha/review.md"),
        &fs::read_to_string(wts.join("1-alpha/.ns/1-alpha/review.md")).unwrap(),
    );
    write(
        archive.join("1-alpha/.ns/1-alpha/pr.md"),
        "---\nphase: ship\nstatus: pass\npr: https://github.com/o/r/pull/5\n---\n",
    );
    assert_eq!(import(&f, &archive, &[], 0)["imported"], json!(["1-alpha"]));
    write(
        wts.join("1-alpha/.ns/1-alpha/review.md"),
        &review("status: pass\ncycles: 0\n", ""),
    );
    let v = quality(&f, &[]);
    assert_eq!(
        sources(&v),
        pairs(&[("1-alpha", "record"), ("2-beta", "worktree")])
    );
    assert_eq!(v["per_unit"][0]["critical"], 1);
    assert_eq!(v["per_unit"][0]["status"], "blocked");

    // With the run log gone, the outcome and PR come from the record.
    fs::remove_file(f.root.join(".git/ns/runs.jsonl")).unwrap();
    let run = &quality(&f, &[])["per_unit"][0]["run"];
    assert_eq!(
        (run["outcome"].as_str(), run["pr"].as_u64()),
        (Some("stuck"), Some(5))
    );
}

#[test]
fn an_import_dry_run_pushes_nothing() {
    let f = fixture();
    let remote = with_origin(&f);
    let wts = f.root.with_file_name("myrepo.worktrees");
    let v = import(&f, &wts, &["--dry-run"], 0);
    assert_eq!(v["status"], "planned");
    import(&f, &wts, &[], 0);
    assert_eq!(
        import(&f, &wts, &["--dry-run"], 0)["status"],
        "already-done"
    );
    git(
        &remote,
        &["update-ref", "-d", "refs/heads/nightshift/quality"],
    );
    assert_eq!(
        (v["units"].as_u64(), v["records"].as_u64()),
        (Some(2), Some(3))
    );
    assert_eq!(git(&remote, &["for-each-ref", "refs/heads/nightshift"]), "");
    assert!(!f.root.join(".git/ns/quality-outbox.jsonl").exists());
}

#[test]
fn an_import_with_no_origin_waits_in_the_outbox_and_still_counts() {
    let f = fixture();
    let wts = f.root.with_file_name("myrepo.worktrees");
    let v = import(&f, &wts, &[], 1);
    assert_eq!(
        (v["ok"].as_bool(), v["status"].as_str()),
        (Some(false), Some("outbox"))
    );
    let v = quality(&f, &[]);
    assert_eq!(v["records"]["fetched"], false);
    assert_eq!(v["records"]["in_outbox"], 3);
    assert_eq!(sources(&v)[0].1, "record");
    let remote = with_origin(&f);
    let v = import(&f, &wts, &[], 0);
    assert_eq!(v["status"], "pushed");
    assert_eq!(v["already_recorded"], json!(["1-alpha", "2-beta"]));
    assert_eq!(v["push"]["records"], 3);
    assert_eq!(branch_records(&remote).len(), 3);
    assert!(!f.root.join(".git/ns/quality-outbox.jsonl").exists());
}

#[test]
fn an_import_without_a_git_identity_commits_as_nightshift() {
    let f = fixture();
    let remote = with_origin(&f);
    let home = f.root.with_file_name("empty-home");
    fs::create_dir(&home).unwrap();
    let wts = f.root.with_file_name("myrepo.worktrees");
    let out = ns()
        .current_dir(&f.root)
        .env("HOME", &home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "user.useConfigOnly")
        .env("GIT_CONFIG_VALUE_0", "true")
        .env_remove("EMAIL")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .args(["quality", "import"])
        .arg(&wts)
        .output();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        git(
            &remote,
            &["log", "-1", "--format=%an <%ae>", "nightshift/quality"]
        ),
        "nightshift <nightshift@localhost>"
    );
}

#[test]
fn import_needs_a_directory() {
    let f = fixture();
    ns().current_dir(&f.root)
        .args(["quality", "import", "no-such-dir"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not a directory"))
        .stderr(predicate::str::contains("ns quality import"));
    ns().current_dir(&f.root)
        .args(["quality", "import"])
        .assert()
        .code(2);
}

/// A repo whose `lib.rs` line 2 came from factory PR #41 (branch `ns/9-b`) and line 3 from a
/// person's PR #40, with a fake `gh` that serves the given REST-shaped `type:fix` issues through
/// the `--jq` filter it is passed, run by the real `jq`, and the two PRs.
fn bugs_fixture(issues: &[Value]) -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap().join("myrepo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    commit(&root, "lib.rs", "fn a() {}\n", "init");
    commit(
        &root,
        "lib.rs",
        "fn a() {}\nfn b() {}\n",
        "feat: add b (#41)",
    );
    commit(
        &root,
        "lib.rs",
        "fn a() {}\nfn b() {}\nfn c() {}\n",
        "feat: add c (#40)",
    );
    let fake = tmp.path().join("fake");
    write(fake.join("issues.json"), &json!(issues).to_string());
    let prs = json!([
        {"number": 41, "body": "Closes #9", "headRefName": "ns/9-b", "isCrossRepository": false,
         "mergedAt": "2099-01-06T10:00:00Z", "additions": 300, "deletions": 100},
        {"number": 40, "body": "Closes #8", "headRefName": "someone/c", "isCrossRepository": false,
         "mergedAt": "2099-01-06T09:00:00Z", "additions": 5, "deletions": 0},
        {"number": 42, "body": "", "headRefName": "ns/12-d", "isCrossRepository": false,
         "mergedAt": "2099-01-13T10:00:00Z", "additions": 150, "deletions": 50},
        {"number": 43, "body": "", "headRefName": "ns/13-e", "isCrossRepository": false,
         "mergedAt": "2099-01-14T10:00:00Z", "additions": 100, "deletions": 0},
    ]);
    write(fake.join("prs.json"), &prs.to_string());
    write(fake.join("merged.json"), &prs.to_string());
    common::write_exe(
        &fake.join("bin/gh"),
        format!(
            "#!/bin/sh\necho \"$*\" >> '{d}/calls'\ncase \"$*\" in\n  \"api --paginate repos/{{owner}}/{{repo}}/issues?state=all&labels=type:fix&per_page=100 \"*) for a; do :; done; jq -c \"$a\" '{d}/issues.json' ;;\n  \"pr list --state merged --limit 200 --json number,body,headRefName,isCrossRepository\") cat '{d}/prs.json' ;;\n  \"pr list --state merged --limit 200 --json number,headRefName,isCrossRepository,mergedAt,additions,deletions\"*) cat '{d}/merged.json' ;;\n  *) echo \"fake gh: $*\" >&2; exit 1 ;;\nesac\n",
            d = fake.display()
        ),
    );
    (tmp, root)
}

fn fix_issue(n: u64, opened: &str, title: &str, body: &str, labels: &[&str]) -> Value {
    let labels: Vec<Value> = labels.iter().map(|l| json!({"name": l})).collect();
    json!({"number": n, "title": title, "body": body, "labels": labels,
           "author_association": "OWNER", "created_at": opened, "state": "open", "state_reason": null})
}

fn ns_bugs(tmp: &TempDir, root: &Path, args: &[&str]) -> Ns {
    let path = format!(
        "{}:{}",
        tmp.path().join("fake/bin").display(),
        std::env::var("PATH").unwrap()
    );
    let mut c = ns();
    c.current_dir(root)
        .env("PATH", path)
        .env("NS_NOW", "4072075200")
        .args(["quality", "--bugs"])
        .args(args);
    c
}

fn bugs_mixed_issues() -> Vec<Value> {
    let mut dup = fix_issue(
        14,
        "2099-01-08T00:00:00Z",
        "fix: again",
        "Same as #13.",
        &[],
    );
    dup["state"] = json!("closed");
    dup["state_reason"] = json!("duplicate");
    let mut pull = fix_issue(
        15,
        "2099-01-06T00:00:00Z",
        "fix: a pull request",
        "",
        &["type:fix"],
    );
    pull["pull_request"] = json!({"url": "https://example.test/pulls/15"});
    vec![
        fix_issue(
            10,
            "2099-01-05T09:00:00Z",
            "fix: a drops errors",
            "`lib.rs:1` drops errors.\n\nThis escaped an earlier review.",
            &["type:fix", "priority:high"],
        ),
        fix_issue(
            11,
            "2099-01-11T23:59:59Z",
            "fix: b panics",
            "`lib.rs:2` panics on empty input.",
            &["type:fix", "priority:critical"],
        ),
        fix_issue(
            12,
            "2099-01-12T00:00:00Z",
            "fix: main is red at abcdef0: test",
            "",
            &["type:fix", "priority:critical"],
        ),
        fix_issue(
            13,
            "2099-01-14T10:00:00Z",
            "fix: c is slow",
            "`lib.rs:3` takes a second.",
            &["type:fix", "priority:medium"],
        ),
        dup,
        pull,
    ]
}

#[test]
fn bugs_counts_each_source_per_iso_week_and_leaves_out_duplicates() {
    let (tmp, root) = bugs_fixture(&bugs_mixed_issues());
    let out = ns_bugs(&tmp, &root, &["--json"]).output();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let by: Vec<(u64, &str, &str, &str)> = v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["number"].as_u64().unwrap(),
                i["source"].as_str().unwrap(),
                i["by"].as_str().unwrap(),
                i["severity"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        by,
        [
            (10, "review_escape", "review_text", "critical_high"),
            (11, "factory_introduced", "blame", "critical_high"),
            (12, "ci_main", "ci_title", "critical_high"),
            (13, "human", "default", "rest"),
        ]
    );
    assert_eq!(v["issues"][1]["introduced_by"]["pr"], 41);
    assert_eq!(v["issues"][3]["introduced_by"]["pr"], 40);
    assert_eq!(
        v["excluded"],
        json!([{"number": 14, "title": "fix: again", "week": "2099-W02"}])
    );
    assert_eq!(
        v["weeks"],
        json!([
            {"week": "2099-W02", "monday": "2099-01-05", "opened": 2, "review_escape": 1,
             "factory_introduced": 1, "ci_main": 0, "human": 0, "critical_high": 2, "rest": 0,
             "merged_units": 1, "changed_lines": 400, "per_unit": 2.0, "per_kloc": 5.0,
             "undercounted": false},
            {"week": "2099-W03", "monday": "2099-01-12", "opened": 2, "review_escape": 0,
             "factory_introduced": 0, "ci_main": 1, "human": 1, "critical_high": 1, "rest": 1,
             "merged_units": 2, "changed_lines": 300, "per_unit": 1.0, "per_kloc": 6.667,
             "undercounted": false},
        ])
    );
    assert_eq!(
        v["trend"],
        json!({
            "per_unit": {"direction": "insufficient", "slope": null, "weeks": 2},
            "per_kloc": {"direction": "insufficient", "slope": null, "weeks": 2},
        })
    );
    assert_eq!(
        v["merged_prs"],
        json!({"listed": 4, "truncated": false, "oldest": "2099-01-06T09:00:00Z"})
    );
    assert_eq!(v["total"]["opened"], 4);
    assert_eq!(
        (v["ok"].clone(), v["label"].clone()),
        (json!(true), json!("type:fix"))
    );
}

#[test]
fn bugs_counts_a_review_escape_per_checklist_item() {
    let grouped = "\u{1F916} *Posted by nightshift, an AI agent.*\n\n- [ ] `a.rs:1` (Critical, missed by tests): x. Evidence: e\n- [ ] `a.rs:2` (Important, missed by tests): y. Evidence: e\n- [x] `a.rs:3` (Suggestion, missed by tests): z. Evidence: e\n\nThis escaped an earlier review.\n\nSource: review_escape";
    let (tmp, root) = bugs_fixture(&[
        fix_issue(
            20,
            "2099-01-05T09:00:00Z",
            "fix: a.rs escapes",
            grouped,
            &["type:fix", "priority:critical"],
        ),
        fix_issue(
            21,
            "2099-01-06T09:00:00Z",
            "fix: lone escape",
            "`b.rs:1` x.\n\nThis escaped an earlier review.",
            &["type:fix", "priority:low"],
        ),
    ]);
    let out = ns_bugs(&tmp, &root, &["--json"]).output();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["issues"][0]["number"], 20);
    assert_eq!(v["issues"][0]["bugs"], 3);
    assert_eq!(v["issues"][1]["number"], 21);
    assert_eq!(v["issues"][1]["bugs"], 1);
    assert_eq!(v["weeks"][0]["opened"], 4);
    assert_eq!(v["weeks"][0]["review_escape"], 4);
    assert_eq!(v["weeks"][0]["critical_high"], 3);
    assert_eq!(v["weeks"][0]["rest"], 1);
    assert_eq!(v["total"]["opened"], 4);
}

#[test]
fn bugs_weeks_keeps_only_the_latest_weeks() {
    let (tmp, root) = bugs_fixture(&bugs_mixed_issues());
    let out = ns_bugs(&tmp, &root, &["--weeks", "1"]).output();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("\n2099-W03       2      0       0       1     1         1    1\n"),
        "{text}"
    );
    assert!(
        text.contains("\n2099-W03      2    300     1.00     6.67\n"),
        "{text}"
    );
    assert!(
        text.contains("\ntrend per unit: insufficient, needs 4 weeks with a rate, has 1\n"),
        "{text}"
    );
    assert!(!text.contains("2099-W02"), "{text}");
    assert!(!text.contains("Left out"), "{text}");
}

#[test]
fn bugs_fails_when_the_forge_cannot_list_issues() {
    let (tmp, root) = bugs_fixture(&[]);
    fs::remove_file(tmp.path().join("fake/issues.json")).unwrap();
    ns_bugs(&tmp, &root, &[]).assert().code(1).stderr(
        predicate::str::contains("issues?state=all&labels=type:fix")
            .and(predicate::str::contains("failed:")),
    );
}

#[test]
fn weeks_needs_bugs_and_one_or_more() {
    ns().args(["quality", "--weeks", "2"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--bugs"));
    ns().args(["quality", "--bugs", "--weeks", "0"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--weeks must be 1 or more"));
}

#[test]
fn bugs_fails_when_the_forge_cannot_list_closing_prs() {
    let (tmp, root) = bugs_fixture(&[]);
    fs::remove_file(tmp.path().join("fake/prs.json")).unwrap();
    ns_bugs(&tmp, &root, &[]).assert().code(1).stderr(
        predicate::str::contains("gh pr list --state merged")
            .and(predicate::str::contains("failed:")),
    );
}

#[test]
fn bugs_fails_when_the_forge_cannot_list_merged_prs() {
    let (tmp, root) = bugs_fixture(&[]);
    fs::remove_file(tmp.path().join("fake/merged.json")).unwrap();
    ns_bugs(&tmp, &root, &[]).assert().code(1).stderr(
        predicate::str::contains("mergedAt,additions,deletions")
            .and(predicate::str::contains("failed:")),
    );
}

#[test]
fn bugs_reads_the_merged_prs_since_the_first_week() {
    let (tmp, root) = bugs_fixture(&bugs_mixed_issues());
    ns_bugs(&tmp, &root, &["--json"]).assert().success();
    let calls = fs::read_to_string(tmp.path().join("fake/calls")).unwrap();
    assert!(
        calls.contains(
            "pr list --state merged --limit 200 --json \
             number,headRefName,isCrossRepository,mergedAt,additions,deletions \
             --search merged:>=2099-01-05\n"
        ),
        "{calls}"
    );
    let (tmp, root) = bugs_fixture(&bugs_mixed_issues());
    ns_bugs(&tmp, &root, &["--json", "--since", "2099-01-07T12:00:00Z"])
        .assert()
        .success();
    let calls = fs::read_to_string(tmp.path().join("fake/calls")).unwrap();
    assert!(calls.contains("--search merged:>=2099-01-05\n"), "{calls}");
}

#[test]
fn bugs_a_mid_week_since_counts_units_and_fixes_from_it_on() {
    let (tmp, root) = bugs_fixture(&bugs_mixed_issues());
    let out = ns_bugs(&tmp, &root, &["--json", "--since", "2099-01-06T10:00:00Z"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let w = &v["weeks"][0];
    assert_eq!(w["week"], "2099-W02");
    assert_eq!(w["opened"], 1);
    assert_eq!(
        (w["merged_units"].clone(), w["changed_lines"].clone()),
        (json!(1), json!(400))
    );
    assert_eq!(w["per_unit"], 1.0);
}

#[test]
fn bugs_a_factory_pr_without_a_closing_reference_does_not_make_a_bug_factory_introduced() {
    let (tmp, root) = bugs_fixture(&[fix_issue(
        21,
        "2099-01-14T00:00:00Z",
        "fix: d",
        "`lib.rs:4` breaks.",
        &[],
    )]);
    commit(
        &root,
        "lib.rs",
        "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n",
        "feat: add d (#42)",
    );
    let out = ns_bugs(&tmp, &root, &["--json"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["issues"][0]["introduced_by"]["pr"], 42);
    assert_eq!(v["issues"][0]["source"], "human");
}

#[test]
fn bugs_counts_a_factory_pr_that_shares_its_issue_with_another_pr_whichever_is_listed_first() {
    let factory = json!({"number": 41, "body": "Closes #9", "headRefName": "ns/9-b",
                         "isCrossRepository": false});
    let other = json!({"number": 44, "body": "Closes #9", "headRefName": "someone/b2",
                       "isCrossRepository": false});
    for prs in [
        json!([factory.clone(), other.clone()]),
        json!([other.clone(), factory.clone()]),
    ] {
        let (tmp, root) = bugs_fixture(&[fix_issue(
            11,
            "2099-01-11T23:59:59Z",
            "fix: b panics",
            "`lib.rs:2` panics on empty input.",
            &[],
        )]);
        write(tmp.path().join("fake/prs.json"), &prs.to_string());
        let out = ns_bugs(&tmp, &root, &["--json"]).output();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["issues"][0]["introduced_by"]["pr"], 41, "{prs}");
        assert_eq!(v["issues"][0]["source"], "factory_introduced", "{prs}");
    }
}

const FOUR_WEEKS: [&str; 4] = ["2098-12-16", "2098-12-23", "2098-12-30", "2099-01-06"];

fn four_weeks() -> Vec<Value> {
    (0..4)
        .map(|i| {
            fix_issue(
                30 + i as u64,
                &format!("{}T09:00:00Z", FOUR_WEEKS[i]),
                "fix: x",
                "Source: human",
                &[],
            )
        })
        .collect()
}

fn merged_pr(n: u64, day: &str) -> Value {
    json!({"number": n, "headRefName": "ns/u", "isCrossRepository": false,
           "mergedAt": format!("{day}T10:00:00Z"), "additions": 100, "deletions": 0})
}

#[test]
fn bugs_json_has_a_falling_trend_over_four_weeks() {
    let (tmp, root) = bugs_fixture(&four_weeks());
    let mut prs = Vec::new();
    for (i, n) in [1u64, 2, 3, 4].into_iter().enumerate() {
        for j in 0..n {
            prs.push(merged_pr(100 + i as u64 * 10 + j, FOUR_WEEKS[i]));
        }
    }
    write(tmp.path().join("fake/merged.json"), &json!(prs).to_string());
    let out = ns_bugs(&tmp, &root, &["--json"]).output();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["trend"],
        json!({
            "per_unit": {"direction": "falling", "slope": -0.242, "weeks": 4},
            "per_kloc": {"direction": "falling", "slope": -2.417, "weeks": 4},
        })
    );
    assert_eq!(v["merged_prs"]["truncated"], false);
}

#[test]
fn bugs_json_flags_only_the_weeks_older_than_a_full_merged_list_undercounted() {
    let (tmp, root) = bugs_fixture(&four_weeks());
    let prs: Vec<Value> = (0..200)
        .map(|n| merged_pr(100 + n, FOUR_WEEKS[1]))
        .collect();
    write(tmp.path().join("fake/merged.json"), &json!(prs).to_string());
    let out = ns_bugs(&tmp, &root, &["--json"]).output();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["merged_prs"]["truncated"], true);
    assert_eq!(v["merged_prs"]["listed"], 200);
    assert_eq!(v["merged_prs"]["oldest"], "2098-12-23T10:00:00Z");
    let flags: Vec<bool> = v["weeks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["undercounted"].as_bool().unwrap())
        .collect();
    assert_eq!(flags, [true, true, false, false, false]);
    assert_eq!(v["trend"]["per_unit"]["direction"], "insufficient");
}

#[test]
fn bugs_names_why_an_issue_opened_before_any_commit_was_not_blamed() {
    let (tmp, root) = bugs_fixture(&[fix_issue(
        20,
        "1999-01-04T00:00:00Z",
        "fix: old",
        "`lib.rs:2` panics.",
        &[],
    )]);
    let out = ns_bugs(
        &tmp,
        &root,
        &["--json", "--since", "1999-01-01", "--until", "1999-01-10"],
    )
    .output();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let i = &v["issues"][0];
    assert_eq!(
        (i["number"].as_u64(), i["source"].as_str()),
        (Some(20), Some("human"))
    );
    assert_eq!(
        i["blame_error"],
        "no commit on main before the issue opened"
    );
    assert_eq!(v["weeks"].as_array().unwrap().len(), 2);
}

/// Unit `6-panel`: a reduced first pass whose tests reviewer alone raised a fixed Important.
fn panel_unit(f: &Fixture) {
    let (wt, _) = unit_worktree(&f.root, "6-panel");
    write(
        wt.join(".ns/6-panel/review.md"),
        &review(
            "status: pass\nupdated: 2026-10-11T05:00:00Z\ncycles: 1\n",
            "## Passes
| Pass | Panel | Reviewer | Role | Provider | Tokens | Seconds | Cost (USD) |
|---|---|---|---|---|---|---|---|
| 0 | reduced | correctness | review.correctness | claude | 40000 | 100 | 0.40 |
| 0 | reduced | tests | review.tests | claude | 20000 | 60 | 0.20 |

## Important
### I1. Untested branch
- Axis: tests
- Scope: changed
- Cycle: 0
- Status: fixed (cycle 1, abc1234)
",
        ),
    );
}

#[test]
fn reviewers_json_reports_each_reviewer_and_what_it_left_out() {
    let f = fixture();
    panel_unit(&f);
    let v = quality(&f, &["--reviewers", "--json"]);
    assert_eq!(v["ok"], true);
    assert_eq!(v["units"], 3);
    assert_eq!(
        (v["attempts"].clone(), v["measured_attempts"].clone()),
        (json!(4), json!(1))
    );
    let t = &v["reviewers"]["tests"];
    assert_eq!(t["reviews"], 1);
    assert_eq!(t["important"]["raised"], 2, "1-alpha's I2 and 6-panel's I1");
    assert_eq!(t["important"]["unique_fixed"], 1);
    assert_eq!(t["important"]["unique_fixed_per_review"], 1.0);
    assert_eq!(t["important"]["unique_fixed_per_usd"], 5.0);
    assert_eq!(t["cost"]["cost_usd"]["total"], 0.2);
    assert_eq!(v["reviewers"]["correctness"]["critical"]["unique_fixed"], 1);
    assert_eq!(v["excluded"]["escapes_unattributed"], 1, "1-alpha's I1");
    assert_eq!(v["excluded"]["attempts_without_passes"], 3);
    assert_eq!(v["overlap"].as_array().unwrap().len(), 28);
    assert!(v.get("per_unit").is_none());
    assert_eq!(v["records"]["fetched"], false);
    let since = quality(&f, &["--reviewers", "--json", "--since", "2026-10-11"]);
    assert_eq!(
        (since["units"].clone(), since["attempts"].clone()),
        (json!(2), json!(3))
    );
    assert_eq!(since["since"], "2026-10-11T00:00:00Z");
}

#[test]
fn reviewers_until_drops_later_units() {
    let f = fixture();
    panel_unit(&f);
    let all = quality(&f, &["--reviewers", "--json"]);
    let v = quality(&f, &["--reviewers", "--json", "--until", "2026-10-10"]);
    assert_eq!(v["until"], "2026-10-10T23:59:59Z");
    assert!(v["units"].as_u64() < all["units"].as_u64(), "{v}");
    assert!(v["attempts"].as_u64() < all["attempts"].as_u64(), "{v}");
    assert_eq!(v["reviewers"]["tests"]["reviews"], 0, "6-panel is later");
}

#[test]
fn reviewers_table_header_echoes_a_bounded_window() {
    let f = fixture();
    panel_unit(&f);
    let out = ns()
        .current_dir(&f.root)
        .args([
            "quality",
            "--reviewers",
            "--since",
            "2026-10-11",
            "--until",
            "2026-10-12",
        ])
        .output();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.starts_with("Units: 2 (since 2026-10-11T00:00:00Z, until 2026-10-12T23:59:59Z)\n"),
        "{text}"
    );
}

#[test]
fn reviewers_without_json_prints_a_table() {
    let f = fixture();
    panel_unit(&f);
    let out = ns()
        .current_dir(&f.root)
        .args(["quality", "--reviewers"])
        .output();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(serde_json::from_str::<Value>(&text).is_err(), "{text}");
    assert!(text.starts_with("Units: 3 (since -, until -)\n"), "{text}");
    let row = text
        .lines()
        .find(|l| l.starts_with("tests ") && l.contains("important"))
        .unwrap();
    assert_eq!(
        row.split_whitespace().collect::<Vec<_>>(),
        [
            "tests",
            "important",
            "1",
            "2",
            "1",
            "2",
            "1",
            "1",
            "0",
            "0",
            "0"
        ]
    );
    assert!(text.contains("Overlap: Jaccard"), "{text}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("read only local records"),
        "a failed fetch is said on stderr"
    );
    let plain = quality(&f, &[]);
    assert!(plain.get("per_unit").is_some() && plain.get("reviewers").is_none());
}

#[test]
fn reviewers_table_adds_a_total_row_per_reviewer() {
    let f = fixture();
    panel_unit(&f);
    let out = ns()
        .current_dir(&f.root)
        .args(["quality", "--reviewers"])
        .output();
    let text = String::from_utf8(out.stdout).unwrap();
    let row = text
        .lines()
        .find(|l| l.starts_with("correctness ") && l.contains("total"))
        .unwrap();
    let cols: Vec<&str> = row.split_whitespace().collect();
    let v = quality(&f, &["--reviewers", "--json"]);
    let c = &v["reviewers"]["correctness"];
    assert_eq!(
        cols[3].parse::<u64>().unwrap(),
        c["critical"]["raised"].as_u64().unwrap() + c["important"]["raised"].as_u64().unwrap(),
        "{row}"
    );
    assert_eq!(c["total"]["unique_fixed"], 1);
}

#[test]
fn reviewers_conflicts_with_human_and_bugs() {
    let f = fixture();
    for other in ["--human", "--bugs"] {
        ns().current_dir(&f.root)
            .args(["quality", "--reviewers", other])
            .assert()
            .code(2);
    }
}
