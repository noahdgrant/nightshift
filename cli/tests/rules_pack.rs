//! The Rust rule pack in `rules/rust/`: installed into a temp repo by its `install.sh`, then run
//! by `ns rules check` over a small crate with one failing and one passing example per rule.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

/// Clippy compiles the fixture crate on each check, which is slow on a loaded machine.
const CHECK_TIMEOUT: Duration = Duration::from_secs(300);

const STACK: &str = "# Stack\n\n## Languages\n\n| Language | Version |\n|---|---|\n| Rust | stable |\n\n## Commands\n\n| Task | Command | Takes |\n|---|---|---|\n| test | `cargo test` | 1 s |\n\n## Gotchas\n\nnone\n";

const MAIN: &str = "fn main() {\n    println!(\"{}\", extra::ONE);\n}\n";

fn pack() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../rules/rust")
}

/// Fail, naming the tool, rather than let a missing tool pass as a clean pack.
fn need_tools() {
    for (tool, install) in [
        ("cargo", "https://rustup.rs"),
        ("jq", "your package manager (apt install jq)"),
        ("ast-grep", "cargo install --locked ast-grep"),
        ("cargo-deny", "cargo install --locked cargo-deny"),
    ] {
        let found = Command::new(tool).arg("--version").output();
        assert!(
            found.is_ok_and(|o| o.status.success()),
            "the rule pack tests need `{tool}` on PATH; install it with {install}"
        );
    }
}

struct Repo {
    _tmp: TempDir,
    root: PathBuf,
    krate: String,
}

impl Repo {
    /// A git repo holding a clean crate `app` with a path dependency `extra`, and the pack
    /// installed with `install.sh`.
    fn new() -> Repo {
        let r = Repo::uninstalled(".");
        let (code, out) = r.install();
        assert_eq!(code, Some(0), "{out}");
        r
    }

    /// Like `new`, with the crate in `krate` (a subdirectory of the repo) instead of the root.
    fn in_dir(krate: &str) -> Repo {
        let r = Repo::uninstalled(krate);
        let (code, out) = r.install();
        assert_eq!(code, Some(0), "{out}");
        r
    }

    /// The fixture repo before `install.sh` has run.
    fn uninstalled(krate: &str) -> Repo {
        need_tools();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let git = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(git.status.success());
        let r = Repo {
            _tmp: tmp,
            root,
            krate: krate.to_string(),
        };
        r.write("docs/agents/stack.md", STACK);
        r.write(".gitignore", "target/\n");
        r.write(
            &r.at("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\npublish = false\n\n[dependencies]\nextra = { path = \"extra\" }\n",
        );
        r.write(&r.at("src/main.rs"), MAIN);
        r.write(
            &r.at("extra/Cargo.toml"),
            "[package]\nname = \"extra\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\npublish = false\n",
        );
        r.write(&r.at("extra/src/lib.rs"), "pub const ONE: u32 = 1;\n");
        r
    }

    fn at(&self, rel: &str) -> String {
        if self.krate == "." {
            rel.to_string()
        } else {
            format!("{}/{rel}", self.krate)
        }
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root.join(rel)).unwrap()
    }

    fn install(&self) -> (Option<i32>, String) {
        let out = common::command(pack().join("install.sh"))
            .arg(&self.root)
            .arg(&self.krate)
            .timeout(CHECK_TIMEOUT)
            .output();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.code(), text)
    }

    /// `ns rules check`: its exit code, its JSON report and its stderr. Cargo runs one job at a
    /// time, as on a loaded machine, so a member that clippy skips after another member fails
    /// can show up here rather than only in CI.
    fn check(&self) -> (Option<i32>, Value, String) {
        self.rules(&["check"])
    }

    /// `ns rules <args>`, run as `check` runs it.
    fn rules(&self, args: &[&str]) -> (Option<i32>, Value, String) {
        let out = common::ns()
            .arg("rules")
            .args(args)
            .env("CARGO_BUILD_JOBS", "1")
            .env_remove("CARGO_TARGET_DIR")
            .current_dir(&self.root)
            .timeout(CHECK_TIMEOUT)
            .output();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let v = serde_json::from_str(&stdout).unwrap_or(Value::Null);
        (
            out.status.code(),
            v,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// The new findings, after asserting the check failed on them.
    fn new_findings(&self) -> Vec<Value> {
        let (code, v, err) = self.check();
        assert_eq!(code, Some(1), "{v} {err}");
        v["new"].as_array().unwrap().clone()
    }

    fn assert_clean(&self) {
        let (code, v, err) = self.check();
        assert_eq!(code, Some(0), "{v} {err}");
        assert_eq!(v["findings"], 0, "{v}");
        assert_eq!(v["commands"].as_array().unwrap().len(), 3, "{v}");
    }

    /// The one new finding, whose message must name `link`.
    fn one_finding(&self, link: &str) -> Key {
        let new = self.new_findings();
        assert_eq!(new.len(), 1, "{new:?}");
        let msg = new[0]["message"].as_str().unwrap();
        assert!(msg.contains(link), "{msg}");
        key(&new[0])
    }
}

type Key = (String, String, String, Option<u64>, String);

/// What identifies a finding: `(tool, rule, file, line, snippet)`.
fn key(f: &Value) -> Key {
    let s = |k: &str| f[k].as_str().unwrap().to_string();
    (
        s("tool"),
        s("rule"),
        s("file"),
        f["line"].as_u64(),
        s("snippet"),
    )
}

fn s(x: &str) -> String {
    x.to_string()
}

#[test]
fn install_copies_the_pack_and_writes_one_rules_row_and_the_check_passes() {
    let r = Repo::new();
    for f in [
        "check.sh",
        "deny.toml",
        "ast-grep.yml",
        "clippy-lints.txt",
        "roads.txt",
    ] {
        assert!(
            r.root.join(".nightshift/rules/rust").join(f).is_file(),
            "{f}"
        );
    }
    let crates = r.read(".nightshift/rules/rust/allowed-crates.txt");
    assert_eq!(
        crates
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>(),
        ["app", "extra"]
    );
    let row = "| rules | `.nightshift/rules/rust/check.sh clippy .`, `.nightshift/rules/rust/check.sh deny .`, `.nightshift/rules/rust/check.sh ast-grep .` | 1 min |";
    let stack = r.read("docs/agents/stack.md");
    assert!(
        stack.contains(&format!(
            "| test | `cargo test` | 1 s |\n{row}\n\n## Gotchas"
        )),
        "{stack}"
    );
    r.assert_clean();

    r.write(".nightshift/rules/rust/roads.txt", "kept src/main.rs\n");
    r.write(
        ".nightshift/rules/rust/allowed-crates.txt",
        "app\nextra\nkept\n",
    );
    let (code, out) = r.install();
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(r.read("docs/agents/stack.md"), stack);
    assert_eq!(
        r.read(".nightshift/rules/rust/roads.txt"),
        "kept src/main.rs\n"
    );
    assert_eq!(
        r.read(".nightshift/rules/rust/allowed-crates.txt"),
        "app\nextra\nkept\n"
    );
}

#[test]
fn a_failed_install_writes_nothing() {
    for cargo_toml in [None, Some("not toml [")] {
        let r = Repo::uninstalled(".");
        fs::remove_file(r.root.join("Cargo.toml")).unwrap();
        if let Some(text) = cargo_toml {
            r.write("Cargo.toml", text);
        }
        let stack = r.read("docs/agents/stack.md");
        let (code, out) = r.install();
        assert_eq!(code, Some(2), "{out}");
        assert!(out.contains("cargo metadata failed"), "{out}");
        assert!(!r.root.join(".nightshift").exists());
        assert_eq!(r.read("docs/agents/stack.md"), stack);
    }
}

#[test]
fn a_stack_without_a_commands_table_leaves_the_repo_untouched() {
    let r = Repo::uninstalled(".");
    r.write("docs/agents/stack.md", "# Stack\n");
    let (code, out) = r.install();
    assert_eq!(code, Some(2), "{out}");
    assert!(!r.root.join(".nightshift").exists());
}

#[test]
fn a_crate_in_a_subdirectory_is_checked_with_paths_from_the_repo_root() {
    let r = Repo::in_dir("sub");
    assert!(r
        .read("docs/agents/stack.md")
        .contains("check.sh clippy sub`"));
    r.assert_clean();
    r.write(
        ".nightshift/rules/rust/roads.txt",
        "tail sub/extra/src/lib.rs\n",
    );
    r.write(
        "sub/extra/src/lib.rs",
        "pub const ONE: u32 = 1;\npub fn tail() {}\n",
    );
    r.write(
        "sub/src/main.rs",
        "fn count(v: Vec<u8>) -> usize {\n    v.len()\n}\n\nfn tail() {}\n\nfn main() {\n    tail();\n    println!(\"{} {}\", extra::ONE, count(vec![1]));\n}\n",
    );
    r.write(
        ".nightshift/rules/rust/allowed-crates.txt",
        "# crates\napp\n",
    );
    let found = |tool: &str, rule: &str, file: &str, line| (s(tool), s(rule), s(file), line, s(""));
    let mut got: Vec<_> = r
        .new_findings()
        .iter()
        .map(|f| {
            let mut k = key(f);
            k.4 = s("");
            k
        })
        .collect();
    got.sort();
    let mut want = vec![
        found(
            "clippy",
            "clippy::needless_pass_by_value",
            "sub/src/main.rs",
            Some(1),
        ),
        found("ast-grep", "duplicate-helper", "sub/src/main.rs", Some(5)),
        found("cargo-deny", "not-allowed", "sub/Cargo.toml", None),
    ];
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn clippy_lints_every_workspace_member() {
    let r = Repo::new();
    let root = r.read("Cargo.toml");
    r.write(
        "Cargo.toml",
        &format!("{root}\n[workspace]\nmembers = [\"member\"]\n"),
    );
    r.write(
        "member/Cargo.toml",
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\npublish = false\n",
    );
    r.write(
        "member/src/lib.rs",
        "pub fn count(v: Vec<u8>) -> usize {\n    v.len()\n}\n",
    );
    r.write(
        ".nightshift/rules/rust/allowed-crates.txt",
        "app\nextra\nmember\n",
    );
    assert_eq!(
        r.new_findings().iter().map(key).collect::<Vec<_>>(),
        [(
            s("clippy"),
            s("clippy::needless_pass_by_value"),
            s("member/src/lib.rs"),
            Some(1),
            s("pub fn count(v: Vec<u8>) -> usize {")
        )]
    );
}

/// Assert that `ns rules check` failed as broken, naming each of `want` on stderr, and return it.
fn assert_broken(r: &Repo, want: &[&str]) -> String {
    let (code, v, err) = r.check();
    assert_eq!(code, Some(1), "{v} {err}");
    assert!(err.contains("exited 2 with no findings"), "{err}");
    assert!(err.contains("has compile errors"), "{err}");
    for w in want {
        assert!(err.contains(w), "{w} in {err}");
    }
    err
}

/// `user` depends on `good`, so cargo's one job lints `good` before `bad`, and a clippy that
/// stops at the first failing member never compiles `bad`.
#[test]
fn a_syntax_error_in_one_member_fails_as_broken_and_still_lints_the_others() {
    let r = Repo::new();
    let root = r.read("Cargo.toml");
    r.write(
        "Cargo.toml",
        &format!("{root}\n[workspace]\nmembers = [\"good\", \"bad\", \"user\"]\n"),
    );
    for (name, deps) in [
        ("good", ""),
        ("bad", ""),
        ("user", "good = { path = \"../good\" }\n"),
    ] {
        r.write(
            &format!("{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\npublish = false\n\n[dependencies]\n{deps}"),
        );
    }
    r.write(
        "good/src/lib.rs",
        "pub fn count(v: Vec<u8>) -> usize {\n    v.len()\n}\n",
    );
    r.write("bad/src/lib.rs", "pub fn f( {\n");
    r.write("user/src/lib.rs", "pub use good::count;\n");
    r.write(
        ".nightshift/rules/rust/allowed-crates.txt",
        "app\nextra\ngood\nbad\nuser\n",
    );
    assert_broken(
        &r,
        &[
            "./bad/src/lib.rs:1: error: ",
            "also found clippy::needless_pass_by_value at ./good/src/lib.rs:1",
        ],
    );

    let (code, v, err) = r.rules(&["baseline", "--allow-growth"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert!(err.contains("has compile errors"), "{err}");
    assert!(!r.root.join(".nightshift/rules-baseline.json").exists());
}

#[test]
fn install_adds_the_row_when_the_commands_table_ends_the_file() {
    let r = Repo::new();
    let stack = r.read("docs/agents/stack.md");
    let (head, _) = stack.split_once("\n## Gotchas").unwrap();
    r.write("docs/agents/stack.md", head.trim_end());
    let (code, out) = r.install();
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(r.read("docs/agents/stack.md"), head);
}

#[test]
fn install_replaces_an_old_rules_row() {
    let r = Repo::new();
    let stack = r.read("docs/agents/stack.md");
    r.write(
        "docs/agents/stack.md",
        &stack.replace(".nightshift/rules/rust/check.sh ast-grep .", "old"),
    );
    let (code, out) = r.install();
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(r.read("docs/agents/stack.md"), stack);
}

#[test]
fn install_refuses_a_repo_without_a_commands_table() {
    let r = Repo::new();
    r.write("docs/agents/stack.md", "# Stack\n");
    let (code, out) = r.install();
    assert_eq!(code, Some(2), "{out}");
    assert!(out.contains("no Commands table"), "{out}");
    fs::remove_file(r.root.join("docs/agents/stack.md")).unwrap();
    let (code, out) = r.install();
    assert_eq!(code, Some(2), "{out}");
    assert!(out.contains("ns-setup"), "{out}");
}

#[test]
fn a_suppression_needs_a_reason_on_the_same_line() {
    let r = Repo::new();
    r.write(
        "src/main.rs",
        &format!("#![allow(clippy::needless_pass_by_value)]\n{MAIN}\n#[expect(dead_code)]\nfn unused() {{}}\n"),
    );
    let new = r.new_findings();
    assert!(
        new.iter()
            .all(|f| f["message"].as_str().unwrap().contains("quality-bar.md")),
        "{new:?}"
    );
    let rule = |line, snippet| {
        (
            s("ast-grep"),
            s("suppression-without-reason"),
            s("src/main.rs"),
            Some(line),
            s(snippet),
        )
    };
    assert_eq!(
        new.iter().map(key).collect::<Vec<_>>(),
        [
            rule(1, "#![allow(clippy::needless_pass_by_value)]"),
            rule(6, "#[expect(dead_code)]")
        ]
    );
    r.write(
        "src/main.rs",
        &format!("#![allow(clippy::needless_pass_by_value, reason = \"the demo\")]\n{MAIN}\n#[expect(dead_code, reason = \"the demo\")]\nfn unused() {{}}\n"),
    );
    r.assert_clean();
}

#[test]
fn a_file_past_1000_lines_is_too_long() {
    let r = Repo::new();
    r.write("src/long.rs", &"// line\n".repeat(1001));
    assert_eq!(
        r.one_finding("quality-bar.md"),
        (
            s("ast-grep"),
            s("file-too-long"),
            s("src/long.rs"),
            Some(1),
            s("// line")
        )
    );
    r.write("src/long.rs", &"// line\n".repeat(1000));
    r.assert_clean();
}

#[test]
fn a_second_definition_of_a_road_is_a_duplicate_helper() {
    let r = Repo::new();
    r.write(
        ".nightshift/rules/rust/roads.txt",
        "# helper home\ntail extra/src/lib.rs\nBuffer extra/src/lib.rs\nMode extra/src/lib.rs\nStore extra/src/lib.rs\nBytes extra/src/lib.rs\n",
    );
    r.write(
        "extra/src/lib.rs",
        "pub const ONE: u32 = 1;\npub fn tail() {}\npub struct Buffer;\npub enum Mode {}\npub trait Store {}\npub type Bytes = Vec<u8>;\n",
    );
    r.assert_clean();
    r.write(
        "src/other.rs",
        "fn tail() {}\nstruct Buffer;\nenum Mode {}\ntrait Store {}\ntype Bytes = Vec<u8>;\n",
    );
    let new = r.new_findings();
    assert!(
        new.iter().all(|f| f["message"]
            .as_str()
            .unwrap()
            .contains("ns-principle-pave-the-road")),
        "{new:?}"
    );
    let dup = |line, snippet| {
        (
            s("ast-grep"),
            s("duplicate-helper"),
            s("src/other.rs"),
            Some(line),
            s(snippet),
        )
    };
    assert_eq!(
        new.iter().map(key).collect::<Vec<_>>(),
        [
            dup(1, "fn tail() {}"),
            dup(2, "struct Buffer;"),
            dup(3, "enum Mode {}"),
            dup(4, "trait Store {}"),
            dup(5, "type Bytes = Vec<u8>;")
        ]
    );
}

#[test]
fn a_crate_outside_the_allowlist_is_not_allowed() {
    let r = Repo::new();
    r.write(
        ".nightshift/rules/rust/allowed-crates.txt",
        "# crates\napp\n",
    );
    assert_eq!(
        r.one_finding("ns-principle-pave-the-road"),
        (
            s("cargo-deny"),
            s("not-allowed"),
            s("Cargo.toml"),
            None,
            s("extra")
        )
    );
}

#[test]
fn an_empty_allowlist_is_an_error_not_a_pass() {
    let r = Repo::new();
    r.write(".nightshift/rules/rust/allowed-crates.txt", "# crates\n");
    let (code, v, err) = r.check();
    assert_eq!(code, Some(1), "{v} {err}");
    assert!(err.contains("allowed-crates.txt lists no crates"), "{err}");
}

#[test]
fn clippy_findings_carry_the_lint_as_their_rule() {
    let r = Repo::new();
    r.write(
        "src/main.rs",
        "fn count(v: Vec<u8>) -> usize {\n    v.len()\n}\n\nfn main() {\n    println!(\"{} {}\", extra::ONE, count(vec![1]));\n}\n",
    );
    let new = r.new_findings();
    let msg = new[0]["message"].as_str().unwrap();
    assert!(
        msg.contains("Performance: no needless copies") && msg.contains("quality-bar.md"),
        "{msg}"
    );
    assert_eq!(
        new.iter().map(key).collect::<Vec<_>>(),
        [(
            s("clippy"),
            s("clippy::needless_pass_by_value"),
            s("src/main.rs"),
            Some(1),
            s("fn count(v: Vec<u8>) -> usize {")
        )]
    );
}

#[test]
fn a_missing_tool_fails_the_check_and_names_the_tool() {
    let r = Repo::new();
    let only_ast_grep = r.root.join("bin");
    fs::create_dir(&only_ast_grep).unwrap();
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v ast-grep"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::os::unix::fs::symlink(real.trim(), only_ast_grep.join("ast-grep")).unwrap();
    for (path, mode, missing) in [
        ("/nonexistent", "clippy", "cargo"),
        ("/nonexistent", "deny", "cargo-deny"),
        ("/nonexistent", "ast-grep", "ast-grep"),
        (only_ast_grep.to_str().unwrap(), "ast-grep", "jq"),
    ] {
        r.write(
            "docs/agents/stack.md",
            &format!("| rules | `PATH={path} .nightshift/rules/rust/check.sh {mode} .` | 1 s |\n"),
        );
        let (code, v, err) = r.check();
        assert_eq!(code, Some(1), "{v} {err}");
        assert!(err.contains("exited 127"), "{err}");
        assert!(
            err.contains(&format!("check.sh: {missing} is not on PATH")),
            "{err}"
        );
    }
}

#[test]
fn a_tool_that_fails_with_no_findings_shows_its_output() {
    let r = Repo::new();
    r.write("Cargo.toml", "[package\n");
    let (code, v, err) = r.check();
    assert_eq!(code, Some(1), "{v} {err}");
    assert!(err.contains("with no findings it could read"), "{err}");
    assert!(err.contains("Cargo.toml"), "{err}");
}

#[test]
fn an_unknown_tool_or_a_missing_crate_dir_is_a_usage_error() {
    let r = Repo::new();
    for cmd in ["clippy", "lint ."] {
        r.write(
            "docs/agents/stack.md",
            &format!("| rules | `.nightshift/rules/rust/check.sh {cmd}` | 1 s |\n"),
        );
        let (code, v, err) = r.check();
        assert_eq!(code, Some(1), "{v} {err}");
        assert!(err.contains("exited 2"), "{err}");
        assert!(err.contains("usage: "), "{err}");
    }
}

#[test]
fn a_type_error_fails_as_broken_at_its_primary_span() {
    let r = Repo::new();
    r.write(
        "src/main.rs",
        &format!("{MAIN}\nfn word() -> u32 {{\n    \"one\"\n}}\n"),
    );
    let err = assert_broken(&r, &["./src/main.rs:6: E0308: mismatched types"]);
    assert!(!err.contains("also found"), "{err}");
}

#[test]
fn a_rustc_lint_is_a_finding_not_a_compile_error() {
    let r = Repo::new();
    r.write("src/main.rs", &format!("{MAIN}\nfn unused() {{}}\n"));
    assert_eq!(
        r.new_findings().iter().map(key).collect::<Vec<_>>(),
        [(
            s("clippy"),
            s("dead_code"),
            s("src/main.rs"),
            Some(5),
            s("fn unused() {}")
        )]
    );
}

#[test]
fn install_defaults_the_crate_dir_to_the_repo_root_and_needs_a_repo() {
    let r = Repo::new();
    let stack = r.read("docs/agents/stack.md");
    let run = |args: &[&Path]| {
        common::command(pack().join("install.sh"))
            .args(args)
            .output()
    };
    let out = run(&[&r.root]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(r.read("docs/agents/stack.md"), stack);
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: "));
}

#[test]
fn python_pack_tests_run_the_ns_under_cargo_target_dir() {
    need_tools();
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let marker = tmp.path().join("ns-ran");
    let ns = tmp.path().join("target/debug/ns");
    fs::create_dir_all(ns.parent().unwrap()).unwrap();
    fs::write(
        &ns,
        format!(
            "#!/bin/sh\ntouch '{}'\nexec '{}' \"$@\"\n",
            marker.display(),
            common::ns_path().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&ns, fs::Permissions::from_mode(0o755)).unwrap();
    let run_sh = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rules/python/tests/run.sh");
    let out = common::command("bash")
        .arg(run_sh)
        .timeout(CHECK_TIMEOUT)
        .env_remove("CI")
        .env("CARGO_TARGET_DIR", tmp.path().join("target"))
        .output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("skipped"), "{err}");
    assert!(
        marker.exists(),
        "run.sh did not run the ns under CARGO_TARGET_DIR"
    );
}

#[test]
fn python_pack_tests_run_the_ns_under_a_relative_cargo_target_dir() {
    need_tools();
    use std::os::unix::fs::PermissionsExt;
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let tmp = TempDir::new_in(repo_root).unwrap();
    let marker = tmp.path().join("ns-ran");
    let ns = tmp.path().join("target/debug/ns");
    fs::create_dir_all(ns.parent().unwrap()).unwrap();
    fs::write(
        &ns,
        format!(
            "#!/bin/sh\ntouch '{}'\nexec '{}' \"$@\"\n",
            marker.display(),
            common::ns_path().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&ns, fs::Permissions::from_mode(0o755)).unwrap();
    let run_sh = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rules/python/tests/run.sh");
    let out = common::command("bash")
        .arg(run_sh)
        .timeout(CHECK_TIMEOUT)
        .env_remove("CI")
        .env(
            "CARGO_TARGET_DIR",
            Path::new(tmp.path().file_name().unwrap()).join("target"),
        )
        .output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("skipped"), "{err}");
    assert!(
        marker.exists(),
        "run.sh did not run the ns under a relative CARGO_TARGET_DIR"
    );
}
