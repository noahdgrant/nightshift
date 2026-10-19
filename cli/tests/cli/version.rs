use assert_cmd::Command;

#[test]
fn the_version_flag_prints_the_version() {
    Command::cargo_bin("ns")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("ns {}\n", env!("CARGO_PKG_VERSION")));
}
