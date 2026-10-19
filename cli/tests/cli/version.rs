use assert_cmd::Command;

#[test]
fn version_prints_json() {
    Command::cargo_bin("ns")
        .unwrap()
        .arg("version")
        .assert()
        .success()
        .stdout("{\"version\":\"0.1.0\"}\n");
}
