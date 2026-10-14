//! The Java rule pack in `rules/java/`: installed into a temp repo by its `install.sh`, then run
//! by `ns rules check`, whose baseline holds today's findings and fails only on new ones.
//!
//! `install.sh --offline` takes PMD from the zip `scripts/ci-tools.sh install pmd` caches, so
//! these tests never download it.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

/// PMD starts a JVM on each check, which is slow on a loaded machine.
const CHECK_TIMEOUT: Duration = Duration::from_secs(300);

const STACK: &str = "# Stack\n\n## Commands\n\n| Task | Command | Takes |\n|---|---|---|\n| test | `mvn test` | 1 s |\n\n## Gotchas\n\nnone\n";

const APP: &str = "package com.example.app;\n\nimport java.util.List;\n\npublic class App {\n    public List<String> names() {\n        return List.of(\"a\");\n    }\n}\n";

/// An empty catch block (PMD) and a NOPMD with no reason (ast-grep), on lines 6 and 11.
const BAD: &str = "package com.example.app;\n\nclass Bad {\n    int parse(String s) {\n        try {\n            return Integer.parseInt(s);\n        } catch (NumberFormatException e) {\n        }\n        return 0;\n    }\n    int legacy; // NOPMD\n}\n";

fn pack() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../rules/java")
}

/// Fail, naming the tool, rather than let a missing tool pass as a clean pack.
fn need_tools() {
    for (tool, flag) in [
        ("java", "-version"),
        ("ast-grep", "--version"),
        ("jq", "--version"),
    ] {
        let found = Command::new(tool).arg(flag).output();
        assert!(
            found.is_ok_and(|o| o.status.success()),
            "the Java rule pack tests need `{tool}` on PATH; scripts/ci-tools.sh check {tool} names how to install it"
        );
    }
}

struct Repo {
    _tmp: TempDir,
    root: PathBuf,
}

impl Repo {
    /// A git repo holding one clean class, with the pack installed by `install.sh`.
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
        r.write("src/main/java/com/example/app/App.java", APP);
        let out = common::command(pack().join("install.sh"))
            .arg("--offline")
            .arg(&r.root)
            .timeout(CHECK_TIMEOUT)
            .output();
        assert!(
            out.status.success(),
            "install.sh failed (fetch PMD with scripts/ci-tools.sh install pmd): {}",
            String::from_utf8_lossy(&out.stderr)
        );
        r
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    /// `ns rules <args>`: its exit code, its JSON report and its stderr.
    fn rules(&self, args: &[&str]) -> (Option<i32>, Value, String) {
        let out = common::ns()
            .arg("rules")
            .args(args)
            .current_dir(&self.root)
            .timeout(CHECK_TIMEOUT)
            .output();
        let v = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (
            out.status.code(),
            v,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

/// `tool/rule file:line snippet` for each finding in `list`, sorted.
fn names(list: &Value) -> Vec<String> {
    let mut out: Vec<String> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            format!(
                "{}/{} {}{} {}",
                f["tool"].as_str().unwrap(),
                f["rule"].as_str().unwrap(),
                f["file"].as_str().unwrap(),
                f["line"]
                    .as_u64()
                    .map_or(String::new(), |n| format!(":{n}")),
                f["snippet"].as_str().unwrap()
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn ns_rules_check_reads_the_java_pack_and_its_baseline_holds_only_todays_findings() {
    let r = Repo::new();
    let (code, v, err) = r.rules(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["findings"], 0, "{v}");
    let commands: Vec<&str> = v["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["command"].as_str().unwrap())
        .collect();
    assert_eq!(
        commands,
        [
            ".nightshift/rules/java/check.sh pmd .",
            ".nightshift/rules/java/check.sh ast-grep .",
            ".nightshift/rules/java/check.sh size ."
        ]
    );

    r.write("src/main/java/com/example/app/Bad.java", BAD);
    let (code, v, err) = r.rules(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    let bad = "src/main/java/com/example/app/Bad.java";
    assert_eq!(
        names(&v["new"]),
        [
            format!("ast-grep/suppression-reason {bad}:11 int legacy; // NOPMD"),
            format!("pmd/EmptyCatchBlock {bad}:7 }} catch (NumberFormatException e) {{"),
        ]
    );

    // Accepting today's findings is growth, which only --allow-growth, a human's act, allows.
    let (code, v, err) = r.rules(&["baseline"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert!(!r.root.join(".nightshift/rules-baseline.json").exists());
    let (code, v, err) = r.rules(&["baseline", "--allow-growth"]);
    assert_eq!(code, Some(0), "{v} {err}");
    let (code, v, err) = r.rules(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["baselined"], 2, "{v}");

    // A baselined finding is its snippet, so lines added above it don't make it new; a second
    // copy of a bad line does.
    let moved = BAD.replace(
        "class Bad {\n",
        "class Bad {\n    int first;\n    int second;\n",
    );
    r.write(
        bad,
        &moved.replace(
            "    int legacy; // NOPMD\n",
            "    int legacy; // NOPMD\n    int legacy2; // NOPMD\n",
        ),
    );
    let (code, v, err) = r.rules(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(
        names(&v["new"]),
        [format!(
            "ast-grep/suppression-reason {bad}:14 int legacy2; // NOPMD"
        )]
    );
    assert_eq!(v["baselined"], 2, "{v}");

    // Fixing a baselined finding lets the baseline shrink without --allow-growth.
    r.write(
        bad,
        &BAD.replace("// NOPMD", "// NOPMD - read by reflection"),
    );
    let (code, v, err) = r.rules(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(
        names(&v["fixed"]),
        [format!(
            "ast-grep/suppression-reason {bad} int legacy; // NOPMD"
        )]
    );
    let (code, v, err) = r.rules(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["dropped"].as_array().unwrap().len(), 1, "{v}");
    let (code, v, err) = r.rules(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(
        (v["baselined"].clone(), v["fixed"].clone()),
        (1.into(), serde_json::json!([]))
    );
}

#[test]
fn a_java_file_pmd_cannot_parse_fails_the_check_as_broken() {
    let r = Repo::new();
    r.write("src/Broken.java", "class Broken {\n    void f( {\n}\n");
    let (code, v, err) = r.rules(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert!(err.contains("exited 2 with no findings"), "{err}");
    assert!(err.contains("PMD exited 5"), "{err}");
    assert!(err.contains("src/Broken.java: ParseException"), "{err}");
}
