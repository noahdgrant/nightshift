//! Forge credentials: resolve `[forge.*]` tokens once and export them, so every child
//! (`gh`, `git`, the harness) inherits them. A token never appears in an error message.

use std::process::{Command, Stdio};

use crate::config::{Forge, ForgeKind, Forges, TokenSource};
use crate::error::SfError;

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

/// Set by `ns run` and `ns watch` once they have exported the tokens and checked the repo, so a
/// unit `ns watch` starts inherits both and neither warns nor checks again.
pub const CHECKED_VAR: &str = "NS_FORGE_CHECKED";

/// What `ns run` and `ns watch` do at start: warn when the environment's token overrides the
/// config, export each forge's token, the one for `origin`'s repo on `kind`, and fail when that
/// token can't read the repo.
pub fn start(forges: &Forges, kind: ForgeKind, origin: Option<&str>) -> Result<(), SfError> {
    let inherited = is_set(CHECKED_VAR);
    let repo = origin.and_then(repo_path);
    let source = source(kind, forges.get(kind), repo.as_deref());
    if !inherited {
        if let Some(w) = env_warning(kind, forges.get(kind), repo.as_deref()) {
            eprintln!("ns: warning: {w}");
        }
    }
    export(forges, kind, repo.as_deref())?;
    if let Some(r) = repo.as_deref().filter(|_| !inherited) {
        if let Err(e) = can_read(kind, r, None) {
            return Err(SfError::usage(
                unreadable(kind, r, account(kind, None).as_deref(), &source, &e),
                format!(
                    "check the account with:\n  ns doctor\nand set the token for this repo in \
                     [forge.{0}.repos.\"{r}\"] or its owner's [forge.{0}.owners.\"<owner>\"] in the ns config",
                    kind.name()
                ),
            ));
        }
    }
    std::env::set_var(CHECKED_VAR, "1");
    Ok(())
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
        _ if is_set(kind.token_var()) => format!("{} from the environment", kind.token_var()),
        Some(f) => f.choose(repo).1.label(kind),
        None => format!("{}'s own login", kind.cli()),
    }
}

fn export(forges: &Forges, origin_kind: ForgeKind, repo: Option<&str>) -> Result<(), SfError> {
    for (kind, forge) in forges.each() {
        if let Some(f) = forge {
            let creds = resolve(kind, f, repo.filter(|_| kind == origin_kind))?;
            if let Some(h) = creds.host {
                std::env::set_var(kind.host_var(), h);
            }
            if let Token::Fetched(t) = creds.token {
                std::env::set_var(kind.token_var(), t);
            }
        }
    }
    Ok(())
}

/// The repo path in a remote URL, `owner/name` or `group/sub/name`, without `.git`: from
/// `https://host/owner/name`, `ssh://git@host:22/owner/name` or scp-like `git@alias:owner/name`.
/// `None` for a local path or `file://` URL.
pub fn repo_path(url: &str) -> Option<String> {
    let path = match url.split_once("://") {
        Some((scheme, rest)) if remote_scheme(scheme) => rest.split_once('/')?.1,
        Some(_) => return None,
        None => url.split_once(':').filter(|(h, _)| !h.contains('/'))?.1,
    };
    let path = path.trim_matches('/');
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
    match url.split_once("://") {
        Some(("ssh" | "git+ssh" | "ssh+git", rest)) => {
            let authority = rest.split('/').next()?;
            let (dest, port) = match authority.rsplit_once(':') {
                Some((d, p)) => (d, Some(p.to_string())),
                None => (authority, None),
            };
            Some(SshTarget {
                dest: dest.to_string(),
                port,
            })
        }
        Some(_) => None,
        None => Some(SshTarget {
            dest: url.split_once(':')?.0.to_string(),
            port: None,
        }),
    }
}

/// `kind`'s CLI (`gh` or `glab`), scrubbed, with `creds` on its environment.
fn cli(kind: ForgeKind, creds: Option<&Credentials>) -> Command {
    let mut cmd = Command::new(kind.cli());
    crate::git::scrub(&mut cmd);
    if let Some(c) = creds {
        c.apply_to(kind, &mut cmd);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// The login of the account the token belongs to, from the forge's API.
pub fn account(kind: ForgeKind, creds: Option<&Credentials>) -> Option<String> {
    let mut cmd = cli(kind, creds);
    match kind {
        ForgeKind::Github => cmd.args(["api", "user", "--jq", ".login"]),
        ForgeKind::Gitlab => cmd.args(["api", "user"]),
    };
    let out = cmd.stderr(Stdio::null()).output().ok()?;
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

/// Whether the token can read `repo`: `gh api repos/<repo>` or `glab api projects/<repo>`. The
/// error is the CLI's stderr with the token hidden.
pub fn can_read(kind: ForgeKind, repo: &str, creds: Option<&Credentials>) -> Result<(), String> {
    let mut cmd = cli(kind, creds);
    match kind {
        ForgeKind::Github => cmd.args(["api", &format!("repos/{repo}"), "--jq", ".full_name"]),
        ForgeKind::Gitlab => cmd.args(["api", &format!("projects/{}", repo.replace('/', "%2F"))]),
    };
    let out = cmd
        .output()
        .map_err(|e| format!("cannot run {}: {e}", kind.cli()))?;
    if out.status.success() {
        return Ok(());
    }
    let mut err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let fetched = creds.and_then(|c| match &c.token {
        Token::Fetched(t) => Some(t.clone()),
        Token::AlreadySet => None,
    });
    for t in fetched
        .into_iter()
        .chain(std::env::var(kind.token_var()).ok())
        .filter(|t| !t.is_empty())
    {
        err = err.replace(&t, "[token]");
    }
    Err(if err.is_empty() {
        format!("{} exited with {}", kind.cli(), out.status)
    } else {
        err
    })
}

const HINT: &str =
    "check it with:\n  ns doctor\nor export the token variable yourself before starting ns";

fn fetch(kind: ForgeKind, source: &TokenSource) -> Result<String, SfError> {
    let name = kind.name();
    let token = match source {
        TokenSource::Command(c) => {
            let out = crate::git::scrub(&mut Command::new("sh"))
                .args(["-c", c])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .map_err(|e| {
                    SfError::usage(
                        format!("forge {name}: cannot run token_command `{c}`: {e}"),
                        HINT,
                    )
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
        assert_eq!(ssh_target("https://github.com/o/r"), None);
        assert_eq!(ssh_target("/tmp/remote.git"), None);
        assert_eq!(ssh_target("git@github.com:only"), None);
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

    fn envs(cmd: &Command) -> Vec<(String, String)> {
        cmd.get_envs()
            .map(|(k, v)| {
                let s = |o: &std::ffi::OsStr| o.to_string_lossy().into_owned();
                (s(k), v.map(s).unwrap_or_default())
            })
            .collect()
    }

    #[test]
    fn apply_to_sets_a_fetched_token_and_the_host() {
        let creds = Credentials {
            token: Token::Fetched("tok".into()),
            host: Some("ghe.example.com".into()),
        };
        let mut cmd = Command::new("gh");
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
        let mut cmd = Command::new("glab");
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
