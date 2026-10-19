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
