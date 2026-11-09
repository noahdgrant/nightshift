use crate::common::Repo;

use assert_cmd::Command;

fn upgrade(repo: &Repo, extra: &[&str]) -> serde_json::Value {
    let out = Command::cargo_bin("ns")
        .unwrap()
        .arg("upgrade")
        .args(extra)
        .current_dir(&repo.path)
        .assert()
        .success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

fn manifest(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.join(".nightshift/nightshift.toml")).unwrap()
}

#[test]
fn upgrade_pins_this_version_and_writes_its_skills() {
    let repo = Repo::new();
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "version = \"0.0.1\"\n",
    )
    .unwrap();
    let report = upgrade(&repo, &[]);
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["synced"], true);
    assert!(manifest(&repo).contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))));
    assert!(repo.path.join(".claude/skills/ns-build/SKILL.md").is_file());
}

#[test]
fn upgrade_to_another_version_pins_it_and_leaves_the_skills_to_that_version() {
    let repo = Repo::new();
    let report = upgrade(&repo, &["9.9.9"]);
    assert_eq!(report["synced"], false);
    assert!(manifest(&repo).contains("version = \"9.9.9\""));
    assert!(!repo.path.join(".claude/skills").exists());
}

fn worktree(repo: &Repo) -> std::path::PathBuf {
    let dir = repo.path.parent().unwrap().join("elsewhere");
    crate::common::git(
        &repo.path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            &dir.to_string_lossy(),
        ],
    );
    dir
}

#[test]
fn upgrade_in_a_worktree_changes_the_worktree_and_names_it() {
    let repo = Repo::new();
    let dir = worktree(&repo);
    let out = Command::cargo_bin("ns")
        .unwrap()
        .args(["upgrade", "9.9.9"])
        .current_dir(&dir)
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(
        report["checkout"],
        dir.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    let pin = std::fs::read_to_string(dir.join(".nightshift/nightshift.toml")).unwrap();
    assert!(pin.contains("version = \"9.9.9\""));
    assert!(!repo.path.join(".nightshift/nightshift.toml").exists());
}

#[test]
fn upgrade_in_the_main_checkout_names_it() {
    let repo = Repo::new();
    let report = upgrade(&repo, &["9.9.9"]);
    assert_eq!(
        report["checkout"],
        repo.path.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert!(manifest(&repo).contains("version = \"9.9.9\""));
}
