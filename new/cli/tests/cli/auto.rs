use crate::common::{script, Repo};
use crate::run::{tools, Fakes};

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
    let path = tools(repo, fakes);
    Command::cargo_bin("ns")
        .unwrap()
        .args(["auto", "--ci-poll-seconds", "0"])
        .args(extra)
        .env("PATH", path)
        .env("NS_CONFIG_DIR", &repo.config)
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
        .filter(|l| l.starts_with("issue edit"))
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
    assert_eq!(
        last["status"],
        serde_json::json!(["status:ready-for-human"])
    );
    assert_eq!(
        label_edits(&repo).last().unwrap(),
        "issue edit 5 --add-label status:ready-for-human --remove-label status:in-progress"
    );
}
