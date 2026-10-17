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

fn manifest(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.join(".nightshift/nightshift.toml")).unwrap()
}

fn recorded(repo: &Repo, skill: &str) -> String {
    manifest(repo)
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{skill} = ")))
        .unwrap()
        .trim_matches('"')
        .to_string()
}

fn hash_of(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

#[test]
fn init_records_the_version_and_what_it_copied_in_the_manifest() {
    let repo = Repo::new();
    init(&repo, &[]);
    let text = manifest(&repo);
    assert!(text.contains("[skills]"), "{text}");
    assert!(text.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))));
    let shipped = std::fs::read_to_string(shipped().join("ns-build/SKILL.md")).unwrap();
    assert_eq!(recorded(&repo, "ns-build"), hash_of(&shipped));
}

#[test]
fn a_skill_the_repository_never_touched_is_updated_to_this_version() {
    let repo = Repo::new();
    init(&repo, &[]);
    // As an older `ns` left it: a different text, recorded as what it copied.
    let file = skills_dir(&repo).join("ns-build/SKILL.md");
    std::fs::write(&file, "OLDER VERSION\n").unwrap();
    let text = manifest(&repo).replace(&recorded(&repo, "ns-build"), &hash_of("OLDER VERSION\n"));
    std::fs::write(repo.path.join(".nightshift/nightshift.toml"), text).unwrap();
    let report = init(&repo, &[]);
    assert!(results(&report).contains(&"updated".to_string()));
    let shipped = std::fs::read_to_string(shipped().join("ns-build/SKILL.md")).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), shipped);
}

#[test]
fn a_patch_stays_a_patch_across_runs_even_when_ns_is_newer() {
    let repo = Repo::new();
    init(&repo, &[]);
    let before = recorded(&repo, "ns-build");
    std::fs::write(skills_dir(&repo).join("ns-build/SKILL.md"), "MY PATCH\n").unwrap();
    for _ in 0..2 {
        let report = init(&repo, &[]);
        assert!(results(&report).contains(&"patched, kept".to_string()));
    }
    assert_eq!(recorded(&repo, "ns-build"), before);
}

#[test]
fn the_rest_of_the_manifest_is_left_as_it_was() {
    let repo = Repo::new();
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    let mine = "# my factory\nname = \"mine\"\n";
    std::fs::write(repo.path.join(".nightshift/nightshift.toml"), mine).unwrap();
    init(&repo, &[]);
    assert!(manifest(&repo).starts_with(mine.trim_end()));
    assert!(manifest(&repo).contains("[skills]"));
}
