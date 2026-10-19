use std::path::{Path, PathBuf};

use crate::common::{script, Repo};

use assert_cmd::Command;

const OWN: &str = env!("CARGO_PKG_VERSION");

fn ns(repo: &Repo, home: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .args(args)
        .env("NS_HOME", home)
        .env_remove("NS_DISPATCHED")
        .env_remove("NS_DEV")
        .current_dir(&repo.path)
        .assert()
}

fn pin(repo: &Repo, version: &str) {
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        format!("version = \"{version}\"\n"),
    )
    .unwrap();
}

/// A fake installed `ns` of `version` that says it ran, and what it was run with.
fn installed(home: &Path, version: &str) {
    let dir = home.join("versions").join(version);
    std::fs::create_dir_all(&dir).unwrap();
    script(
        &dir,
        "ns",
        &format!("echo \"ns $NS_DISPATCHED {version} $@\""),
    );
}

fn stdout(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

#[test]
fn a_repository_pinned_to_another_version_runs_under_that_version() {
    let repo = Repo::new();
    let home = tempfile::tempdir().unwrap();
    pin(&repo, "9.9.9");
    installed(home.path(), "9.9.9");
    let out = stdout(ns(&repo, home.path(), &["triage", "--help"]).success());
    assert_eq!(out, "ns 1 9.9.9 triage --help\n");
}

#[test]
fn a_pinned_version_that_is_not_installed_says_how_to_install_it() {
    let repo = Repo::new();
    let home = tempfile::tempdir().unwrap();
    pin(&repo, "9.9.9");
    let assert = ns(&repo, home.path(), &["triage", "--help"]).failure();
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        err.contains("pinned to ns 9.9.9") && err.contains("ns self install 9.9.9"),
        "{err}"
    );
}

#[test]
fn the_version_the_repository_is_pinned_to_runs_without_handing_over() {
    let repo = Repo::new();
    let home = tempfile::tempdir().unwrap();
    pin(&repo, OWN);
    ns(&repo, home.path(), &["--version"]).success();
    ns(&repo, home.path(), &["plan", "--help"]).success();
}

#[test]
fn the_commands_that_set_the_pin_run_under_any_version() {
    let repo = Repo::new();
    let home = tempfile::tempdir().unwrap();
    pin(&repo, "9.9.9");
    ns(&repo, home.path(), &["--version"]).success();
    ns(&repo, home.path(), &["upgrade"]).success();
    assert!(
        std::fs::read_to_string(repo.path.join(".nightshift/nightshift.toml"))
            .unwrap()
            .contains(OWN)
    );
}

#[test]
fn ns_dev_runs_this_version_whatever_the_pin() {
    let repo = Repo::new();
    let home = tempfile::tempdir().unwrap();
    pin(&repo, "9.9.9");
    Command::cargo_bin("ns")
        .unwrap()
        .args(["plan", "--help"])
        .env("NS_HOME", home.path())
        .env("NS_DEV", "1")
        .current_dir(&repo.path)
        .assert()
        .success();
}

#[test]
fn outside_a_repository_there_is_no_pin() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("ns")
        .unwrap()
        .args(["plan", "--help"])
        .env("NS_HOME", tmp.path())
        .env_remove("NS_DISPATCHED")
        .current_dir(tmp.path())
        .assert()
        .success();
}

/// A `bin` with a `gh` that serves release 9.9.9: a tarball holding a fake `ns`, and the checksum
/// it is published with, which `checksum` can spoil. It logs its arguments.
fn gh_serving_release(bin: &Path, checksum: &str) -> PathBuf {
    std::fs::create_dir_all(bin).unwrap();
    let stage = bin.parent().unwrap().join("stage");
    let name = "ns-9.9.9-x86_64-unknown-linux-gnu";
    std::fs::create_dir_all(stage.join(name)).unwrap();
    script(&stage.join(name), "ns", "echo installed");
    let tarball = format!("{name}.tar.gz");
    let tar = std::process::Command::new("tar")
        .args(["-czf", &tarball, name])
        .current_dir(&stage)
        .status()
        .unwrap();
    assert!(tar.success());
    let hash = match checksum {
        "right" => {
            let out = std::process::Command::new("sha256sum")
                .arg(&tarball)
                .current_dir(&stage)
                .output()
                .unwrap();
            String::from_utf8(out.stdout)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_string()
        }
        other => other.to_string(),
    };
    std::fs::write(
        stage.join(format!("{tarball}.sha256")),
        format!("{hash}  {tarball}\n"),
    )
    .unwrap();
    script(
        bin,
        "gh",
        &format!(
            "echo \"$@\" >> \"{log}\"\ncp \"{stage}/{tarball}\" \"{stage}/{tarball}.sha256\" .",
            log = bin.join("gh.log").display(),
            stage = stage.display(),
        ),
    );
    bin.join("gh.log")
}

fn self_install(repo: &Repo, home: &Path, bin: &Path) -> assert_cmd::assert::Assert {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    Command::cargo_bin("ns")
        .unwrap()
        .args(["self", "install", "9.9.9"])
        .env("NS_HOME", home)
        .env("PATH", path)
        .current_dir(&repo.path)
        .assert()
}

#[test]
fn self_install_downloads_checks_and_installs_a_version() {
    let repo = Repo::new();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let log = gh_serving_release(&tmp.path().join("bin"), "right");
    self_install(&repo, &home, &tmp.path().join("bin")).success();
    let asked = std::fs::read_to_string(log).unwrap();
    assert!(
        asked.contains("release download v9.9.9 --repo noahdgrant/nightshift"),
        "{asked}"
    );
    let binary = home.join("versions/9.9.9/ns");
    let ran = std::process::Command::new(&binary).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "installed\n");
    assert!(!std::fs::read_dir(&home).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("tmp-")));
}

#[test]
fn a_download_that_does_not_match_its_checksum_is_not_installed() {
    let repo = Repo::new();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    gh_serving_release(&tmp.path().join("bin"), &"0".repeat(64));
    let assert = self_install(&repo, &home, &tmp.path().join("bin")).failure();
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(err.contains("does not match its checksum"), "{err}");
    assert!(!home.join("versions/9.9.9/ns").exists());
}

#[test]
fn a_version_that_is_installed_is_not_downloaded_again() {
    let repo = Repo::new();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    installed(&home, "9.9.9");
    let log = gh_serving_release(&tmp.path().join("bin"), "right");
    self_install(&repo, &home, &tmp.path().join("bin")).success();
    assert!(!log.exists());
}

#[test]
fn a_version_that_is_a_path_is_refused() {
    let repo = Repo::new();
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("ns")
        .unwrap()
        .args(["self", "install", "../evil"])
        .env("NS_HOME", tmp.path())
        .current_dir(&repo.path)
        .assert()
        .failure();
}
