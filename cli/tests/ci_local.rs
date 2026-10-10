//! `scripts/ci-local.sh`, run in a temp repo against stub `cargo`, `ns`, `python3` and `python`.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

const ALL_STEPS: [&str; 11] = [
    "cargo fmt --check",
    "cargo clippy",
    "cargo test",
    "cargo test under a decoy GIT_DIR",
    "build ns",
    "ns lint",
    "ns check-markers",
    "ns eval --dry-run",
    "py-inventory fixture tests",
    "ns-troubleshoot brief checker tests",
    "private guard",
];

const FAST_ONLY_SKIPS: [&str; 5] = [
    "cargo test",
    "cargo test under a decoy GIT_DIR",
    "ns eval --dry-run",
    "py-inventory fixture tests",
    "ns-troubleshoot brief checker tests",
];

/// Fails when its command line contains `$FAIL_ON`.
const CARGO: &str = r#"#!/bin/sh
case "$FAIL_ON" in ?*) case "cargo $*" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac
"#;

/// Fails when `$FAIL_ON` is `decoy`.
const DECOY: &str = r#"#!/bin/sh
[ "$FAIL_ON" != decoy ]
"#;

/// Prints `$PLAN` for `eval --dry-run`, even when it then fails because its command line
/// contains `$FAIL_ON`.
const NS: &str = r#"#!/bin/sh
[ -n "$PLAN" ] || PLAN='{"skipped":[]}'
[ "$1" = eval ] && printf '%s\n' "$PLAN"
case "$FAIL_ON" in ?*) case "ns $*" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac
exit 0
"#;

/// Imports pytest only when `$PYTEST` is set, and hands the eval plan filter (`-I -`) to the
/// real python3 so the filter itself is what runs. Any other run logs its directory and fails
/// when that log line contains `$FAIL_ON`.
fn python_stub(real: &Path) -> String {
    format!(
        r#"#!/bin/sh
case "$*" in
  "-I -c import pytest") [ -n "$PYTEST" ] ;;
  "-I - "*) exec {real:?} "$@" ;;
  *) line="python ran in ${{PWD##*/}}: $*"
     echo "$line" >&2
     case "$FAIL_ON" in ?*) case "$line" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac ;;
esac
"#
    )
}

fn real_python3() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("python3"))
        .find(|p| p.is_file())
        .expect("python3 is required to run the eval plan filter")
}

fn write_exe(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/ci-local.sh");
        write_exe(
            &root.join("scripts/ci-local.sh"),
            &fs::read_to_string(script).unwrap(),
        );
        write_exe(&root.join("scripts/check-private.sh"), "#!/bin/sh\n");
        write_exe(&root.join("scripts/check-git-dir-decoy.sh"), DECOY);
        write_exe(&root.join("cli/target/debug/ns"), NS);
        write_exe(&root.join("bin/cargo"), CARGO);
        let python = python_stub(&real_python3());
        write_exe(&root.join("bin/python3"), &python);
        write_exe(&root.join("bin/python"), &python);
        fs::create_dir_all(root.join("evals/fixtures/py-inventory")).unwrap();
        let cases = root.join("skills/ns-troubleshoot/evals/cases");
        for case in ["py-hold-expires-early", "py-lost-hold-needs-redesign"] {
            write_exe(&cases.join(case).join("test_check_brief.py"), "");
        }
        fs::create_dir_all(cases.join("py-no-checker")).unwrap();
        Repo { dir }
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let root = self.dir.path();
        let mut cmd = common::command("bash");
        cmd.arg(root.join("scripts/ci-local.sh"))
            .args(args)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", root.join("bin").display()),
            )
            .env_remove("FAIL_ON")
            .env_remove("PLAN")
            .env_remove("PYTEST");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn steps(out: &Output) -> Vec<String> {
    stderr(out)
        .lines()
        .filter_map(|l| l.strip_prefix("==> ").map(str::to_owned))
        .collect()
}

#[test]
fn every_step_runs_in_order_and_the_run_passes() {
    let out = Repo::new().run(&[], &[("PYTEST", "1")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), ALL_STEPS);
    assert!(stderr(&out).contains("python ran in py-inventory: -m pytest -q"));
    for case in ["py-hold-expires-early", "py-lost-hold-needs-redesign"] {
        assert!(stderr(&out).contains(&format!(
            "python ran in {case}: -m unittest -q test_check_brief"
        )));
    }
    assert!(!stderr(&out).contains("python ran in py-no-checker"));
    assert!(stderr(&out).contains("ci-local: all checks passed"));
}

#[test]
fn a_failing_step_stops_the_run() {
    for (fail_on, step) in [
        ("cargo clippy", "cargo clippy"),
        ("decoy", "cargo test under a decoy GIT_DIR"),
        ("ns check-markers", "ns check-markers"),
        ("ns eval", "ns eval --dry-run"),
        (
            "python ran in py-hold-expires-early",
            "ns-troubleshoot brief checker tests",
        ),
    ] {
        let out = Repo::new().run(&[], &[("FAIL_ON", fail_on)]);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        let at = ALL_STEPS.iter().position(|s| *s == step).unwrap();
        assert_eq!(steps(&out), ALL_STEPS[..=at]);
        assert!(stderr(&out).contains(&format!("ci-local: FAILED at step: {step}\n")));
        assert!(!stderr(&out).contains("all checks passed"));
    }
}

#[test]
fn fast_skips_tests_eval_and_python_checks() {
    let out = Repo::new().run(&["--fast"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let want: Vec<_> = ALL_STEPS
        .iter()
        .copied()
        .filter(|s| !FAST_ONLY_SKIPS.contains(s))
        .collect();
    assert_eq!(steps(&out), want);
}

#[test]
fn fixture_tests_skip_without_pytest() {
    let out = Repo::new().run(&[], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("SKIPPED: py-inventory fixture tests"));
    assert!(!stderr(&out).contains("-m pytest"));
    assert_eq!(steps(&out), ALL_STEPS);
}

fn plan(reasons: &[&str]) -> String {
    let skipped: Vec<_> = reasons
        .iter()
        .map(|r| format!(r#"{{"skill":"ns-x","case":"c","reason":"{r}"}}"#))
        .collect();
    format!(r#"{{"skipped":[{}]}}"#, skipped.join(","))
}

#[test]
fn the_eval_filter_allows_only_zephyr_skips() {
    let zephyr = plan(&[
        "missing capability: zephyr",
        "missing capability: zephyr (west)",
    ]);
    let out = Repo::new().run(&[], &[("PLAN", &zephyr)]);
    assert!(out.status.success(), "{}", stderr(&out));

    let other = plan(&["missing capability: zephyr", "missing capability: python"]);
    let out = Repo::new().run(&[], &[("PLAN", &other)]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("skipped: ns-x/c: missing capability: python"));
    assert!(!stderr(&out).contains("skipped: ns-x/c: missing capability: zephyr"));
    assert!(stderr(&out).contains("ci-local: FAILED at step: ns eval --dry-run"));
}

#[test]
fn an_unknown_argument_prints_usage_and_exits_2() {
    let out = Repo::new().run(&["--bogus"], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr(&out), "usage: scripts/ci-local.sh [--fast]\n");
}
