use crate::common::{script, Repo};

use assert_cmd::Command;

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

/// Runs `ns triage 4` with `extra` arguments. The fake `gh` knows unit 4, which
/// needs triage, and the fake `claude` answers `reply`.
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
    let read = unit(4, &["status:needs-triage"], association);
    script(
        &bin,
        "gh",
        &format!(
            r#"echo "$@" >> "{bin}/gh.log"
case "$1 $2" in
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
        .args(["triage", "4"])
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

fn gh_log(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/gh.log")).unwrap()
}

#[test]
fn a_unit_triaged_to_build_is_made_ready_and_nothing_is_posted() {
    let repo = Repo::new();
    let reply = "Clear and small.\n\nLABELS: type:feat, priority:high\nROUTE: build";
    let out = triage(&repo, reply, &[]).success();
    let event: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(event["unit"], 4);
    assert_eq!(event["route"], "build");
    assert_eq!(event["why"], "Clear and small.");
    let log = gh_log(&repo);
    assert!(
        log.contains(
            "issue edit 4 --add-label type:feat,priority:high,status:ready-for-agent --remove-label status:needs-triage"
        ),
        "{log}"
    );
    assert!(
        !log.contains("comment") && !log.contains("issue close"),
        "{log}"
    );
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

/// Runs `ns triage` with `extra` arguments against a fake `gh` that lists units 7 and 8 as
/// untriaged, 9 as ready and 6 as the one a stranger filed, and a fake `claude` that answers
/// `reply` to every unit.
fn triage_many(repo: &Repo, reply: &str, extra: &[&str]) -> assert_cmd::assert::Assert {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let list = [
        unit(8, &[], "OWNER"),
        unit(7, &["status:needs-triage"], "OWNER"),
        unit(9, &["status:ready-for-agent"], "OWNER"),
        unit(6, &[], "NONE"),
    ]
    .join(",");
    script(
        &bin,
        "gh",
        &format!(
            r#"echo "$@" >> "{bin}/gh.log"
case "$1 $2" in
"api --paginate") echo '[[{list}]]';;
"api repos/{{owner}}/{{repo}}/issues/7") echo '{seven}';;
"api repos/{{owner}}/{{repo}}/issues/8") echo '{eight}';;
esac"#,
            bin = bin.display(),
            seven = unit(7, &["status:needs-triage"], "OWNER"),
            eight = unit(8, &[], "OWNER"),
        ),
    );
    script(
        &bin,
        "claude",
        &format!("cat >/dev/null\ncat <<'EOF'\n{reply}\nEOF"),
    );
    Command::cargo_bin("ns")
        .unwrap()
        .arg("triage")
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

fn triaged(out: &assert_cmd::assert::Assert) -> Vec<u64> {
    String::from_utf8(out.get_output().stdout.clone())
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["unit"]
                .as_u64()
                .unwrap()
        })
        .collect()
}

#[test]
fn without_a_unit_the_first_untriaged_unit_is_triaged() {
    let repo = Repo::new();
    let out = triage_many(&repo, "Text.\nROUTE: build", &[]).success();
    assert_eq!(triaged(&out), [7]);
}

#[test]
fn max_units_triages_that_many_untriaged_units_in_order() {
    let repo = Repo::new();
    let out = triage_many(&repo, "Text.\nROUTE: build", &["--max-units", "2"]).success();
    assert_eq!(triaged(&out), [7, 8]);
    let log = gh_log(&repo);
    assert!(
        !log.contains("issue edit 9") && !log.contains("issue edit 6"),
        "{log}"
    );
}

#[test]
fn all_triages_every_untriaged_unit() {
    let repo = Repo::new();
    let out = triage_many(&repo, "Text.\nROUTE: build", &["--all"]).success();
    assert_eq!(triaged(&out), [7, 8]);
}

#[test]
fn a_unit_that_cannot_be_triaged_does_not_stop_the_others() {
    let repo = Repo::new();
    let out = triage_many(&repo, "No route line.", &["--all"]).failure();
    let log = gh_log(&repo);
    assert!(!log.contains("issue edit"), "{log}");
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(
        err.contains("unit 7 was not triaged") && err.contains("unit 8 was not triaged"),
        "{err}"
    );
}

#[test]
fn a_unit_number_cannot_be_combined_with_a_count() {
    let repo = Repo::new();
    triage_many(&repo, "Text.\nROUTE: build", &["7", "--all"]).failure();
    triage_many(&repo, "Text.\nROUTE: build", &["7", "--max-units", "2"]).failure();
}

#[test]
fn all_and_a_count_exclude_each_other() {
    let repo = Repo::new();
    triage_many(&repo, "Text.\nROUTE: build", &["--all", "--max-units", "2"]).failure();
}

/// Runs `ns triage 4` against a fake `gh` where units 4, 3 and 5 are open and issue 6 is closed,
/// and a fake `claude` that answers `reply`. `list` and `post` are shell commands that stand for
/// the forge's answer to listing the open units and to recording a blocker.
fn triage_blocked(repo: &Repo, reply: &str, list: &str, post: &str) -> assert_cmd::assert::Assert {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "gh",
        &format!(
            r#"echo "$@" >> "{bin}/gh.log"
case "$1 $2" in
"api repos/{{owner}}/{{repo}}/issues/4") echo '{four}';;
"api repos/{{owner}}/{{repo}}/issues/3") echo '{{"id":333,"number":3}}';;
"api repos/{{owner}}/{{repo}}/issues/5") echo '{{"id":555,"number":5}}';;
"api --paginate") {list};;
"api --method") {post};;
esac"#,
            bin = bin.display(),
            four = unit(4, &["status:needs-triage"], "OWNER"),
        ),
    );
    script(
        &bin,
        "claude",
        &format!("cat >/dev/null\ncat <<'EOF'\n{reply}\nEOF"),
    );
    Command::cargo_bin("ns")
        .unwrap()
        .args(["triage", "4"])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

fn open_units() -> String {
    format!(
        "echo '[[{}]]'",
        [
            unit(4, &[], "OWNER"),
            unit(3, &[], "OWNER"),
            unit(5, &[], "OWNER")
        ]
        .join(",")
    )
}

fn blocked_by(out: &assert_cmd::assert::Assert) -> serde_json::Value {
    let event: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    event["blocked_by"].clone()
}

fn stderr(out: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(out.get_output().stderr.clone()).unwrap()
}

#[test]
fn each_listed_blocker_is_recorded_and_printed() {
    let repo = Repo::new();
    let out = triage_blocked(
        &repo,
        "Needs two others.\nBLOCKED_BY: 3, 5\nROUTE: build",
        &open_units(),
        "true",
    )
    .success();
    assert_eq!(blocked_by(&out), serde_json::json!([3, 5]));
    let log = gh_log(&repo);
    for (blocker, id) in [(3, 333), (5, 555)] {
        let post = format!(
            "api --method POST repos/{{owner}}/{{repo}}/issues/4/dependencies/blocked_by -F issue_id={id}"
        );
        assert!(log.contains(&post), "blocker {blocker}: {log}");
    }
}

#[test]
fn a_reply_without_blockers_records_none() {
    let repo = Repo::new();
    let out = triage_blocked(&repo, "Alone.\nROUTE: build", &open_units(), "true").success();
    assert_eq!(blocked_by(&out), serde_json::json!([]));
    assert!(!gh_log(&repo).contains("dependencies"), "{}", gh_log(&repo));
}

#[test]
fn the_unit_itself_a_closed_issue_and_an_unknown_number_are_ignored_with_a_note() {
    let repo = Repo::new();
    let out = triage_blocked(
        &repo,
        "Text.\nBLOCKED_BY: 4, 6, 99, 3\nROUTE: build",
        &open_units(),
        "true",
    )
    .success();
    assert_eq!(blocked_by(&out), serde_json::json!([3]));
    let (err, log) = (stderr(&out), gh_log(&repo));
    assert!(err.contains("unit 4 cannot be blocked by itself"), "{err}");
    assert!(err.contains("unit 6 is not an open unit"), "{err}");
    assert!(err.contains("unit 99 is not an open unit"), "{err}");
    assert_eq!(log.matches("dependencies").count(), 1, "{log}");
}

#[test]
fn a_blocker_that_is_already_recorded_is_not_an_error() {
    let repo = Repo::new();
    let out = triage_blocked(
        &repo,
        "Text.\nBLOCKED_BY: 3\nROUTE: build",
        &open_units(),
        "echo 'Dependency already exists (HTTP 422)' >&2; exit 1",
    )
    .success();
    assert_eq!(blocked_by(&out), serde_json::json!([3]));
}

#[test]
fn a_blocker_the_forge_refuses_is_noted_and_the_triage_still_succeeds() {
    let repo = Repo::new();
    let out = triage_blocked(
        &repo,
        "Text.\nBLOCKED_BY: 3, 5\nROUTE: build",
        &open_units(),
        "case \"$*\" in *issue_id=333*) echo 'HTTP 500' >&2; exit 1;; esac",
    )
    .success();
    assert_eq!(blocked_by(&out), serde_json::json!([5]));
    assert!(
        stderr(&out).contains("not recorded as blocked by 3"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn when_the_open_units_cannot_be_listed_nothing_is_recorded_and_the_triage_still_succeeds() {
    let repo = Repo::new();
    let out = triage_blocked(
        &repo,
        "Text.\nBLOCKED_BY: 3\nROUTE: build",
        "echo 'HTTP 500' >&2; exit 1",
        "true",
    )
    .success();
    assert_eq!(blocked_by(&out), serde_json::json!([]));
    assert!(
        stderr(&out).contains("was not recorded as blocked"),
        "{}",
        stderr(&out)
    );
    assert!(!gh_log(&repo).contains("dependencies"));
}
