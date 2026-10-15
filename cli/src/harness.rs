//! The one way to start a coding-agent harness (claude, codex, ...). `run`, `eval` and `ask`
//! all build theirs here, so the billing and setup each kind needs can't be skipped; clippy
//! rejects `Proc::harness` anywhere else (`clippy.toml`).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::SfError;
use crate::memcap::{self, Cap, Watch};
use crate::proc::{Kind, Proc};

/// Headless claude kills background tasks this long after its last turn (default 10 minutes).
pub const BG_WAIT_ENV: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";

/// Turns off Claude Code auto-memory, so a phase or trial can't write into the operator's
/// `~/.claude/projects/` memory: https://code.claude.com/docs/en/memory
pub const AUTO_MEMORY_ENV: &str = "CLAUDE_CODE_DISABLE_AUTO_MEMORY";

fn is_claude(program: &OsStr) -> bool {
    Path::new(program)
        .file_name()
        .is_some_and(|n| n == "claude")
}

/// `argv` as it runs under the billing mode: a subscription claude drops `--bare`, which
/// forces API-key auth.
pub fn argv(argv: Vec<String>, subscription: bool) -> Vec<String> {
    match argv.first() {
        Some(p) if subscription_claude(p.as_ref(), subscription) => {
            argv.into_iter().filter(|a| a != "--bare").collect()
        }
        _ => argv,
    }
}

fn subscription_claude(program: &OsStr, subscription: bool) -> bool {
    subscription && is_claude(program)
}

fn credentials_path() -> PathBuf {
    crate::config::expand_tilde("~/.claude/.credentials.json")
}

/// A subscription claude refuses to start without a login: `~/.claude/.credentials.json` or
/// `CLAUDE_CODE_OAUTH_TOKEN`. Other harnesses, and API billing, need none.
pub fn check_login(program: impl AsRef<OsStr>, subscription: bool) -> anyhow::Result<()> {
    if !subscription_claude(program.as_ref(), subscription) {
        return Ok(());
    }
    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok_and(|v| !v.is_empty());
    if token || credentials_path().is_file() {
        return Ok(());
    }
    Err(SfError::usage(
        format!(
            "billing = \"subscription\" but {} is missing, so claude would fall back to API credits",
            credentials_path().display()
        ),
        "log in with your Claude subscription first:\n  claude /login\nor opt into API billing in .nightshift/nightshift.toml:\n  [defaults]\n  billing = \"api\"",
    )
    .into())
}

/// A harness child that runs `program` with `args`, after the login check. The caller adds
/// the rest: its directory, streams, timeout and environment.
#[allow(
    clippy::disallowed_methods,
    reason = "the builder is the one caller of Proc::harness"
)]
pub fn command(
    program: impl AsRef<OsStr>,
    args: &[String],
    subscription: bool,
    caller: impl FnOnce(&mut Proc),
) -> anyhow::Result<Proc> {
    let (cmd, ()) = start(program.as_ref(), args, subscription, caller, |p| {
        (Proc::harness(p, subscription), ())
    })?;
    Ok(cmd)
}

/// As [`command`], under the memory `cap`, with how to tell afterwards whether it hit it.
pub fn capped(
    cap: Option<&Cap>,
    program: impl AsRef<OsStr>,
    args: &[String],
    subscription: bool,
    caller: impl FnOnce(&mut Proc),
) -> anyhow::Result<(Proc, Watch)> {
    start(program.as_ref(), args, subscription, caller, |p| {
        memcap::command(cap, Kind::Harness { subscription }, p)
    })
}

fn start<T>(
    program: &OsStr,
    args: &[String],
    subscription: bool,
    caller: impl FnOnce(&mut Proc),
    new: impl FnOnce(&OsStr) -> (Proc, T),
) -> anyhow::Result<(Proc, T)> {
    check_login(program, subscription)?;
    let (mut cmd, extra) = new(program);
    let drop_bare = subscription_claude(program, subscription);
    cmd.args(args.iter().filter(|a| !drop_bare || *a != "--bare"));
    let claude = is_claude(program);
    if claude {
        // Lift the ceiling so the caller's timeout is the only bound, unless the user set one.
        if std::env::var_os(BG_WAIT_ENV).is_none() {
            cmd.env(BG_WAIT_ENV, "0");
        }
    }
    caller(&mut cmd);
    if claude {
        // Last, so no caller's environment (a phase's, a fixture's) can turn it back on.
        cmd.env(AUTO_MEMORY_ENV, "1");
    }
    Ok((cmd, extra))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_a_subscription_claude_drops_bare() {
        assert_eq!(
            argv(v(&["/usr/bin/claude", "--bare", "-p"]), true),
            v(&["/usr/bin/claude", "-p"])
        );
        assert_eq!(
            argv(v(&["claude", "--bare", "-p"]), false),
            v(&["claude", "--bare", "-p"])
        );
        assert_eq!(argv(v(&["codex", "--bare"]), true), v(&["codex", "--bare"]));
        assert!(argv(Vec::new(), true).is_empty());
    }

    #[test]
    fn only_a_subscription_claude_needs_a_login() {
        assert!(check_login("codex", true).is_ok());
        assert!(check_login("claude", false).is_ok());
    }

    fn args(cmd: &Proc) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// The value `cmd` sets `key` to, if it sets it.
    fn env(cmd: &Proc, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    }

    #[test]
    fn claude_gets_auto_memory_off_and_the_bg_wait_lifted() {
        let cmd = command("claude", &v(&["--bare", "-p"]), false, |_| {}).unwrap();
        assert_eq!(args(&cmd), ["--bare", "-p"]);
        assert_eq!(env(&cmd, AUTO_MEMORY_ENV).as_deref(), Some("1"));
        let lifted = std::env::var_os(BG_WAIT_ENV).is_none();
        assert_eq!(env(&cmd, BG_WAIT_ENV).as_deref(), lifted.then_some("0"));
    }

    #[test]
    fn a_caller_cannot_turn_auto_memory_back_on() {
        let set_off = |c: &mut Proc| {
            c.env(AUTO_MEMORY_ENV, "0");
            c.env(BG_WAIT_ENV, "123");
        };
        let cmd = command("claude", &[], false, set_off).unwrap();
        assert_eq!(env(&cmd, AUTO_MEMORY_ENV).as_deref(), Some("1"));
        assert_eq!(env(&cmd, BG_WAIT_ENV).as_deref(), Some("123"));
        let (cmd, _) = capped(None, "claude", &[], false, set_off).unwrap();
        assert_eq!(env(&cmd, AUTO_MEMORY_ENV).as_deref(), Some("1"));
    }

    #[test]
    fn another_harness_keeps_its_args_and_environment() {
        let (cmd, _) = capped(None, "codex", &v(&["exec", "--bare"]), true, |_| {}).unwrap();
        assert_eq!(args(&cmd), ["exec", "--bare"]);
        assert_eq!(env(&cmd, AUTO_MEMORY_ENV), None);
        assert_eq!(env(&cmd, BG_WAIT_ENV), None);
    }
}
