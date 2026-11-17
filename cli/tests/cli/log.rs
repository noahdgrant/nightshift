use std::time::{Duration, SystemTime};

use crate::common::Repo;

use assert_cmd::Command;

/// A session whose agent said `said`, ran `command` and saw `seen`.
fn session(said: &str, command: &str, seen: &str) -> String {
    format!(
        concat!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{said}"}},{{"type":"tool_use","name":"Bash","input":{{"command":"{command}"}}}}]}}}}"#,
            "\n",
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","content":"{seen}"}}]}}}}"#,
            "\n",
            r#"{{"type":"result","result":"{said}"}}"#,
            "\n"
        ),
        said = said,
        command = command,
        seen = seen
    )
}

/// Saves `text` as a session of unit 5, written `age` seconds after the epoch so the order is
/// the test's own.
fn keep(repo: &Repo, name: &str, age: u64, text: &str) {
    let dir = repo.path.join(".git/ns/transcripts/5");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), text).unwrap();
    std::fs::File::options()
        .write(true)
        .open(dir.join(name))
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(age))
        .unwrap();
}

/// Unit 5 built, verified, then was sent back and built again.
pub fn kept() -> Repo {
    let repo = Repo::new();
    keep(
        &repo,
        "build-1.jsonl",
        1,
        &session("Building.", "make", "made"),
    );
    keep(
        &repo,
        "verify-1.jsonl",
        2,
        &session("Checking.", "ns check", "failed"),
    );
    keep(
        &repo,
        "build-2.jsonl",
        3,
        &session("Fixing.", "make fix", "fixed"),
    );
    std::fs::write(
        repo.path.join(".git/ns/transcripts/5/build-1.model"),
        "opus",
    )
    .unwrap();
    repo
}

pub fn log(repo: &Repo, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .arg("log")
        .args(args)
        .current_dir(&repo.path)
        .assert()
}

fn stdout(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.success().get_output().stdout.clone()).unwrap()
}

#[test]
fn log_prints_each_session_in_order_with_its_calls_and_results() {
    let out = stdout(log(&kept(), &["5"]));
    assert_eq!(
        out,
        "== build 1 (opus) ==\nBuilding.\n> Bash: make\n  = made\n\n\
         == verify 1 (harness default model) ==\nChecking.\n> Bash: ns check\n  = failed\n\n\
         == build 2 (harness default model) ==\nFixing.\n> Bash: make fix\n  = fixed\n\n"
    );
}

#[test]
fn log_state_prints_only_that_states_sessions() {
    let out = stdout(log(&kept(), &["5", "--state", "build"]));
    assert!(out.starts_with("== build 1 (opus) =="), "{out}");
    assert!(out.contains("== build 2 ("), "{out}");
    assert!(!out.contains("verify"), "{out}");
}

#[test]
fn log_last_prints_only_the_most_recent_session() {
    let out = stdout(log(&kept(), &["5", "--last"]));
    assert!(out.starts_with("== build 2 ("), "{out}");
    assert_eq!(out.matches("==").count(), 2);
}

#[test]
fn log_last_with_a_state_prints_the_last_session_of_that_state() {
    let out = stdout(log(&kept(), &["5", "--state", "verify", "--last"]));
    assert!(out.starts_with("== verify 1 ("), "{out}");
    assert_eq!(out.matches("==").count(), 2);
}

#[test]
fn log_of_a_unit_with_no_transcripts_prints_nothing_and_succeeds() {
    assert_eq!(stdout(log(&Repo::new(), &["5"])), "");
}

#[test]
fn log_of_a_state_that_does_not_exist_is_an_error() {
    log(&kept(), &["5", "--state", "nowhere"]).failure();
}

#[test]
fn log_changes_nothing() {
    let repo = kept();
    let before = std::fs::read_dir(repo.path.join(".git/ns/transcripts/5"))
        .unwrap()
        .count();
    stdout(log(&repo, &["5"]));
    let after = std::fs::read_dir(repo.path.join(".git/ns/transcripts/5"))
        .unwrap()
        .count();
    assert_eq!(before, after);
    assert!(!repo.path.join(".git/ns/runs.jsonl").exists());
}

fn lines(out: &str) -> Vec<serde_json::Value> {
    out.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn log_json_prints_each_message_as_a_line_with_its_kind_state_and_n() {
    let out = stdout(log(&kept(), &["5", "--json"]));
    let got = lines(&out);
    assert_eq!(got.len(), 9);
    assert_eq!(
        got[0],
        serde_json::json!({"state":"build","n":1,"kind":"text","text":"Building."})
    );
    assert_eq!(
        got[1],
        serde_json::json!({"state":"build","n":1,"kind":"tool_call","tool":"Bash","input":{"command":"make"}})
    );
    assert_eq!(
        got[2],
        serde_json::json!({"state":"build","n":1,"kind":"tool_result","text":"made"})
    );
    let kinds: Vec<_> = got[3..6]
        .iter()
        .map(|l| l["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["text", "tool_call", "tool_result"]);
    assert_eq!(
        (got[3]["state"].as_str(), got[3]["n"].as_u64()),
        (Some("verify"), Some(1))
    );
    assert_eq!(
        (got[8]["state"].as_str(), got[8]["n"].as_u64()),
        (Some("build"), Some(2))
    );
}

#[test]
fn log_json_state_and_last_pick_the_sessions_the_text_form_does() {
    let repo = kept();
    let sessions = |args: &[&str]| -> Vec<(String, u64)> {
        let mut all: Vec<(String, u64)> = lines(&stdout(log(&repo, args)))
            .iter()
            .map(|l| {
                (
                    l["state"].as_str().unwrap().to_string(),
                    l["n"].as_u64().unwrap(),
                )
            })
            .collect();
        all.dedup();
        all
    };
    assert_eq!(
        sessions(&["5", "--json", "--state", "build"]),
        [("build".to_string(), 1), ("build".to_string(), 2)]
    );
    assert_eq!(
        sessions(&["5", "--json", "--last"]),
        [("build".to_string(), 2)]
    );
    assert_eq!(
        sessions(&["5", "--json", "--state", "verify", "--last"]),
        [("verify".to_string(), 1)]
    );
}
