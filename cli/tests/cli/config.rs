use std::path::Path;

use crate::common::{script, Repo};

use assert_cmd::Command;

fn config(file: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .arg("config")
        .args(args)
        .env("NS_CONFIG", file)
        .assert()
}

#[test]
fn the_account_is_recorded_and_the_rest_of_the_config_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("nightshift/config.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "# mine\n[other]\nkey = 1\n").unwrap();
    config(&file, &["forge", "github", "--account", "alice-1"]).success();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# mine\n[other]\nkey = 1\n"), "{text}");
    assert!(
        text.contains("[forge.github]\naccount = \"alice-1\""),
        "{text}"
    );
}

#[test]
fn an_account_that_is_not_a_name_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("config.toml");
    config(&file, &["forge", "github", "--account", "a b; rm"]).failure();
    assert!(!file.exists());
}

/// Runs `ns auto --dry-run`, which asks the forge for the open units, with a `gh` that logs the
/// `GH_TOKEN` each call has, and answers `auth token` with `tok-<account>`.
fn dry_run(config_text: Option<&str>, token_in_env: Option<&str>) -> String {
    let repo = Repo::new();
    let tmp = repo.path.parent().unwrap().to_path_buf();
    let bin = tmp.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "gh",
        &format!(
            "if [ \"$1\" = auth ]; then echo \"tok-$4\"; exit 0; fi\necho \"[${{GH_TOKEN-unset}}]\" >> \"{}\"\necho '[]'",
            tmp.join("tokens.log").display()
        ),
    );
    let file = tmp.join("config.toml");
    if let Some(text) = config_text {
        std::fs::write(&file, text).unwrap();
    }
    let mut cmd = Command::cargo_bin("ns").unwrap();
    cmd.args(["auto", "--dry-run"])
        .env("NS_CONFIG", &file)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env_remove("GH_TOKEN")
        .current_dir(&repo.path);
    if let Some(token) = token_in_env {
        cmd.env("GH_TOKEN", token);
    }
    cmd.assert().success();
    std::fs::read_to_string(tmp.join("tokens.log")).unwrap()
}

#[test]
fn gh_runs_with_the_login_of_the_account_the_config_names() {
    let log = dry_run(Some("[forge.github]\naccount = \"alice\"\n"), None);
    assert!(
        log.lines().all(|l| l == "[tok-alice]") && !log.is_empty(),
        "{log}"
    );
}

#[test]
fn a_gh_token_already_set_wins_over_the_config() {
    let log = dry_run(Some("[forge.github]\naccount = \"alice\"\n"), Some("mine"));
    assert!(
        log.lines().all(|l| l == "[mine]") && !log.is_empty(),
        "{log}"
    );
}

#[test]
fn without_an_account_gh_is_left_as_it_is() {
    let log = dry_run(None, None);
    assert!(
        log.lines().all(|l| l == "[unset]") && !log.is_empty(),
        "{log}"
    );
}
