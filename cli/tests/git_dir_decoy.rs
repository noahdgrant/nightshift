//! `scripts/check-git-dir-decoy.sh`, run with a stand-in for `cargo test` that does or does not
//! touch the decoy repo.

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

fn run(cmd: &[&str], tmp: &Path, git_dir: Option<&Path>) -> Output {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/check-git-dir-decoy.sh");
    let mut c = common::command("bash");
    c.arg(script).args(cmd).env("TMPDIR", tmp);
    match git_dir {
        Some(d) => c.env("GIT_DIR", d),
        None => c.env_remove("GIT_DIR"),
    };
    c.env_remove("GIT_WORK_TREE").output()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_command_that_leaves_the_decoy_alone_passes_and_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(&["true"], tmp.path(), None);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn the_command_sees_the_decoy_as_its_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let check = r#"[ "$(git rev-parse --show-toplevel)" = "$GIT_WORK_TREE" ] && [ "$(git rev-parse --absolute-git-dir)" = "$GIT_DIR" ] && git rev-parse --verify -q refs/heads/decoy-branch"#;
    let out = run(&["sh", "-c", check], tmp.path(), None);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn the_command_runs_from_the_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(&["test", "-f", "cli/Cargo.toml"], tmp.path(), None);
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
        let out = run(&["sh", "-c", cmd], tmp.path(), None);
        assert_eq!(out.status.code(), Some(1), "{cmd}: {}", stderr(&out));
        assert!(stderr(&out).contains(changed), "{cmd}: {}", stderr(&out));
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0, "{cmd}");
    }
}

#[test]
fn a_failing_command_fails_the_check() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(&["false"], tmp.path(), None);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("command failed"), "{}", stderr(&out));
}

#[test]
fn an_inherited_git_dir_is_not_where_the_decoy_goes() {
    let tmp = tempfile::tempdir().unwrap();
    let outer = tempfile::tempdir().unwrap();
    let outer_git = outer.path().join(".git");
    let out = run(&["true"], tmp.path(), Some(&outer_git));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!outer_git.exists());
}
