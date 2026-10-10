//! `scripts/check-git-dir-decoy.sh`, run with a stand-in for `cargo test` that does or does not
//! touch the decoy repo.

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

const LOCATION_VARS: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
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
        (
            "git config ns.touched 1",
            "check-git-dir-decoy: the decoy's .git/config changed",
        ),
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
fn the_command_does_not_inherit_any_location_var_but_the_decoy_pair() {
    for var in LOCATION_VARS {
        let tmp = tempfile::tempdir().unwrap();
        let outer = tempfile::tempdir().unwrap();
        let junk = outer.path().join("junk");
        let check = format!("test -z \"${{{var}+set}}\" || [ \"{var}\" = GIT_DIR ] || [ \"{var}\" = GIT_WORK_TREE ]");
        let out = run(&["sh", "-c", &check], tmp.path(), &[(var, &junk)]);
        assert!(
            out.status.success(),
            "{var} leaked to the command: {}",
            stderr(&out)
        );
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
            "#!/bin/sh\n{{ echo \"args=$*\"; echo \"pwd=$PWD\"; echo \"git_dir=$GIT_DIR\"; echo \"work_tree=$GIT_WORK_TREE\"; echo \"top=$(git -C \"$GIT_WORK_TREE\" rev-parse --show-toplevel)\"; echo \"branch=$(git -C \"$GIT_WORK_TREE\" rev-parse --verify -q refs/heads/decoy-branch >/dev/null && echo yes)\"; [ -d \"$GIT_WORK_TREE\" ] && echo is_dir=yes; }} > '{}'\n",
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
    let field = |name: &str| {
        rec.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name} in {rec}"))
            .to_string()
    };
    let (git_dir, work_tree) = (field("git_dir"), field("work_tree"));
    assert!(!work_tree.is_empty(), "{rec}");
    assert_eq!(git_dir, format!("{work_tree}/.git"), "{rec}");
    assert_eq!(field("is_dir"), "yes", "{rec}");
    assert_eq!(field("top"), work_tree, "{rec}");
    assert_eq!(field("branch"), "yes", "{rec}");
    for p in [&git_dir, &work_tree] {
        assert!(
            !Path::new(p).starts_with(&root),
            "{p} is inside the repo: {rec}"
        );
    }
}

#[test]
fn a_command_that_removes_the_decoy_or_its_config_fails_naming_the_decoy() {
    for cmd in ["rm -rf \"$GIT_DIR\"", "rm \"$GIT_DIR/config\""] {
        let tmp = tempfile::tempdir().unwrap();
        let out = run(&["sh", "-c", cmd], tmp.path(), &[]);
        let err = stderr(&out);
        assert_eq!(out.status.code(), Some(1), "{cmd}: {err}");
        assert!(
            err.contains("the decoy repo was removed or unreadable"),
            "{cmd}: {err}"
        );
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0, "{cmd}");
    }
}

#[test]
fn a_command_that_removes_the_decoy_and_fails_reports_both() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(
        &["sh", "-c", "rm -rf \"$GIT_DIR\"; exit 4"],
        tmp.path(),
        &[],
    );
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(
        err.contains("the decoy repo was removed or unreadable"),
        "{err}"
    );
    assert!(err.contains("command failed (exit 4)"), "{err}");
}
