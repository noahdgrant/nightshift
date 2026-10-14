/// An env whose `origin` is the GitHub repo `o/r` and whose fake gh reports its owner as
/// `owner_type`, or prints nothing when `None`.
fn github_owned_by(owner_type: Option<&str>) -> Env {
    let e = Env::new();
    git(
        &e.root,
        &["remote", "set-url", "origin", "https://github.com/o/r.git"],
    );
    if let Some(t) = owner_type {
        e.gh_file("owner-type", &format!("{t}\n"));
    }
    e
}

#[test]
fn factory_validate_warns_when_build_only_runs_on_a_personal_account_repo() {
    let e = github_owned_by(Some("User"));
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["errors"], serde_json::json!([]), "{v}");
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{v}");
    let w = warnings[0].as_str().unwrap();
    assert!(
        w.contains("no GitHub merge queue")
            && w.contains("updates it only when the default branch changed one of its files")
            && w.contains("local = \"every-step\""),
        "{w}"
    );
    assert!(e.gh_calls().contains("api repos/o/r --jq .owner.type"));
}

#[test]
fn factory_validate_looks_up_the_owner_on_the_origin_host() {
    let e = github_owned_by(Some("User"));
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://ghe.example.com/o/r.git",
        ],
    );
    e.factory("[gate]\nlocal = \"build-only\"\n");
    e.run(&["factory", "validate"], 0);
    let calls = e.gh_calls();
    assert!(
        calls.contains("api --hostname ghe.example.com repos/o/r --jq .owner.type"),
        "{calls}"
    );
}

#[test]
fn factory_validate_warns_once_for_a_personal_account_repo_on_another_host() {
    let e = github_owned_by(Some("User"));
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://ghe.example.com/o/r.git",
        ],
    );
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    let owner_warnings = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w.as_str().unwrap().contains("no GitHub merge queue"))
        .count();
    assert_eq!(owner_warnings, 1, "{v}");
    assert_eq!(
        fs::read_to_string(e.ghd.join("hosts")).unwrap().trim(),
        "ghe.example.com"
    );
}

#[test]
fn factory_validate_keeps_an_ambient_gh_host_off_a_github_com_origin() {
    let e = github_owned_by(Some("User"));
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let out = e
        .ns()
        .env("GH_HOST", "other.example.com")
        .args(["factory", "validate"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["warnings"].as_array().unwrap().len(), 1, "{v}");
    let hosts = fs::read_to_string(e.ghd.join("hosts")).unwrap();
    assert_eq!(hosts.trim(), "", "{hosts:?}");
}

#[test]
fn factory_validate_applies_the_configured_token_when_gh_host_is_the_origin_host() {
    let e = github_owned_by(Some("User"));
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://other.example.com/o/r.git",
        ],
    );
    e.config("[forge.github]\nhost = \"ghe.example.com\"\ntoken_env = \"DEFAULT_TOKEN\"\n");
    e.factory("[gate]\nlocal = \"build-only\"\n");
    e.ns()
        .env("DEFAULT_TOKEN", "default-tok")
        .env("GH_HOST", "other.example.com")
        .args(["factory", "validate"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), ["default-tok"], "{}", e.gh_calls());
}

#[test]
fn factory_validate_withholds_the_token_when_gh_host_is_not_the_origin_host() {
    let e = github_owned_by(Some("User"));
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://ghe.example.com/o/r.git",
        ],
    );
    e.config("[forge.github]\nhost = \"ghe.example.com\"\ntoken_env = \"DEFAULT_TOKEN\"\n");
    e.factory("[gate]\nlocal = \"build-only\"\n");
    e.ns()
        .env("DEFAULT_TOKEN", "default-tok")
        .env("GH_HOST", "other.example.com")
        .args(["factory", "validate"])
        .assert()
        .code(0);
    assert!(
        !e.tokens_seen().contains(&"default-tok".to_string()),
        "{}",
        e.gh_calls()
    );
}

#[test]
fn factory_validate_looks_up_the_owner_with_the_configured_token() {
    let e = github_owned_by(Some("User"));
    e.config("[forge.github]\ntoken_env = \"DEFAULT_TOKEN\"\n");
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let out = e
        .ns()
        .env("DEFAULT_TOKEN", "default-tok")
        .args(["factory", "validate"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["warnings"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(e.tokens_seen(), ["default-tok"], "{}", e.gh_calls());
}

#[test]
fn factory_validate_looks_up_the_owner_with_the_owner_table_token() {
    let e = github_owned_by(Some("User"));
    e.config(
        "[forge.github]\ntoken_env = \"DEFAULT_TOKEN\"\n[forge.github.owners.\"o\"]\ntoken_env = \"OWNER_TOKEN\"\n",
    );
    e.factory("[gate]\nlocal = \"build-only\"\n");
    e.ns()
        .env("DEFAULT_TOKEN", "default-tok")
        .env("OWNER_TOKEN", "owner-tok")
        .args(["factory", "validate"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), ["owner-tok"], "{}", e.gh_calls());
}

#[test]
fn factory_validate_keeps_the_configured_token_off_an_origin_on_another_host() {
    let e = github_owned_by(Some("User"));
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://other.example.com/o/r.git",
        ],
    );
    e.config("[forge.github]\nhost = \"ghe.corp\"\ntoken_env = \"DEFAULT_TOKEN\"\n");
    e.factory("[gate]\nlocal = \"build-only\"\n");
    e.ns()
        .env("DEFAULT_TOKEN", "default-tok")
        .args(["factory", "validate"])
        .assert()
        .code(0);
    assert!(
        e.gh_calls().contains("--hostname other.example.com"),
        "{}",
        e.gh_calls()
    );
    assert!(
        !e.tokens_seen().contains(&"default-tok".to_string()),
        "{}",
        e.gh_calls()
    );
}

#[test]
fn factory_validate_keeps_the_token_off_another_host_when_no_forge_host_is_set() {
    let e = github_owned_by(Some("User"));
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://other.example.com/o/r.git",
        ],
    );
    e.config("[forge.github]\ntoken_env = \"DEFAULT_TOKEN\"\n");
    e.factory("[gate]\nlocal = \"build-only\"\n");
    e.ns()
        .env("DEFAULT_TOKEN", "default-tok")
        .args(["factory", "validate"])
        .assert()
        .code(0);
    assert!(
        e.gh_calls().contains("--hostname other.example.com"),
        "{}",
        e.gh_calls()
    );
    assert!(
        !e.tokens_seen().contains(&"default-tok".to_string()),
        "{}",
        e.gh_calls()
    );
}

#[test]
fn factory_validate_applies_the_configured_token_to_an_origin_on_the_configured_host() {
    let e = github_owned_by(Some("User"));
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://ghe.example.com/o/r.git",
        ],
    );
    e.config("[forge.github]\nhost = \"ghe.example.com\"\ntoken_env = \"DEFAULT_TOKEN\"\n");
    e.factory("[gate]\nlocal = \"build-only\"\n");
    e.ns()
        .env("DEFAULT_TOKEN", "default-tok")
        .args(["factory", "validate"])
        .assert()
        .code(0);
    assert!(
        e.gh_calls().contains("--hostname ghe.example.com"),
        "{}",
        e.gh_calls()
    );
    assert_eq!(e.tokens_seen(), ["default-tok"], "{}", e.gh_calls());
}

#[test]
fn factory_validate_still_looks_up_the_owner_without_credentials_when_the_token_does_not_resolve() {
    let e = github_owned_by(Some("User"));
    e.config("[forge.github]\ntoken_env = \"UNSET_VAR\"\n");
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let out = e
        .ns()
        .env_remove("UNSET_VAR")
        .args(["factory", "validate"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        e.gh_calls().contains("api repos/o/r --jq .owner.type"),
        "{}",
        e.gh_calls()
    );
    assert_eq!(e.tokens_seen(), [""], "{}", e.gh_calls());
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("no GitHub merge queue")),
        "{v}"
    );
}

#[test]
fn factory_validate_with_factory_path_looks_up_the_owner_of_the_cwd_repo() {
    let e = github_owned_by(Some("User"));
    let other = e.base.join("other-checkout");
    fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q"]);
    git(
        &other,
        &["remote", "add", "origin", "https://github.com/x/y.git"],
    );
    fs::write(
        other.join("nightshift.toml"),
        "[gate]\nlocal = \"build-only\"\n",
    )
    .unwrap();
    let v = e.run(
        &["factory", "validate", "--factory", other.to_str().unwrap()],
        0,
    );
    assert_eq!(v["ok"], true, "{v}");
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{v}");
    assert!(
        warnings[0].as_str().unwrap().contains("no GitHub merge queue"),
        "{v}"
    );
    let calls = e.gh_calls();
    assert!(calls.contains("api repos/o/r --jq .owner.type"), "{calls}");
    assert!(!calls.contains("repos/x/y"), "{calls}");
}

#[test]
fn factory_validate_owner_warning_joins_the_fast_warning() {
    let e = github_owned_by(Some("User"));
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command | Takes |\n|---|---|---|\n| ci-local | `make ci` | 2 min |\n",
    )
    .unwrap();
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    let warnings = v["warnings"].to_string();
    assert!(warnings.contains("`make ci --fast`"), "{v}");
    assert!(warnings.contains("no GitHub merge queue"), "{v}");
}

#[test]
fn factory_validate_gives_no_owner_warning_without_a_personal_account_answer() {
    for owner in [Some("Organization"), Some("not json {"), None] {
        let e = github_owned_by(owner);
        e.factory("[gate]\nlocal = \"build-only\"\n");
        let v = e.run(&["factory", "validate"], 0);
        assert_eq!(v["warnings"], serde_json::json!([]), "{owner:?}: {v}");
    }
    let e = github_owned_by(Some("User"));
    e.gh_file("repo.fail", "HTTP 404: Not Found\n");
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    fs::remove_file(e.ghd.join("repo.fail")).unwrap();
    e.gh_file(
        "hook.sh",
        "[[ \" $* \" == *\" .owner.type \"* ]] && { echo User; exit 1; }\n",
    );
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "a failed lookup: {v}");
}

#[test]
fn factory_validate_skips_the_owner_lookup_off_build_only_or_github() {
    let e = github_owned_by(Some("User"));
    for text in ["", "[gate]\nlocal = \"every-step\"\n"] {
        e.factory(text);
        let v = e.run(&["factory", "validate"], 0);
        assert_eq!(v["warnings"], serde_json::json!([]), "{text:?}: {v}");
    }
    assert!(!e.gh_calls().contains(".owner.type"), "{}", e.gh_calls());
    git(
        &e.root,
        &["remote", "set-url", "origin", "https://gitlab.com/o/r.git"],
    );
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    assert!(!e.gh_calls().contains(".owner.type"), "{}", e.gh_calls());
}

#[test]
fn factory_validate_gives_no_owner_warning_when_gh_is_missing() {
    let e = github_owned_by(Some("User"));
    let only_git = e.base.join("only-git");
    fs::create_dir(&only_git).unwrap();
    let real_git = String::from_utf8(
        std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::os::unix::fs::symlink(real_git.trim(), only_git.join("git")).unwrap();
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e
        .ns()
        .env("PATH", &only_git)
        .args(["factory", "validate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&v).unwrap();
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    assert_eq!(e.gh_calls(), "", "the fake gh is off PATH");
}
