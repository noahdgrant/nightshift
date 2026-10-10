//! `scripts/check-git-dir-decoy.sh`, run with a stand-in for `cargo test` that does or does not
//! touch the decoy repo.

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

const LOCATION_VARS: [&str; 4] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
];

fn script() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/check-git-dir-decoy.sh")
}

fn run(cmd: &[&str], tmp: &Path, inherited: &[(&str, &Path)]) -> Output {
    let mut c = common::command("bash");
    c.arg(script()).args(cmd).env("TMPDIR", tmp);
    for v in LOCATION_VARS {
        c.env_remove(v);
    }
    for (k, v) in inherited {
        c.env(k, v);
    }
    c.output()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_command_that_leaves_the_decoy_alone_passes_and_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(&["true"], tmp.path(), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn the_command_sees_the_decoy_as_its_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let check = r#"[ "$(git rev-parse --show-toplevel)" = "$GIT_WORK_TREE" ] && [ "$(git rev-parse --absolute-git-dir)" = "$GIT_DIR" ] && git rev-parse --verify -q refs/heads/decoy-branch"#;
    let out = run(&["sh", "-c", check], tmp.path(), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn the_command_runs_from_the_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(&["test", "-f", "cli/Cargo.toml"], tmp.path(), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn each_kind_of_mutation_fails_naming_what_changed() {
    for (cmd, changed) in [
        ("git config ns.touched 1", ".git/config"),
        ("git branch extra", "git for-each-ref"),
        (
            "git update-ref -d refs/heads/decoy-branch",
            "git for-each-ref",
        ),
        ("git config core.bare true", "became bare"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let out = run(&["sh", "-c", cmd], tmp.path(), &[]);
        assert_eq!(out.status.code(), Some(1), "{cmd}: {}", stderr(&out));
        assert!(stderr(&out).contains(changed), "{cmd}: {}", stderr(&out));
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0, "{cmd}");
    }
}

#[test]
fn a_failing_command_fails_the_check_and_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(&["false"], tmp.path(), &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("command failed"), "{}", stderr(&out));
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn a_command_that_mutates_the_decoy_and_fails_reports_both() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(&["sh", "-c", "git config ns.x 1; exit 3"], tmp.path(), &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains(".git/config"), "{err}");
    assert!(err.contains("command failed (exit 3)"), "{err}");
}

#[test]
fn inherited_git_location_vars_are_not_where_the_decoy_goes() {
    for var in LOCATION_VARS {
        let tmp = tempfile::tempdir().unwrap();
        let outer = tempfile::tempdir().unwrap();
        let sentinel = outer.path().join("sentinel");
        let out = run(&["true"], tmp.path(), &[(var, &sentinel)]);
        assert!(out.status.success(), "{var}: {}", stderr(&out));
        assert!(!sentinel.exists(), "{var}");
        assert_eq!(fs::read_dir(outer.path()).unwrap().count(), 0, "{var}");
    }
}

#[test]
fn with_no_arguments_it_runs_cargo_test_on_the_cli_manifest_from_the_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let record = bin.path().join("record");
    let stub = bin.path().join("cargo");
    fs::write(
        &stub,
        format!(
            "#!/bin/sh\n{{ echo \"args=$*\"; echo \"pwd=$PWD\"; echo \"git_dir=$GIT_DIR\"; echo \"work_tree=$GIT_WORK_TREE\"; }} > '{}'\n",
            record.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut c = common::command("bash");
    c.arg(script()).env("TMPDIR", tmp.path()).env("PATH", path);
    for v in LOCATION_VARS {
        c.env_remove(v);
    }
    let out = c.output();
    assert!(out.status.success(), "{}", stderr(&out));
    let rec = fs::read_to_string(&record).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap();
    assert!(
        rec.contains("args=test --manifest-path cli/Cargo.toml\n"),
        "{rec}"
    );
    assert!(rec.contains(&format!("pwd={}\n", root.display())), "{rec}");
    assert!(
        rec.contains("git_dir=") && !rec.contains("git_dir=\n"),
        "{rec}"
    );
    assert!(
        rec.contains("work_tree=") && !rec.contains("work_tree=\n"),
        "{rec}"
    );
}
