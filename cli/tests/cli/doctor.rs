use crate::common::{script, Repo};

use assert_cmd::Command;
use serde_json::Value;

const ALL_LABELS: [&str; 23] = [
    "status:needs-triage",
    "status:needs-define",
    "status:needs-repro",
    "status:needs-info",
    "status:ready-for-agent",
    "status:in-progress",
    "status:ready-for-human",
    "status:wontfix",
    "type:feat",
    "type:fix",
    "type:docs",
    "type:refactor",
    "type:test",
    "type:perf",
    "type:ci",
    "type:build",
    "type:chore",
    "type:style",
    "type:revert",
    "priority:critical",
    "priority:high",
    "priority:medium",
    "priority:low",
];

/// A repository whose setup is complete, with a `gh` that is logged in as `octo`, a `claude` that
/// starts, and a config that names no account. Each test breaks one thing.
struct Setup {
    repo: Repo,
}

impl Setup {
    fn new() -> Setup {
        let repo = Repo::new();
        let setup = Setup { repo };
        std::fs::create_dir_all(setup.repo.path.join(".nightshift")).unwrap();
        setup.pin(env!("CARGO_PKG_VERSION"));
        setup.repo.set_check("scripts/ci-local.sh");
        setup.labels(&ALL_LABELS);
        setup.gh_login("octo");
        setup.claude("echo 2.0.0");
        setup
    }

    fn tmp(&self) -> &std::path::Path {
        self.repo.path.parent().unwrap()
    }

    fn bin(&self) -> std::path::PathBuf {
        let bin = self.tmp().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        bin
    }

    fn pin(&self, version: &str) {
        std::fs::write(
            self.repo.path.join(".nightshift/nightshift.toml"),
            format!("version = \"{version}\"\nharness = [\"claude\"]\n"),
        )
        .unwrap();
    }

    fn labels(&self, names: &[&str]) {
        let json: Vec<String> = names
            .iter()
            .map(|n| format!(r#"{{"name":"{n}"}}"#))
            .collect();
        std::fs::write(
            self.tmp().join("labels.json"),
            format!("[{}]", json.join(",")),
        )
        .unwrap();
    }

    fn gh_login(&self, login: &str) {
        script(
            &self.bin(),
            "gh",
            &format!(
                "case \"$1 $2\" in\n  \"label list\") cat \"{}\";;\n  \"api user\") echo '{{\"login\":\"{login}\"}}';;\nesac",
                self.tmp().join("labels.json").display()
            ),
        );
    }

    fn gh_down(&self) {
        script(&self.bin(), "gh", "echo 'bad credentials' >&2; exit 1");
    }

    fn claude(&self, body: &str) {
        script(&self.bin(), "claude", body);
    }

    fn account(&self, name: &str) {
        std::fs::write(
            self.tmp().join("config.toml"),
            format!("[forge.github]\naccount = \"{name}\"\n"),
        )
        .unwrap();
    }

    fn doctor(&self) -> (bool, Value) {
        let out = Command::cargo_bin("ns")
            .unwrap()
            .arg("doctor")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("NS_CONFIG", self.tmp().join("config.toml"))
            .env_remove("GH_TOKEN")
            .current_dir(&self.repo.path)
            .output()
            .unwrap();
        let ok = out.status.success();
        (ok, serde_json::from_slice(&out.stdout).unwrap())
    }
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()
}

/// Asserts that only the check `name` failed, and that its detail contains `fix`.
fn only_fails(setup: &Setup, name: &str, fix: &str) {
    let (ok, report) = setup.doctor();
    assert!(!ok, "{report}");
    assert_eq!(report["ok"], false);
    for c in report["checks"].as_array().unwrap() {
        assert_eq!(c["ok"], c["name"] != name, "{report}");
    }
    let detail = check(&report, name)["detail"].as_str().unwrap();
    assert!(detail.contains(fix), "{detail}");
}

#[test]
fn a_complete_setup_passes_every_check() {
    let setup = Setup::new();
    let (ok, report) = setup.doctor();
    assert!(ok, "{report}");
    assert_eq!(report["ok"], true);
    let names: Vec<_> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["pin", "harness", "forge", "checks", "labels"]);
    assert_eq!(check(&report, "pin")["detail"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_pin_to_another_version_fails_and_says_to_upgrade() {
    let setup = Setup::new();
    setup.pin("9.9.9");
    only_fails(&setup, "pin", "ns upgrade");
}

#[test]
fn a_repository_that_is_not_pinned_fails_the_pin_and_says_to_init() {
    let setup = Setup::new();
    std::fs::write(
        setup.repo.path.join(".nightshift/nightshift.toml"),
        "harness = [\"claude\"]\n",
    )
    .unwrap();
    only_fails(&setup, "pin", "ns init");
}

#[test]
fn a_harness_that_cannot_be_started_fails() {
    let setup = Setup::new();
    setup.claude("exit 1");
    only_fails(&setup, "harness", "claude cannot be started");
}

#[test]
fn a_harness_the_manifest_names_wrongly_fails_only_the_harness() {
    let setup = Setup::new();
    std::fs::write(
        setup.repo.path.join(".nightshift/nightshift.toml"),
        format!(
            "version = \"{}\"\nharness = [\"vim\"]\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    only_fails(&setup, "harness", "no harness named vim");
}

#[test]
fn a_forge_that_does_not_answer_fails_and_says_how_to_name_the_account() {
    let setup = Setup::new();
    setup.gh_down();
    let (ok, report) = setup.doctor();
    assert!(!ok);
    let forge = check(&report, "forge");
    assert_eq!(forge["ok"], false);
    assert!(forge["detail"]
        .as_str()
        .unwrap()
        .contains("ns config forge github --account"));
}

#[test]
fn the_forge_login_must_be_the_account_the_config_names() {
    let setup = Setup::new();
    setup.account("someone-else");
    // `gh auth token` is answered by the fake as an api call that is not `api user`.
    only_fails(&setup, "forge", "ns config forge github --account");
}

#[test]
fn the_forge_passes_when_the_login_is_the_account_the_config_names() {
    let setup = Setup::new();
    setup.account("octo");
    let (ok, report) = setup.doctor();
    assert!(ok, "{report}");
    assert_eq!(check(&report, "forge")["detail"], "octo");
}

#[test]
fn an_empty_checks_file_fails_and_says_where_to_put_the_command() {
    let setup = Setup::new();
    std::fs::write(setup.repo.path.join(".nightshift/check"), "\n").unwrap();
    only_fails(&setup, "checks", ".nightshift/check");
}

#[test]
fn a_missing_label_fails_and_says_to_init() {
    let setup = Setup::new();
    setup.labels(&ALL_LABELS[1..]);
    only_fails(&setup, "labels", "status:needs-triage");
    let (_, report) = setup.doctor();
    assert!(check(&report, "labels")["detail"]
        .as_str()
        .unwrap()
        .contains("ns init"));
}

#[test]
fn doctor_changes_nothing() {
    let setup = Setup::new();
    setup.pin("9.9.9");
    setup.doctor();
    let status = crate::common::git(&setup.repo.path, &["status", "--porcelain", "-uall"]);
    assert_eq!(status.trim(), "?? .nightshift/nightshift.toml");
}

#[test]
fn an_unreadable_manifest_fails_the_pin_and_the_harness_and_says_to_init() {
    let setup = Setup::new();
    std::fs::write(
        setup.repo.path.join(".nightshift/nightshift.toml"),
        "= not toml",
    )
    .unwrap();
    let (ok, report) = setup.doctor();
    assert!(!ok);
    for name in ["pin", "harness"] {
        let c = check(&report, name);
        assert_eq!(c["ok"], false, "{report}");
        assert!(
            c["detail"].as_str().unwrap().contains("ns init"),
            "{report}"
        );
    }
}

#[test]
fn an_unreadable_config_fails_the_forge_and_the_labels_and_says_how_to_fix_it() {
    let setup = Setup::new();
    std::fs::write(setup.tmp().join("config.toml"), "= not toml").unwrap();
    let (ok, report) = setup.doctor();
    assert!(!ok);
    let forge = check(&report, "forge");
    assert_eq!(forge["ok"], false, "{report}");
    assert!(forge["detail"]
        .as_str()
        .unwrap()
        .contains("ns config forge github --account"));
    let labels = check(&report, "labels");
    assert_eq!(labels["ok"], false, "{report}");
    assert!(labels["detail"].as_str().unwrap().contains("ns init"));
}

#[test]
fn an_unreadable_checks_file_fails_and_says_what_to_put_there() {
    let setup = Setup::new();
    let path = setup.repo.path.join(".nightshift/check");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    only_fails(&setup, "checks", "make .nightshift/check a file");
}

#[test]
fn a_forge_that_names_no_login_fails_and_says_how_to_name_the_account() {
    let setup = Setup::new();
    script(&setup.bin(), "gh", "echo '{}'");
    let (ok, report) = setup.doctor();
    assert!(!ok);
    let forge = check(&report, "forge");
    assert_eq!(forge["ok"], false);
    let detail = forge["detail"].as_str().unwrap();
    assert!(detail.contains("did not name the login"), "{detail}");
    assert!(
        detail.contains("ns config forge github --account"),
        "{detail}"
    );
}

#[test]
fn a_forge_that_prints_invalid_json_fails_the_forge() {
    let setup = Setup::new();
    script(&setup.bin(), "gh", "echo 'nope'");
    let (_, report) = setup.doctor();
    let detail = check(&report, "forge")["detail"].as_str().unwrap();
    assert_eq!(check(&report, "forge")["ok"], false);
    assert!(detail.contains("invalid JSON"), "{detail}");
}

#[test]
fn labels_that_cannot_be_listed_fail_and_say_to_fix_the_forge_first() {
    let setup = Setup::new();
    script(
        &setup.bin(),
        "gh",
        "case \"$1 $2\" in\n  \"api user\") echo '{\"login\":\"octo\"}';;\n  *) echo 'boom' >&2; exit 1;;\nesac",
    );
    only_fails(
        &setup,
        "labels",
        "fix the forge check first, then run `ns init`",
    );
}

#[test]
fn every_harness_is_checked_and_the_first_that_fails_is_named() {
    let setup = Setup::new();
    std::fs::write(
        setup.repo.path.join(".nightshift/nightshift.toml"),
        format!(
            "version = \"{}\"\nharness = [\"claude\", \"claude\"]\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    let (ok, report) = setup.doctor();
    assert!(ok, "{report}");
    assert_eq!(
        check(&report, "harness")["detail"],
        "claude 2.0.0, claude 2.0.0"
    );
    setup.claude("exit 1");
    only_fails(&setup, "harness", "claude cannot be started");
}
