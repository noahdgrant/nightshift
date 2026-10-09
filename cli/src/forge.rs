//! Forge credentials: resolve `[forge.*]` tokens once and export them, so every child
//! (`gh`, `git`, the harness) inherits them. A token never appears in an error message.

use std::process::{Command, Stdio};

use crate::config::{Forge, ForgeKind, Forges, TokenSource};
use crate::error::SfError;

pub fn is_set(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| !v.is_empty())
}

#[derive(Debug, Default)]
pub struct Credentials {
    pub token: Option<String>,
    pub host: Option<String>,
}

pub fn resolve(kind: ForgeKind, forge: &Forge) -> Result<Credentials, SfError> {
    let host = forge.host.clone().filter(|_| !is_set(kind.host_var));
    let token = if is_set(kind.token_var) {
        None
    } else {
        Some(fetch(kind.name, &forge.token)?)
    };
    Ok(Credentials { token, host })
}

pub fn export(forges: &Forges) -> Result<(), SfError> {
    for (kind, forge) in forges.each() {
        if let Some(f) = forge {
            let creds = resolve(kind, f)?;
            if let Some(h) = creds.host {
                std::env::set_var(kind.host_var, h);
            }
            if let Some(t) = creds.token {
                std::env::set_var(kind.token_var, t);
            }
        }
    }
    Ok(())
}

const HINT: &str =
    "check it with:\n  ns doctor\nor export the token variable yourself before starting ns";

fn fetch(name: &str, source: &TokenSource) -> Result<String, SfError> {
    let token = match source {
        TokenSource::Command(c) => {
            let out = Command::new("sh")
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
        let t = fetch("github", &TokenSource::Command("printf '  tok\\n'".into())).unwrap();
        assert_eq!(t, "tok");
    }

    #[test]
    fn errors_name_the_forge_and_command_but_not_the_output() {
        let c = "printf 'secret-%s' out; exit 3";
        let e = fetch("gitlab", &TokenSource::Command(c.into())).unwrap_err();
        assert_eq!(e.code, crate::error::EXIT_USAGE);
        assert!(e.message.contains("forge gitlab"), "{}", e.message);
        assert!(e.message.contains(c), "{}", e.message);
        assert!(!e.message.contains("secret-out"), "{}", e.message);
        let e = fetch("github", &TokenSource::Command("true".into())).unwrap_err();
        assert!(e.message.contains("printed no token"), "{}", e.message);
    }
}
