use assert_cmd::Command;

use crate::common::Repo;

fn install(repo: &Repo, source: &std::path::Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .args(["install", "--source"])
        .arg(source)
        .env("NS_CONFIG_DIR", &repo.config)
        .assert()
}

#[test]
fn install_links_each_skill_and_can_be_run_again() {
    let repo = Repo::new();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills");
    let first = install(&repo, &source).success();
    let report: serde_json::Value = serde_json::from_slice(&first.get_output().stdout).unwrap();
    assert_eq!(
        report["installed"],
        serde_json::json!(["ns-build", "ns-review", "ns-tdd", "ns-triage", "ns-verify"])
    );
    install(&repo, &source).success();
    let link = repo.config.join("skills/ns-build");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(link.join("SKILL.md").is_file());
}

#[test]
fn install_will_not_replace_a_real_directory() {
    let repo = Repo::new();
    std::fs::remove_file(repo.config.join("skills/ns-build")).unwrap();
    std::fs::create_dir_all(repo.config.join("skills/ns-build")).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills");
    install(&repo, &source).failure();
}
