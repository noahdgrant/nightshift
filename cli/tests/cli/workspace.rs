use crate::common;

use assert_cmd::Command;
use common::Repo;

fn ns(repo: &Repo, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .args(args)
        .current_dir(&repo.path)
        .assert()
}

#[test]
fn new_creates_the_workspace_on_its_branch_and_is_idempotent() {
    let repo = Repo::new();
    ns(&repo, &["workspace", "new", "u1"]).success();
    assert!(repo.workspace("u1").is_dir());
    assert!(repo.branches().contains(&"ns/u1".to_string()));
    ns(&repo, &["workspace", "new", "u1"]).success();
}

#[test]
fn remove_keeps_the_branch_and_new_checks_it_out_again() {
    let repo = Repo::new();
    repo.standards_commit();
    ns(&repo, &["workspace", "new", "u1"]).success();
    ns(&repo, &["workspace", "remove", "u1"]).success();
    assert!(!repo.workspace("u1").exists());
    assert!(repo.branches().contains(&"ns/u1".to_string()));
    ns(&repo, &["workspace", "new", "u1"]).success();
    assert!(repo.workspace("u1").is_dir());
}

#[test]
fn a_bad_unit_id_is_refused() {
    let repo = Repo::new();
    ns(&repo, &["workspace", "new", "Bad Id"]).failure();
}

#[test]
fn a_new_workspace_has_the_skills_a_session_in_it_finds() {
    let repo = Repo::new();
    repo.standards_commit();
    ns(&repo, &["workspace", "new", "7"]).success();
    let workspace = repo.workspace("7");
    assert!(workspace.join(".claude/skills/ns-build/SKILL.md").is_file());
    assert_eq!(common::git(&workspace, &["status", "--porcelain"]), "");
}
