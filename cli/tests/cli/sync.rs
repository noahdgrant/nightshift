use crate::common::{git, Repo};

use assert_cmd::Command;

fn sync_in(dir: &std::path::Path) -> serde_json::Value {
    let out = Command::cargo_bin("ns")
        .unwrap()
        .arg("sync")
        .current_dir(dir)
        .assert()
        .success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

fn skill_file(repo: &Repo, name: &str) -> std::path::PathBuf {
    repo.path.join(".claude/skills").join(name).join("SKILL.md")
}

fn shipped(name: &str) -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills");
    std::fs::read_to_string(dir.join(name).join("SKILL.md")).unwrap()
}

#[test]
fn sync_writes_every_skill_where_claude_finds_it() {
    let repo = Repo::new();
    let report = sync_in(&repo.path);
    assert_eq!(report["synced"][0]["harness"], "claude");
    assert_eq!(report["synced"][0]["dir"], ".claude/skills");
    assert_eq!(report["synced"][0]["written"], 9);
    assert_eq!(
        std::fs::read_to_string(skill_file(&repo, "ns-grilling")).unwrap(),
        shipped("ns-grilling")
    );
}

#[test]
fn a_state_skill_runs_only_when_the_user_types_its_name() {
    let repo = Repo::new();
    sync_in(&repo.path);
    for name in ["ns-triage", "ns-verify", "ns-review"] {
        let text = std::fs::read_to_string(skill_file(&repo, name)).unwrap();
        assert!(
            text.contains("\ndisable-model-invocation: true\n"),
            "{name}"
        );
    }
    let build = std::fs::read_to_string(skill_file(&repo, "ns-build")).unwrap();
    assert!(!build.contains("disable-model-invocation"));
}

#[test]
fn sync_run_again_writes_nothing_and_repairs_an_edited_skill() {
    let repo = Repo::new();
    sync_in(&repo.path);
    assert_eq!(sync_in(&repo.path)["synced"][0]["written"], 0);
    std::fs::write(skill_file(&repo, "ns-tdd"), "EDITED\n").unwrap();
    assert_eq!(sync_in(&repo.path)["synced"][0]["written"], 1);
    assert_eq!(
        std::fs::read_to_string(skill_file(&repo, "ns-tdd")).unwrap(),
        shipped("ns-tdd")
    );
}

#[test]
fn the_skills_are_not_changes_to_the_repository() {
    let repo = Repo::new();
    sync_in(&repo.path);
    assert_eq!(git(&repo.path, &["status", "--porcelain"]), "");
}

#[test]
fn a_skill_of_the_users_own_is_left_alone_and_one_this_version_lacks_is_removed() {
    let repo = Repo::new();
    sync_in(&repo.path);
    let mine = repo.path.join(".claude/skills/my-skill");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("SKILL.md"), "MINE\n").unwrap();
    let old = repo.path.join(".claude/skills/ns-gone");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("SKILL.md"), "OLD\n").unwrap();
    let report = sync_in(&repo.path);
    assert_eq!(report["synced"][0]["removed"], 1);
    assert!(mine.join("SKILL.md").is_file());
    assert!(!old.exists());
}

#[test]
fn a_workspace_of_the_repository_ignores_the_skills_too() {
    let repo = Repo::new();
    sync_in(&repo.path);
    let workspace = repo.path.parent().unwrap().join("other");
    git(
        &repo.path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            &workspace.to_string_lossy(),
        ],
    );
    sync_in(&workspace);
    assert!(workspace.join(".claude/skills/ns-build/SKILL.md").is_file());
    assert_eq!(git(&workspace, &["status", "--porcelain"]), "");
}
