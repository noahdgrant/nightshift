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

#[test]
fn a_failing_token_command_stops_before_any_child_and_hides_its_output() {
    let e = Env::new();
    let cmd = e.script("token.sh", &format!("echo {FAKE_TOKEN}; exit 1"));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    for args in [&["run", "--issue", "7"][..], &["watch", "--once"][..]] {
        let out = e.ns().args(args).assert().code(2).get_output().clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("github"), "{err}");
        assert!(err.contains(cmd.to_str().unwrap()), "{err}");
        assert!(!err.contains(FAKE_TOKEN), "{err}");
        assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
    }
    assert!(e.gh_calls().is_empty());
    assert!(e.calls().is_empty());
}

#[test]
fn an_empty_token_is_an_error() {
    let e = Env::new();
    let cmd = e.script("token.sh", "echo '   '");
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    let out = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("printed no token"), "{err}");
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    let out = e
        .ns()
        .env("MY_GH_TOKEN", "")
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("MY_GH_TOKEN"), "{err}");
    assert!(e.calls().is_empty());
}

#[test]
fn forge_host_is_exported_unless_already_set() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\nhost = \"ghe.example.com\"\n");
    let hosts = || fs::read_to_string(e.ghd.join("hosts")).unwrap_or_default();
    e.ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .env_remove("GH_HOST")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!hosts().is_empty());
    assert!(
        hosts().lines().all(|h| h == "ghe.example.com"),
        "{}",
        hosts()
    );
    fs::remove_file(e.ghd.join("hosts")).unwrap();
    e.ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .env("GH_HOST", "mine.example.com")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!hosts().is_empty());
    assert!(
        hosts().lines().all(|h| h == "mine.example.com"),
        "{}",
        hosts()
    );
}

#[test]
fn a_configured_gitlab_forge_exports_its_token_and_host() {
    let e = Env::new();
    e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\nhost = \"gitlab.example.com\"\n");
    let seen = || fs::read_to_string(e.ctrl.join("gitlab")).unwrap_or_default();
    e.ns()
        .env("MY_GL_TOKEN", FAKE_TOKEN)
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!seen().is_empty());
    assert!(
        seen()
            .lines()
            .all(|l| l == format!("{FAKE_TOKEN} gitlab.example.com")),
        "{}",
        seen()
    );
}

#[test]
fn a_set_gitlab_token_wins_and_no_command_runs() {
    let e = Env::new();
    let marker = e.base.join("ran");
    let cmd = e.script("token.sh", &format!("touch {}; exit 1", marker.display()));
    e.config(&format!(
        "[forge.gitlab]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ns()
        .env("GITLAB_TOKEN", "from-env")
        .env_remove("GITLAB_HOST")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(
        !marker.exists(),
        "token_command ran though GITLAB_TOKEN was set"
    );
    let seen = fs::read_to_string(e.ctrl.join("gitlab")).unwrap();
    assert!(seen.lines().all(|l| l == "from-env "), "{seen}");
}

#[test]
fn an_empty_gh_token_does_not_count_as_set() {
    let e = Env::new();
    let cmd = e.script("token.sh", &format!("echo {FAKE_TOKEN}"));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ns()
        .env("GH_TOKEN", "")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
}

#[test]
fn a_token_env_value_is_exported_trimmed() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    e.ns()
        .env("MY_GH_TOKEN", format!("  {FAKE_TOKEN}\n"))
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
}

#[test]
fn a_token_with_a_nul_byte_is_an_error_that_hides_it() {
    let e = Env::new();
    let cmd = e.script("token.sh", "printf 'secretA\\000secretB'");
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    let out = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("forge github"), "{err}");
    assert!(!err.contains("secret"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(e.calls().is_empty());
}

#[test]
fn token_command_stderr_is_discarded() {
    let e = Env::new();
    let cmd = e.script(
        "token.sh",
        &format!("echo {FAKE_TOKEN} >&2; echo {FAKE_TOKEN}"),
    );
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    let out = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&out.stderr).contains(FAKE_TOKEN));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
}

#[test]
fn an_unsettable_host_is_an_error_that_hides_the_token() {
    let e = Env::new();
    for host in ["a\\u0000b", ""] {
        e.config(&format!(
            "[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\nhost = \"{host}\"\n"
        ));
        let out = e
            .ns()
            .env("MY_GH_TOKEN", FAKE_TOKEN)
            .env_remove("GH_HOST")
            .args(["run", "--issue", "7"])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("forge github"), "{err}");
        assert!(err.contains("host"), "{err}");
        assert!(!err.contains(FAKE_TOKEN), "{err}");
        assert!(!err.contains("panicked"), "{err}");
        assert!(e.calls().is_empty());
    }
}

#[test]
fn a_set_gitlab_host_wins_over_the_configured_one() {
    let e = Env::new();
    e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\nhost = \"gitlab.example.com\"\n");
    e.ns()
        .env("MY_GL_TOKEN", FAKE_TOKEN)
        .env_remove("GITLAB_TOKEN")
        .env("GITLAB_HOST", "mine.example.com")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let seen = fs::read_to_string(e.ctrl.join("gitlab")).unwrap();
    assert!(!seen.is_empty());
    assert!(
        seen.lines()
            .all(|l| l == format!("{FAKE_TOKEN} mine.example.com")),
        "{seen}"
    );
}

#[test]
fn watch_pauses_on_a_session_limit_reported_only_in_structured_fields() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("build", &["session", "pass:commit"]);
    let v = e.run(&["watch", "--once", "--until", "06:30"], 0);
    let units = v["units"].as_array().unwrap();
    assert_eq!(units[0]["outcome"], "paused", "{v}");
    assert_eq!(units[0]["reset_at"], "2026-10-09T01:00:00+00:00");
    assert_eq!(units[1]["outcome"], "done");
}

#[test]
fn watch_stops_the_night_when_the_harness_keeps_failing_instantly() {
    let e = Env::new();
    for n in [2, 3, 4] {
        e.ready(n, &format!("Fix {n}"), &["type:fix"], "");
    }
    e.queue("triage", &["crash"; 6]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "harness failing", "{v}");
    let units = v["units"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{v}");
    assert!(units.iter().all(|u| u["outcome"] == "harness_failing"));
    let calls = e.gh_calls();
    assert!(
        calls.contains(
            "issue edit 2 --remove-label status:in-progress --add-label status:ready-for-agent"
        ),
        "{calls}"
    );
    assert!(
        calls.contains(
            "issue edit 3 --remove-label status:in-progress --add-label status:ready-for-agent"
        ),
        "{calls}"
    );
    assert!(!calls.contains("issue edit 4"), "{calls}");
    assert!(!calls.contains("got stuck"), "{calls}");
}

// ---------------------------------------------------------------- the triage pass

fn numbers(v: &Value) -> Vec<u64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().or(i["issue"].as_u64()).unwrap())
        .collect()
}

fn outcomes(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i["outcome"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn watch_triages_a_needs_triage_issue_ready_and_builds_it_the_same_night() {
    let e = Env::new();
    e.untriaged(5, "Fix a follow-up", &["status:needs-triage", "type:fix"]);
    e.triage_sets(&["status:ready-for-agent", "priority:high"]);
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(v["triaged"][0]["outcome"], "done", "{v}");
    assert_eq!(
        v["triaged"][0]["state"],
        serde_json::json!(["status:ready-for-agent"])
    );
    assert_eq!(numbers(&v["units"]), [5], "{v}");
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(v["stopped"], "queue empty");
    // The unit picks up the brief the triage-only run wrote, so triage runs once.
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship"],
        "{v}"
    );
    assert!(e.prompt(1, "triage").contains("Triage only"));
    assert!(!e.prompt(2, "build").contains("Triage only"));
    let calls = e.gh_calls();
    let triaged = calls
        .find("issue edit 5 --remove-label status:needs-triage --add-label status:ready-for-agent")
        .expect(&calls);
    let started = calls.find("--add-label status:in-progress").expect(&calls);
    assert!(triaged < started, "{calls}");
    assert_eq!(
        e.labels(5),
        ["type:fix", "priority:high", "status:in-review"]
    );
}
