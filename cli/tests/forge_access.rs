//! Whether `ns run` and `ns watch` can read the forge repo before they start, and which token
//! each repo, owner or host override table picks.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

#[path = "common/mod.rs"]
mod common;
#[path = "common/factory_env.rs"]
mod factory_env;

use std::fs;

use factory_env::*;
use serde_json::Value;

#[test]
fn a_repo_the_token_cannot_read_stops_run_and_watch_with_exit_2() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/frontend.git",
        ],
    );
    e.config(OVERRIDES);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file(
        "repo.fail",
        "HTTP 404: Not Found (https://api.github.com/repos/example-org/frontend)",
    );
    let table = "[forge.github.owners.\"example-org\"]";
    for login in [Some("work-account"), None] {
        match login {
            Some(l) => e.gh_file("login", &format!("{l}\n")),
            None => fs::remove_file(e.ghd.join("login")).unwrap(),
        }
        for args in [&["run", "--issue", "7"][..], &["watch", "--once"][..]] {
            let out = e
                .ns()
                .env("OWNER_TOKEN", FAKE_TOKEN)
                .args(args)
                .assert()
                .code(2)
                .get_output()
                .clone();
            let err = String::from_utf8_lossy(&out.stderr);
            let want = match login {
                Some(l) => {
                    format!("account {l} cannot read example-org/frontend (token from {table})")
                }
                None => format!("the token from {table} cannot read example-org/frontend"),
            };
            assert!(err.contains(&want), "{args:?}: {err}");
            assert!(err.contains("HTTP 404"), "{err}");
            assert!(err.contains("ns doctor"), "{err}");
            assert!(
                err.contains("[forge.github.repos.\"example-org/frontend\"]"),
                "{err}"
            );
            assert!(!err.contains(FAKE_TOKEN), "{err}");
        }
    }
    assert!(e.calls().is_empty(), "a phase ran: {:?}", e.calls());
    assert_eq!(e.labels(2), ["status:ready-for-agent", "type:fix"]);

    e.config("");
    e.gh_file("repo.fail", "");
    let _ = fs::remove_file(e.ghd.join("calls"));
    e.ns().args(["run", "--issue", "7"]).assert().code(0);
    assert!(
        !e.gh_calls().contains(".full_name"),
        "a forge with no table was checked: {}",
        e.gh_calls()
    );
}

fn unreadable_github_repo(e: &Env) {
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/frontend.git",
        ],
    );
    e.config(OVERRIDES);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file(
        "repo.fail",
        &format!("HTTP 404: Not Found: token {FAKE_TOKEN}"),
    );
}

#[test]
fn a_stale_or_foreign_checked_mark_never_skips_the_probe() {
    let e = Env::new();
    unreadable_github_repo(&e);
    for mark in [
        "1",
        "github:example-org/other",
        "gitlab:example-org/frontend",
    ] {
        for args in [&["run", "--issue", "7"][..], &["watch", "--once"][..]] {
            let out = e
                .ns()
                .env("OWNER_TOKEN", FAKE_TOKEN)
                .env("NS_FORGE_CHECKED", mark)
                .args(args)
                .assert()
                .code(2)
                .get_output()
                .clone();
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(
                err.contains("cannot read example-org/frontend"),
                "{mark} {args:?}: {err}"
            );
        }
    }
    assert!(e.calls().is_empty(), "a phase ran: {:?}", e.calls());
}

#[test]
fn an_unreadable_repo_names_the_environment_token_that_overrides_the_table() {
    let e = Env::new();
    unreadable_github_repo(&e);
    for args in [&["run", "--issue", "7"][..], &["watch", "--once"][..]] {
        let out = e
            .ns()
            .env("GH_TOKEN", FAKE_TOKEN)
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains(
                "the token from GH_TOKEN from the environment, overriding [forge.github.owners.\"example-org\"]"
            ),
            "{args:?}: {err}"
        );
        assert!(err.contains("[token]"), "{err}");
        assert!(!err.contains(FAKE_TOKEN), "{err}");
    }
}

fn checked_seen(e: &Env) -> Vec<String> {
    fs::read_to_string(e.ctrl.join("checked"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn a_forge_with_no_config_marks_nothing_checked_for_its_children() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/backend.git",
        ],
    );
    e.config("");
    e.ns()
        .env_remove("NS_FORGE_CHECKED")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!e.calls().is_empty());
    assert!(
        checked_seen(&e).iter().all(|c| c == "unset"),
        "{:?}",
        checked_seen(&e)
    );
}

#[test]
fn a_configured_forge_that_was_checked_marks_its_children() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/backend.git",
        ],
    );
    e.config(OVERRIDES);
    e.ns()
        .env_remove("NS_FORGE_CHECKED")
        .env("DEFAULT_TOKEN", "d")
        .env("OWNER_TOKEN", "o")
        .env("REPO_TOKEN", "r")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let seen = checked_seen(&e);
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|c| c == "github:example-org/backend"),
        "{seen:?}"
    );
}

#[test]
fn a_dry_run_probes_nothing_even_when_the_repo_is_unreadable() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/backend.git",
        ],
    );
    e.config(OVERRIDES);
    e.gh_file("repo.fail", "HTTP 404: Not Found");
    e.ns()
        .env("DEFAULT_TOKEN", "d")
        .env("OWNER_TOKEN", "o")
        .env("REPO_TOKEN", "r")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0);
    assert!(!e.gh_calls().contains(".full_name"), "{}", e.gh_calls());
}

#[test]
fn a_forge_host_keeps_another_hosts_origin_on_the_default_token() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(
        &e.root,
        &["remote", "set-url", "origin", "git@gitlab.com:team/p.git"],
    );
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\nhost = \"gitlab.example.com\"\n[forge.gitlab.owners.\"team\"]\ntoken_env = \"GL_TEAM\"\n",
    );
    e.ns()
        .env("GL_DEFAULT", "default-tok")
        .env("GL_TEAM", "team-tok")
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0);
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(!tokens.is_empty());
    assert!(tokens.lines().all(|t| t == "default-tok"), "{tokens}");
}

fn gitlab_host_env_case(origin: &str, host_var: Option<&str>) -> String {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(&e.root, &["remote", "set-url", "origin", origin]);
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\n[forge.gitlab.owners.\"team\"]\ntoken_env = \"GL_TEAM\"\n",
    );
    let mut ns = e.ns();
    ns.env("GL_DEFAULT", "default-tok")
        .env("GL_TEAM", "team-tok")
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST");
    if let Some(h) = host_var {
        ns.env("GITLAB_HOST", h);
    }
    ns.args(["watch", "--dry-run"]).assert().code(0);
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(!tokens.is_empty());
    let first = tokens.lines().next().unwrap().to_string();
    assert!(tokens.lines().all(|t| t == first), "{tokens}");
    first
}

#[test]
fn a_gitlab_host_variable_scopes_the_override_tables_to_its_host() {
    let origin = "git@gitlab.com:team/p.git";
    assert_eq!(
        gitlab_host_env_case(origin, Some("gitlab.example.com")),
        "default-tok"
    );
    assert_eq!(gitlab_host_env_case(origin, Some("gitlab.com")), "team-tok");
    assert_eq!(gitlab_host_env_case(origin, None), "team-tok");
}

#[test]
fn a_gh_host_variable_scopes_the_override_tables_to_its_host() {
    for (gh_host, want) in [
        ("ghe.example.com", "default-tok"),
        ("github.com", "owner-tok"),
    ] {
        let e = Env::new();
        e.ready(2, "Fix a", &["type:fix"], "");
        e.config(OVERRIDES);
        git(
            &e.root,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/example-org/frontend.git",
            ],
        );
        e.ns()
            .env("GH_HOST", gh_host)
            .env("DEFAULT_TOKEN", "default-tok")
            .env("OWNER_TOKEN", "owner-tok")
            .env("REPO_TOKEN", "repo-tok")
            .args(["watch", "--dry-run"])
            .assert()
            .code(0);
        assert_eq!(e.tokens_seen(), [want], "{gh_host}");
    }
}

const ALIAS_WARNING: &str =
    "origin host gitlab-work is not gitlab.example.com: ignoring [forge.gitlab.owners.\"team\"]";

#[test]
fn an_ssh_alias_origin_under_a_forge_host_warns_that_the_override_tables_are_ignored() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(
        &e.root,
        &["remote", "set-url", "origin", "git@gitlab-work:team/p.git"],
    );
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\nhost = \"gitlab.example.com\"\n[forge.gitlab.owners.\"team\"]\ntoken_env = \"GL_TEAM\"\n",
    );
    let out = e
        .ns()
        .env("GL_DEFAULT", "default-tok")
        .env("GL_TEAM", "team-tok")
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.matches(ALIAS_WARNING).count(), 1, "{err}");
    assert!(err.contains("ssh alias for gitlab.example.com"), "{err}");
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(tokens.lines().all(|t| t == "default-tok"), "{tokens}");

    e.config("[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\nhost = \"gitlab.example.com\"\n");
    let out = e
        .ns()
        .env("GL_DEFAULT", "default-tok")
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("ignoring"), "no table was dropped: {err}");
}

#[test]
fn a_token_ns_exported_is_marked_even_when_the_repo_was_not_probed() {
    let e = Env::new();
    e.config(OVERRIDES);
    e.ns()
        .env_remove("NS_FORGE_EXPORTED")
        .env_remove("NS_FORGE_CHECKED")
        .env("DEFAULT_TOKEN", "default-tok")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let exported = fs::read_to_string(e.ctrl.join("exported")).unwrap();
    assert!(!exported.is_empty());
    assert!(exported.lines().all(|l| l == "github:"), "{exported}");
    assert!(
        checked_seen(&e).iter().all(|c| c == "unset"),
        "{:?}",
        checked_seen(&e)
    );
}

#[test]
fn an_origin_with_no_repo_path_uses_the_default_token_and_checks_nothing() {
    let e = Env::new();
    e.config(OVERRIDES);
    e.gh_file("repo.fail", "HTTP 404: Not Found");
    let local = git(&e.root, &["remote", "get-url", "origin"]);
    assert!(local.trim().starts_with('/'), "{local}");
    for remove in [false, true] {
        if remove {
            git(&e.root, &["remote", "remove", "origin"]);
        }
        let _ = fs::remove_file(e.ghd.join("tokens"));
        let _ = fs::remove_file(e.ghd.join("calls"));
        let _ = fs::remove_file(e.ctrl.join("tokens"));
        e.ns()
            .env("DEFAULT_TOKEN", "default-tok")
            .env("OWNER_TOKEN", "owner-tok")
            .env("REPO_TOKEN", "repo-tok")
            .args(["run", "--issue", "7"])
            .assert()
            .code(0);
        assert_eq!(e.tokens_seen(), ["default-tok"], "origin removed: {remove}");
        assert!(
            !e.gh_calls().contains("api repos/"),
            "origin removed: {remove}: {}",
            e.gh_calls()
        );
    }
}

#[test]
fn an_override_naming_an_unset_variable_stops_run_with_exit_2() {
    for (origin, var) in [
        ("https://github.com/example-org/frontend.git", "OWNER_TOKEN"),
        ("https://github.com/example-org/backend.git", "REPO_TOKEN"),
    ] {
        let e = Env::new();
        git(&e.root, &["remote", "set-url", "origin", origin]);
        e.config(OVERRIDES);
        let out = e
            .ns()
            .env("DEFAULT_TOKEN", FAKE_TOKEN)
            .args(["run", "--issue", "7"])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains(&format!("token_env {var} is unset or empty")),
            "{err}"
        );
        assert!(!err.contains(FAKE_TOKEN), "{err}");
        assert!(e.calls().is_empty(), "a phase ran: {:?}", e.calls());
    }
}

#[test]
fn a_gitlab_repo_table_beats_the_nested_group_owner_table() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.com:team/sub/project.git",
        ],
    );
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\n[forge.gitlab.owners.\"team/sub\"]\ntoken_env = \"GL_SUB\"\n[forge.gitlab.repos.\"team/sub/project\"]\ntoken_env = \"GL_REPO\"\n",
    );
    let ns = || {
        let mut c = e.ns();
        c.env("GL_SUB", "owner-tok")
            .env("GL_REPO", FAKE_TOKEN)
            .env_remove("GITLAB_TOKEN")
            .env_remove("GITLAB_HOST");
        c
    };
    ns().args(["watch", "--dry-run"]).assert().code(0);
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(!tokens.is_empty());
    assert!(tokens.lines().all(|t| t == FAKE_TOKEN), "{tokens}");

    e.gh_file("gl-repo.fail", "404 Project Not Found");
    e.gh_file("gl-login", "gl-work");
    let out = ns()
        .args(["watch", "--once"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "account gl-work cannot read team/sub/project (token from [forge.gitlab.repos.\"team/sub/project\"])"
        ),
        "{err}"
    );
    assert!(!err.contains("token from [forge.gitlab.owners"), "{err}");
}

#[test]
fn a_gitlab_nested_group_picks_its_table_and_an_unreadable_project_exits_2() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.com:team/sub/project.git",
        ],
    );
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\n[forge.gitlab.owners.\"team/sub\"]\ntoken_env = \"GL_SUB\"\n",
    );
    let ns = || {
        let mut c = e.ns();
        c.env("GL_DEFAULT", "default-tok")
            .env("GL_SUB", FAKE_TOKEN)
            .env_remove("GITLAB_TOKEN")
            .env_remove("GITLAB_HOST");
        c
    };
    let out = ns()
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(queued(&v), [2], "{v}");
    assert!(
        !e.glab_calls().contains("team%2Fsub%2Fproject"),
        "a dry run checked the project: {}",
        e.glab_calls()
    );
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(tokens.lines().all(|t| t == FAKE_TOKEN), "{tokens}");
    e.gh_file(
        "gl-repo.fail",
        &format!("404 Project Not Found: token {FAKE_TOKEN}"),
    );
    e.gh_file("gl-login", "gl-work");
    let out = ns()
        .args(["watch", "--once"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("forge gitlab: account gl-work cannot read team/sub/project"),
        "{err}"
    );
    assert!(err.contains("[forge.gitlab.owners.\"team/sub\"]"), "{err}");
    assert!(err.contains("ns doctor"), "{err}");
    assert!(
        err.contains("[forge.gitlab.repos.\"team/sub/project\"]"),
        "{err}"
    );
    assert!(!err.contains(FAKE_TOKEN), "{err}");
    assert!(err.contains("[token]"), "{err}");
    assert!(
        e.glab_calls().contains("api projects/team%2Fsub%2Fproject"),
        "the project was not read by its encoded path: {}",
        e.glab_calls()
    );

    fs::remove_file(e.ghd.join("gl-repo.fail")).unwrap();
    let _ = fs::remove_file(e.ghd.join("glab-calls"));
    ns().args(["watch", "--once"]).assert().code(0);
    assert!(
        e.glab_calls().contains("api projects/team%2Fsub%2Fproject"),
        "a readable project was not checked: {}",
        e.glab_calls()
    );

    let _ = fs::remove_file(e.ghd.join("glab-tokens"));
    let out = ns()
        .env("GITLAB_TOKEN", "from-env")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    let warning = "GITLAB_TOKEN from the environment overrides the config's choice";
    assert_eq!(err.matches(warning).count(), 1, "{err}");
    assert!(err.contains("[forge.gitlab.owners.\"team/sub\"]"), "{err}");
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(!tokens.is_empty());
    assert!(tokens.lines().all(|t| t == "from-env"), "{tokens}");
}

const OVERRIDES: &str = "[forge.github]
token_env = \"DEFAULT_TOKEN\"
[forge.github.owners.\"example-org\"]
token_env = \"OWNER_TOKEN\"
[forge.github.repos.\"example-org/backend\"]
token_env = \"REPO_TOKEN\"
";

const OVERRIDE_WARNING: &str = "GH_TOKEN from the environment overrides the config's choice";

#[test]
fn the_token_comes_from_the_table_origin_matches() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.config(OVERRIDES);
    for (origin, want) in [
        ("git@github-work:example-org/backend.git", "repo-tok"),
        ("https://github.com/Example-Org/frontend.git", "owner-tok"),
        ("ssh://git@github.com/someone/else", "default-tok"),
        ("https://github.com/example-org", "default-tok"),
    ] {
        git(&e.root, &["remote", "set-url", "origin", origin]);
        let _ = fs::remove_file(e.ghd.join("tokens"));
        let _ = fs::remove_file(e.ghd.join("calls"));
        let out = e
            .ns()
            .env("DEFAULT_TOKEN", "default-tok")
            .env("OWNER_TOKEN", "owner-tok")
            .env("REPO_TOKEN", "repo-tok")
            .args(["watch", "--dry-run"])
            .assert()
            .code(0)
            .get_output()
            .clone();
        assert_eq!(e.tokens_seen(), [want], "{origin}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!err.contains(OVERRIDE_WARNING), "{err}");
        let checked = e
            .gh_calls()
            .lines()
            .filter(|l| l.starts_with("api repos/") && l.ends_with(".full_name"))
            .count();
        assert_eq!(checked, 0, "{origin}: a dry run checked: {}", e.gh_calls());
    }
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:example-org/backend.git",
        ],
    );
    let _ = fs::remove_file(e.ghd.join("tokens"));
    let gitlab_default = e.script(
        "gl-default.sh",
        &format!("touch {}/gl-default; echo gl", e.base.display()),
    );
    let gitlab_owner = e.script(
        "gl-owner.sh",
        &format!("touch {}/gl-owner; echo gl", e.base.display()),
    );
    e.config(&format!(
        "{OVERRIDES}[forge.gitlab]\ntoken_command = {:?}\n[forge.gitlab.owners.\"example-org\"]\ntoken_command = {:?}\n",
        gitlab_default.to_str().unwrap(),
        gitlab_owner.to_str().unwrap()
    ));
    e.ns()
        .env_remove("GITLAB_TOKEN")
        .env("DEFAULT_TOKEN", "default-tok")
        .env("OWNER_TOKEN", "owner-tok")
        .env("REPO_TOKEN", "repo-tok")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), ["repo-tok"]);
    assert!(
        e.base.join("gl-default").exists(),
        "the other forge's default table ran no command"
    );
    assert!(
        !e.base.join("gl-owner").exists(),
        "a github origin matched a gitlab owner table"
    );
}

#[test]
fn a_set_gh_token_wins_over_a_matching_table_and_warns_once_a_night() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:example-org/backend.git",
        ],
    );
    e.config(OVERRIDES);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let out = e
        .ns()
        .env("GH_TOKEN", "from-env")
        .env("REPO_TOKEN", "repo-tok")
        .args(["watch", "--once"])
        .assert()
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.matches(OVERRIDE_WARNING).count(), 1, "{err}");
    assert!(
        err.contains("[forge.github.repos.\"example-org/backend\"]"),
        "{err}"
    );
    assert_eq!(e.tokens_seen(), ["from-env"]);
    assert!(!e.calls().is_empty(), "the unit never ran: {err}");
    let checks = e
        .gh_calls()
        .lines()
        .filter(|l| *l == "api repos/example-org/backend --jq .full_name")
        .count();
    assert_eq!(
        checks,
        1,
        "the unit checked the repo again: {}",
        e.gh_calls()
    );

    for args in [&["run", "--issue", "7"][..], &["watch", "--dry-run"][..]] {
        let out = e
            .ns()
            .env("GH_TOKEN", "from-env")
            .args(args)
            .assert()
            .code(0)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(err.matches(OVERRIDE_WARNING).count(), 1, "{args:?}: {err}");
    }
    e.config("");
    let out = e
        .ns()
        .env("GH_TOKEN", "from-env")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains(OVERRIDE_WARNING),
        "no config, nothing to override: {err}"
    );
}
