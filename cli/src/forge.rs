//! Forge credentials: resolve `[forge.*]` tokens once and export them, so every child
//! (`gh`, `git`, the harness) inherits them. A token never appears in an error message.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use crate::config::{Config, Forge, ForgeKind, Forges, Table, TokenSource};
use crate::error::SfError;
use crate::proc::Proc;
use crate::watch::forge as watch_forge;

pub fn is_set(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| !v.is_empty())
}

#[derive(Debug)]
pub enum Token {
    AlreadySet,
    Fetched(String),
}

#[derive(Debug)]
pub struct Credentials {
    pub token: Token,
    pub host: Option<String>,
}

impl Credentials {
    /// Set the host, and the token when this run fetched it, on `cmd`'s environment.
    pub fn apply_to(&self, kind: ForgeKind, cmd: &mut Proc) {
        if let Some(h) = &self.host {
            cmd.env(kind.host_var(), h);
        }
        if let Token::Fetched(t) = &self.token {
            cmd.env(kind.token_var(), t);
        }
    }
}

/// The host a `kind` command targets: the host variable when set and non-empty, else the
/// configured one.
pub fn effective_host(kind: ForgeKind, forge: Option<&Forge>) -> Option<String> {
    pick_host(std::env::var(kind.host_var()).ok(), forge)
}

fn pick_host(env: Option<String>, forge: Option<&Forge>) -> Option<String> {
    env.filter(|h| !h.is_empty())
        .or_else(|| forge?.host.clone())
}

/// [`Forge::scope`] on the host `resolve` would use, the environment's or the configured one.
pub fn scope<'a>(
    kind: ForgeKind,
    forge: &Forge,
    repo: Option<&'a str>,
    origin_url: Option<&str>,
) -> Option<&'a str> {
    let host = effective_host(kind, Some(forge));
    Forge::scope(host.as_deref(), repo, origin_url)
}

/// The warning that [`scope`] dropped the owner or repo table `repo` would have matched, because
/// `origin_url` is on another host than the forge's.
pub fn scope_warning(
    kind: ForgeKind,
    forge: &Forge,
    repo: Option<&str>,
    origin_url: Option<&str>,
) -> Option<String> {
    if scope(kind, forge, repo, origin_url).is_some() {
        return None;
    }
    let table = match forge.choose(repo).1 {
        Table::Default => return None,
        t => t.label(kind),
    };
    let want = effective_host(kind, Some(forge))?;
    let got = origin_url.and_then(watch_forge::host)?;
    Some(format!(
        "origin host {got} is not {want}: ignoring {table}; if {got} is an ssh alias for {want}, \
         the default [forge.{0}] token applies: put that token there, or drop `host` and unset {1}",
        kind.name(),
        kind.host_var()
    ))
}

/// The credentials for `repo`, from the table [`Forge::choose`] picks; with no repo, the
/// default table.
pub fn resolve(kind: ForgeKind, forge: &Forge, repo: Option<&str>) -> Result<Credentials, SfError> {
    let host = effective_host(kind, Some(forge)).filter(|_| !is_set(kind.host_var()));
    if host
        .as_deref()
        .is_some_and(|h| h.is_empty() || h.contains(['\0', '=']))
    {
        return Err(SfError::usage(
            format!("forge {}: host is empty or has a NUL or `=`", kind.name()),
            HINT,
        ));
    }
    let (source, _) = forge.choose(repo);
    let token = if is_set(kind.token_var()) {
        Token::AlreadySet
    } else {
        Token::Fetched(fetch(kind, source)?)
    };
    Ok(Credentials { token, host })
}

/// Set by `ns run` and `ns watch` to [`checked_value`] once they have checked the repo, so a unit
/// `ns watch` starts doesn't check again. Any other value, from a stale shell or another repo, is
/// ignored.
pub const CHECKED_VAR: &str = "NS_FORGE_CHECKED";

/// Set to [`checked_value`] whenever `ns run` or `ns watch` settles a configured forge's token,
/// probed or not, so a child that sees the token doesn't warn that the environment overrides the config.
pub const EXPORTED_VAR: &str = "NS_FORGE_EXPORTED";

/// How long a probe of the forge may take before it counts as failed.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

pub fn checked_value(kind: ForgeKind, repo: Option<&str>) -> String {
    format!("{}:{}", kind.name(), repo.unwrap_or_default())
}

fn marked(var: &str, kind: ForgeKind, repo: Option<&str>) -> bool {
    std::env::var(var).is_ok_and(|v| v == checked_value(kind, repo))
}

pub fn checked_by_parent(kind: ForgeKind, repo: Option<&str>) -> bool {
    marked(CHECKED_VAR, kind, repo)
}

pub fn exported_by_parent(kind: ForgeKind, repo: Option<&str>) -> bool {
    marked(EXPORTED_VAR, kind, repo)
}

/// `origin`'s URL, the forge it is on, its repo path, and the configured forges.
pub struct Origin {
    pub url: Option<String>,
    pub kind: ForgeKind,
    pub repo: Option<String>,
    pub forges: Forges,
}

impl Origin {
    /// Read `dir`'s `origin`. The forge is `source` when it names one, else inferred from the URL.
    pub fn read(dir: &Path, source: Option<&str>, cfg: Option<&Config>) -> Origin {
        let url = crate::git::run(dir, &["remote", "get-url", "origin"]).ok();
        let kind = crate::watch::forge::kind(
            source,
            url.as_deref(),
            crate::watch::forge::gitlab_host(cfg).as_deref(),
        );
        let repo = url.as_deref().and_then(repo_path);
        Origin {
            url,
            kind,
            repo,
            forges: cfg.map(|c| c.forge.clone()).unwrap_or_default(),
        }
    }

    /// The URL, only for the forge `origin` is on.
    pub fn url_for(&self, kind: ForgeKind) -> Option<&str> {
        self.url.as_deref().filter(|_| kind == self.kind)
    }

    /// The repo path, only for the forge `origin` is on.
    pub fn repo_for(&self, kind: ForgeKind) -> Option<&str> {
        self.repo.as_deref().filter(|_| kind == self.kind)
    }

    /// The repo that picks `kind`'s token table: [`Origin::repo_for`], unless the forge's `host`
    /// is not the host `origin` is on.
    pub fn table_repo(&self, kind: ForgeKind) -> Option<&str> {
        match self.forges.get(kind) {
            Some(f) => scope(kind, f, self.repo_for(kind), self.url.as_deref()),
            None => self.repo_for(kind),
        }
    }

    /// The warning that [`Origin::table_repo`] dropped a table that would have matched `origin`.
    pub fn scope_warning(&self, kind: ForgeKind) -> Option<String> {
        let f = self.forges.get(kind)?;
        scope_warning(kind, f, self.repo_for(kind), self.url.as_deref())
    }
}

/// What `ns run` and `ns watch` do at start: warn when the environment's token overrides the
/// config, export each forge's token, the one for `origin`'s repo on its forge, and, unless this
/// is a dry run, fail when a configured forge's token can't read the repo.
pub fn start(origin: &Origin, dry_run: bool) -> Result<(), SfError> {
    let (kind, forges) = (origin.kind, &origin.forges);
    let repo = origin.repo.as_deref();
    let table_repo = origin.table_repo(kind);
    let source = source(kind, forges.get(kind), table_repo);
    if !exported_by_parent(kind, repo) {
        let warnings = env_warning(kind, forges.get(kind), table_repo)
            .into_iter()
            .chain(origin.scope_warning(kind));
        for w in warnings {
            eprintln!("ns: warning: {w}");
        }
    }
    export(origin)?;
    let checked = checked_by_parent(kind, repo);
    if let Some(r) = checks_repo(repo, forges.get(kind), dry_run, checked) {
        if let Err(e) = can_read(kind, r, None) {
            return Err(SfError::usage(
                unreadable(kind, r, account(kind, None).as_deref(), &source, &e),
                format!(
                    "check the account with:\n  ns doctor\nif it is the wrong one, set the token for this repo in \
                     [forge.{0}.repos.\"{r}\"] or its owner's [forge.{0}.owners.\"<owner>\"] in the ns config",
                    kind.name()
                ),
            ));
        }
        std::env::set_var(CHECKED_VAR, checked_value(kind, Some(r)));
    }
    Ok(())
}

/// The repo `start` probes: only for a configured forge, on a real run that no parent checked.
fn checks_repo<'a>(
    repo: Option<&'a str>,
    forge: Option<&Forge>,
    dry_run: bool,
    checked: bool,
) -> Option<&'a str> {
    repo.filter(|_| forge.is_some() && !dry_run && !checked)
}

/// The error for a token that can't read `repo`, naming the account, else where the token came
/// from.
pub fn unreadable(
    kind: ForgeKind,
    repo: &str,
    account: Option<&str>,
    source: &str,
    err: &str,
) -> String {
    let name = kind.name();
    match account {
        Some(a) => {
            format!("forge {name}: account {a} cannot read {repo} (token from {source}): {err}")
        }
        None => format!("forge {name}: the token from {source} cannot read {repo}: {err}"),
    }
}

/// The warning that `kind`'s token variable, set in the environment, wins over `forge`'s table.
pub fn env_warning(kind: ForgeKind, forge: Option<&Forge>, repo: Option<&str>) -> Option<String> {
    let table = forge?.choose(repo).1;
    is_set(kind.token_var()).then(|| {
        format!(
            "{} from the environment overrides the config's choice ({})",
            kind.token_var(),
            table.label(kind)
        )
    })
}

/// Where `kind`'s token comes from: the environment, the table `forge` picks for `repo`, or the
/// forge CLI's own login.
pub fn source(kind: ForgeKind, forge: Option<&Forge>, repo: Option<&str>) -> String {
    match forge {
        Some(f) if is_set(kind.token_var()) => format!(
            "{} from the environment, overriding {}",
            kind.token_var(),
            f.choose(repo).1.label(kind)
        ),
        _ if is_set(kind.token_var()) => format!("{} from the environment", kind.token_var()),
        Some(f) => f.choose(repo).1.label(kind),
        None => format!("{}'s own login", kind.cli()),
    }
}

fn export(origin: &Origin) -> Result<(), SfError> {
    for (kind, forge) in origin.forges.each() {
        if let Some(f) = forge {
            let creds = resolve(kind, f, origin.table_repo(kind))?;
            if let Some(h) = creds.host {
                std::env::set_var(kind.host_var(), h);
            }
            if let Token::Fetched(t) = creds.token {
                std::env::set_var(kind.token_var(), t);
            }
            if kind == origin.kind {
                std::env::set_var(EXPORTED_VAR, checked_value(kind, origin.repo.as_deref()));
            }
        }
    }
    Ok(())
}

/// The repo path in a remote URL, `owner/name` or `group/sub/name`, without `.git`: from
/// `https://host/owner/name`, `ssh://git@host:22/owner/name` or scp-like `git@alias:owner/name`.
/// `None` for a local path or `file://` URL.
pub fn repo_path(url: &str) -> Option<String> {
    if url
        .split_once("://")
        .is_some_and(|(scheme, _)| !remote_scheme(scheme))
    {
        return None;
    }
    watch_forge::host(url).filter(|h| !h.is_empty())?;
    let path = watch_forge::path(url)?.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    (segments.len() >= 2 && segments.iter().all(|s| !s.is_empty())).then(|| path.to_string())
}

fn remote_scheme(scheme: &str) -> bool {
    matches!(
        scheme,
        "https" | "http" | "git" | "ssh" | "git+ssh" | "ssh+git"
    )
}

/// Where `ssh -T` connects for an SSH remote URL: `[user@]host` as written, so an alias in
/// `~/.ssh/config` still applies, and the port when the URL names one.
#[derive(Debug, PartialEq)]
pub struct SshTarget {
    pub dest: String,
    pub port: Option<String>,
}

pub fn ssh_target(url: &str) -> Option<SshTarget> {
    repo_path(url)?;
    let (userinfo, hostport) = match url.split_once("://") {
        Some(("ssh" | "git+ssh" | "ssh+git", rest)) => {
            let authority = rest.split('/').next()?;
            match authority.rsplit_once('@') {
                Some((u, h)) => (Some(u), h),
                None => (None, authority),
            }
        }
        Some(_) => return None,
        None => {
            let dest = url.split_once(':')?.0;
            match dest.rsplit_once('@') {
                Some((u, h)) => (Some(u), h),
                None => (None, dest),
            }
        }
    };
    let scp = !url.contains("://");
    let (host, port) = if scp {
        (hostport, None)
    } else {
        split_port(hostport)
    };
    if !safe_host(host) || !userinfo.is_none_or(safe_name) {
        return None;
    }
    let dest = match userinfo {
        Some(u) => format!("{u}@{host}"),
        None => host.to_string(),
    };
    Some(SshTarget { dest, port })
}

fn split_port(hostport: &str) -> (&str, Option<String>) {
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !hostport.ends_with(']') => (h, p),
        _ => (hostport, ""),
    };
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|p| *p >= 1)
        .map(|p| p.to_string());
    (host, port)
}

fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn safe_host(host: &str) -> bool {
    match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(v6) => !v6.is_empty() && v6.chars().all(|c| c.is_ascii_hexdigit() || c == ':'),
        None => safe_name(host) && !host.starts_with('-'),
    }
}

/// `program` (`kind`'s CLI) with `creds` on its environment, killed after `limit` or at a stop.
fn cli(program: &OsStr, kind: ForgeKind, creds: Option<&Credentials>, limit: Duration) -> Proc {
    let mut cmd = Proc::forge(program);
    if let Some(c) = creds {
        c.apply_to(kind, &mut cmd);
    }
    cmd.timeout(limit).until_stop();
    cmd
}

/// Why `what` did not run: a stop says so, anything else names `what` as unrunnable.
fn cannot_run(what: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::Interrupted => e.to_string(),
        _ => format!("cannot run {what}: {e}"),
    }
}

/// How a call past `limit` is reported, after the program it names.
fn timed_out(limit: Duration) -> String {
    format!("timed out after {}s", limit.as_secs_f32())
}

/// The login of the account the token belongs to, from the forge's API.
pub fn account(kind: ForgeKind, creds: Option<&Credentials>) -> Option<String> {
    let out = account_output(kind, creds).ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let login = match kind {
        ForgeKind::Github => text,
        ForgeKind::Gitlab => serde_json::from_str::<serde_json::Value>(&text)
            .ok()?
            .get("username")?
            .as_str()?
            .to_string(),
    };
    (out.status.success() && !login.is_empty()).then_some(login)
}

fn account_output(
    kind: ForgeKind,
    creds: Option<&Credentials>,
) -> std::io::Result<std::process::Output> {
    let mut cmd = cli(OsStr::new(kind.cli()), kind, creds, PROBE_TIMEOUT);
    match kind {
        ForgeKind::Github => cmd.args(["api", "user", "--jq", ".login"]),
        ForgeKind::Gitlab => cmd.args(["api", "user"]),
    };
    cmd.output()
}

/// Whether the token can read `repo`: `gh api repos/<repo>` or `glab api projects/<repo>`. The
/// error is the CLI's stderr with the token hidden.
pub fn can_read(kind: ForgeKind, repo: &str, creds: Option<&Credentials>) -> Result<(), String> {
    can_read_within(OsStr::new(kind.cli()), kind, repo, creds, PROBE_TIMEOUT)
}

fn can_read_within(
    program: &OsStr,
    kind: ForgeKind,
    repo: &str,
    creds: Option<&Credentials>,
    limit: Duration,
) -> Result<(), String> {
    let mut cmd = cli(program, kind, creds, limit);
    match kind {
        ForgeKind::Github => cmd.args(["api", &format!("repos/{repo}"), "--jq", ".full_name"]),
        ForgeKind::Gitlab => cmd.args(["api", &format!("projects/{}", repo.replace('/', "%2F"))]),
    };
    let out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::TimedOut => format!("{} {}", kind.cli(), timed_out(limit)),
        _ => cannot_run(kind.cli(), &e),
    })?;
    if out.status.success() {
        return Ok(());
    }
    let fetched = creds.and_then(|c| match &c.token {
        Token::Fetched(t) => Some(t.clone()),
        Token::AlreadySet => None,
    });
    let tokens = fetched
        .into_iter()
        .chain(std::env::var(kind.token_var()).ok());
    let err = hide(String::from_utf8_lossy(&out.stderr).trim(), tokens);
    Err(if err.is_empty() {
        format!("{} exited with {}", kind.cli(), out.status)
    } else {
        err
    })
}

fn hide(text: &str, tokens: impl IntoIterator<Item = String>) -> String {
    tokens
        .into_iter()
        .filter(|t| !t.is_empty())
        .fold(text.to_string(), |acc, t| acc.replace(&t, "[token]"))
}

const HINT: &str =
    "check it with:\n  ns doctor\nor export the token variable yourself before starting ns";

/// How long a `token_command` may take before it is killed.
const TOKEN_TIMEOUT: Duration = PROBE_TIMEOUT;

fn fetch(kind: ForgeKind, source: &TokenSource) -> Result<String, SfError> {
    fetch_within(kind, source, TOKEN_TIMEOUT)
}

fn fetch_within(kind: ForgeKind, source: &TokenSource, limit: Duration) -> Result<String, SfError> {
    let name = kind.name();
    let token = match source {
        TokenSource::Command(c) => {
            let out = Proc::forge("sh")
                .args(["-c", c])
                .timeout(limit)
                .until_stop()
                .output()
                .map_err(|e| {
                    let what = match e.kind() {
                        std::io::ErrorKind::TimedOut => {
                            format!("token_command `{c}` {}", timed_out(limit))
                        }
                        _ => cannot_run(&format!("token_command `{c}`"), &e),
                    };
                    SfError::usage(format!("forge {name}: {what}"), HINT)
                })?;
            if !out.status.success() {
                return Err(SfError::usage(
                    format!("forge {name}: token_command `{c}` failed ({})", out.status),
                    HINT,
                ));
            }
            let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if t.is_empty() {
                return Err(SfError::usage(
                    format!("forge {name}: token_command `{c}` printed no token"),
                    HINT,
                ));
            }
            t
        }
        TokenSource::Env(v) => std::env::var(v)
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                SfError::usage(
                    format!("forge {name}: token_env {v} is unset or empty"),
                    HINT,
                )
            })?,
    };
    if token.contains('\0') {
        return Err(SfError::usage(
            format!("forge {name}: the token contains a NUL byte"),
            HINT,
        ));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
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
}
