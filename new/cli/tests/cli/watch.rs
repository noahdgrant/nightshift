use crate::common::{script, Repo};

use assert_cmd::Command;

/// Runs `ns watch --dry-run` against a fake `gh` that lists `issues` on one page.
fn dry_run(issues: &str) -> serde_json::Value {
    let repo = Repo::new();
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(&bin, "gh", &format!("cat <<'EOF'\n[[{issues}]]\nEOF"));
    let out = Command::cargo_bin("ns")
        .unwrap()
        .args(["watch", "--dry-run"])
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
fn watch_without_dry_run_is_not_built_yet() {
    let repo = Repo::new();
    Command::cargo_bin("ns")
        .unwrap()
        .arg("watch")
        .current_dir(&repo.path)
        .assert()
        .failure();
}

/// Runs `ns watch --triage-only` with `extra` arguments. The fake `gh` lists unit 4, which the
/// team filed without a state, and the fake `claude` answers `reply`.
fn triage(repo: &Repo, reply: &str, extra: &[&str]) -> assert_cmd::assert::Assert {
    triage_by(repo, reply, "OWNER", extra)
}

/// Like [`triage`], with the unit's author known to the forge as `association` when it is read.
fn triage_by(
    repo: &Repo,
    reply: &str,
    association: &str,
    extra: &[&str],
) -> assert_cmd::assert::Assert {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let one = unit(4, &["status:needs-triage"], "OWNER");
    let read = unit(4, &["status:needs-triage"], association);
    script(
        &bin,
        "gh",
        &format!(
            r#"echo "$@" >> "{bin}/gh.log"
case "$1 $2" in
"api --paginate") echo '[[{one}]]';;
"api repos/{{owner}}/{{repo}}/issues/4") echo '{read}';;
esac"#,
            bin = bin.display()
        ),
    );
    script(
        &bin,
        "claude",
        &format!("cat >/dev/null\ncat <<'EOF'\n{reply}\nEOF"),
    );
    Command::cargo_bin("ns")
        .unwrap()
        .args(["watch", "--triage-only"])
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("NS_CONFIG_DIR", &repo.config)
        .current_dir(&repo.path)
        .assert()
}

fn gh_log(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/gh.log")).unwrap()
}

#[test]
fn a_unit_triaged_to_build_gets_its_brief_as_a_comment_and_is_made_ready() {
    let repo = Repo::new();
    let reply = "Summary: add the thing.\n\nLABELS: type:feat, priority:high\nROUTE: build";
    let out = triage(&repo, reply, &[]).success();
    let event: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(event["unit"], 4);
    assert_eq!(event["route"], "build");
    let log = gh_log(&repo);
    assert!(
        log.contains("issue comment 4 --body 🤖 *Posted by nightshift, an AI agent.*\n\nSummary: add the thing."),
        "{log}"
    );
    assert!(
        log.contains(
            "issue edit 4 --add-label type:feat,priority:high,status:ready-for-agent --remove-label status:needs-triage"
        ),
        "{log}"
    );
    assert!(!log.contains("issue close"), "{log}");
}

#[test]
fn a_wontfix_unit_is_closed() {
    let repo = Repo::new();
    triage(
        &repo,
        "Already done.\nLABELS: type:chore\nROUTE: wontfix",
        &[],
    )
    .success();
    let log = gh_log(&repo);
    assert!(
        log.contains("--add-label type:chore,status:wontfix"),
        "{log}"
    );
    assert!(log.contains("issue close 4 --reason not planned"), "{log}");
}

#[test]
fn an_answer_ns_cannot_use_leaves_the_unit_as_it_was() {
    let repo = Repo::new();
    for reply in [
        "No route line.",
        "Text.\nLABELS: status:ready-for-agent\nROUTE: build",
    ] {
        triage(&repo, reply, &[]).failure();
        let log = gh_log(&repo);
        assert!(!log.contains("issue comment"), "{log}");
        assert!(!log.contains("issue edit"), "{log}");
    }
}

#[test]
fn a_unit_whose_author_turns_out_not_to_be_on_the_team_is_not_triaged() {
    let repo = Repo::new();
    triage_by(&repo, "Text.\nROUTE: build", "NONE", &[]).failure();
    let log = gh_log(&repo);
    assert!(
        !log.contains("issue comment") && !log.contains("issue edit"),
        "{log}"
    );
}
