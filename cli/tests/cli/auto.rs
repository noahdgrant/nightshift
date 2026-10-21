use crate::common::{script, Repo};
use crate::run::{tools, Fakes, FAIL};

use std::time::Duration;

use assert_cmd::Command;

/// Runs `ns auto --dry-run` against a fake `gh` that lists `issues` on one page.
fn dry_run(issues: &str) -> serde_json::Value {
    let repo = Repo::new();
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(&bin, "gh", &format!("cat <<'EOF'\n[[{issues}]]\nEOF"));
    let out = Command::cargo_bin("ns")
        .unwrap()
        .args(["auto", "--dry-run"])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
        .success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

fn unit(number: u64, labels: &[&str], association: &str) -> String {
    let labels: Vec<_> = labels
        .iter()
        .map(|l| format!(r#"{{"name":"{l}"}}"#))
        .collect();
    format!(
        r#"{{"number":{number},"title":"unit {number}","body":"","labels":[{}],"author_association":"{association}"}}"#,
        labels.join(",")
    )
}

fn numbers(list: &serde_json::Value) -> Vec<u64> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|u| u["unit"].as_u64().unwrap())
        .collect()
}

#[test]
fn the_plan_lists_the_queue_and_what_to_triage_in_order() {
    let issues = [
        unit(3, &["status:ready-for-agent"], "OWNER"),
        unit(1, &["status:ready-for-agent", "priority:high"], "MEMBER"),
        unit(4, &[], "OWNER"),
        unit(5, &["status:ready-for-agent"], "NONE"),
    ]
    .join(",");
    let plan = dry_run(&issues);
    assert_eq!(numbers(&plan["queue"]), [1, 3]);
    assert_eq!(numbers(&plan["triage"]), [4]);
    assert_eq!(numbers(&plan["skipped"]), [5]);
}

const UNTRIAGED: &str = "[]";

/// Runs `ns auto` with `extra` arguments against the fakes of `ns run`, which serve one unit, 5.
fn auto(repo: &Repo, fakes: &Fakes, extra: &[&str]) -> assert_cmd::assert::Assert {
    auto_by(repo, fakes, extra, None)
}

/// Like [`auto`], killed after `hang_guard` if it has not ended. The guard is never what a test
/// checks.
fn auto_by(
    repo: &Repo,
    fakes: &Fakes,
    extra: &[&str],
    hang_guard: Option<Duration>,
) -> assert_cmd::assert::Assert {
    let path = tools(repo, fakes);
    let mut cmd = Command::cargo_bin("ns").unwrap();
    if let Some(guard) = hang_guard {
        cmd.timeout(guard);
    }
    cmd.args(["auto", "--ci-poll-seconds", "0"])
        .args(extra)
        .env("PATH", path)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .current_dir(&repo.path)
        .assert()
}

fn events(out: &assert_cmd::assert::Assert) -> Vec<serde_json::Value> {
    String::from_utf8(out.get_output().stdout.clone())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn label_edits(repo: &Repo) -> Vec<String> {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/gh.log"))
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("issue edit") && !l.contains("assignee"))
        .map(String::from)
        .collect()
}

#[test]
fn auto_triages_the_untriaged_unit_then_runs_it() {
    let repo = Repo::new();
    let fakes = Fakes {
        labels: UNTRIAGED,
        ..Fakes::passing()
    };
    let out = auto(&repo, &fakes, &["--merge"]).success();
    let events = events(&out);
    assert_eq!(events[0]["route"], "build");
    let last = events.last().unwrap();
    assert_eq!(last["unit"], 5);
    assert_eq!(last["ok"], true);
    assert_eq!(last["status"], serde_json::json!([]));
    assert_eq!(last["change_request"], "https://forge.example/9");
    assert_eq!(last["error"], serde_json::Value::Null);
    assert_eq!(
        label_edits(&repo),
        [
            "issue edit 5 --add-label type:feat,status:ready-for-agent",
            "issue edit 5 --add-label status:in-progress --remove-label status:ready-for-agent",
            "issue edit 5 --remove-label status:in-progress",
        ]
    );
}

#[test]
fn auto_runs_a_ready_unit_without_triaging_it() {
    let repo = Repo::new();
    let out = auto(&repo, &Fakes::passing(), &["--merge"]).success();
    assert!(events(&out).iter().all(|e| e.get("route").is_none()));
    assert!(!label_edits(&repo)[0].contains("type:feat"));
}

#[test]
fn max_units_limits_how_many_ready_units_are_run() {
    let repo = Repo::new();
    let out = auto(&repo, &Fakes::passing(), &["--max-units", "0"]).success();
    assert!(events(&out).is_empty());
    assert!(label_edits(&repo).is_empty());
}

#[test]
fn a_stuck_unit_is_left_for_a_human_and_auto_reports_it() {
    let repo = Repo::new();
    let broken = Fakes {
        exit: 1,
        ..Fakes::passing()
    };
    let out = auto(&repo, &broken, &[]).failure();
    let events = events(&out);
    let last = events.last().unwrap();
    assert_eq!(last["ok"], false);
    assert_eq!(last["error"], "unit 5 is stuck in build");
    assert_eq!(last["change_request"], serde_json::Value::Null);
    assert_eq!(
        last["status"],
        serde_json::json!(["status:ready-for-human"])
    );
    assert_eq!(
        label_edits(&repo).last().unwrap(),
        "issue edit 5 --add-label status:ready-for-human --remove-label status:in-progress"
    );
}

#[test]
fn a_unit_is_run_once_even_when_it_is_still_ready_afterwards() {
    let repo = Repo::new();
    let stuck_ready = Fakes {
        labels_follow_edits: false,
        ..Fakes::passing()
    };
    let out = auto_by(
        &repo,
        &stuck_ready,
        &["--merge"],
        Some(Duration::from_secs(120)),
    )
    .success();
    let ran = events(&out)
        .into_iter()
        .filter(|e| e.get("ok").is_some())
        .count();
    assert_eq!(ran, 1);
}

fn lock_path(repo: &Repo) -> std::path::PathBuf {
    repo.path.join(".git/ns/auto.lock")
}

/// Holds the lock `ns auto` takes, as another `ns auto` would.
fn lock_held(repo: &Repo) -> std::fs::File {
    std::fs::create_dir_all(lock_path(repo).parent().unwrap()).unwrap();
    let file = std::fs::File::create(lock_path(repo)).unwrap();
    file.lock().unwrap();
    file
}

#[test]
fn a_second_auto_does_not_start_while_one_runs() {
    let repo = Repo::new();
    let held = lock_held(&repo);
    let out = auto(&repo, &Fakes::passing(), &[]).failure();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("another ns auto is running"), "{err}");
    assert!(label_edits_none(&repo));
    drop(held);
    auto(&repo, &Fakes::passing(), &["--merge"]).success();
}

fn label_edits_none(repo: &Repo) -> bool {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/gh.log"))
        .map_or(true, |log| !log.contains("issue edit"))
}

#[test]
fn a_dry_run_does_not_need_the_lock() {
    let repo = Repo::new();
    let _held = lock_held(&repo);
    auto(&repo, &Fakes::passing(), &["--dry-run"]).success();
}

#[test]
fn a_unit_someone_is_running_is_reported_and_left_alone() {
    let repo = Repo::new();
    let path = repo.path.join(".git/ns/unit-5.lock");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let held = std::fs::File::create(path).unwrap();
    held.lock().unwrap();
    let out = auto(&repo, &Fakes::passing(), &[]).failure();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("unit 5 is already being run"), "{err}");
    assert!(label_edits_none(&repo));
}

/// A time of day a minute before now, which `--until` takes to be tomorrow.
fn a_minute_ago() -> String {
    (chrono::Local::now() - chrono::Duration::minutes(1))
        .format("%H:%M")
        .to_string()
}

#[test]
fn units_run_while_the_time_to_stop_is_still_ahead() {
    let repo = Repo::new();
    let out = auto(
        &repo,
        &Fakes::passing(),
        &["--merge", "--until", &a_minute_ago()],
    )
    .success();
    assert!(events(&out)
        .iter()
        .any(|e| e["unit"] == 5 && e["ok"] == true));
}

#[test]
fn a_time_that_is_not_a_time_of_day_is_refused() {
    let repo = Repo::new();
    auto(&repo, &Fakes::passing(), &["--until", "late"]).failure();
    assert!(label_edits_none(&repo));
}

#[test]
fn auto_troubleshoots_a_unit_that_needs_a_reproduction_before_running_it() {
    let repo = Repo::new();
    let fakes = Fakes {
        labels: r#"[{"name":"status:needs-repro"}]"#,
        ..Fakes::passing()
    };
    let out = auto(&repo, &fakes, &["--merge"]).success();
    let events = events(&out);
    assert_eq!(events[0]["route"], "build");
    assert_eq!(events.last().unwrap()["ok"], true);
    let edits = label_edits(&repo);
    assert!(edits[0].starts_with("issue edit 5 --body"), "{edits:?}");
    assert!(edits.iter().any(
        |e| e.contains("--add-label status:ready-for-agent --remove-label status:needs-repro")
    ));
}

#[test]
fn a_change_ready_to_merge_is_reported_with_its_address_and_left_for_a_human() {
    let repo = Repo::new();
    let out = auto(&repo, &Fakes::passing(), &[]).success();
    let events = events(&out);
    let last = events.last().unwrap();
    assert_eq!(last["ok"], true);
    assert_eq!(last["change_request"], "https://forge.example/9");
    assert_eq!(
        last["status"],
        serde_json::json!(["status:ready-for-human"])
    );
}

/// Three ready units, 1 to 3, whose harness always exits non-zero.
fn three_units_on_a_broken_harness() -> Fakes {
    Fakes {
        exit: 1,
        labels_follow_edits: false,
        units: &[1, 2, 3],
        ..Fakes::passing()
    }
}

fn started_units(out: &assert_cmd::assert::Assert) -> Vec<u64> {
    events(out)
        .iter()
        .filter(|e| e.get("ok").is_some())
        .map(|e| e["unit"].as_u64().unwrap())
        .collect()
}

#[test]
fn auto_stops_the_night_after_two_units_in_a_row_go_wrong_on_the_harness() {
    let repo = Repo::new();
    let out = auto(&repo, &three_units_on_a_broken_harness(), &[]).failure();
    assert_eq!(started_units(&out), [1, 2]);
    let stop = events(&out)
        .into_iter()
        .find(|e| e.get("stopped").is_some())
        .unwrap();
    assert_eq!(stop["stopped"], "went_wrong");
    assert_eq!(stop["units"], serde_json::json!([1, 2]));
    assert!(stop["reason"].as_str().unwrap().contains("in a row"));
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(
        err.contains("stopped: 2 units in a row went wrong (1, 2)"),
        "{err}"
    );
}

#[test]
fn a_limit_of_zero_runs_every_unit_however_many_go_wrong() {
    let repo = Repo::new();
    let out = auto(
        &repo,
        &three_units_on_a_broken_harness(),
        &["--max-failures-in-a-row", "0"],
    )
    .failure();
    assert_eq!(started_units(&out), [1, 2, 3]);
    assert!(events(&out).iter().all(|e| e.get("stopped").is_none()));
}

#[test]
fn units_stuck_for_failing_checks_do_not_stop_the_night() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_review: FAIL,
        review: FAIL,
        labels_follow_edits: false,
        units: &[1, 2, 3],
        ..Fakes::passing()
    };
    let out = auto(&repo, &fakes, &["--max-failures-in-a-row", "1"]).failure();
    assert_eq!(started_units(&out), [1, 2, 3]);
    assert!(events(&out).iter().all(|e| e.get("stopped").is_none()));
}
