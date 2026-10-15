//! `ns ratchet check` and `ns ratchet update` against commands that print canned numbers.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command as StdCommand;

use serde_json::{json, Value};
use tempfile::TempDir;

/// Prints 0, then 1, then 2, ...: a metric that changes on every run.
const COUNTER: &str = "n=$(cat count 2>/dev/null || echo 0); echo $((n+1)) > count; echo $n";

struct Repo {
    _tmp: TempDir,
    root: PathBuf,
}

impl Repo {
    /// A git repo whose `.nightshift/nightshift.toml` holds `ratchets`.
    fn new(ratchets: &str) -> Repo {
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
        fs::create_dir_all(root.join(".nightshift")).unwrap();
        fs::write(root.join(".nightshift/nightshift.toml"), ratchets).unwrap();
        Repo { _tmp: tmp, root }
    }

    /// A ratchet `name` whose command prints the contents of `<name>.txt`.
    fn cat(name: &str, better: &str, tolerance: &str) -> String {
        format!(
            "[ratchets.{name}]\ncommand = \"cat {name}.txt\"\nbetter = \"{better}\"\n{tolerance}\n"
        )
    }

    fn measure(&self, name: &str, value: &str) {
        fs::write(self.root.join(format!("{name}.txt")), value).unwrap();
    }

    fn record(&self, values: &Value) {
        fs::write(
            self.root.join(".nightshift/ratchets.json"),
            json!({"version": 1, "values": values}).to_string(),
        )
        .unwrap();
    }

    fn recorded(&self) -> Option<Value> {
        let t = fs::read_to_string(self.root.join(".nightshift/ratchets.json")).ok()?;
        Some(serde_json::from_str::<Value>(&t).unwrap()["values"].clone())
    }

    fn cmd(&self) -> common::Ns {
        let mut c = common::ns();
        c.arg("ratchet")
            .current_dir(&self.root)
            .env_remove("NS_RATCHET_FACTORY")
            .env_remove("NS_RATCHET_STEADY")
            .env_remove("NS_RATCHET_REPORT");
        c
    }

    fn ns(&self, args: &[&str]) -> (Option<i32>, Value, String) {
        let out = self.cmd().args(args).output();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let v = serde_json::from_str(&stdout).unwrap_or(Value::Null);
        (out.status.code(), v, stderr)
    }

    /// The single ratchet's status after `ns ratchet check`, and the exit code.
    fn check_one(&self) -> (Option<i32>, Value) {
        let (code, v, _) = self.ns(&["check"]);
        (code, v["ratchets"][0].clone())
    }

    fn count(&self) -> String {
        fs::read_to_string(self.root.join("count")).unwrap_or_default()
    }
}

#[test]
fn lower_is_better_fails_only_past_the_recorded_value() {
    let r = Repo::new(&Repo::cat("flash", "lower", ""));
    r.record(&json!({"flash": 100}));
    r.measure("flash", "101\n");
    let (code, o) = r.check_one();
    assert_eq!(code, Some(1));
    assert_eq!(o["status"], "worse");
    assert_eq!(
        (
            o["recorded"].as_f64(),
            o["measured"].as_f64(),
            o["limit"].as_f64()
        ),
        (Some(100.0), Some(101.0), Some(100.0))
    );
    r.measure("flash", "100");
    let (code, o) = r.check_one();
    assert_eq!((code, o["status"].as_str()), (Some(0), Some("ok")));
    r.measure("flash", "99.5");
    let (code, o) = r.check_one();
    assert_eq!((code, o["status"].as_str()), (Some(0), Some("better")));
}

#[test]
fn higher_is_better_fails_only_below_the_recorded_value() {
    let r = Repo::new(&Repo::cat("coverage", "higher", ""));
    r.record(&json!({"coverage": 80}));
    r.measure("coverage", "79.9");
    let (code, o) = r.check_one();
    assert_eq!((code, o["status"].as_str()), (Some(1), Some("worse")));
    r.measure("coverage", "80");
    assert_eq!(r.check_one().1["status"], "ok");
    r.measure("coverage", "81");
    assert_eq!(r.check_one().1["status"], "better");
}

#[test]
fn a_percent_tolerance_is_a_share_of_the_recorded_value() {
    let r = Repo::new(&Repo::cat("size", "lower", "tolerance = \"1%\""));
    r.record(&json!({"size": 1000}));
    r.measure("size", "1010");
    let (code, o) = r.check_one();
    assert_eq!((code, o["status"].as_str()), (Some(0), Some("ok")));
    assert_eq!(o["limit"].as_f64(), Some(1010.0));
    assert_eq!(o["tolerance"], "1%");
    r.measure("size", "1010.5");
    assert_eq!(r.check_one().1["status"], "worse");

    let r = Repo::new(&Repo::cat("cov", "higher", "tolerance = \"10%\""));
    r.record(&json!({"cov": 50}));
    r.measure("cov", "45");
    assert_eq!(r.check_one().1["status"], "ok");
    r.measure("cov", "44.9");
    assert_eq!(r.check_one().1["status"], "worse");
}

#[test]
fn an_absolute_tolerance_is_in_the_ratchets_own_unit() {
    let r = Repo::new(&Repo::cat("warnings", "lower", "tolerance = 2"));
    r.record(&json!({"warnings": 10}));
    r.measure("warnings", "12");
    let (code, o) = r.check_one();
    assert_eq!((code, o["status"].as_str()), (Some(0), Some("ok")));
    assert_eq!(o["tolerance"], "2");
    r.measure("warnings", "13");
    assert_eq!(r.check_one().1["status"], "worse");

    let r = Repo::new(&Repo::cat("cov", "higher", "tolerance = \"0.5\""));
    r.record(&json!({"cov": 50}));
    r.measure("cov", "49.5");
    assert_eq!(r.check_one().1["status"], "ok");
    r.measure("cov", "49.4");
    assert_eq!(r.check_one().1["status"], "worse");
}

#[test]
fn a_ratchet_with_no_recorded_value_is_untracked_and_nothing_is_recorded() {
    let r = Repo::new(&Repo::cat("flash", "lower", ""));
    r.measure("flash", "100");
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(0), "{err}");
    let o = &v["ratchets"][0];
    assert_eq!(o["status"], "untracked");
    assert_eq!(o["measured"].as_f64(), Some(100.0));
    assert!(o["recorded"].is_null() && o["limit"].is_null());
    assert!(err.contains("ns ratchet update"), "{err}");
    assert_eq!(r.recorded(), None);

    r.record(&json!({"other": 1}));
    assert_eq!(r.check_one().1["status"], "untracked");
    assert_eq!(r.recorded(), Some(json!({"other": 1})));
}

#[test]
fn a_command_that_fails_or_prints_a_non_number_is_broken() {
    let r = Repo::new(
        "[ratchets.a]\ncommand = \"echo boom >&2; exit 3\"\nbetter = \"lower\"\n\
         [ratchets.b]\ncommand = \"echo twelve\"\nbetter = \"lower\"\n\
         [ratchets.c]\ncommand = \"printf '1\\\\n2\\\\n'\"\nbetter = \"lower\"\n\
         [ratchets.d]\ncommand = \"echo inf\"\nbetter = \"lower\"\n\
         [ratchets.f]\ncommand = \"kill -9 $$\"\nbetter = \"lower\"\n\
         [ratchets.g]\ncommand = \"echo 7\"\nbetter = \"lower\"\n",
    );
    r.record(&json!({"g": 7}));
    let (code, v, _) = r.ns(&["check"]);
    assert_eq!(code, Some(1));
    let by: Vec<(String, String, String)> = v["ratchets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| {
            (
                o["name"].as_str().unwrap().to_string(),
                o["status"].as_str().unwrap().to_string(),
                o["error"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(by[0].1, "broken");
    assert_eq!(by[0].2, "exited 3; stderr: boom");
    assert_eq!(by[1].2, "printed \"twelve\", not one number");
    assert_eq!(by[2].2, "printed \"1\\n2\", not one number");
    assert_eq!(by[3].2, "printed \"inf\", not one number");
    assert_eq!(by[4].2, "was killed by a signal");
    assert_eq!((by[5].1.as_str(), by[5].2.as_str()), ("ok", ""));
    assert!(v["ratchets"][0]["measured"].is_null());
}

#[test]
fn a_command_that_outlives_the_timeout_is_broken() {
    let r = Repo::new("[ratchets.e]\ncommand = \"sleep 30\"\nbetter = \"lower\"\n");
    let out = r
        .cmd()
        .env("NS_RATCHET_TIMEOUT_MS", "300")
        .args(["check"])
        .output();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ratchets"][0]["status"], "broken");
    assert_eq!(v["ratchets"][0]["error"], "timed out after 0.3 s");
}

#[test]
fn a_timeout_that_is_not_a_positive_number_falls_back_to_the_default() {
    let r = Repo::new(&Repo::cat("a", "lower", ""));
    r.measure("a", "1");
    for bad in ["soon", "0", "-5", ""] {
        let out = r
            .cmd()
            .env("NS_RATCHET_TIMEOUT_MS", bad)
            .args(["check"])
            .output();
        assert_eq!(out.status.code(), Some(0), "{bad:?}");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ratchets"][0]["status"], "untracked", "{bad:?}");
    }
}

#[test]
fn a_command_that_prints_too_much_is_broken() {
    let r = Repo::new(
        "[ratchets.big]\ncommand = \"head -c 70000 /dev/zero | tr '\\\\0' 1\"\nbetter = \"lower\"\n",
    );
    let (code, o) = r.check_one();
    assert_eq!(code, Some(1));
    assert_eq!(o["error"], "printed more than 65536 bytes, not one number");
}

#[test]
fn the_stdout_cap_accepts_exactly_the_limit_and_breaks_one_byte_past_it() {
    let printing = |digits: u32| {
        Repo::new(&format!(
            "[ratchets.big]\ncommand = \"head -c {digits} /dev/zero | tr '\\\\0' 0; echo\"\nbetter = \"lower\"\n"
        ))
    };
    let (code, o) = printing(65535).check_one();
    assert_eq!((code, o["status"].as_str()), (Some(0), Some("untracked")));
    assert_eq!(o["measured"], 0.0);

    let (code, o) = printing(65536).check_one();
    assert_eq!(code, Some(1));
    assert_eq!(o["error"], "printed more than 65536 bytes, not one number");
}

#[test]
fn two_runs_further_apart_than_the_tolerance_make_the_ratchet_flaky() {
    let r = Repo::new(&format!(
        "[ratchets.jitter]\ncommand = \"{COUNTER}\"\nbetter = \"lower\"\n"
    ));
    r.record(&json!({"jitter": 100}));
    let (code, v, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1));
    let o = &v["ratchets"][0];
    assert_eq!(o["status"], "flaky");
    assert_eq!(o["runs"], json!([0.0, 1.0]));
    assert!(err.contains("ns ratchet check"), "{err}");

    let r = Repo::new(&format!(
        "[ratchets.jitter]\ncommand = \"{COUNTER}\"\nbetter = \"lower\"\ntolerance = 1\n"
    ));
    r.record(&json!({"jitter": 100}));
    let (code, o) = r.check_one();
    assert_eq!((code, o["status"].as_str()), (Some(0), Some("better")));
    assert_eq!(o["measured"].as_f64(), Some(1.0));

    // A percentage is of the recorded value, else of the first run.
    let r = Repo::new(&format!(
        "[ratchets.jitter]\ncommand = \"{COUNTER}\"\nbetter = \"lower\"\ntolerance = \"1%\"\n"
    ));
    r.record(&json!({"jitter": 100}));
    assert_eq!(r.check_one().1["status"], "better");
    fs::remove_file(r.root.join(".nightshift/ratchets.json")).unwrap();
    fs::write(r.root.join("count"), "50").unwrap();
    assert_eq!(r.check_one().1["status"], "flaky");
}

/// A ratchet that prints the first, then the second of `runs`.
fn straddle(better: &str, tolerance: &str, runs: [&str; 2], recorded: f64) -> Repo {
    let cmd = "n=$(cat count 2>/dev/null || echo 0); echo $((n+1)) > count; sed -n \\\"$((n+1))p\\\" runs.txt";
    let r = Repo::new(&format!(
        "[ratchets.m]\ncommand = \"{cmd}\"\nbetter = \"{better}\"\n{tolerance}\n"
    ));
    fs::write(
        r.root.join("runs.txt"),
        format!("{}\n{}\n", runs[0], runs[1]),
    )
    .unwrap();
    r.record(&json!({"m": recorded}));
    r
}

#[test]
fn the_worst_of_two_runs_is_judged_whichever_order_they_come_in() {
    for (better, tol, a, b) in [
        ("lower", "tolerance = 5", "104", "109"),
        ("higher", "tolerance = 5", "96", "91"),
        ("lower", "tolerance = \"5%\"", "104", "109"),
    ] {
        for (first, second) in [(a, b), (b, a)] {
            let r = straddle(better, tol, [first, second], 100.0);
            let (code, o) = r.check_one();
            assert_eq!(
                (code, o["status"].as_str()),
                (Some(1), Some("worse")),
                "{better} {first} {second}: {o}"
            );
            assert_eq!(o["measured"].as_f64(), b.parse().ok(), "{o}");
            assert_eq!(o["runs"].as_array().unwrap().len(), 2);
        }
    }
}

#[test]
fn two_runs_inside_the_limit_are_ok_in_either_order() {
    for (first, second) in [("101", "104"), ("104", "101")] {
        let r = straddle("lower", "tolerance = 5", [first, second], 100.0);
        let (code, o) = r.check_one();
        assert_eq!((code, o["status"].as_str()), (Some(0), Some("ok")));
        assert_eq!(o["measured"].as_f64(), Some(104.0));
    }
}

#[test]
fn update_records_the_worst_of_two_runs_when_it_tightens() {
    let r = straddle("lower", "tolerance = 5", ["90", "93"], 100.0);
    let (code, v, _) = r.ns(&["update"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["ratchets"][0]["status"], "tightened");
    assert_eq!(r.recorded(), Some(json!({"m": 93.0})));

    let r = straddle("higher", "tolerance = 5", ["93", "90"], 80.0);
    r.ns(&["update"]);
    assert_eq!(r.recorded(), Some(json!({"m": 90.0})));
}

#[test]
fn update_keeps_the_recorded_value_when_the_worst_run_is_worse_even_if_the_first_is_not() {
    let r = straddle("lower", "tolerance = 5", ["99", "103"], 100.0);
    let (_, v, _) = r.ns(&["update"]);
    assert_eq!(v["ratchets"][0]["status"], "kept");
    assert_eq!(r.recorded(), Some(json!({"m": 100.0})));
}

#[test]
fn under_a_night_only_a_ratchets_first_check_runs_it_twice() {
    let r = Repo::new(&format!(
        "[ratchets.jitter]\ncommand = \"{COUNTER}\"\nbetter = \"lower\"\ntolerance = 5\n"
    ));
    let steady = r.root.join("night/ratchets-steady.json");
    let check = || {
        r.cmd()
            .env("NS_RATCHET_STEADY", &steady)
            .args(["check"])
            .output()
    };
    assert_eq!(check().status.code(), Some(0));
    assert_eq!(r.count(), "2\n");
    let names: Value = serde_json::from_str(&fs::read_to_string(&steady).unwrap()).unwrap();
    assert_eq!(names, json!(["jitter"]));
    let out = check();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ratchets"][0]["runs"], json!([2.0]));
    assert_eq!(r.count(), "3\n");
    // Without the night's file, every check runs twice.
    r.ns(&["check"]);
    assert_eq!(r.count(), "5\n");
}

#[test]
fn a_flaky_or_broken_ratchet_is_not_marked_steady() {
    let r = Repo::new(&format!(
        "[ratchets.jitter]\ncommand = \"{COUNTER}\"\nbetter = \"lower\"\n\
         [ratchets.gone]\ncommand = \"exit 1\"\nbetter = \"lower\"\n"
    ));
    let steady = r.root.join("steady.json");
    let out = r
        .cmd()
        .env("NS_RATCHET_STEADY", &steady)
        .args(["check"])
        .output();
    assert_eq!(out.status.code(), Some(1));
    assert!(!steady.exists());
}

#[test]
fn the_report_variable_gets_a_copy_of_the_json() {
    let r = Repo::new(&Repo::cat("flash", "lower", ""));
    r.record(&json!({"flash": 100}));
    r.measure("flash", "120");
    let report = r.root.join("out/report.json");
    let out = r
        .cmd()
        .env("NS_RATCHET_REPORT", &report)
        .args(["check", "--human"])
        .output();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["ratchets"][0]["status"], "worse");
}

#[test]
fn a_ratchets_command_runs_without_the_ratchet_variables() {
    let r = Repo::new(
        "[ratchets.a]\ncommand = \"echo ${NS_RATCHET_FACTORY:-}${NS_RATCHET_STEADY:-}${NS_RATCHET_REPORT:-}1\"\nbetter = \"lower\"\n",
    );
    r.record(&json!({"a": 1}));
    let out = r
        .cmd()
        .env("NS_RATCHET_FACTORY", r.root.join(".nightshift"))
        .env("NS_RATCHET_STEADY", r.root.join("steady"))
        .env("NS_RATCHET_REPORT", r.root.join("report.json"))
        .arg("check")
        .output();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ratchets"][0]["measured"], 1.0);
    assert_eq!(v["ratchets"][0]["runs"], json!([1.0, 1.0]));
}

#[test]
fn check_and_update_run_from_a_subdirectory_at_the_top_of_the_checkout() {
    let r = Repo::new(&Repo::cat("flash", "lower", ""));
    r.measure("flash", "7");
    let sub = r.root.join("sub");
    fs::create_dir(&sub).unwrap();
    let out = r.cmd().current_dir(&sub).arg("check").output();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ratchets"][0]["measured"], 7.0);
    let out = r.cmd().current_dir(&sub).arg("update").output();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(r.recorded(), Some(json!({"flash": 7.0})));
    assert!(!sub.join(".nightshift").exists());
}

#[test]
fn human_lines_name_the_ratchet_and_both_values() {
    let r = Repo::new(&format!(
        "{}{}[ratchets.c]\ncommand = \"exit 4\"\nbetter = \"lower\"\n[ratchets.d]\ncommand = \"{COUNTER}\"\nbetter = \"higher\"\n",
        Repo::cat("a", "lower", "tolerance = \"0.5%\""),
        Repo::cat("b", "higher", ""),
    ));
    r.record(&json!({"a": 1000}));
    r.measure("a", "1010");
    r.measure("b", "3");
    let out = r.cmd().args(["check", "--human"]).output();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "ratchet a is worse: measured 1010, recorded 1000, tolerance 0.5%, limit 1005, lower is better\n\
         ratchet b is untracked: measured 3, nothing recorded\n\
         ratchet c is broken: `exit 4` exited 4\n\
         ratchet d is flaky: two runs of `n=$(cat count 2>/dev/null || echo 0); echo $((n+1)) > count; echo $n` measured 0 and 1, more apart than its tolerance 0\n\
         4 ratchets: 1 worse, 1 untracked, 1 broken, 1 flaky\n"
    );
    r.measure("a", "900");
    r.record(&json!({"a": 1000, "b": 1}));
    let out = r.cmd().args(["check", "--human"]).output();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with(
        "ratchet a is better: measured 900, recorded 1000, tolerance 0.5%, limit 1005, lower is better\n\
         ratchet b is better: measured 3, recorded 1, tolerance 0, limit 1, higher is better\n"
    ), "{text}");
}

#[test]
fn no_ratchets_checks_nothing_and_passes() {
    let r = Repo::new("");
    let (code, v, _) = r.ns(&["check", "--json"]);
    assert_eq!(code, Some(0));
    assert_eq!(v["ok"], true);
    assert_eq!(v["ratchets"], json!([]));
    let out = r.cmd().args(["check", "--human"]).output();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0 ratchets\n");
}

#[test]
fn a_bad_definition_or_recorded_file_is_an_error_naming_the_fix() {
    for tol in ["\"-1%\"", "-1", "\"x%\"", "\"lots\"", "nan"] {
        let r = Repo::new(&Repo::cat("a", "lower", &format!("tolerance = {tol}")));
        let (code, _, err) = r.ns(&["check"]);
        assert_eq!(code, Some(2), "{tol}: {err}");
        assert!(err.contains("tolerance"), "{tol}: {err}");
    }
    let r = Repo::new("[ratchets.a]\ncommand = \"true\"\nbetter = \"smaller\"\n");
    assert_eq!(r.ns(&["check"]).0, Some(2));

    let r = Repo::new(&Repo::cat("a", "lower", ""));
    fs::write(r.root.join(".nightshift/ratchets.json"), "{}").unwrap();
    let (code, _, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1));
    assert!(err.contains("is not a ratchets file"), "{err}");
    assert!(err.contains("git checkout --"), "{err}");
    fs::remove_file(r.root.join(".nightshift/ratchets.json")).unwrap();
    fs::create_dir(r.root.join(".nightshift/ratchets.json")).unwrap();
    let (code, _, err) = r.ns(&["check"]);
    assert_eq!(code, Some(1));
    assert!(err.contains("cannot read"), "{err}");
}

#[test]
fn outside_a_git_repo_is_a_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let out = common::ns()
        .args(["ratchet", "check"])
        .current_dir(tmp.path())
        .env("GIT_CEILING_DIRECTORIES", tmp.path())
        .output();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside a git repository"));
}

#[test]
fn the_factory_flag_beats_the_variable_which_beats_the_default() {
    let r = Repo::new("");
    let other = r.root.join("elsewhere");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("nightshift.toml"), Repo::cat("a", "lower", "")).unwrap();
    r.measure("a", "1");
    let names = |c: &mut common::Ns| -> usize {
        let out = c.output();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        v["ratchets"].as_array().unwrap().len()
    };
    assert_eq!(names(r.cmd().args(["check"])), 0);
    assert_eq!(
        names(r.cmd().env("NS_RATCHET_FACTORY", &other).args(["check"])),
        1
    );
    let empty = r.root.join("empty");
    fs::create_dir(&empty).unwrap();
    let flag = r
        .cmd()
        .env("NS_RATCHET_FACTORY", &other)
        .arg("check")
        .arg("--factory")
        .arg(&empty)
        .output();
    let v: Value = serde_json::from_slice(&flag.stdout).unwrap();
    assert_eq!(v["ratchets"], json!([]));
}

#[test]
fn update_records_tightens_and_keeps_worse_values() {
    let r = Repo::new(&format!(
        "{}{}{}{}",
        Repo::cat("new", "lower", ""),
        Repo::cat("tight", "lower", ""),
        Repo::cat("same", "higher", ""),
        Repo::cat("worse", "higher", "tolerance = 5"),
    ));
    r.record(&json!({"tight": 100, "same": 50, "worse": 80, "removed": 1}));
    for (n, v) in [
        ("new", "7"),
        ("tight", "90"),
        ("same", "50"),
        ("worse", "79"),
    ] {
        r.measure(n, v);
    }
    let (code, v, err) = r.ns(&["update", "--dry-run"]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(v["status"], "planned");
    assert_eq!(r.recorded().unwrap()["tight"], 100);

    let (code, v, err) = r.ns(&["update"]);
    assert_eq!(code, Some(0));
    assert_eq!(v["status"], "written");
    let st: Vec<&str> = v["ratchets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["status"].as_str().unwrap())
        .collect();
    assert_eq!(st, ["recorded", "unchanged", "tightened", "kept"]);
    assert_eq!(v["dropped"], json!(["removed"]));
    assert!(err.contains("--allow-loosen"), "{err}");
    assert_eq!(
        r.recorded().unwrap(),
        json!({"new": 7.0, "same": 50.0, "tight": 90.0, "worse": 80.0})
    );

    let (code, v, _) = r.ns(&["update"]);
    assert_eq!(
        (code, v["status"].as_str()),
        (Some(0), Some("already-done"))
    );

    let (code, v, _) = r.ns(&["update", "--allow-loosen"]);
    assert_eq!(code, Some(0));
    assert_eq!(v["ratchets"][3]["status"], "loosened");
    assert_eq!(r.recorded().unwrap()["worse"], 79.0);
}

#[test]
fn update_with_only_a_stale_entry_to_drop_still_writes() {
    let r = Repo::new(&Repo::cat("a", "lower", ""));
    r.measure("a", "3");
    r.record(&json!({"a": 3, "gone": 1}));

    let (code, v, err) = r.ns(&["update", "--dry-run"]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(v["status"], "planned");
    assert_eq!(v["dropped"], json!(["gone"]));
    assert_eq!(r.recorded().unwrap(), json!({"a": 3, "gone": 1}));

    let (code, v, err) = r.ns(&["update"]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(v["status"], "written");
    assert_eq!(v["dropped"], json!(["gone"]));
    assert_eq!(r.recorded().unwrap(), json!({"a": 3.0}));
}

#[test]
fn update_with_no_recorded_file_writes_one_even_when_values_match_nothing() {
    let r = Repo::new(&Repo::cat("a", "lower", ""));
    r.measure("a", "3");
    let (code, v, _) = r.ns(&["update"]);
    assert_eq!((code, v["status"].as_str()), (Some(0), Some("written")));
    assert_eq!(r.recorded(), Some(json!({"a": 3.0})));
}

#[test]
fn update_refuses_a_broken_or_flaky_ratchet_and_writes_nothing() {
    for cmd in ["exit 1", COUNTER] {
        let r = Repo::new(&format!(
            "{}[ratchets.bad]\ncommand = \"{cmd}\"\nbetter = \"lower\"\n",
            Repo::cat("a", "lower", "")
        ));
        r.measure("a", "3");
        let (code, v, err) = r.ns(&["update"]);
        assert_eq!(code, Some(1), "{cmd}");
        assert_eq!(v["status"], "refused");
        assert!(err.contains("nothing written"), "{err}");
        assert_eq!(r.recorded(), None);
    }
}

#[test]
fn update_with_no_ratchets_is_a_usage_error() {
    let r = Repo::new("");
    let (code, _, err) = r.ns(&["update"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("[ratchets.flash]"), "{err}");
}

#[test]
fn update_human_output_ends_with_the_status() {
    let r = Repo::new(&Repo::cat("a", "lower", ""));
    r.measure("a", "3");
    let out = r.cmd().args(["update", "--human", "--dry-run"]).output();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "ratchet a is recorded: measured 3, nothing recorded\nplanned: 1 ratchets: 1 recorded\n"
    );
}

#[test]
fn json_and_human_conflict() {
    let r = Repo::new(&Repo::cat("a", "lower", ""));
    r.measure("a", "1");
    for sub in ["check", "update"] {
        let out = r.cmd().args([sub, "--json", "--human"]).output();
        assert_eq!(out.status.code(), Some(2), "{sub}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"));
    }
    assert_eq!(r.recorded(), None);
}

#[test]
fn update_json_prints_the_json() {
    let r = Repo::new(&Repo::cat("a", "lower", ""));
    r.measure("a", "3");
    let (code, v, err) = r.ns(&["update", "--json"]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(v["command"], "ratchet update");
    assert_eq!(v["status"], "written");
    assert_eq!(v["ratchets"][0]["status"], "recorded");
}

#[test]
fn update_ignores_the_factory_variable_the_gate_sets() {
    let r = Repo::new(&Repo::cat("a", "lower", ""));
    r.measure("a", "3");
    let other = r.root.join("elsewhere");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("nightshift.toml"), Repo::cat("a", "lower", "")).unwrap();
    fs::write(
        other.join("ratchets.json"),
        json!({"version": 1, "values": {"a": 1}}).to_string(),
    )
    .unwrap();
    let before = fs::read_to_string(other.join("ratchets.json")).unwrap();
    let out = r
        .cmd()
        .env("NS_RATCHET_FACTORY", &other)
        .args(["update", "--allow-loosen"])
        .output();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(other.join("ratchets.json")).unwrap(),
        before
    );
    assert_eq!(r.recorded(), Some(json!({"a": 3.0})));
}

#[test]
fn a_corrupt_steady_file_counts_as_no_steady_ratchets() {
    let r = Repo::new(&format!(
        "[ratchets.jitter]\ncommand = \"{COUNTER}\"\nbetter = \"lower\"\ntolerance = 5\n"
    ));
    let steady = r.root.join("steady.json");
    fs::write(&steady, "not json").unwrap();
    let out = r
        .cmd()
        .env("NS_RATCHET_STEADY", &steady)
        .args(["check"])
        .output();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(r.count(), "2\n");
    let names: Value = serde_json::from_str(&fs::read_to_string(&steady).unwrap()).unwrap();
    assert_eq!(names, json!(["jitter"]));
}

#[test]
fn a_check_that_finds_nothing_new_leaves_the_steady_file_alone() {
    let r = Repo::new(&format!(
        "[ratchets.jitter]\ncommand = \"{COUNTER}\"\nbetter = \"lower\"\ntolerance = 5\n"
    ));
    let steady = r.root.join("steady.json");
    fs::write(&steady, r#"["jitter"]"#).unwrap();
    let out = r
        .cmd()
        .env("NS_RATCHET_STEADY", &steady)
        .args(["check"])
        .output();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(r.count(), "1\n");
    assert_eq!(fs::read_to_string(&steady).unwrap(), r#"["jitter"]"#);
}

#[test]
fn a_ratchet_that_breaks_on_its_second_run_is_not_marked_steady() {
    let r = Repo::new(
        "[ratchets.once]\ncommand = \"[ -f seen ] && exit 1; touch seen; echo 1\"\nbetter = \"lower\"\n",
    );
    let steady = r.root.join("steady.json");
    let out = r
        .cmd()
        .env("NS_RATCHET_STEADY", &steady)
        .args(["check"])
        .output();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ratchets"][0]["status"], "broken");
    assert!(!steady.exists());
}

#[test]
fn a_report_path_that_cannot_be_created_is_an_error() {
    let r = Repo::new(&Repo::cat("a", "lower", ""));
    r.measure("a", "1");
    fs::write(r.root.join("file"), "").unwrap();
    let out = r
        .cmd()
        .env("NS_RATCHET_REPORT", r.root.join("file/sub/report.json"))
        .args(["check"])
        .output();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot create"));
}

#[test]
fn an_unknown_key_or_a_missing_command_is_a_usage_error_naming_it() {
    let r = Repo::new("[ratchets.a]\ncommand = \"true\"\nbetter = \"lower\"\ntolerence = 1\n");
    let (code, _, err) = r.ns(&["check"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("tolerence"), "{err}");
    let r = Repo::new("[ratchets.a]\nbetter = \"lower\"\n");
    let (code, _, err) = r.ns(&["check"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("command"), "{err}");
}

#[test]
fn every_rule_pack_test_that_runs_ns_ratchet_check_clears_the_gates_ratchet_variables() {
    let rules = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rules");
    let unset = "\nunset NS_RATCHET_FACTORY NS_RATCHET_STEADY NS_RATCHET_REPORT\n";
    let mut checked = 0;
    for pack in fs::read_dir(&rules).unwrap() {
        let script = pack.unwrap().path().join("tests/run.sh");
        let Ok(text) = fs::read_to_string(&script) else {
            continue;
        };
        if text.contains("ratchet check") {
            checked += 1;
            assert!(text.contains(unset), "{} lacks{unset}", script.display());
        }
    }
    assert!(checked >= 2, "{checked}");
}
