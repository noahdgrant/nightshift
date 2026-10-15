//! `ns rules check` and `ns rules baseline` against fake rule tools that print canned findings.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use serde_json::Value;
use tempfile::TempDir;

const BASELINE: &str = ".nightshift/rules-baseline.json";

/// Prints `<its name>.out` and fails when it printed anything, as linters do.
const FAKE_TOOL: &str = "#!/bin/sh\nout=\"$0.out\"\ncat \"$out\" 2>/dev/null\n[ ! -s \"$out\" ]\n";

const SRC: &str = "import os\nx = eval(s)\ny = eval(s)\n";

struct Repo {
    _tmp: TempDir,
    root: PathBuf,
}

impl Repo {
    /// A git repo whose `stack.md` lists `./lint-a` and `./lint-b` as its rule pack.
    fn new() -> Repo {
        Repo::with_stack(
            "| Task | Command |\n|---|---|\n| rules | `./lint-a`, `./lint-b` | 1 s |\n",
        )
    }

    fn with_stack(stack: &str) -> Repo {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let out = StdCommand::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(out.status.success());
        fs::create_dir_all(root.join("docs/agents")).unwrap();
        fs::write(root.join("docs/agents/stack.md"), stack).unwrap();
        fs::write(root.join("src.py"), SRC).unwrap();
        for tool in ["lint-a", "lint-b"] {
            let p = root.join(tool);
            common::write_exe(&p, FAKE_TOOL);
        }
        Repo { _tmp: tmp, root }
    }

    fn findings(&self, tool: &str, lines: &str) {
        fs::write(self.root.join(format!("{tool}.out")), lines).unwrap();
    }

    fn ns(&self, args: &[&str]) -> (Option<i32>, Value, String) {
        let out = common::ns()
            .arg("rules")
            .args(args)
            .current_dir(&self.root)
            .output();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let v = serde_json::from_str(&stdout).unwrap_or(Value::Null);
        (out.status.code(), v, stderr)
    }

    fn baseline(&self) -> Option<Value> {
        let text = fs::read_to_string(self.root.join(BASELINE)).ok()?;
        Some(serde_json::from_str(&text).unwrap())
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

fn files_and_lines(list: &Value) -> Vec<(String, u64)> {
    list.as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .map(|f| {
            (
                f["file"].as_str().unwrap().to_string(),
                f["line"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// The baseline file's entries as `(file, snippet)`, checking none stores a line.
fn baselined(r: &Repo) -> Vec<(String, String)> {
    r.baseline().unwrap()["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            assert!(f.get("line").is_none(), "a baseline entry has a line: {f}");
            (
                f["file"].as_str().unwrap().to_string(),
                f["snippet"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_new_violation_fails_and_names_tool_rule_file_line_and_snippet() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    r.findings(
        "lint-b",
        "{\"rule\": \"S307\", \"file\": \"src.py\", \"line\": 3, \"message\": \"no eval\"}\n",
    );
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(v["ok"], false);
    assert_eq!(v["findings"], 2);
    assert_eq!(v["baselined"], 0);
    let new = v["new"].as_array().unwrap();
    assert_eq!(new.len(), 2, "{v}");
    assert_eq!(new[0]["tool"], "lint-a");
    assert_eq!(new[0]["rule"], "");
    assert_eq!(new[0]["file"], "src.py");
    assert_eq!(new[0]["line"], 2);
    assert_eq!(new[0]["message"], "eval is banned");
    assert_eq!(new[0]["snippet"], "x = eval(s)");
    assert_eq!(new[1]["tool"], "lint-b");
    assert_eq!(new[1]["rule"], "S307");
    assert_eq!(new[1]["snippet"], "y = eval(s)");
    assert_eq!(
        v["commands"],
        serde_json::json!([
            {"command": "./lint-a", "tool": "lint-a", "exit": 1, "findings": 1},
            {"command": "./lint-b", "tool": "lint-b", "exit": 1, "findings": 1},
        ])
    );
}

#[test]
fn a_baselined_violation_passes() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    let (code, v, err) = r.ns(&["baseline", "--allow-growth"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "written");
    assert_eq!(files_and_lines(&v["added"]), [("src.py".into(), 2)]);
    let b = r.baseline().unwrap();
    let names: Vec<_> = fs::read_dir(r.path(".nightshift"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["rules-baseline.json"]);
    assert_eq!(b["version"], 1);
    assert_eq!(baselined(&r), [("src.py".into(), "x = eval(s)".into())]);

    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["baselined"], 1);
    assert_eq!(v["new"], serde_json::json!([]));
}

#[test]
fn a_fixed_violation_is_reported_by_check_and_dropped_by_baseline() {
    let r = Repo::new();
    r.findings(
        "lint-a",
        "src.py:2:5: eval is banned\nsrc.py:3:5: eval is banned\n",
    );
    r.ns(&["baseline", "--allow-growth"]);
    r.findings("lint-a", "src.py:3:5: eval is banned\n");

    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    let fixed = v["fixed"].as_array().unwrap();
    assert_eq!(fixed.len(), 1, "{v}");
    assert_eq!(fixed[0]["snippet"], "x = eval(s)");
    assert!(fixed[0]["line"].is_null(), "{v}");
    assert!(err.contains("1 baselined violations are gone"), "{err}");
    assert_eq!(baselined(&r).len(), 2);
    let out = common::ns()
        .args(["rules", "check", "--human"])
        .current_dir(&r.root)
        .output();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("fixed: src.py: lint-a: eval is banned\n"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "written");
    assert_eq!(v["dropped"][0]["snippet"], "x = eval(s)", "{v}");
    assert_eq!(baselined(&r), [("src.py".into(), "y = eval(s)".into())]);

    let (code, v, _) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0));
    assert_eq!(v["status"], "already-done");
    let out = common::ns()
        .args(["rules", "baseline", "--human"])
        .current_dir(&r.root)
        .output();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "already-done: 1 violations, 0 added, 0 dropped\n"
    );
}

#[test]
fn growth_is_refused_without_allow_growth() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(v["status"], "refused");
    assert!(err.contains("ns rules baseline --allow-growth"), "{err}");
    assert!(r.baseline().is_none());

    r.ns(&["baseline", "--allow-growth"]);
    let before = fs::read_to_string(r.path(BASELINE)).unwrap();
    r.findings(
        "lint-a",
        "src.py:2:5: eval is banned\nsrc.py:3:5: eval is banned\n",
    );
    r.findings("lint-b", "");
    let (code, v, _) = r.ns(&["baseline"]);
    assert_eq!(code, Some(1));
    assert_eq!(v["ok"], false);
    assert_eq!(files_and_lines(&v["added"]), [("src.py".into(), 3)]);
    assert_eq!(fs::read_to_string(r.path(BASELINE)).unwrap(), before);

    let (code, v, _) = r.ns(&["baseline", "--allow-growth"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(baselined(&r).len(), 2);
}

#[test]
fn a_line_number_shift_is_not_a_new_violation() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    r.ns(&["baseline", "--allow-growth"]);
    fs::write(r.path("src.py"), format!("# a new first line\n{SRC}")).unwrap();
    r.findings("lint-a", "src.py:3:5: eval is banned\n");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["baselined"], 1);
    assert_eq!(v["fixed"], serde_json::json!([]));
}

#[test]
fn a_line_number_shift_leaves_the_baseline_byte_for_byte_unchanged() {
    let r = Repo::new();
    r.findings(
        "lint-a",
        "src.py:2:5: eval is banned\nsrc.py:3:5: eval is banned\n",
    );
    r.ns(&["baseline", "--allow-growth"]);
    let before = fs::read_to_string(r.path(BASELINE)).unwrap();
    fs::write(r.path("src.py"), format!("\n\n{SRC}")).unwrap();
    r.findings(
        "lint-a",
        "src.py:4:5: eval is banned\nsrc.py:5:5: eval is banned\n",
    );
    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "already-done");
    assert_eq!(fs::read_to_string(r.path(BASELINE)).unwrap(), before);
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["new"][0], Value::Null, "{v}");
    assert_eq!(v["baselined"], 2);

    r.findings("lint-a", "src.py:5:5: eval is banned\n");
    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "written");
    assert_eq!(baselined(&r), [("src.py".into(), "y = eval(s)".into())]);
}

#[test]
fn a_baseline_with_line_fields_still_loads_and_ignores_them() {
    let r = Repo::new();
    fs::create_dir_all(r.path(".nightshift")).unwrap();
    let entry = |line: u64, snippet: &str| {
        serde_json::json!({"tool": "lint-a", "rule": "", "file": "src.py", "line": line,
            "message": "eval is banned", "snippet": snippet})
    };
    let old = serde_json::json!({"version": 1, "violations": [entry(9, "x = eval(s)"), entry(9, "y = eval(s)")]});
    fs::write(r.path(BASELINE), old.to_string()).unwrap();
    r.findings("lint-a", "src.py:3:5: eval is banned\n");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["baselined"], 1);
    assert_eq!(v["fixed"][0]["snippet"], "x = eval(s)", "{v}");
    assert!(v["fixed"][0]["line"].is_null(), "{v}");

    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "written");
    assert_eq!(baselined(&r), [("src.py".into(), "y = eval(s)".into())]);
}

#[test]
fn an_identical_violation_on_a_second_identical_line_is_new() {
    let r = Repo::new();
    fs::write(r.path("src.py"), "x = eval(s)\n").unwrap();
    r.findings("lint-a", "src.py:1:5: eval is banned\n");
    r.ns(&["baseline", "--allow-growth"]);
    fs::write(r.path("src.py"), "x = eval(s)\nx  =  eval(s)\n").unwrap();
    r.findings(
        "lint-a",
        "src.py:1:5: eval is banned\nsrc.py:2:5: eval is banned\n",
    );
    let (code, v, _) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{v}");
    assert_eq!(v["baselined"], 1);
    assert_eq!(v["new"].as_array().unwrap().len(), 1);
}

#[test]
fn a_tool_that_fails_without_findings_is_an_error() {
    let r = Repo::with_stack("| rules | `./lint-a`, `exit 3` |\n");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert!(err.contains("`exit 3` exited 3"), "{err}");
    let r = Repo::with_stack("| rules | `kill -9 $$` |\n");
    let (code, _, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1));
    assert!(err.contains("was killed by a signal"), "{err}");
}

#[test]
fn no_rules_row_checks_nothing_and_baseline_says_how_to_add_one() {
    let r = Repo::with_stack("| build | `make` |\n");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["commands"], serde_json::json!([]));
    assert!(err.contains("no rules row"), "{err}");

    let (code, _, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("| rules |"), "{err}");
}

#[test]
fn dry_run_writes_nothing() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    let (code, v, _) = r.ns(&["baseline", "--allow-growth", "--dry-run"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["status"], "planned");
    assert!(r.baseline().is_none());
}

#[test]
fn human_output_is_one_line_per_finding() {
    let r = Repo::new();
    r.findings("lint-a", "./src.py:2:5: eval is banned\n");
    let out = common::ns()
        .args(["rules", "check", "--human"])
        .current_dir(&r.root)
        .output();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("new: src.py:2: lint-a: eval is banned"),
        "{stdout}"
    );
    assert!(
        stdout.contains("1 findings, 0 baselined, 1 new, 0 fixed"),
        "{stdout}"
    );
}

#[test]
fn outside_a_git_repo_is_a_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let out = common::ns()
        .args(["rules", "check"])
        .current_dir(tmp.path())
        .env(
            "GIT_CEILING_DIRECTORIES",
            tmp.path().parent().unwrap_or(Path::new("/")),
        )
        .output();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_clean_repo_gets_an_empty_baseline_without_allow_growth() {
    let r = Repo::new();
    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "written");
    assert_eq!(r.baseline().unwrap()["violations"], serde_json::json!([]));
}

#[test]
fn the_baseline_is_sorted_by_tool_then_line_whatever_order_the_tools_print() {
    let r = Repo::new();
    fs::write(r.path("src.py"), "import os\ny = eval(s)\nx = eval(s)\n").unwrap();
    r.findings("lint-b", "src.py:2:5: eval is banned\n");
    r.findings(
        "lint-a",
        "src.py:3:5: eval is banned\nsrc.py:2:5: eval is banned\n",
    );
    r.ns(&["baseline", "--allow-growth"]);
    let tools_and_snippets: Vec<(String, String)> = r.baseline().unwrap()["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["tool"].as_str().unwrap().into(),
                f["snippet"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        tools_and_snippets,
        [
            ("lint-a".into(), "y = eval(s)".into()),
            ("lint-a".into(), "x = eval(s)".into()),
            ("lint-b".into(), "y = eval(s)".into())
        ]
    );
}

#[test]
fn the_baseline_orders_by_tool_rule_file_then_line_then_message() {
    let r = Repo::new();
    fs::write(r.path("a.py"), "bb\naa\n").unwrap();
    fs::write(r.path("b.py"), "cc\n").unwrap();
    let f = |rule: &str, file: &str, line: u32, message: &str| {
        format!(
            "{{\"rule\": \"{rule}\", \"file\": \"{file}\", \"line\": {line}, \"message\": \"{message}\"}}\n"
        )
    };
    r.findings("lint-b", &f("R1", "b.py", 1, "m"));
    r.findings(
        "lint-a",
        &[
            f("R2", "a.py", 1, "m"),
            f("R1", "b.py", 1, "m"),
            f("R1", "a.py", 2, "a"),
            f("R1", "a.py", 1, "z"),
            f("R1", "a.py", 1, "a"),
        ]
        .concat(),
    );
    r.ns(&["baseline", "--allow-growth"]);
    let written: Vec<[String; 5]> = r.baseline().unwrap()["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            ["tool", "rule", "file", "snippet", "message"].map(|k| e[k].as_str().unwrap().into())
        })
        .collect();
    let row = |a: &str, b: &str, c: &str, d: &str, e: &str| [a, b, c, d, e].map(String::from);
    assert_eq!(
        written,
        [
            row("lint-a", "R1", "a.py", "bb", "a"),
            row("lint-a", "R1", "a.py", "bb", "z"),
            row("lint-a", "R1", "a.py", "aa", "a"),
            row("lint-a", "R1", "b.py", "cc", "m"),
            row("lint-a", "R2", "a.py", "bb", "m"),
            row("lint-b", "R1", "b.py", "cc", "m"),
        ]
    );
}

#[test]
fn dropping_one_violation_removes_only_its_entry_from_the_file() {
    let r = Repo::new();
    fs::write(r.path("src.py"), "zz\naa\nmm\n").unwrap();
    r.findings(
        "lint-a",
        "src.py:3:1: banned\nsrc.py:1:1: banned\nsrc.py:2:1: banned\n",
    );
    r.ns(&["baseline", "--allow-growth"]);
    let before = fs::read_to_string(r.path(BASELINE)).unwrap();
    for e in r.baseline().unwrap()["violations"].as_array().unwrap() {
        let keys: Vec<&String> = e.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            ["tool", "rule", "file", "message", "snippet"],
            "{before}"
        );
    }
    assert_eq!(
        baselined(&r),
        [
            ("src.py".into(), "zz".into()),
            ("src.py".into(), "aa".into()),
            ("src.py".into(), "mm".into())
        ]
    );

    r.findings("lint-a", "src.py:1:1: banned\nsrc.py:3:1: banned\n");
    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "written");
    let entry = "    {\n      \"tool\": \"lint-a\",\n      \"rule\": \"\",\n      \"file\": \"src.py\",\n      \"message\": \"banned\",\n      \"snippet\": \"aa\"\n    },\n";
    assert!(before.contains(entry), "{before}");
    assert_eq!(
        fs::read_to_string(r.path(BASELINE)).unwrap(),
        before.replacen(entry, "", 1)
    );
}

#[test]
fn a_message_only_change_passes_check_and_baseline_refreshes_it() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    r.ns(&["baseline", "--allow-growth"]);
    let before = fs::read_to_string(r.path(BASELINE)).unwrap();
    r.findings("lint-a", "src.py:2:5: eval is forbidden\n");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["baselined"], 1);

    let (code, v, err) = r.ns(&["baseline", "--dry-run"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "planned");
    assert_eq!(v["refreshed"], 1, "{v}");
    assert_eq!(fs::read_to_string(r.path(BASELINE)).unwrap(), before);

    let out = common::ns()
        .args(["rules", "baseline", "--human"])
        .current_dir(&r.root)
        .output();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "written: 1 violations, 0 added, 0 dropped, 1 refreshed\n"
    );
    assert_eq!(
        r.baseline().unwrap()["violations"][0]["message"],
        "eval is forbidden"
    );

    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "already-done");
    assert_eq!(v["refreshed"], 0, "{v}");
    assert_eq!(v["added"], serde_json::json!([]));
    assert_eq!(v["dropped"], serde_json::json!([]));
}

#[test]
fn duplicate_findings_with_new_messages_refresh_to_one_stable_file() {
    let r = Repo::new();
    r.findings(
        "lint-a",
        "src.py:2:5: eval is banned\nsrc.py:2:1: assignment from eval\n",
    );
    r.ns(&["baseline", "--allow-growth"]);
    r.findings(
        "lint-a",
        "src.py:2:1: z is last\nsrc.py:2:5: eval is banned\n",
    );
    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "written");
    assert_eq!(v["refreshed"], 1, "{v}");
    let first = fs::read_to_string(r.path(BASELINE)).unwrap();
    assert!(
        first.contains("z is last") && !first.contains("assignment"),
        "{first}"
    );

    r.findings(
        "lint-a",
        "src.py:2:5: eval is banned\nsrc.py:2:1: z is last\n",
    );
    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["status"], "already-done");
    assert_eq!(fs::read_to_string(r.path(BASELINE)).unwrap(), first);
}

#[test]
fn a_baseline_with_snippet_before_message_still_loads() {
    let r = Repo::new();
    fs::create_dir_all(r.path(".nightshift")).unwrap();
    fs::write(
        r.path(BASELINE),
        r#"{"version": 1, "violations": [{"tool": "lint-a", "rule": "", "file": "src.py", "snippet": "x = eval(s)", "message": "eval is banned"}]}"#,
    )
    .unwrap();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["baselined"], 1);
    assert_eq!(v["new"], serde_json::json!([]));
}

#[test]
fn findings_printed_only_on_stderr_are_read() {
    let r = Repo::with_stack("| rules | `echo 'src.py:2:5: eval is banned' >&2; exit 1` |\n");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(v["findings"], 1);
    assert_eq!(files_and_lines(&v["new"]), [("src.py".into(), 2)]);
}

#[test]
fn changed_text_at_the_same_line_is_one_new_and_one_fixed() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    r.ns(&["baseline", "--allow-growth"]);
    fs::write(r.path("src.py"), "import os\nx = exec(s)\ny = eval(s)\n").unwrap();
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(v["baselined"], 0);
    assert_eq!(v["new"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["new"][0]["snippet"], "x = exec(s)");
    assert_eq!(v["fixed"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["fixed"][0]["snippet"], "x = eval(s)");
}

#[test]
fn a_finding_on_line_one_takes_its_snippet_from_the_source() {
    let r = Repo::new();
    fs::write(r.path("src.py"), "import   os\n").unwrap();
    r.findings("lint-a", "src.py:1:1: unused import\n");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(v["new"][0]["snippet"], "import os");
}

#[test]
fn a_tool_that_cannot_run_is_an_error_even_after_printing_a_finding() {
    let r = Repo::with_stack("| rules | `echo 'src.py:2:5: eval is banned'; exit 127` |\n");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(v, Value::Null);
    assert!(err.contains("exited 127"), "{err}");

    let r = Repo::with_stack("| rules | `echo 'src.py:2:5: eval is banned'; kill -9 $$` |\n");
    let (code, _, err) = r.ns(&["baseline", "--allow-growth"]);
    assert_eq!(code, Some(1));
    assert!(err.contains("was killed by a signal"), "{err}");
    assert!(r.baseline().is_none());
}

#[test]
fn a_tool_that_outlives_the_timeout_is_an_error_naming_it() {
    let r = Repo::with_stack("| rules | `sleep 30` |\n");
    let out = common::ns()
        .args(["rules", "check"])
        .env("NS_RULES_TIMEOUT_MS", "300")
        .current_dir(&r.root)
        .output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("`sleep 30` timed out"), "{err}");
}

#[test]
fn a_malformed_baseline_is_an_error_that_names_the_fix() {
    let r = Repo::new();
    fs::create_dir_all(r.path(".nightshift")).unwrap();
    fs::write(r.path(BASELINE), "{not json").unwrap();
    for args in [&["check"][..], &["baseline"][..]] {
        let (code, _, err) = r.ns(args);
        assert_eq!(code, Some(1), "{args:?} {err}");
        assert!(err.contains("is not a rules baseline"), "{err}");
        assert!(
            err.contains("git checkout -- .nightshift/rules-baseline.json"),
            "{err}"
        );
        assert!(err.contains("delete it"), "{err}");
    }
    assert_eq!(fs::read_to_string(r.path(BASELINE)).unwrap(), "{not json");
    fs::remove_file(r.path(BASELINE)).unwrap();
    let (code, _, err) = r.ns(&["baseline", "--allow-growth"]);
    assert_eq!(code, Some(0), "{err}");
}

fn ns_with_env(r: &Repo, args: &[&str], env: (&str, &str)) -> (Option<i32>, String) {
    let out = common::ns()
        .arg("rules")
        .args(args)
        .env(env.0, env.1)
        .current_dir(&r.root)
        .output();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn output_over_the_cap_is_an_error_not_a_truncation() {
    let r = Repo::with_stack(
        "| rules | `i=0; while [ $i -lt 200 ]; do echo \"src.py:2:5: eval $i\"; i=$((i+1)); done` |\n",
    );
    let (code, err) = ns_with_env(&r, &["check"], ("NS_RULES_MAX_OUTPUT_BYTES", "500"));
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("printed more than 500 bytes"), "{err}");
    assert!(err.contains("while [ $i -lt 200 ]"), "{err}");
    let (code, err) = ns_with_env(
        &r,
        &["baseline", "--allow-growth"],
        ("NS_RULES_MAX_OUTPUT_BYTES", "500"),
    );
    assert_eq!(code, Some(1), "{err}");
    assert!(r.baseline().is_none());

    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{err}");
    assert_eq!(v["findings"], 200);
}

#[test]
fn no_rules_row_succeeds_before_reading_the_baseline() {
    let r = Repo::with_stack("| build | `make` |\n");
    fs::create_dir_all(r.path(".nightshift")).unwrap();
    fs::write(r.path(BASELINE), "{not json").unwrap();
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["new"], serde_json::json!([]));
    assert_eq!(v["fixed"], serde_json::json!([]));
}

#[test]
fn dry_run_status_follows_the_same_precedence_as_a_real_run() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    let (code, v, _) = r.ns(&["baseline", "--dry-run"]);
    assert_eq!(code, Some(1), "{v}");
    assert_eq!(v["status"], "refused");
    assert!(r.baseline().is_none());

    r.ns(&["baseline", "--allow-growth"]);
    let before = fs::read_to_string(r.path(BASELINE)).unwrap();
    r.findings("lint-a", "");
    let (code, v, _) = r.ns(&["baseline", "--dry-run"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["status"], "planned");
    assert_eq!(fs::read_to_string(r.path(BASELINE)).unwrap(), before);

    r.ns(&["baseline"]);
    let (code, v, _) = r.ns(&["baseline", "--dry-run"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["status"], "already-done");
}

#[test]
fn the_error_shows_only_the_last_twenty_lines() {
    let r = Repo::with_stack("| rules | `seq 1 30; exit 3` |\n");
    let (code, _, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("\n11\n") && err.contains("\n30"), "{err}");
    assert!(!err.contains("\n10\n"), "{err}");
}

#[test]
fn a_non_numeric_timeout_falls_back_to_the_default() {
    let r = Repo::new();
    let (code, err) = ns_with_env(&r, &["check"], ("NS_RULES_TIMEOUT_MS", "soon"));
    assert_eq!(code, Some(0), "{err}");
}

/// A `clippy::too_many_lines` finding at `src.py:1`, as the Rust pack prints it.
fn too_many_lines(lines: u64) -> String {
    let message = format!("this function has too many lines ({lines}/100) (quality bar)");
    format!(
        "{{\"tool\":\"clippy\",\"rule\":\"clippy::too_many_lines\",\"file\":\"src.py\",\"line\":1,\"message\":\"{message}\",\"snippet\":\"fn execute() {{\"}}\n"
    )
}

#[test]
fn measured_values_ratchet_against_the_baseline() {
    let r = Repo::new();
    r.findings("lint-a", &too_many_lines(221));
    r.ns(&["baseline", "--allow-growth"]);
    for lines in [221, 100] {
        r.findings("lint-a", &too_many_lines(lines));
        let (code, v, err) = r.ns(&["check"]);
        assert_eq!(code, Some(0), "{lines}: {v} {err}");
        assert_eq!(v["baselined"], 1, "{lines}: {v}");
        assert_eq!(v["new"], serde_json::json!([]), "{lines}: {v}");
    }

    r.findings("lint-a", &too_many_lines(261));
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(v["baselined"], 0, "{v}");
    assert_eq!(v["new"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["fixed"], serde_json::json!([]), "{v}");
    let out = common::ns()
        .args(["rules", "check", "--human"])
        .current_dir(&r.root)
        .output();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("grew from 221 to 261"), "{stdout}");
}

#[test]
fn prune_lowers_a_shrunk_measured_value_and_reports_it() {
    let r = Repo::new();
    r.findings("lint-a", &too_many_lines(221));
    r.ns(&["baseline", "--allow-growth"]);
    r.findings("lint-a", &too_many_lines(100));
    let (_, v, err) = r.ns(&["check", "--prune", "--only", "lint-a"]);
    assert_eq!(v["lowered"], 0, "{v} {err}");
    let (code, v, err) = r.ns(&["check", "--prune"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["pruned"], 0, "{v}");
    assert_eq!(v["lowered"], 1, "{v}");
    assert!(err.contains("lowered 1 values"), "{err}");
    let after = fs::read_to_string(r.path(BASELINE)).unwrap();
    assert!(
        after.contains("(100/100)") && !after.contains("(221/100)"),
        "{after}"
    );

    let (code, v, _) = r.ns(&["check", "--prune"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["lowered"], 0, "{v}");
}

#[test]
fn baseline_refuses_a_grown_measured_value_until_allow_growth() {
    let r = Repo::new();
    r.findings("lint-a", &too_many_lines(221));
    r.ns(&["baseline", "--allow-growth"]);
    let before = fs::read_to_string(r.path(BASELINE)).unwrap();
    r.findings("lint-a", &too_many_lines(261));
    let (code, v, err) = r.ns(&["baseline"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(v["status"], "refused", "{v}");
    assert_eq!(files_and_lines(&v["added"]), [("src.py".into(), 1)], "{v}");
    assert_eq!(fs::read_to_string(r.path(BASELINE)).unwrap(), before);

    let (code, v, err) = r.ns(&["baseline", "--allow-growth"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert!(r.baseline().unwrap()["violations"][0]["message"]
        .as_str()
        .unwrap()
        .contains("(261/100)"));
}

fn gate(r: &Repo, base: &str) -> (Option<i32>, String) {
    let ns = common::ns_path();
    let cmd = format!("({base}) && '{}' rules check", ns.display());
    let out = common::command("sh")
        .args(["-c", &cmd])
        .current_dir(&r.root)
        .output();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_gate_shape_fails_on_a_new_violation_and_passes_once_baselined() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    let (code, err) = gate(&r, "true");
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("1 new violations"), "{err}");

    r.ns(&["baseline", "--allow-growth"]);
    let (code, err) = gate(&r, "true");
    assert_eq!(code, Some(0), "{err}");
}

#[test]
fn a_failing_base_command_short_circuits_the_rules_check() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    let (code, err) = gate(&r, "echo base failed >&2; exit 7");
    assert_eq!(code, Some(7), "{err}");
    assert!(err.contains("base failed"), "{err}");
    assert!(!err.contains("ns rules:"), "{err}");
}

#[test]
fn only_runs_the_commands_containing_its_text_and_reports_no_fixed() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    r.findings("lint-b", "src.py:3:5: eval is banned\n");
    r.ns(&["baseline", "--allow-growth"]);
    r.findings("lint-a", "");
    r.findings(
        "lint-b",
        "src.py:2:5: eval is banned\nsrc.py:3:5: eval is banned\n",
    );

    let (code, v, err) = r.ns(&["check", "--only", "lint-a"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(
        v["commands"],
        serde_json::json!([{"command": "./lint-a", "tool": "lint-a", "exit": 0, "findings": 0}])
    );
    assert_eq!(v["fixed"], serde_json::json!([]));
    assert!(!err.contains("are gone"), "{err}");

    let (code, v, err) = r.ns(&["check", "--only", "lint-b"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(files_and_lines(&v["new"]), [("src.py".into(), 2)]);
    assert_eq!(v["baselined"], 1);
}

#[test]
fn only_matching_no_command_is_a_usage_error_naming_the_commands() {
    let r = Repo::new();
    let (code, v, err) = r.ns(&["check", "--only", "ast-grep"]);
    assert_eq!(code, Some(2), "{v} {err}");
    assert!(
        err.contains("no command in the rules row contains `ast-grep`"),
        "{err}"
    );
    assert!(err.contains("./lint-a"), "{err}");
}

#[test]
fn only_with_no_rules_row_checks_nothing() {
    let r = Repo::with_stack("| build | `make` |\n");
    let (code, v, err) = r.ns(&["check", "--only", "ast-grep"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["commands"], serde_json::json!([]));
}

#[test]
fn prune_drops_only_the_fixed_entries_and_keeps_the_rest_verbatim() {
    let r = Repo::new();
    r.findings(
        "lint-a",
        "src.py:2:5: eval is banned\nsrc.py:3:5: eval is banned\n",
    );
    r.ns(&["baseline", "--allow-growth"]);
    // The kept finding's message drifted: it still matches, and its entry stays as it was.
    r.findings("lint-a", "src.py:3:5: eval is still banned\n");

    let (code, v, err) = r.ns(&["check", "--prune"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["pruned"], 1, "{v}");
    assert_eq!(v["fixed"][0]["snippet"], "x = eval(s)", "{v}");
    assert!(
        err.contains("dropped 1 fixed entries from .nightshift/rules-baseline.json"),
        "{err}"
    );
    assert_eq!(baselined(&r), [("src.py".into(), "y = eval(s)".into())]);
    assert_eq!(
        r.baseline().unwrap()["violations"][0]["message"],
        "eval is banned"
    );

    let (code, v, err) = r.ns(&["check", "--prune"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["pruned"], 0, "{v}");
    assert_eq!(v["fixed"], serde_json::json!([]));
}

#[test]
fn prune_writes_nothing_when_a_violation_is_new_or_with_only() {
    let r = Repo::new();
    r.findings("lint-a", "src.py:2:5: eval is banned\n");
    r.findings("lint-b", "src.py:3:5: eval is banned\n");
    r.ns(&["baseline", "--allow-growth"]);
    let before = fs::read(r.path(BASELINE)).unwrap();

    r.findings("lint-a", "src.py:1:1: os is unused\n");
    let (code, v, err) = r.ns(&["check", "--prune"]);
    assert_eq!(code, Some(1), "{v} {err}");
    assert_eq!(v["pruned"], 0, "{v}");
    assert_eq!(fs::read(r.path(BASELINE)).unwrap(), before);

    r.findings("lint-a", "");
    let (code, v, err) = r.ns(&["check", "--prune", "--only", "lint-a"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["pruned"], 0, "{v}");
    assert_eq!(fs::read(r.path(BASELINE)).unwrap(), before);
}

#[test]
fn prune_with_no_baseline_writes_none() {
    let r = Repo::new();
    let (code, v, err) = r.ns(&["check", "--prune"]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["pruned"], 0, "{v}");
    assert!(r.baseline().is_none());
}
