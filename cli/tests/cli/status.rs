use crate::common::{git, script, Repo};

use assert_cmd::Command;
use serde_json::{json, Value};

/// A unit as the fake `gh` lists it.
fn unit(number: u64, labels: &[&str]) -> String {
    let labels: Vec<_> = labels
        .iter()
        .map(|l| format!(r#"{{"name":"{l}"}}"#))
        .collect();
    format!(
        r#"{{"number":{number},"title":"unit {number}","body":"","labels":[{}],"author_association":"OWNER"}}"#,
        labels.join(",")
    )
}

/// Runs `ns status` against a fake `gh` that lists `issues` and has a change request only on the
/// branches of `change_requests`, each with its state. `runs` is the run log.
fn run_status(
    issues: &[String],
    change_requests: &[(u64, &str)],
    runs: &str,
) -> assert_cmd::assert::Assert {
    run_status_in(&Repo::new(), issues, change_requests, runs)
}

fn run_status_in(
    repo: &Repo,
    issues: &[String],
    change_requests: &[(u64, &str)],
    runs: &str,
) -> assert_cmd::assert::Assert {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cases: String = change_requests
        .iter()
        .map(|(unit, state)| {
            format!(
                r#"*"--head ns/{unit} "*) echo '[{{"number":9,"url":"https://forge.example/{unit}","state":"{state}"}}]';;"#
            )
        })
        .collect();
    script(
        &bin,
        "gh",
        &format!(
            r#"case "$1" in
api) cat <<'EOF'
[[{}]]
EOF
;;
pr) case "$*" in {cases} *) echo '[]';; esac;;
esac"#,
            issues.join(",")
        ),
    );
    std::fs::create_dir_all(repo.path.join(".git/ns")).unwrap();
    std::fs::write(repo.path.join(".git/ns/runs.jsonl"), runs).unwrap();
    Command::cargo_bin("ns")
        .unwrap()
        .arg("status")
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

fn status(issues: &[String], change_requests: &[(u64, &str)], runs: &str) -> Value {
    let out = run_status(issues, change_requests, runs).success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

#[test]
fn a_unit_that_never_ran_has_no_state_and_no_change_request() {
    let out = status(&[unit(3, &["status:ready-for-agent"])], &[], "");
    assert_eq!(
        out,
        json!({"units": [{
            "unit": 3, "title": "unit 3", "status": "status:ready-for-agent",
            "state": null, "moved_at": null, "change_request": null,
        }]})
    );
}

#[test]
fn a_unit_without_a_status_label_has_a_null_status() {
    let out = status(&[unit(3, &["priority:high"])], &[], "");
    assert_eq!(out["units"][0]["status"], Value::Null);
}

#[test]
fn a_done_unit_shows_its_last_move_and_its_open_change_request() {
    let runs = concat!(
        r#"{"unit":"7","from":"setup","to":"build","at":100}"#,
        "\n",
        r#"{"unit":"8","from":"setup","to":"stuck","at":150}"#,
        "\n",
        r#"{"unit":"7","from":"ship","to":"done","at":200}"#,
        "\n",
    );
    let out = status(
        &[unit(7, &["status:ready-for-human"])],
        &[(7, "OPEN")],
        runs,
    );
    assert_eq!(
        out["units"][0],
        json!({
            "unit": 7, "title": "unit 7", "status": "status:ready-for-human",
            "state": "done", "moved_at": 200,
            "change_request": {"url": "https://forge.example/7", "state": "open"},
        })
    );
}

#[test]
fn a_stuck_unit_shows_stuck() {
    let runs = r#"{"unit":"4","from":"verify","to":"stuck","at":300}"#;
    let out = status(&[unit(4, &["status:ready-for-human"])], &[], runs);
    assert_eq!(out["units"][0]["state"], "stuck");
    assert_eq!(out["units"][0]["moved_at"], 300);
}

#[test]
fn merged_and_closed_change_requests_are_named() {
    let out = status(
        &[unit(1, &[]), unit(2, &[])],
        &[(1, "MERGED"), (2, "CLOSED")],
        "",
    );
    assert_eq!(out["units"][0]["change_request"]["state"], "merged");
    assert_eq!(out["units"][1]["change_request"]["state"], "closed");
}

#[test]
fn units_come_in_the_order_ns_auto_takes_them() {
    let out = status(
        &[
            unit(3, &[]),
            unit(1, &["priority:low"]),
            unit(2, &["priority:high"]),
        ],
        &[],
        "",
    );
    let numbers: Vec<u64> = out["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["unit"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [2, 1, 3]);
}

#[test]
fn a_repository_with_no_open_units_lists_none() {
    assert_eq!(status(&[], &[], ""), json!({"units": []}));
}

#[test]
fn a_last_move_without_a_time_has_a_null_moved_at() {
    let runs = r#"{"unit":"5","from":"ship","to":"done"}"#;
    let out = status(&[unit(5, &[])], &[], runs);
    assert_eq!(out["units"][0]["state"], "done");
    assert_eq!(out["units"][0]["moved_at"], Value::Null);
}

#[test]
fn a_last_move_with_a_bad_from_state_still_gives_its_state() {
    let runs = r#"{"unit":"5","from":"nowhere","to":"done","at":1}"#;
    let out = run_status(&[unit(5, &[])], &[], runs).success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(json["units"][0]["state"], "done");
    assert_eq!(json["units"][0]["moved_at"], 1);
}

/// A repository whose base branch is published and which has a workspace for unit 7 on `ns/7`,
/// made from that base. Unit 7 has an open change request.
fn open_unit_repo() -> Repo {
    let repo = Repo::new();
    git(&repo.path, &["push", "-q", "origin", "main"]);
    git(
        &repo.path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "ns/7",
            &repo.workspace("7").to_string_lossy(),
        ],
    );
    repo
}

/// Adds a commit to the origin's `main`, through a clone of its own.
fn base_moves(repo: &Repo) {
    let other = repo.path.parent().unwrap().join("other");
    let origin = repo.path.parent().unwrap().join("origin.git");
    git(
        repo.path.parent().unwrap(),
        &["clone", "-q", &origin.to_string_lossy(), "other"],
    );
    git(&other, &["commit", "-q", "--allow-empty", "-m", "other"]);
    git(&other, &["push", "-q", "origin", "main"]);
}

fn base_of(repo: &Repo, runs: &str) -> Value {
    let out = run_status_in(repo, &[unit(7, &[])], &[(7, "OPEN")], runs).success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    json["units"][0]["change_request"]["base"].clone()
}

#[test]
fn an_open_change_request_with_the_base_tip_is_current() {
    let repo = open_unit_repo();
    assert_eq!(base_of(&repo, ""), "current");
}

#[test]
fn an_open_change_request_without_the_base_tip_is_behind_and_status_changes_nothing() {
    let repo = open_unit_repo();
    base_moves(&repo);
    let head = git(&repo.workspace("7"), &["rev-parse", "HEAD"]);
    assert_eq!(base_of(&repo, ""), "behind");
    assert_eq!(git(&repo.workspace("7"), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        std::fs::read_to_string(repo.path.join(".git/ns/runs.jsonl")).unwrap(),
        ""
    );
}

#[test]
fn an_unreachable_origin_leaves_the_base_unknown_and_the_rest_listed() {
    let repo = open_unit_repo();
    std::fs::remove_dir_all(repo.path.parent().unwrap().join("origin.git")).unwrap();
    let out = run_status_in(&repo, &[unit(7, &[])], &[(7, "OPEN")], "").success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(json["units"][0]["change_request"]["base"], "unknown");
    assert_eq!(json["units"][0]["unit"], 7);
    assert_eq!(json["units"][0]["change_request"]["state"], "open");
}

#[test]
fn a_base_tip_a_refresh_tested_is_current() {
    let repo = open_unit_repo();
    base_moves(&repo);
    let tip = git(
        &repo.path.parent().unwrap().join("origin.git"),
        &["rev-parse", "main"],
    );
    let runs = format!("{{\"unit\":\"7\",\"base_tested\":\"{}\"}}\n", tip.trim());
    assert_eq!(base_of(&repo, &runs), "current");
}

#[test]
fn with_the_queue_off_status_shows_no_base() {
    let repo = open_unit_repo();
    base_moves(&repo);
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "[merge]\nqueue = false\n",
    )
    .unwrap();
    assert_eq!(base_of(&repo, ""), Value::Null);
}

#[test]
fn a_merged_change_request_shows_no_base() {
    let repo = open_unit_repo();
    base_moves(&repo);
    let out = run_status_in(&repo, &[unit(7, &[])], &[(7, "MERGED")], "").success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(json["units"][0]["change_request"]["base"], Value::Null);
}
