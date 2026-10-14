//! The entry points that keep an inherited git location (`GIT_DIR` and the rest) from reaching
//! a git process (#120, #638): `ns` clears them at the top of `main`, the shared test helpers
//! refuse to run with one set, and the repo's scripts and hooks unset them first.

mod common;

use std::fs;

use common::LOCATION_VARS;

const MARK: &str = "NS_TEST_GIT_LOCATION_PROBE";

#[test]
fn command_probe() {
    if std::env::var_os(MARK).is_none() {
        return;
    }
    common::command("git").arg("--version").output();
}

#[test]
fn command_panics_naming_each_git_location_variable_the_test_process_has() {
    let out = common::command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "command_probe",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(MARK, "1")
        .env("GIT_DIR", "/nonexistent")
        .env("GIT_PREFIX", "")
        .output();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains(r#"git location variables set: ["GIT_DIR", "GIT_PREFIX"]"#),
        "{text}"
    );
}

#[test]
fn ns_started_with_git_location_variables_passes_none_to_a_child_script() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let init = common::command("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .output();
    assert!(init.status.success());
    fs::create_dir_all(root.join("docs/agents")).unwrap();
    fs::write(
        root.join("docs/agents/stack.md"),
        "| Task | Command |\n|---|---|\n| rules | `./lint` | 1 s |\n",
    )
    .unwrap();
    let report = root.join("env.txt");
    let check: String = LOCATION_VARS
        .iter()
        .map(|v| {
            format!(
                "[ -z \"${{{v}+set}}\" ] || echo {v} >> '{}'\n",
                report.display()
            )
        })
        .collect();
    common::write_exe(
        &root.join("lint"),
        format!("#!/bin/sh\n: > '{}'\n{check}", report.display()),
    );

    let decoy = tmp.path().join("decoy");
    let out = common::ns()
        .args(["rules", "check"])
        .current_dir(&root)
        .envs(LOCATION_VARS.iter().map(|v| (*v, &decoy)))
        .output();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&report).unwrap(), "");
}
