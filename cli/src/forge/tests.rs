use super::*;
use std::time::Instant;

fn fake_cli(dir: &Path, body: &str) -> std::path::PathBuf {
    let fake = dir.join("fake");
    let made = Proc::tool("sh")
        .arg("-c")
        .arg(r#"printf '%s\n' '#!/bin/sh' "$1" > "$0" && chmod 755 "$0""#)
        .arg(&fake)
        .arg(body)
        .output()
        .unwrap();
    assert!(made.status.success());
    fake
}

fn can_read_fake(body: &str, limit: Duration) -> Result<(), String> {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), body);
    can_read_within(fake.as_os_str(), ForgeKind::Github, "o/r", None, limit)
}

#[test]
fn can_read_names_the_cli_that_outlives_its_limit() {
    let started = Instant::now();
    let e = can_read_fake("exec sleep 30", Duration::from_millis(300)).unwrap_err();
    assert_eq!(e, "gh timed out after 0.3s");
    assert!(started.elapsed() < Duration::from_secs(10));
}

const STOP_MARK: &str = "NS_FORGE_STOP_PROBE";

#[test]
fn a_stop_ends_a_forge_cli_and_a_token_command_as_interrupted() {
    crate::testutil::probe("forge::tests::stop_probe", STOP_MARK, "1");
}

#[test]
fn stop_probe() {
    if std::env::var_os(STOP_MARK).is_none() {
        return;
    }
    crate::stop::request(libc::SIGTERM);
    let e = can_read_fake("exec sleep 600", PROBE_TIMEOUT).unwrap_err();
    assert!(e.ends_with("stopped by a signal"), "{e}");
    assert!(!e.contains("cannot run"), "{e}");
    let source = TokenSource::Command("sleep 600".into());
    let e = fetch_within(ForgeKind::Github, &source, PROBE_TIMEOUT).unwrap_err();
    assert!(e.to_string().contains("stopped by a signal"), "{e}");
}

const ACCOUNT_MARK: &str = "NS_FORGE_ACCOUNT_STOP_PROBE";

#[test]
fn a_stop_ends_the_account_lookup_at_once_as_a_negative() {
    crate::testutil::probe("forge::tests::account_stop_probe", ACCOUNT_MARK, "1");
}

#[test]
fn account_stop_probe() {
    if std::env::var_os(ACCOUNT_MARK).is_none() {
        return;
    }
    let _gh = crate::testutil::fake_on_path("gh", "exec sleep 600");
    crate::stop::request(libc::SIGTERM);
    assert_eq!(account(ForgeKind::Github, None), None);
    let e = account_output(ForgeKind::Github, None).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::Interrupted, "{e}");
}

#[test]
fn a_stop_is_not_reported_as_a_command_that_cannot_run() {
    let stopped = std::io::Error::new(std::io::ErrorKind::Interrupted, "`gh api` stopped");
    assert_eq!(cannot_run("gh", &stopped), "`gh api` stopped");
    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
    assert!(cannot_run("gh", &missing).starts_with("cannot run gh: "));
}

#[test]
fn can_read_says_when_the_cli_cannot_run() {
    let e = can_read_within(
        OsStr::new("/nonexistent/gh"),
        ForgeKind::Github,
        "o/r",
        None,
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(e.starts_with("cannot run gh: "), "{e}");
}

#[test]
fn can_read_reports_the_exit_status_when_the_cli_prints_no_error() {
    let e = can_read_fake("exit 3", Duration::from_secs(30)).unwrap_err();
    assert!(e.starts_with("gh exited with "), "{e}");
    assert!(e.contains('3'), "{e}");
}

#[test]
fn can_read_returns_the_cli_stderr() {
    let e = can_read_fake("echo 'HTTP 404' >&2; exit 1", Duration::from_secs(30)).unwrap_err();
    assert_eq!(e, "HTTP 404");
    assert_eq!(can_read_fake("exit 0", Duration::from_secs(30)), Ok(()));
}

#[test]
fn an_empty_env_host_is_unset_so_the_configured_host_wins() {
    let forge = Forge {
        token: TokenSource::Command("true".into()),
        host: Some("cfg.example".into()),
        owners: Default::default(),
        repos: Default::default(),
    };
    let pick = |env: Option<&str>, f| pick_host(env.map(String::from), f);
    assert_eq!(
        pick(Some("env.example"), Some(&forge)).as_deref(),
        Some("env.example")
    );
    assert_eq!(pick(Some(""), Some(&forge)).as_deref(), Some("cfg.example"));
    assert_eq!(pick(None, Some(&forge)).as_deref(), Some("cfg.example"));
    assert_eq!(pick(Some(""), None), None);
}

#[test]
fn repo_path_reads_https_ssh_and_alias_urls() {
    for (url, want) in [
        (
            "https://github.com/example-org/backend.git",
            Some("example-org/backend"),
        ),
        (
            "https://github.com/example-org/backend/",
            Some("example-org/backend"),
        ),
        ("http://user@ghe.example.com/o/r", Some("o/r")),
        (
            "git@github.com:example-org/backend.git",
            Some("example-org/backend"),
        ),
        (
            "git@github-work:example-org/backend.git",
            Some("example-org/backend"),
        ),
        ("github-work:o/r", Some("o/r")),
        (
            "ssh://git@gitlab.example.com:2222/team/sub/project.git",
            Some("team/sub/project"),
        ),
        ("git@gitlab.com:team/sub/project", Some("team/sub/project")),
        ("git://github.com/o/r.git", Some("o/r")),
        ("git+ssh://git@github.com/o/r", Some("o/r")),
        ("ssh+git://git@github.com/o/r", Some("o/r")),
        ("SSH://git@github.com/o/r", None),
        ("HTTPS://github.com/o/r", None),
        ("ftp://github.com/o/r", None),
        ("https://github.com/only-owner", None),
        ("https://github.com//r", None),
        ("https://github.com", None),
        ("file:///tmp/a/remote.git", None),
        ("/tmp/a/remote.git", None),
        ("./dir:x/remote.git", None),
        ("https://github.com/o/my.repo", Some("o/my.repo")),
        ("https://github.com/o/..r", Some("o/..r")),
        ("https://github.com/o/r.", Some("o/r.")),
        ("https://github.com/o/../../user", None),
        ("https://github.com/o/./r", None),
        ("https://github.com/./r", None),
        ("https://github.com/o/..", None),
        ("https://github.com/o/...git", None),
        ("https://github.com/o/r?x=1", None),
        ("https://github.com/o/r#frag", None),
        ("https://github.com/o/r.git?x=1", None),
        ("git@alias:o/../r", None),
        ("git@alias:o/r#x", None),
    ] {
        assert_eq!(repo_path(url).as_deref(), want, "{url}");
    }
}

#[test]
fn ssh_target_keeps_the_alias_and_port() {
    let t = |dest: &str, port: Option<&str>| {
        Some(SshTarget {
            dest: dest.into(),
            port: port.map(String::from),
        })
    };
    assert_eq!(
        ssh_target("git@github-work:o/r.git"),
        t("git@github-work", None)
    );
    assert_eq!(
        ssh_target("ssh://git@gl.example.com:2222/g/p.git"),
        t("git@gl.example.com", Some("2222"))
    );
    assert_eq!(
        ssh_target("ssh://git@github.com/o/r"),
        t("git@github.com", None)
    );
    for url in [
        "git+ssh://git@github.com/o/r",
        "ssh+git://git@github.com/o/r",
    ] {
        assert_eq!(ssh_target(url), t("git@github.com", None), "{url}");
    }
    assert_eq!(ssh_target("SSH://git@github.com/o/r"), None);
    assert_eq!(ssh_target("git://github.com/o/r"), None);
    assert_eq!(ssh_target("https://github.com/o/r"), None);
    assert_eq!(ssh_target("/tmp/remote.git"), None);
    assert_eq!(ssh_target("git@github.com:only"), None);
}

#[test]
fn ssh_target_refuses_shell_metacharacters_ssh_would_expand() {
    for url in [
        "git@h$(cmd):o/r",
        "git@h`cmd`:o/r",
        "git@h;cmd:o/r",
        "git@h|cmd:o/r",
        "git@h&cmd:o/r",
        "git@h%h:o/r",
        "g$x@host:o/r",
        "g(x)@host:o/r",
        "ssh://git@h$(cmd)/o/r",
        "ssh://git@h%h:22/o/r",
        "ssh://git@h;cmd:22/o/r",
    ] {
        assert_eq!(ssh_target(url), None, "{url}");
    }
}

#[test]
fn ssh_target_uses_the_default_port_unless_the_url_names_a_valid_one() {
    let dest = |url: &str| ssh_target(url).map(|t| (t.dest, t.port));
    let plain = Some(("git@host".to_string(), None));
    assert_eq!(dest("ssh://git@host:/o/r"), plain);
    assert_eq!(dest("ssh://git@host:abc/o/r"), plain);
    assert_eq!(dest("ssh://git@host:0/o/r"), plain);
    assert_eq!(dest("ssh://git@host:65536/o/r"), plain);
    assert_eq!(
        dest("ssh://git@host:65535/o/r"),
        Some(("git@host".to_string(), Some("65535".to_string())))
    );
    assert_eq!(
        dest("ssh://git@[::1]:2222/o/r"),
        Some(("git@[::1]".to_string(), Some("2222".to_string())))
    );
    assert_eq!(
        dest("ssh://git@[::1]/o/r"),
        Some(("git@[::1]".to_string(), None))
    );
    assert_eq!(
        dest("git@my-alias_1.example:o/r"),
        Some(("git@my-alias_1.example".to_string(), None))
    );
}

#[test]
fn ssh_target_refuses_a_destination_ssh_would_read_as_an_option() {
    for url in [
        "-oProxyCommand=x:o/r",
        "git@-oProxyCommand=x:o/r",
        "ssh://-oProxyCommand=x/o/r",
        "ssh://git@-oProxyCommand=x/o/r",
        "ssh://a b/o/r",
        "a\tb:o/r",
        "ssh:///o/r",
    ] {
        assert_eq!(ssh_target(url), None, "{url}");
    }
}

#[test]
fn hide_replaces_every_token_but_ignores_an_empty_one() {
    let text = "HTTP 404 for tok-1 and tok-2";
    let got = hide(
        text,
        ["tok-1".to_string(), String::new(), "tok-2".to_string()],
    );
    assert_eq!(got, "HTTP 404 for [token] and [token]");
    assert_eq!(hide(text, [String::new()]), text);
}

#[test]
fn the_repo_is_checked_only_for_a_configured_forge_on_a_real_run_not_yet_checked() {
    let forge = Forge {
        token: TokenSource::Command("true".into()),
        host: None,
        owners: Default::default(),
        repos: Default::default(),
    };
    let (r, f) = (Some("o/r"), Some(&forge));
    assert_eq!(checks_repo(r, f, false, false), r);
    assert_eq!(checks_repo(r, None, false, false), None);
    assert_eq!(checks_repo(r, f, true, false), None);
    assert_eq!(checks_repo(r, f, false, true), None);
    assert_eq!(checks_repo(None, f, false, false), None);
}

#[test]
fn a_checked_value_is_inherited_only_for_its_forge_and_repo() {
    assert_eq!(checked_value(ForgeKind::Github, Some("o/r")), "github:o/r");
    assert_ne!(
        checked_value(ForgeKind::Gitlab, Some("o/r")),
        checked_value(ForgeKind::Github, Some("o/r"))
    );
    assert_ne!(
        checked_value(ForgeKind::Github, None),
        checked_value(ForgeKind::Github, Some("o/r"))
    );
}

#[test]
fn command_output_is_trimmed() {
    let t = fetch(
        ForgeKind::Github,
        &TokenSource::Command("printf '  tok\\n'".into()),
    )
    .unwrap();
    assert_eq!(t, "tok");
}

#[test]
fn errors_name_the_forge_and_command_but_not_the_output() {
    let c = "printf 'secret-%s' out; exit 3";
    let e = fetch(ForgeKind::Gitlab, &TokenSource::Command(c.into())).unwrap_err();
    assert_eq!(e.code, crate::error::EXIT_USAGE);
    assert!(e.message.contains("forge gitlab"), "{}", e.message);
    assert!(e.message.contains(c), "{}", e.message);
    assert!(!e.message.contains("secret-out"), "{}", e.message);
    let e = fetch(ForgeKind::Github, &TokenSource::Command("true".into())).unwrap_err();
    assert!(e.message.contains("printed no token"), "{}", e.message);
}

#[test]
fn a_token_command_that_outlives_its_limit_is_killed_with_an_error() {
    let started = Instant::now();
    let e = fetch_within(
        ForgeKind::Github,
        &TokenSource::Command("printf 'secret-%s' out; sleep 30".into()),
        Duration::from_millis(200),
    )
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(e.code, crate::error::EXIT_USAGE);
    assert!(e.message.contains("forge github"), "{}", e.message);
    assert!(e.message.contains("timed out after"), "{}", e.message);
    assert!(!e.message.contains("cannot run"), "{}", e.message);
    assert!(!e.message.contains("secret-out"), "{}", e.message);
}

#[test]
fn a_token_command_that_finishes_in_time_returns_its_token() {
    let t = fetch_within(
        ForgeKind::Gitlab,
        &TokenSource::Command("echo tok".into()),
        Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(t, "tok");
}

/// The variables set on `cmd`, leaving out the ones its kind removes.
fn envs(cmd: &Proc) -> Vec<(String, String)> {
    let s = |o: &std::ffi::OsStr| o.to_string_lossy().into_owned();
    cmd.get_envs()
        .filter_map(|(k, v)| Some((s(k), s(v?))))
        .collect()
}

#[test]
fn apply_to_sets_a_fetched_token_and_the_host() {
    let creds = Credentials {
        token: Token::Fetched("tok".into()),
        host: Some("ghe.example.com".into()),
    };
    let mut cmd = Proc::forge("gh");
    creds.apply_to(ForgeKind::Github, &mut cmd);
    let mut got = envs(&cmd);
    got.sort();
    assert_eq!(
        got,
        [
            ("GH_HOST".to_string(), "ghe.example.com".to_string()),
            ("GH_TOKEN".to_string(), "tok".to_string()),
        ]
    );
}

#[test]
fn apply_to_leaves_an_already_set_token_alone() {
    let creds = Credentials {
        token: Token::AlreadySet,
        host: None,
    };
    let mut cmd = Proc::forge("glab");
    creds.apply_to(ForgeKind::Gitlab, &mut cmd);
    assert!(envs(&cmd).is_empty(), "{:?}", envs(&cmd));
    let creds = Credentials {
        token: Token::AlreadySet,
        host: Some("gl.example.com".into()),
    };
    creds.apply_to(ForgeKind::Gitlab, &mut cmd);
    assert_eq!(
        envs(&cmd),
        [("GITLAB_HOST".to_string(), "gl.example.com".to_string())]
    );
}

fn owner_fake(body: &str, origin: Option<&str>, limit: Duration) -> Option<String> {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), body);
    repo_owner_type_within(fake.as_os_str(), "o/r", origin, None, limit)
}

#[test]
fn owner_type_is_none_when_the_cli_outlives_its_limit() {
    assert_eq!(
        owner_fake("exec sleep 30", None, Duration::from_millis(300)),
        None
    );
}

#[test]
fn owner_type_is_what_the_cli_prints() {
    assert_eq!(
        owner_fake("echo User", None, Duration::from_secs(30)),
        Some("User".to_string())
    );
}

#[test]
fn owner_type_never_runs_the_cli_for_an_unsafe_host() {
    for url in [
        "https://-oProxy/o/r.git",
        "https://a;b.example.com/o/r.git",
        "https://a$(x).example.com/o/r.git",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let ran = dir.path().join("ran");
        let body = format!("touch '{}'; echo User", ran.display());
        let fake = fake_cli(dir.path(), &body);
        let got = repo_owner_type_within(fake.as_os_str(), "o/r", Some(url), None, Duration::MAX);
        assert_eq!(got, None, "{url}");
        assert!(!ran.exists(), "{url}");
    }
}

fn origin_with(url: Option<&str>, forge: Option<&str>) -> Origin {
    let forges = Forges {
        github: forge.map(|t| toml::from_str(t).unwrap()),
        gitlab: None,
    };
    Origin {
        url: url.map(String::from),
        kind: ForgeKind::Github,
        repo: Some("o/r".into()),
        forges,
    }
}

#[test]
fn creds_fit_host_compares_the_configured_host_with_the_origin_host() {
    let host = |h: &str| format!("token_env = \"T\"\nhost = \"{h}\"\n");
    let unset = "token_env = \"T\"\n".to_string();
    let cases = [
        (None, "https://github.com/o/r.git", true),
        (Some(unset.clone()), "https://github.com/o/r.git", true),
        (
            Some(unset.clone()),
            "https://other.example.com/o/r.git",
            false,
        ),
        (
            Some(host("ghe.example.com")),
            "https://ghe.example.com/o/r.git",
            true,
        ),
        (
            Some(host("ghe.example.com")),
            "git@ghe.example.com:o/r.git",
            true,
        ),
        (
            Some(host("GHE.Example.com")),
            "https://ghe.example.com/o/r.git",
            true,
        ),
        (
            Some(host("ghe.example.com")),
            "https://GHE.EXAMPLE.COM/o/r.git",
            true,
        ),
        (
            Some(host("https://ghe.example.com")),
            "https://ghe.example.com/o/r.git",
            true,
        ),
        (
            Some(host("https://ghe.example.com:8443")),
            "https://ghe.example.com/o/r.git",
            true,
        ),
        (
            Some(host("ghe.example.com")),
            "https://github.com/o/r.git",
            false,
        ),
        (
            Some(host("ghe.example.com")),
            "https://other.example.com/o/r.git",
            false,
        ),
        (Some(host("github.com")), "https://github.com/o/r.git", true),
    ];
    for (forge, url, want) in cases {
        let o = origin_with(Some(url), forge.as_deref());
        assert_eq!(
            o.creds_fit_env_host(ForgeKind::Github, None),
            want,
            "{forge:?} {url}"
        );
    }
    assert!(origin_with(None, Some(&unset)).creds_fit_env_host(ForgeKind::Github, None));
}

#[test]
fn creds_fit_host_lets_the_environment_host_win_over_the_configured_one() {
    let cfg = "token_env = \"T\"\nhost = \"ghe.example.com\"\n";
    let on_env = |env: Option<&str>, url: &str| {
        origin_with(Some(url), Some(cfg))
            .creds_fit_env_host(ForgeKind::Github, env.map(String::from))
    };
    assert!(on_env(
        Some("other.example.com"),
        "https://other.example.com/o/r.git"
    ));
    assert!(!on_env(
        Some("other.example.com"),
        "https://ghe.example.com/o/r.git"
    ));
    assert!(on_env(Some(""), "https://ghe.example.com/o/r.git"));
}
