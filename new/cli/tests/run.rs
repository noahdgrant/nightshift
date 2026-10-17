mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use assert_cmd::Command;
use common::Repo;

/// Puts a script named `name` in `bin`.
fn script(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// How the fake `claude` behaves. It builds, and when asked to verify it checks and ends with a
/// verdict.
struct Fakes {
    /// The verdict the first check ends with.
    first_verdict: &'static str,
    /// The verdict every later check ends with.
    verdict: &'static str,
    check_edits: CheckEdits,
    /// The exit code of `claude`.
    exit: i32,
    /// Whether the fake dies without a word in a check, as a crashed harness would.
    crash_in_first_check: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum CheckEdits {
    Never,
    /// The first check writes a file into the workspace; later checks remove it again.
    FirstThenFixes,
    /// Every check writes to the workspace.
    Always,
}

const PASS: &str = "VERDICT: PASS";
const FAIL: &str = "VERDICT: FAIL";

impl Fakes {
    fn passing() -> Fakes {
        Fakes {
            first_verdict: PASS,
            verdict: PASS,
            check_edits: CheckEdits::Never,
            exit: 0,
            crash_in_first_check: false,
        }
    }
}

/// A `bin` directory holding a `gh` that serves one unit and the fake `claude`. Returns the
/// `PATH` to start `ns` with, that directory first.
fn tools(repo: &Repo, fakes: &Fakes) -> String {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "gh",
        r#"echo '{"number":5,"title":"Add a thing","body":"Make the thing.","labels":[]}'"#,
    );
    let edit = match fakes.check_edits {
        CheckEdits::Never => "",
        CheckEdits::FirstThenFixes => {
            r#"if [ -e "$seen" ]; then rm -f touched.txt; else echo x > touched.txt; fi; "#
        }
        CheckEdits::Always => "echo x >> touched.txt; ",
    };
    let crash = if fakes.crash_in_first_check {
        r#"if [ ! -e "$seen" ]; then touch "$seen"; exit 9; fi; "#
    } else {
        ""
    };
    script(
        &bin,
        "claude",
        &format!(
            r#"seen="{bin}/checked"
case "$(cat)" in
*"Do not change any source"*)
  {crash}{edit}echo checked it
  if [ -e "$seen" ]; then echo '{verdict}'; else touch "$seen"; echo '{first}'; fi;;
*"VERDICT: FAIL"*) echo rebuilt with the evidence;;
*) echo built the thing;;
esac
exit {exit}"#,
            bin = bin.display(),
            verdict = fakes.verdict,
            first = fakes.first_verdict,
            exit = fakes.exit,
        ),
    );
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap())
}

fn run(repo: &Repo, path: &str) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .args(["run", "5"])
        .env("PATH", path)
        .current_dir(&repo.path)
        .assert()
}

fn moves(out: Vec<u8>) -> Vec<String> {
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            match v.get("to") {
                Some(to) => format!("{}>{}", v["from"].as_str().unwrap(), to.as_str().unwrap()),
                None => format!("start {}", v["state"].as_str().unwrap()),
            }
        })
        .collect()
}

fn read(repo: &Repo, file: &str) -> String {
    std::fs::read_to_string(repo.workspace("5").join(".ns/5").join(file)).unwrap()
}

#[test]
fn run_sets_up_builds_verifies_and_prints_each_move() {
    let repo = Repo::new();
    let out = run(&repo, &tools(&repo, &Fakes::passing()))
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        moves(out),
        [
            "start setup",
            "setup>build",
            "build>verify",
            "verify>review",
            "review>ship",
            "ship>clean_up",
            "clean_up>done",
        ]
    );
    assert!(repo.branches().contains(&"ns/5".to_string()));
    assert_eq!(read(&repo, "build.md"), "built the thing\n");
    assert_eq!(read(&repo, "evidence.md"), "checked it\nVERDICT: PASS\n");
}

#[test]
fn a_build_that_keeps_failing_sticks_the_unit_and_keeps_its_output() {
    let repo = Repo::new();
    let fakes = Fakes {
        exit: 1,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    assert_eq!(moves(out).last().unwrap(), "build>stuck");
    assert!(repo.workspace("5").join(".ns/5/build.md").is_file());
}

#[test]
fn a_failed_check_sends_the_unit_back_to_build_with_the_evidence() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: FAIL,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        moves(out),
        [
            "start setup",
            "setup>build",
            "build>verify",
            "verify>build",
            "build>verify",
            "verify>review",
            "review>ship",
            "ship>clean_up",
            "clean_up>done",
        ]
    );
    assert_eq!(read(&repo, "build.md"), "rebuilt with the evidence\n");
    assert_eq!(read(&repo, "evidence.md"), "checked it\nVERDICT: PASS\n");
}

#[test]
fn a_check_that_keeps_failing_sticks_the_unit_after_going_back_to_build_once() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: FAIL,
        verdict: FAIL,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(
        &moves[moves.len() - 3..],
        ["verify>build", "build>verify", "verify>stuck"]
    );
}

#[test]
fn evidence_with_no_verdict_is_a_broken_check_that_is_run_again_not_a_failed_build() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: "all fine",
        verdict: "all fine",
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["verify>verify", "verify>stuck"]);
    assert!(!moves.contains(&"verify>build".to_string()), "{moves:?}");
    let evidence = read(&repo, "evidence.md");
    assert!(
        evidence.contains("does not end with a verdict"),
        "{evidence}"
    );
    assert!(evidence.ends_with("VERDICT: ERROR\n"), "{evidence}");
}

#[test]
fn a_check_that_changes_the_workspace_is_run_again_and_must_put_it_right() {
    let repo = Repo::new();
    let fakes = Fakes {
        check_edits: CheckEdits::FirstThenFixes,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert!(moves.contains(&"verify>verify".to_string()), "{moves:?}");
    assert!(!moves.contains(&"verify>build".to_string()), "{moves:?}");
    assert_eq!(moves.last().unwrap(), "clean_up>done");
    assert!(!repo.workspace("5").join("touched.txt").exists());
    assert_eq!(read(&repo, "build.md"), "built the thing\n");
}

#[test]
fn a_check_that_keeps_changing_the_workspace_sticks_the_unit_without_going_back_to_build() {
    let repo = Repo::new();
    let fakes = Fakes {
        check_edits: CheckEdits::Always,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["verify>verify", "verify>stuck"]);
    assert!(!moves.contains(&"verify>build".to_string()), "{moves:?}");
    let evidence = read(&repo, "evidence.md");
    assert!(evidence.contains("not as the build left it"), "{evidence}");
    assert!(evidence.ends_with("VERDICT: ERROR\n"), "{evidence}");
}

#[test]
fn a_crashed_harness_in_a_check_runs_the_check_again_not_the_build() {
    let repo = Repo::new();
    let fakes = Fakes {
        crash_in_first_check: true,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert!(moves.contains(&"verify>verify".to_string()), "{moves:?}");
    assert!(!moves.contains(&"verify>build".to_string()), "{moves:?}");
    assert_eq!(read(&repo, "build.md"), "built the thing\n");
}

#[test]
fn artifacts_do_not_block_removing_the_workspace() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    Command::cargo_bin("ns")
        .unwrap()
        .args(["workspace", "remove", "5"])
        .current_dir(&repo.path)
        .assert()
        .success();
    assert!(!repo.workspace("5").exists());
}
