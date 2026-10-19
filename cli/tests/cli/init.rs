use crate::common::Repo;

use assert_cmd::Command;

fn init_in(dir: &std::path::Path) -> serde_json::Value {
    let out = Command::cargo_bin("ns")
        .unwrap()
        .arg("init")
        .current_dir(dir)
        .assert()
        .success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

fn manifest(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.join(".nightshift/nightshift.toml")).unwrap()
}

#[test]
fn init_pins_the_repository_to_this_version_with_the_claude_harness() {
    let repo = Repo::new();
    let report = init_in(&repo.path);
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["harness"], serde_json::json!(["claude"]));
    let text = manifest(&repo);
    assert!(text.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))));
    assert!(text.contains("harness = [\"claude\"]"));
}

#[test]
fn init_run_again_changes_nothing() {
    let repo = Repo::new();
    init_in(&repo.path);
    let once = manifest(&repo);
    init_in(&repo.path);
    assert_eq!(manifest(&repo), once);
}

#[test]
fn init_works_from_inside_the_repository() {
    let repo = Repo::new();
    let inner = repo.path.join("a/b");
    std::fs::create_dir_all(&inner).unwrap();
    init_in(&inner);
    assert!(repo.path.join(".nightshift/nightshift.toml").is_file());
    assert!(!inner.join(".nightshift").exists());
}

#[test]
fn the_rest_of_the_manifest_is_left_as_it_was() {
    let repo = Repo::new();
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    let mine = "# my factory\nname = \"mine\"\n";
    std::fs::write(repo.path.join(".nightshift/nightshift.toml"), mine).unwrap();
    init_in(&repo.path);
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
    assert_eq!(
        init_in(&repo.path)["harness"],
        serde_json::json!(["claude"])
    );
}

fn settings(repo: &Repo) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(repo.path.join(".claude/settings.json")).unwrap())
        .unwrap()
}

#[test]
fn init_writes_the_skills_and_a_hook_that_keeps_them_current_in_a_session() {
    let repo = Repo::new();
    let report = init_in(&repo.path);
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
    assert_eq!(init_in(&repo.path)["hooks"][0]["written"], true);
    assert_eq!(init_in(&repo.path)["hooks"][0]["written"], false);
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
    init_in(&repo.path);
    let status = crate::common::git(&repo.path, &["status", "--porcelain", "-uall"]);
    assert_eq!(
        status.lines().map(|l| &l[3..]).collect::<Vec<_>>(),
        [".claude/settings.json", ".nightshift/nightshift.toml"]
    );
}
