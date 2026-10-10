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
}

impl Repo {
    /// A git repo holding a clean crate `app` with a path dependency `extra`, and the pack
    /// installed with `install.sh`.
    fn new() -> Repo {
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
        let r = Repo { _tmp: tmp, root };
        r.write("docs/agents/stack.md", STACK);
        r.write(".gitignore", "target/\n");
        r.write(
            "Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\npublish = false\n\n[dependencies]\nextra = { path = \"extra\" }\n",
        );
        r.write("src/main.rs", MAIN);
        r.write(
            "extra/Cargo.toml",
            "[package]\nname = \"extra\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\npublish = false\n",
        );
        r.write("extra/src/lib.rs", "pub const ONE: u32 = 1;\n");
        let (code, out) = r.install();
        assert_eq!(code, Some(0), "{out}");
        r
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
            .arg(".")
            .timeout(CHECK_TIMEOUT)
            .output();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.code(), text)
    }

    /// `ns rules check`: its exit code, its JSON report and its stderr.
    fn check(&self) -> (Option<i32>, Value, String) {
        let out = common::ns()
            .args(["rules", "check"])
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
    assert_eq!(
        r.new_findings().iter().map(key).collect::<Vec<_>>(),
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
    r.write("src/main.rs", "fn main( {\n");
    let (code, v, err) = r.check();
    assert_eq!(code, Some(1), "{v} {err}");
    assert!(err.contains("with no findings it could read"), "{err}");
    assert!(err.contains("src/main.rs"), "{err}");
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
fn a_compile_error_is_one_finding_at_its_primary_span() {
    let r = Repo::new();
    r.write(
        "src/main.rs",
        &format!("{MAIN}\nfn word() -> u32 {{\n    \"one\"\n}}\n"),
    );
    assert_eq!(
        r.new_findings().iter().map(key).collect::<Vec<_>>(),
        [(
            s("clippy"),
            s("E0308"),
            s("src/main.rs"),
            Some(6),
            s("\"one\"")
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
