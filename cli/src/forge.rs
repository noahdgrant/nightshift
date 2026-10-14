//! Forge credentials: resolve `[forge.*]` tokens once and export them, so every child
//! (`gh`, `git`, the harness) inherits them. A token never appears in an error message.

use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use crate::config::{Config, Forge, ForgeKind, Forges, Table, TokenSource};
use crate::error::SfError;
use crate::eval::trial::run_process;
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
    pub fn apply_to(&self, kind: ForgeKind, cmd: &mut Command) {
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

    /// Whether `kind`'s configured credentials are for the host `origin` is on: true when no
    /// forge is configured, else when `origin`'s host is the forge's host (github.com when unset).
    pub fn creds_fit_host(&self, kind: ForgeKind) -> bool {
        self.creds_fit_env_host(kind, std::env::var(kind.host_var()).ok())
    }

    fn creds_fit_env_host(&self, kind: ForgeKind, env_host: Option<String>) -> bool {
        let Some(forge) = self.forges.get(kind) else {
            return true;
        };
        let want = pick_host(env_host, Some(forge)).unwrap_or_else(|| "github.com".into());
        let got = self
            .url_for(kind)
            .and_then(watch_forge::host)
            .unwrap_or("github.com");
        watch_forge::host(&want)
            .unwrap_or(&want)
            .eq_ignore_ascii_case(got)
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

/// `program` (`kind`'s CLI), scrubbed, with `creds` on its environment.
fn cli(program: &OsStr, kind: ForgeKind, creds: Option<&Credentials>) -> Command {
    let mut cmd = Command::new(program);
    crate::git::scrub(&mut cmd);
    if let Some(c) = creds {
        c.apply_to(kind, &mut cmd);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// Run `cmd` to completion, killing its process group after `limit`. Its output goes to temp
/// files read after it exits, so no descendant holding a pipe open can delay or drop it.
pub fn output_within(mut cmd: Command, limit: Duration) -> std::io::Result<Output> {
    let mut out = tempfile::tempfile()?;
    let mut err = tempfile::tempfile()?;
    cmd.stdout(out.try_clone()?).stderr(err.try_clone()?);
    let (status, timed_out, _) = run_process(cmd, None, limit)?;
    let status = match status {
        Some(s) if !timed_out => s,
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("timed out after {}s", limit.as_secs_f32()),
            ))
        }
    };
    let read = |f: &mut std::fs::File| -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        f.seek(SeekFrom::Start(0))?;
        f.read_to_end(&mut buf)?;
        Ok(buf)
    };
    Ok(Output {
        status,
        stdout: read(&mut out)?,
        stderr: read(&mut err)?,
    })
}

/// The login of the account the token belongs to, from the forge's API.
pub fn account(kind: ForgeKind, creds: Option<&Credentials>) -> Option<String> {
    let mut cmd = cli(OsStr::new(kind.cli()), kind, creds);
    match kind {
        ForgeKind::Github => cmd.args(["api", "user", "--jq", ".login"]),
        ForgeKind::Gitlab => cmd.args(["api", "user"]),
    };
    let out = output_within(cmd, PROBE_TIMEOUT).ok()?;
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

/// `owner.type` of GitHub `repo` (`User` or `Organization`), with `creds` as the other probes
/// get them. `origin_url`'s host is passed to gh as `--hostname` unless it is github.com, so the
/// lookup asks the host `origin` is on even when no forge host is configured; `None` for a host
/// that fails `safe_host`, or when gh is missing, fails or times out.
pub fn repo_owner_type(
    repo: &str,
    origin_url: Option<&str>,
    creds: Option<&Credentials>,
) -> Option<String> {
    repo_owner_type_within(
        OsStr::new(ForgeKind::Github.cli()),
        repo,
        origin_url,
        creds,
        PROBE_TIMEOUT,
    )
}

fn repo_owner_type_within(
    program: &OsStr,
    repo: &str,
    origin_url: Option<&str>,
    creds: Option<&Credentials>,
    limit: Duration,
) -> Option<String> {
    let host = origin_url
        .and_then(crate::watch::forge::host)
        .filter(|h| !h.eq_ignore_ascii_case("github.com"));
    if host.is_some_and(|h| !safe_host(h)) {
        return None;
    }
    let kind = ForgeKind::Github;
    let mut cmd = cli(program, kind, creds);
    cmd.arg("api");
    if let Some(h) = host {
        cmd.args(["--hostname", h]);
    }
    cmd.args([&format!("repos/{repo}"), "--jq", ".owner.type"]);
    let out = output_within(cmd, limit).ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    out.status.success().then_some(text)
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
    let mut cmd = cli(program, kind, creds);
    match kind {
        ForgeKind::Github => cmd.args(["api", &format!("repos/{repo}"), "--jq", ".full_name"]),
        ForgeKind::Gitlab => cmd.args(["api", &format!("projects/{}", repo.replace('/', "%2F"))]),
    };
    let out = output_within(cmd, limit).map_err(|e| match e.kind() {
        std::io::ErrorKind::TimedOut => format!("{} {e}", kind.cli()),
        _ => format!("cannot run {}: {e}", kind.cli()),
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
            let mut cmd = Command::new("sh");
            crate::git::scrub(&mut cmd).args(["-c", c]);
            let out = output_within(cmd, limit).map_err(|e| {
                let what = match e.kind() {
                    std::io::ErrorKind::TimedOut => format!("token_command `{c}` {e}"),
                    _ => format!("cannot run token_command `{c}`: {e}"),
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
mod tests;
