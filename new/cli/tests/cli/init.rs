use crate::common::Repo;

use assert_cmd::Command;

fn init(repo: &Repo, extra: &[&str]) -> serde_json::Value {
    let out = Command::cargo_bin("ns")
        .unwrap()
        .arg("init")
        .args(extra)
        .current_dir(&repo.path)
        .assert()
        .success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

fn results(report: &serde_json::Value) -> Vec<String> {
    report["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["result"].as_str().unwrap().to_string())
        .collect()
}

fn skills_dir(repo: &Repo) -> std::path::PathBuf {
    repo.path.join(".nightshift/skills")
}

fn shipped() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills")
}

#[test]
fn init_copies_every_skill_into_the_repository_as_it_shipped() {
    let repo = Repo::new();
    let report = init(&repo, &[]);
    assert!(results(&report).iter().all(|r| r == "created"));
    for entry in std::fs::read_dir(shipped()).unwrap() {
        let name = entry.unwrap().file_name();
        let shipped = std::fs::read_to_string(shipped().join(&name).join("SKILL.md")).unwrap();
        let copy = std::fs::read_to_string(skills_dir(&repo).join(&name).join("SKILL.md")).unwrap();
        assert_eq!(copy, shipped, "{name:?}");
    }
}

#[test]
fn init_run_again_changes_nothing() {
    let repo = Repo::new();
    init(&repo, &[]);
    let report = init(&repo, &[]);
    assert!(results(&report).iter().all(|r| r == "same"));
}

#[test]
fn a_patched_skill_is_kept_unless_forced() {
    let repo = Repo::new();
    init(&repo, &[]);
    let file = skills_dir(&repo).join("ns-build/SKILL.md");
    std::fs::write(&file, "MY PATCH\n").unwrap();
    let report = init(&repo, &[]);
    assert!(results(&report).contains(&"patched, kept".to_string()));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "MY PATCH\n");
    let report = init(&repo, &["--force"]);
    assert!(results(&report).contains(&"overwritten".to_string()));
    assert_ne!(std::fs::read_to_string(&file).unwrap(), "MY PATCH\n");
}

#[test]
fn init_works_from_inside_the_repository() {
    let repo = Repo::new();
    let inner = repo.path.join("a/b");
    std::fs::create_dir_all(&inner).unwrap();
    Command::cargo_bin("ns")
        .unwrap()
        .arg("init")
        .current_dir(&inner)
        .assert()
        .success();
    assert!(skills_dir(&repo).join("ns-build/SKILL.md").is_file());
    assert!(!inner.join(".nightshift").exists());
}
