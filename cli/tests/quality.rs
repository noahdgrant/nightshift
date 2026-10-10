//! `ns quality` end to end: a temp repo with unit worktrees holding fixture review artifacts.

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
