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

fn sync_dev(repo: &Repo) {
    Command::cargo_bin("ns")
        .unwrap()
        .arg("sync")
        .env("NS_DEV", "1")
        .current_dir(&repo.path)
        .assert()
        .success();
}

fn source_skill(repo: &Repo, name: &str, text: &str) {
    let dir = repo.path.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), text).unwrap();
}

#[test]
fn in_development_a_skill_is_a_link_to_the_repositorys_own_so_an_edit_shows_at_once() {
    let repo = Repo::new();
    source_skill(&repo, "ns-build", "FIRST\n");
    sync_dev(&repo);
    let link = repo.path.join(".claude/skills/ns-build");
    assert!(link.symlink_metadata().unwrap().is_symlink());
    assert_eq!(
        std::fs::read_to_string(link.join("SKILL.md")).unwrap(),
        "FIRST\n"
    );
    source_skill(&repo, "ns-build", "SECOND\n");
    assert_eq!(
        std::fs::read_to_string(link.join("SKILL.md")).unwrap(),
        "SECOND\n"
    );
    assert_eq!(
        git(&repo.path, &["status", "--porcelain", "-uall"]).trim(),
        "?? skills/ns-build/SKILL.md"
    );
}

#[test]
fn a_skill_the_repository_has_no_source_for_is_still_written_in_development() {
    let repo = Repo::new();
    source_skill(&repo, "ns-build", "FIRST\n");
    sync_dev(&repo);
    let tdd = repo.path.join(".claude/skills/ns-tdd");
    assert!(!tdd.symlink_metadata().unwrap().is_symlink());
    assert_eq!(
        std::fs::read_to_string(tdd.join("SKILL.md")).unwrap(),
        shipped("ns-tdd")
    );
}

#[test]
fn leaving_development_replaces_the_links_and_never_writes_through_them() {
    let repo = Repo::new();
    source_skill(&repo, "ns-triage", "SOURCE\n");
    sync_dev(&repo);
    sync_in(&repo.path);
    let triage = repo.path.join(".claude/skills/ns-triage");
    assert!(!triage.symlink_metadata().unwrap().is_symlink());
    assert_eq!(
        std::fs::read_to_string(triage.join("SKILL.md")).unwrap(),
        crate_file_for_session("ns-triage")
    );
    assert_eq!(
        std::fs::read_to_string(repo.path.join("skills/ns-triage/SKILL.md")).unwrap(),
        "SOURCE\n"
    );
}

#[test]
fn without_a_skills_directory_ns_dev_changes_nothing() {
    let repo = Repo::new();
    sync_dev(&repo);
    let build = repo.path.join(".claude/skills/ns-build");
    assert!(!build.symlink_metadata().unwrap().is_symlink());
}

fn crate_file_for_session(name: &str) -> String {
    let text = shipped(name);
    text.replacen("\n---\n", "\ndisable-model-invocation: true\n---\n", 1)
}
