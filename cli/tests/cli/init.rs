use crate::common::{script, Repo};

use assert_cmd::Command;

/// A `gh` that has the labels listed in `labels.json` next to it, and logs every call.
fn gh(repo: &Repo) -> std::path::PathBuf {
    let tmp = repo.path.parent().unwrap();
    let bin = tmp.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let have = tmp.join("labels.json");
    if !have.exists() {
        std::fs::write(&have, "[]").unwrap();
    }
    script(
        &bin,
        "gh",
        &format!(
            "echo \"$@\" >> \"{log}\"\ncase \"$1 $2\" in\n  \"label list\") cat \"{have}\";;\n  \"api orgs/{{owner}}/issue-types\") cat \"{types}\";;\n  \"label create\") if [ -e \"{fail}\" ]; then echo denied >&2; exit 1; fi;;\nesac\nexit 0",
            log = tmp.join("gh.log").display(),
            have = have.display(),
            fail = tmp.join("gh-fails").display(),
            types = tmp.join("types.json").display(),
        ),
    );
    bin
}

fn gh_log(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.parent().unwrap().join("gh.log")).unwrap_or_default()
}

fn init_cmd(repo: &Repo, dir: &std::path::Path) -> Command {
    let bin = gh(repo);
    let mut cmd = Command::cargo_bin("ns").unwrap();
    cmd.arg("init")
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(dir);
    cmd
}

fn init_at(repo: &Repo, dir: &std::path::Path) -> serde_json::Value {
    let out = init_cmd(repo, dir).assert().success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

fn init_in(repo: &Repo) -> serde_json::Value {
    init_at(repo, &repo.path)
}

fn manifest(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.join(".nightshift/nightshift.toml")).unwrap()
}

#[test]
fn init_pins_the_repository_to_this_version_with_the_claude_harness() {
    let repo = Repo::new();
    let report = init_in(&repo);
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["harness"], serde_json::json!(["claude"]));
    let text = manifest(&repo);
    assert!(text.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))));
    assert!(text.contains("harness = [\"claude\"]"));
}

#[test]
fn init_run_again_changes_nothing() {
    let repo = Repo::new();
    init_in(&repo);
    let once = manifest(&repo);
    init_in(&repo);
    assert_eq!(manifest(&repo), once);
}

#[test]
fn init_works_from_inside_the_repository() {
    let repo = Repo::new();
    let inner = repo.path.join("a/b");
    std::fs::create_dir_all(&inner).unwrap();
    init_at(&repo, &inner);
    assert!(repo.path.join(".nightshift/nightshift.toml").is_file());
    assert!(!inner.join(".nightshift").exists());
}

#[test]
fn the_rest_of_the_manifest_is_left_as_it_was() {
    let repo = Repo::new();
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    let mine = "# my factory\nname = \"mine\"\n";
    std::fs::write(repo.path.join(".nightshift/nightshift.toml"), mine).unwrap();
    init_in(&repo);
    assert!(manifest(&repo).starts_with(mine.trim_end()));
}

#[test]
fn the_harnesses_a_repository_names_are_kept() {
    let repo = Repo::new();
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "harness = [\"claude\"]\n",
    )
    .unwrap();
    assert_eq!(init_in(&repo)["harness"], serde_json::json!(["claude"]));
}

fn settings(repo: &Repo) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(repo.path.join(".claude/settings.json")).unwrap())
        .unwrap()
}

#[test]
fn init_writes_the_skills_and_a_hook_that_keeps_them_current_in_a_session() {
    let repo = Repo::new();
    let report = init_in(&repo);
    assert_eq!(report["skills_written"], 9);
    assert!(repo.path.join(".claude/skills/ns-build/SKILL.md").is_file());
    let hook = &settings(&repo)["hooks"]["SessionStart"][0]["hooks"][0];
    assert_eq!(hook["type"], "command");
    assert!(hook["command"].as_str().unwrap().contains("ns sync"));
}

#[test]
fn the_hook_is_added_once_and_the_rest_of_the_settings_stay() {
    let repo = Repo::new();
    std::fs::create_dir_all(repo.path.join(".claude")).unwrap();
    std::fs::write(
        repo.path.join(".claude/settings.json"),
        r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
    )
    .unwrap();
    assert_eq!(init_in(&repo)["hooks"][0]["written"], true);
    assert_eq!(init_in(&repo)["hooks"][0]["written"], false);
    let settings = settings(&repo);
    assert_eq!(settings["permissions"]["allow"][0], "Bash(ls)");
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn the_skills_init_writes_are_not_changes_but_the_manifest_and_hook_are() {
    let repo = Repo::new();
    init_in(&repo);
    let status = crate::common::git(&repo.path, &["status", "--porcelain", "-uall"]);
    assert_eq!(
        status.lines().map(|l| &l[3..]).collect::<Vec<_>>(),
        [".claude/settings.json", ".nightshift/nightshift.toml"]
    );
}

#[test]
fn init_creates_the_labels_the_forge_lacks_and_only_those() {
    let repo = Repo::new();
    gh(&repo);
    std::fs::write(
        repo.path.parent().unwrap().join("labels.json"),
        r#"[{"name":"status:wontfix"},{"name":"type:feat"}]"#,
    )
    .unwrap();
    let report = init_in(&repo);
    let created = report["labels_created"].as_array().unwrap();
    assert_eq!(created.len(), 24 - 2);
    assert!(!created.iter().any(|l| l == "status:wontfix"));
    let log = gh_log(&repo);
    assert_eq!(log.matches("label create").count(), 22, "{log}");
    assert!(
        log.contains("label create status:ready-for-agent --description"),
        "{log}"
    );
    assert!(
        log.contains("label create needs-human-merge --description"),
        "{log}"
    );
    assert!(!log.contains("label create type:feat"), "{log}");
    assert!(log.contains("label create type:fix --description"), "{log}");
    assert!(
        log.contains("label create priority:high --description"),
        "{log}"
    );
    assert!(!log.contains("label create area:"), "{log}");
}

#[test]
fn on_a_repository_with_native_types_init_skips_the_feat_and_fix_labels() {
    let repo = Repo::new();
    gh(&repo);
    std::fs::write(
        repo.path.parent().unwrap().join("types.json"),
        r#"[{"name":"Task"},{"name":"Bug"},{"name":"Feature"}]"#,
    )
    .unwrap();
    let report = init_in(&repo);
    assert_eq!(report["labels_created"].as_array().unwrap().len(), 24 - 2);
    let log = gh_log(&repo);
    assert!(!log.contains("label create type:feat"), "{log}");
    assert!(!log.contains("label create type:fix"), "{log}");
    assert!(
        log.contains("label create type:refactor --description"),
        "{log}"
    );
    assert!(!log.contains("PATCH"), "{log}");
}

#[test]
fn init_run_again_with_every_label_there_creates_none() {
    let repo = Repo::new();
    let all = [
        "status:needs-triage",
        "status:needs-define",
        "status:needs-repro",
        "status:needs-info",
        "status:ready-for-agent",
        "status:in-progress",
        "status:ready-for-human",
        "status:wontfix",
        "type:feat",
        "type:fix",
        "type:docs",
        "type:refactor",
        "type:test",
        "type:perf",
        "type:ci",
        "type:build",
        "type:chore",
        "type:style",
        "type:revert",
        "priority:critical",
        "priority:high",
        "priority:medium",
        "priority:low",
        "needs-human-merge",
    ]
    .iter()
    .map(|name| format!(r#"{{"name":"{name}"}}"#))
    .collect::<Vec<_>>()
    .join(",");
    gh(&repo);
    std::fs::write(
        repo.path.parent().unwrap().join("labels.json"),
        format!("[{all}]"),
    )
    .unwrap();
    assert_eq!(
        init_in(&repo)["labels_created"].as_array().unwrap().len(),
        0
    );
    assert!(!gh_log(&repo).contains("label create"));
}

#[test]
fn labels_that_cannot_be_created_fail_init_after_it_wrote_the_rest() {
    let repo = Repo::new();
    gh(&repo);
    std::fs::write(repo.path.parent().unwrap().join("gh-fails"), "").unwrap();
    let assert = init_cmd(&repo, &repo.path).assert().failure();
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        err.contains("labels were not created") && err.contains("denied"),
        "{err}"
    );
    assert!(repo.path.join(".nightshift/nightshift.toml").is_file());
}
