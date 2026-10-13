//! `scripts/ci-affected.sh`'s ast-grep rules step, run with the real `ns`, `ast-grep` and Rust
//! rule pack against this checkout's `rules` row and a temp crate.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{affected, common, git, stderr, steps, write_exe, Repo};

const STEP: &str = "ns rules check --only ast-grep";

const WAITS: &str =
    "#[test]\nfn waits() {\n    std::thread::sleep(std::time::Duration::from_millis(1));\n}\n";

fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The `ast-grep` on the test's PATH, outside the repo's stubs.
fn real_ast_grep() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("ast-grep"))
        .find(|p| p.is_file())
        .expect("the ast-grep rules step needs ast-grep on PATH; install it with cargo install --locked ast-grep")
}

/// A repo with this checkout's `stack.md` and Rust pack, the real `ns` where `ci-affected.sh`
/// looks for it, and a clean crate in `cli/`, committed.
fn repo() -> Repo {
    let repo = Repo::new();
    let root = repo.dir.path();
    for file in ["check.sh", "ast-grep.yml", "roads.txt"] {
        common::copy_exe(
            &checkout().join("rules/rust").join(file),
            &root.join("rules/rust").join(file),
        );
    }
    fs::create_dir_all(root.join("docs/agents")).unwrap();
    fs::copy(
        checkout().join("docs/agents/stack.md"),
        root.join("docs/agents/stack.md"),
    )
    .unwrap();
    write_exe(
        &root.join("bin/ast-grep"),
        format!("#!/bin/sh\nexec {:?} \"$@\"\n", real_ast_grep()),
    );
    write_exe(
        &root.join("cli/target/debug/ns"),
        format!("#!/bin/sh\nexec {:?} \"$@\"\n", common::ns_path()),
    );
    fs::create_dir_all(root.join("cli/src")).unwrap();
    fs::write(
        root.join("cli/src/lib.rs"),
        "pub fn one() -> u32 {\n    1\n}\n",
    )
    .unwrap();
    repo.commit_all();
    repo
}

fn commit(root: &Path, file: &str, text: &str) {
    write_exe(&root.join(file), text);
    git(root, &["add", file]);
    git(root, &["commit", "-q", "-m", file]);
}

#[test]
fn a_new_wall_clock_wait_fails_the_step_and_a_baselined_one_passes() {
    let repo = repo();
    let root = repo.dir.path();
    commit(root, "cli/tests/old.rs", WAITS);
    let out = common::ns()
        .args(["rules", "check", "--only", "ast-grep"])
        .current_dir(root)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["new"][0]["rule"], "wall-clock-in-test", "{v}");
    commit(
        root,
        ".nightshift/rules-baseline.json",
        &json!({"version": 1, "violations": v["new"]}).to_string(),
    );

    let out = affected(&repo, Some("cli/tests/old.rs"), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), ["cargo test", STEP]);

    let out = affected(&repo, Some(""), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), Vec::<String>::new());

    commit(root, "cli/tests/new.rs", WAITS);
    let out = affected(&repo, Some("cli/tests/new.rs"), &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains(&format!("ci-affected: FAILED at step: {STEP}")),
        "{err}"
    );
    let found = String::from_utf8_lossy(&out.stdout);
    assert!(found.contains("new: cli/tests/new.rs:"), "{found}");
    assert!(found.contains("ast-grep/wall-clock-in-test"), "{found}");
}

#[test]
fn the_step_runs_the_ns_under_cargo_target_dir() {
    let repo = Repo::new();
    let target = repo.dir.path().join("target");
    write_exe(
        &target.join("debug/ns"),
        "#!/bin/sh\necho target ns ran: \"$*\" >&2\n",
    );
    let out = affected(
        &repo,
        Some("cli/src/lib.rs"),
        &[("CARGO_TARGET_DIR", target.to_str().unwrap())],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("target ns ran: rules check --only ast-grep --human"),
        "{}",
        stderr(&out)
    );
}
