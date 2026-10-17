use crate::common::{script, Repo};

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

#[test]
fn auto_without_dry_run_is_not_built_yet() {
    let repo = Repo::new();
    Command::cargo_bin("ns")
        .unwrap()
        .arg("auto")
        .current_dir(&repo.path)
        .assert()
        .failure();
}
