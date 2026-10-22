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
"api orgs/{{owner}}/issue-types") cat "{bin}/types.json";;
"api --method") if [ -f "{bin}/patch-fails" ]; then echo denied >&2; exit 1; fi;;
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

/// Gives the repository the native types in `json`, as the forge lists them for an organization.
fn native_types(repo: &Repo, json: &str) {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("types.json"), json).unwrap();
}

const ALL_NATIVE: &str = r#"[{"name":"Task"},{"name":"Bug"},{"name":"Feature"}]"#;

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
fn without_native_types_the_type_is_a_label_and_no_type_is_set() {
    let repo = Repo::new();
    triage(&repo, "Ok.\nLABELS: type:feat\nROUTE: build", &[]).success();
    let log = gh_log(&repo);
    assert!(log.contains("--add-label type:feat,"), "{log}");
    assert!(!log.contains("PATCH"), "{log}");
}

#[test]
fn a_failed_lookup_of_native_types_means_labels() {
    let repo = Repo::new();
    triage(&repo, "Ok.\nLABELS: type:fix\nROUTE: build", &[]).success();
    let log = gh_log(&repo);
    assert!(log.contains("orgs/{owner}/issue-types"), "{log}");
    assert!(log.contains("--add-label type:fix,"), "{log}");
}

#[test]
fn with_native_types_feat_is_a_feature_without_a_label() {
    let repo = Repo::new();
    native_types(&repo, ALL_NATIVE);
    triage(
        &repo,
        "Ok.\nLABELS: type:feat, priority:high\nROUTE: build",
        &[],
    )
    .success();
    let log = gh_log(&repo);
    assert!(
        log.contains("--add-label priority:high,status:ready-for-agent "),
        "{log}"
    );
    assert!(!log.contains("type:feat"), "{log}");
    assert!(
        log.contains("api --method PATCH repos/{owner}/{repo}/issues/4 -f type=Feature"),
        "{log}"
    );
}

#[test]
fn with_native_types_fix_is_a_bug_without_a_label() {
    let repo = Repo::new();
    native_types(&repo, ALL_NATIVE);
    triage(&repo, "Ok.\nLABELS: type:fix\nROUTE: build", &[]).success();
    let log = gh_log(&repo);
    assert!(!log.contains("type:fix"), "{log}");
    assert!(log.contains("-f type=Bug"), "{log}");
}

#[test]
fn with_native_types_another_type_is_a_task_and_keeps_its_label() {
    let repo = Repo::new();
    native_types(&repo, ALL_NATIVE);
    triage(&repo, "Ok.\nLABELS: type:refactor\nROUTE: build", &[]).success();
    let log = gh_log(&repo);
    assert!(log.contains("--add-label type:refactor,"), "{log}");
    assert!(log.contains("-f type=Task"), "{log}");
}

#[test]
fn a_failed_native_type_set_leaves_the_unit_untouched() {
    let repo = Repo::new();
    native_types(&repo, ALL_NATIVE);
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::write(bin.join("patch-fails"), "").unwrap();
    triage(&repo, "Ok.\nLABELS: type:feat\nROUTE: wontfix", &[]).failure();
    let log = gh_log(&repo);
    assert!(!log.contains("issue edit"), "{log}");
    assert!(!log.contains("issue close"), "{log}");
}

#[test]
fn a_native_type_the_organization_removed_falls_back_to_the_label() {
    let repo = Repo::new();
    native_types(&repo, r#"[{"name":"Task"},{"name":"Bug"}]"#);
    triage(&repo, "Ok.\nLABELS: type:feat\nROUTE: build", &[]).success();
    let log = gh_log(&repo);
    assert!(log.contains("--add-label type:feat,"), "{log}");
    assert!(!log.contains("PATCH"), "{log}");
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
