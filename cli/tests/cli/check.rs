use crate::common;

use assert_cmd::Command;
use common::Repo;

fn ns(dir: &std::path::Path, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .args(args)
        .current_dir(dir)
        .assert()
}

fn report(assert: assert_cmd::assert::Assert) -> serde_json::Value {
    serde_json::from_slice(&assert.get_output().stdout).unwrap()
}

fn runs(log: &std::path::Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |t| t.lines().count())
}

/// A repository whose checks append to a log, and the workspace of unit 1.
fn with_logging_checks(command_tail: &str) -> (Repo, std::path::PathBuf, std::path::PathBuf) {
    let repo = Repo::new();
    let log = repo.path.parent().unwrap().join("checks.log");
    repo.set_check(&format!("echo ran >> {}\n{command_tail}", log.display()));
    ns(&repo.path, &["workspace", "new", "1"]).success();
    let workspace = repo.workspace("1");
    (repo, workspace, log)
}

#[test]
fn checks_that_passed_on_the_same_work_are_not_run_again() {
    let (_repo, ws, log) = with_logging_checks("exit 0");
    let first = report(ns(&ws, &["check"]).success());
    assert_eq!(
        (first["ok"].as_bool(), first["cached"].as_bool()),
        (Some(true), Some(false))
    );
    let second = report(ns(&ws, &["check"]).success());
    assert_eq!(second["cached"].as_bool(), Some(true));
    assert_eq!(runs(&log), 1);
    std::fs::write(ws.join("new.txt"), "changed").unwrap();
    ns(&ws, &["check"]).success();
    assert_eq!(runs(&log), 2);
}

#[test]
fn checks_that_failed_are_run_again() {
    let (_repo, ws, log) = with_logging_checks("exit 1");
    assert_eq!(
        report(ns(&ws, &["check"]).failure())["ok"].as_bool(),
        Some(false)
    );
    ns(&ws, &["check"]).failure();
    assert_eq!(runs(&log), 2);
}

#[test]
fn a_unit_that_changes_the_checks_is_held_to_its_change() {
    let (_repo, ws, log) = with_logging_checks("exit 0");
    ns(&ws, &["check"]).success();
    let changed = format!("echo changed >> {}\nexit 1", log.display());
    std::fs::write(ws.join(".nightshift/check"), changed).unwrap();
    ns(&ws, &["check"]).failure();
    assert_eq!(
        runs(&log),
        2,
        "the changed command ran, not the cached pass"
    );
}

#[test]
fn a_check_added_later_reaches_units_started_after_it_and_not_before() {
    let (repo, early, _log) = with_logging_checks("exit 0");
    let later_log = repo.path.parent().unwrap().join("later.log");
    repo.set_check(&format!("echo later >> {}", later_log.display()));
    ns(&repo.path, &["workspace", "new", "2"]).success();
    ns(&early, &["check"]).success();
    assert_eq!(runs(&later_log), 0);
    ns(&repo.workspace("2"), &["check"]).success();
    assert_eq!(runs(&later_log), 1);
}

#[test]
fn a_repository_with_no_checks_passes_them() {
    let repo = Repo::new();
    ns(&repo.path, &["workspace", "new", "1"]).success();
    let r = report(ns(&repo.workspace("1"), &["check"]).success());
    assert_eq!(r["configured"].as_bool(), Some(false));
}

#[test]
fn checks_that_run_out_of_time_fail_and_are_not_remembered() {
    let (_repo, ws, _log) = with_logging_checks("sleep 30");
    let out = report(ns(&ws, &["check", "--timeout-seconds", "0"]).failure());
    assert_eq!(out["ok"].as_bool(), Some(false));
    // A remembered pass would make this succeed.
    ns(&ws, &["check", "--timeout-seconds", "0"]).failure();
}

const START: &str = concat!("ns:human-", "review start");
const END: &str = concat!("ns:human-", "review end");

/// `text` with `@start` and `@end` written as the markers, so this file holds none itself.
fn marked(text: &str) -> Vec<u8> {
    text.replace("@start", START)
        .replace("@end", END)
        .into_bytes()
}

/// A repository whose checks pass, holding `files` in the workspace of unit 1.
fn with_files(files: &[(&str, &[u8])]) -> (Repo, std::path::PathBuf) {
    let repo = Repo::new();
    repo.set_check("exit 0");
    for (name, content) in files {
        std::fs::write(repo.path.join(name), content).unwrap();
    }
    common::git(&repo.path, &["add", "."]);
    common::git(&repo.path, &["commit", "-q", "-m", "files"]);
    ns(&repo.path, &["workspace", "new", "1"]).success();
    let ws = repo.workspace("1");
    (repo, ws)
}

#[test]
fn good_markers_pass() {
    let (_repo, ws) = with_files(&[
        ("a.rs", &marked("// @start: money\nlet x = 1;\n// @end\n")),
        ("b.md", &marked("<!-- @start -->\ntext\n<!-- @end -->\n")),
        ("plain.txt", b"nothing here\n"),
    ]);
    assert_eq!(
        report(ns(&ws, &["check"]).success())["ok"].as_bool(),
        Some(true)
    );
}

#[test]
fn a_bad_marker_fails_naming_the_file_and_line() {
    let (_repo, ws) = with_files(&[("src.py", &marked("x = 1\n# @start: why\ny = 2\n"))]);
    let out = ns(&ws, &["check"]).failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        stderr.contains(&format!("src.py:2: `{START}` has no `{END}`")),
        "{stderr}"
    );
}

#[test]
fn files_that_are_not_text_or_not_tracked_are_skipped() {
    let (_repo, ws) = with_files(&[(
        "blob.bin",
        &[0xff, 0xfe, b' ']
            .iter()
            .chain(&marked("@end\n"))
            .copied()
            .collect::<Vec<u8>>(),
    )]);
    std::fs::write(ws.join("untracked.txt"), marked("@end\n")).unwrap();
    ns(&ws, &["check"]).success();
}

#[test]
fn every_bad_file_is_reported_with_its_own_line() {
    let (_repo, ws) = with_files(&[
        ("a.rs", &marked("x\n// @end\n")),
        ("b.py", &marked("# @start\n# @start\n# @end\n")),
    ]);
    let out = ns(&ws, &["check"]).failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        stderr.contains(&format!("a.rs:2: `{END}` has no `{START}`")),
        "{stderr}"
    );
    assert!(stderr.contains("b.py:2: "), "{stderr}");
}
