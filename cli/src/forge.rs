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

pub fn resolve(kind: ForgeKind, forge: &Forge) -> Result<Credentials, SfError> {
    let host = forge.host.clone().filter(|_| !is_set(kind.host_var()));
    if host
        .as_deref()
        .is_some_and(|h| h.is_empty() || h.contains(['\0', '=']))
    {
        return Err(SfError::usage(
            format!("forge {}: host is empty or has a NUL or `=`", kind.name()),
            HINT,
        ));
    }
    let token = if is_set(kind.token_var()) {
        Token::AlreadySet
    } else {
        Token::Fetched(fetch(kind, &forge.token)?)
    };
    Ok(Credentials { token, host })
}

pub fn export(forges: &Forges) -> Result<(), SfError> {
    for (kind, forge) in forges.each() {
        if let Some(f) = forge {
            let creds = resolve(kind, f)?;
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
