use crate::common::{script, Repo};

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
    let repo = Repo::new();
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
