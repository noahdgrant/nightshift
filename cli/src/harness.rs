//! The one way to start a coding-agent harness (claude, codex, ...). `run`, `eval` and `ask`
//! all build theirs here, so the billing and setup each kind needs can't be skipped; clippy
//! rejects `Proc::harness` anywhere else (`clippy.toml`).

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::ExitStatus;

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

/// Whether `path` holds claude's OAuth login: JSON with a non-empty `claudeAiOauth.accessToken`.
fn holds_token(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|v| {
            v.pointer("/claudeAiOauth/accessToken")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|t| !t.is_empty())
        })
}

/// A subscription claude refuses to start without a login: `CLAUDE_CODE_OAUTH_TOKEN`, or an
/// OAuth token in `~/.claude/.credentials.json`. Other harnesses, and API billing, need none.
pub fn check_login(program: impl AsRef<OsStr>, subscription: bool) -> anyhow::Result<()> {
    if !subscription_claude(program.as_ref(), subscription) {
        return Ok(());
    }
    if std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok_and(|v| !v.is_empty()) {
        return Ok(());
    }
    let path = credentials_path();
    if holds_token(&path) {
        return Ok(());
    }
    let state = if path.exists() {
        "is empty or invalid (no claudeAiOauth.accessToken)"
    } else {
        "is missing"
    };
    Err(SfError::usage(
        format!(
            "billing = \"subscription\" but {} {state}, so claude would fall back to API credits",
            path.display()
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

/// As [`command`], under the memory `cap` and in `tmp` when given, with how to tell afterwards
/// whether it hit the cap.
pub fn capped(
    cap: Option<&Cap>,
    tmp: Option<&PrivateTmp>,
    program: impl AsRef<OsStr>,
    args: &[String],
    subscription: bool,
    caller: impl FnOnce(&mut Proc),
) -> anyhow::Result<(Proc, Watch)> {
    let inner = tmp.map(PrivateTmp::launcher).unwrap_or_default();
    start(program.as_ref(), args, subscription, caller, |p| {
        memcap::wrapped(cap, Kind::Harness { subscription }, &inner, p)
    })
}

/// The program that gives a phase a private `/tmp`.
const BWRAP: &str = "bwrap";

/// Ok when bubblewrap can give a phase a private `/tmp` here, else why not.
pub fn private_tmp_works() -> Result<(), String> {
    if crate::which::which(BWRAP).is_none() {
        return Err(format!("{BWRAP} is not on PATH"));
    }
    let mut c = Proc::tool(BWRAP);
    c.args(["--dev-bind", "/", "/", "--bind"])
        .arg(std::env::temp_dir())
        .args(["/tmp", "--", "true"]);
    memcap::probe(c, BWRAP, memcap::PROBE_TIMEOUT).map(|_| ())
}

/// A phase's private `/tmp`: bubblewrap mounts `dir` on `/tmp`, then binds back each host `/tmp`
/// entry that holds a path in `keep`, so `TMPDIR`, the worktree and the ssh agent still resolve.
/// Same user, same `/proc` and `/dev`; only `/tmp` differs.
pub struct PrivateTmp {
    dir: PathBuf,
    keep: Vec<PathBuf>,
}

impl PrivateTmp {
    /// `dir` on `/tmp`, keeping `dir`, `paths`, and what this process's `HOME`, `PATH` and
    /// `SSH_AUTH_SOCK` name.
    pub fn new(dir: &Path, paths: &[&Path]) -> PrivateTmp {
        let mut keep: Vec<PathBuf> = paths.iter().map(|p| p.to_path_buf()).collect();
        keep.push(dir.to_path_buf());
        keep.extend(
            ["HOME", "SSH_AUTH_SOCK"]
                .iter()
                .filter_map(std::env::var_os)
                .map(PathBuf::from),
        );
        if let Some(path) = std::env::var_os("PATH") {
            keep.extend(std::env::split_paths(&path));
        }
        PrivateTmp {
            dir: dir.to_path_buf(),
            keep,
        }
    }

    /// The bubblewrap command the harness starts under.
    fn launcher(&self) -> Vec<String> {
        let mut c: Vec<String> = [BWRAP, "--dev-bind", "/", "/", "--die-with-parent", "--bind"]
            .map(String::from)
            .into();
        c.push(self.dir.to_string_lossy().into_owned());
        c.push("/tmp".to_string());
        for entry in tmp_entries(&self.keep).into_iter().filter(|e| e.exists()) {
            let entry = entry.to_string_lossy().into_owned();
            c.extend(["--bind".to_string(), entry.clone(), entry]);
        }
        c.push("--".to_string());
        c
    }
}

/// The host `/tmp` entries that hold `paths`, each once. `launcher` binds only those that exist:
/// bubblewrap refuses a missing source, which would fail every phase.
fn tmp_entries(paths: &[PathBuf]) -> BTreeSet<PathBuf> {
    paths
        .iter()
        .filter_map(
            |p| match p.strip_prefix("/tmp").ok()?.components().next()? {
                Component::Normal(first) => Some(Path::new("/tmp").join(first)),
                _ => None,
            },
        )
        .collect()
}

/// The status a program run under [`PrivateTmp`] ended with: bubblewrap exits 128 + n when the
/// program was killed by signal n.
pub fn unwrapped_status(status: ExitStatus) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    match status.code() {
        Some(c @ 129..=192) => ExitStatus::from_raw(c - 128),
        _ => status,
    }
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

    fn token_in(content: &[u8]) -> bool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        std::fs::write(&path, content).unwrap();
        holds_token(&path)
    }

    #[test]
    fn a_credentials_file_without_a_token_is_no_login() {
        for bad in [
            "",
            "{}",
            "not json",
            "[]",
            r#"{"claudeAiOauth":{}}"#,
            r#"{"claudeAiOauth":{"accessToken":""}}"#,
            r#"{"claudeAiOauth":{"accessToken":7}}"#,
            r#"{"claudeAiOauth":"t"}"#,
            r#"{"accessToken":"t"}"#,
        ] {
            assert!(!token_in(bad.as_bytes()), "{bad:?}");
        }
        assert!(!token_in(&[0xff, 0xfe]));
    }

    #[test]
    fn a_missing_or_unreadable_credentials_file_is_no_login() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!holds_token(&dir.path().join("absent.json")));
        assert!(!holds_token(dir.path()));
    }

    #[test]
    fn a_credentials_file_with_a_token_is_a_login() {
        assert!(token_in(br#"{"claudeAiOauth":{"accessToken":"t"}}"#));
        assert!(token_in(
            br#"{"other":1,"claudeAiOauth":{"accessToken":"t","refreshToken":"r"}}"#
        ));
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
        let (cmd, _) = capped(None, None, "claude", &[], false, set_off).unwrap();
        assert_eq!(env(&cmd, AUTO_MEMORY_ENV).as_deref(), Some("1"));
    }

    fn private(dir: &str, keep: &[&str]) -> PrivateTmp {
        PrivateTmp {
            dir: PathBuf::from(dir),
            keep: keep.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn a_private_tmp_mounts_the_phase_dir_and_binds_back_the_tmp_entries_it_keeps() {
        let host = tempfile::Builder::new()
            .prefix("ns-harness-host-")
            .tempdir_in("/tmp")
            .unwrap();
        let top = host.path().to_str().unwrap();
        std::fs::create_dir_all(host.path().join("ns-r-u/build-1")).unwrap();
        std::fs::create_dir_all(host.path().join("ssh-x")).unwrap();
        let phase = format!("{top}/ns-r-u/build-1");
        let keep = [
            phase.clone(),
            format!("{top}/ssh-x/agent.1"),
            format!("{top}/ssh-x/agent.2"),
            "/srv/home".to_string(),
            "/tmp".to_string(),
            "/tmp/../etc".to_string(),
            "/tmpfoo/bin".to_string(),
        ];
        let keep: Vec<&str> = keep.iter().map(String::as_str).collect();
        let (cmd, _) = capped(
            None,
            Some(&private(&phase, &keep)),
            "claude",
            &v(&["-p"]),
            false,
            |_| {},
        )
        .unwrap();
        assert_eq!(
            cmd.argv(),
            [
                "bwrap",
                "--dev-bind",
                "/",
                "/",
                "--die-with-parent",
                "--bind",
                &phase,
                "/tmp",
                "--bind",
                top,
                top,
                "--",
                "claude",
                "-p",
            ]
        );
        assert_eq!(env(&cmd, AUTO_MEMORY_ENV).as_deref(), Some("1"));
    }

    #[test]
    fn a_private_tmp_binds_back_only_the_tmp_entries_that_exist() {
        let here = tempfile::Builder::new()
            .prefix("ns-harness-keep-")
            .tempdir_in("/tmp")
            .unwrap();
        let gone = "/tmp/ns-harness-no-such-entry/agent.1";
        let keep_path = here.path().join("bin");
        let (cmd, _) = capped(
            None,
            Some(&private(
                "/tmp/ns-r-u/build-1",
                &[gone, keep_path.to_str().unwrap()],
            )),
            "claude",
            &v(&[]),
            false,
            |_| {},
        )
        .unwrap();
        let argv = cmd.argv();
        let here = here.path().to_str().unwrap();
        assert!(argv.iter().any(|a| a == here), "{argv:?}");
        assert!(
            !argv.iter().any(|a| a.contains("no-such-entry")),
            "{argv:?}"
        );
    }

    #[test]
    fn a_private_tmp_keeps_the_paths_it_is_given_and_its_own_dir() {
        let p = PrivateTmp::new(Path::new("/tmp/ns-r-u/build-1"), &[Path::new("/w")]);
        assert_eq!(p.dir, Path::new("/tmp/ns-r-u/build-1"));
        assert_eq!(p.keep[..2], [PathBuf::from("/w"), p.dir.clone()]);
        let env: Vec<PathBuf> = ["HOME", "SSH_AUTH_SOCK"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .chain(
                std::env::var_os("PATH")
                    .iter()
                    .flat_map(std::env::split_paths),
            )
            .collect();
        assert_eq!(p.keep[2..], env);
    }

    #[test]
    fn bwraps_exit_for_a_signal_reads_as_that_signal() {
        use std::os::unix::process::ExitStatusExt;
        let exited = |c: i32| ExitStatus::from_raw(c << 8);
        assert_eq!(unwrapped_status(exited(137)).signal(), Some(9));
        assert_eq!(unwrapped_status(exited(129)).signal(), Some(1));
        assert_eq!(unwrapped_status(exited(192)).signal(), Some(64));
        for c in [0, 1, 128, 193] {
            assert_eq!(unwrapped_status(exited(c)).code(), Some(c), "{c}");
        }
        let killed = ExitStatus::from_raw(9);
        assert_eq!(unwrapped_status(killed), killed);
    }

    #[test]
    fn another_harness_keeps_its_args_and_environment() {
        let (cmd, _) = capped(None, None, "codex", &v(&["exec", "--bare"]), true, |_| {}).unwrap();
        assert_eq!(args(&cmd), ["exec", "--bare"]);
        assert_eq!(env(&cmd, AUTO_MEMORY_ENV), None);
        assert_eq!(env(&cmd, BG_WAIT_ENV), None);
    }
}
