use assert_cmd::Command;

#[test]
fn the_version_flag_prints_the_version_outside_a_repository() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("ns")
        .unwrap()
        .arg("--version")
        .current_dir(tmp.path())
        .env_remove("NS_DISPATCHED")
        .assert()
        .success()
        .stdout(format!("ns {}\n", env!("CARGO_PKG_VERSION")));
}
