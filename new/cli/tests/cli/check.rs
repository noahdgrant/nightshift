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
fn a_changed_check_command_is_not_covered_by_an_old_pass() {
    let (repo, ws, log) = with_logging_checks("exit 0");
    ns(&ws, &["check"]).success();
    repo.set_check(&format!("echo again >> {}", log.display()));
    assert_eq!(
        report(ns(&ws, &["check"]).success())["cached"].as_bool(),
        Some(false)
    );
}

#[test]
fn a_repository_with_no_checks_passes_them() {
    let repo = Repo::new();
    ns(&repo.path, &["workspace", "new", "1"]).success();
    let r = report(ns(&repo.workspace("1"), &["check"]).success());
    assert_eq!(r["configured"].as_bool(), Some(false));
}
