//! The entry points that keep an inherited git location (`GIT_DIR` and the rest) from reaching
//! a git process (#120, #638): `ns` clears them at the top of `main`, the shared test helpers
//! refuse to run with one set, and the repo's scripts and hooks unset them first.

mod common;

const MARK: &str = "NS_TEST_GIT_LOCATION_PROBE";

#[test]
fn command_probe() {
    if std::env::var_os(MARK).is_none() {
        return;
    }
    common::command("git").arg("--version").output();
}

#[test]
fn command_panics_naming_each_git_location_variable_the_test_process_has() {
    let out = common::command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "command_probe",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(MARK, "1")
        .env("GIT_DIR", "/nonexistent")
        .env("GIT_PREFIX", "")
        .output();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains(r#"git location variables set: ["GIT_DIR", "GIT_PREFIX"]"#),
        "{text}"
    );
}
