use std::ffi::CString;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;

const KINDS: [Kind; 6] = [
    Kind::GitLocal,
    Kind::GitNetwork,
    Kind::Harness { subscription: true },
    Kind::Harness {
        subscription: false,
    },
    Kind::Forge,
    Kind::Tool,
];

fn sh(kind: Kind, script: &str) -> Proc {
    let mut p = Proc::new(kind, "sh");
    p.arg("-c").arg(script);
    p
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `kind`'s child, started from a parent whose environment holds each of `vars`, prints its
/// environment.
fn child_env(kind: Kind, program: &str, vars: &[&str]) -> String {
    let mut p = Proc::inheriting(kind, program, vars.iter().map(|v| (*v, "/decoy")));
    p.arg("-c").arg("env");
    stdout(&p.output().unwrap())
}

/// All of `f` from its start: the child moved the offset this shared descriptor has.
fn read_back(f: &mut File) -> String {
    let mut text = String::new();
    f.seek(SeekFrom::Start(0)).unwrap();
    f.read_to_string(&mut text).unwrap();
    text
}

fn mkfifo(path: &Path) {
    let c = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: mkfifo(3) with a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

fn has_line(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.ends_with('\n'))
}

#[test]
fn every_kind_removes_the_inherited_git_location_variables() {
    for kind in KINDS {
        let env = child_env(kind, "sh", git::LOCATION_VARS);
        for v in git::LOCATION_VARS {
            assert!(!env.contains(&format!("{v}=")), "{kind:?} passed {v}");
        }
        assert!(env.contains("PATH="), "{kind:?}");
    }
}

#[test]
fn a_forge_child_such_as_token_command_never_sees_the_git_dir_of_its_parent() {
    let env = child_env(Kind::Forge, "sh", &["GIT_DIR", "GIT_WORK_TREE"]);
    assert!(!env.contains("GIT_DIR="), "{env}");
    assert!(!env.contains("GIT_WORK_TREE="), "{env}");
}

#[test]
fn a_git_location_variable_the_caller_sets_reaches_the_child() {
    let mut p = sh(Kind::GitLocal, "echo $GIT_INDEX_FILE");
    p.env("GIT_INDEX_FILE", "/scratch/index");
    assert_eq!(stdout(&p.output().unwrap()), "/scratch/index\n");
}

#[test]
fn a_subscription_harness_never_sees_the_api_billing_variables() {
    let dir = tempfile::tempdir().unwrap();
    for (name, vars) in [
        ("claude", billing::API_ENV),
        ("codex", billing::CODEX_API_ENV),
    ] {
        let bin = dir.path().join(name);
        crate::testutil::write_exe(&bin, "#!/bin/sh\nexec sh \"$@\"\n");
        let bin = bin.to_str().unwrap();
        let mut p = Proc::inheriting(
            Kind::Harness { subscription: true },
            bin,
            vars.iter().map(|v| (*v, "decoy")),
        );
        p.arg("-c").arg("env").env(vars[0], "set by the caller");
        let env = stdout(&p.output().unwrap());
        for v in vars {
            assert!(!env.contains(&format!("{v}=")), "{name} got {v}");
        }
        let api = child_env(
            Kind::Harness {
                subscription: false,
            },
            bin,
            vars,
        );
        for v in vars {
            assert!(api.contains(&format!("{v}=/decoy")), "{name} lost {v}");
        }
    }
}

#[test]
fn only_network_git_disables_credential_prompts() {
    for kind in KINDS {
        let env = child_env(kind, "sh", &[]);
        let network = kind == Kind::GitNetwork;
        assert_eq!(env.contains("GIT_TERMINAL_PROMPT=0\n"), network, "{kind:?}");
        assert_eq!(env.contains("GCM_INTERACTIVE=never\n"), network, "{kind:?}");
    }
}

#[test]
fn network_git_fails_instead_of_asking_for_a_password() {
    let mut p = Proc::git_network();
    p.args(["credential", "fill"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .stdin(b"protocol=https\nhost=example.com\n\n".to_vec());
    let out = p.output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("terminal prompts disabled"), "{err}");
}

/// An unattended run has no stdin and no terminal (#515): git asked for credentials must fail
/// at once rather than wait for an answer nobody can give.
#[test]
fn unattended_network_git_with_no_stdin_fails_fast_instead_of_prompting() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = conn.read(&mut buf);
            let _ = conn.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let mut p = Proc::git_network();
    p.arg("ls-remote")
        .arg(format!("http://127.0.0.1:{port}/repo.git"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS");
    let out = p.output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("terminal prompts disabled"), "{err}");
}

#[test]
fn the_git_kinds_run_git() {
    for mut p in [Proc::git_local(), Proc::git_network()] {
        p.arg("--version");
        assert!(stdout(&p.output().unwrap()).starts_with("git version"));
    }
}

const PROBE_MARK: &str = "NS_PROC_STDIN_PROBE";

#[test]
fn stdin_is_closed_unless_the_caller_writes_to_it() {
    let me = std::env::current_exe().unwrap();
    let out = command(me)
        .args(["--exact", "proc::tests::stdin_probe", "--test-threads=1"])
        .env(PROBE_MARK, "1")
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{log}");
    assert!(log.contains("1 passed"), "the probe did not run: {log}");
    let mut p = sh(Kind::Tool, "cat");
    p.stdin(b"hello".to_vec());
    assert_eq!(stdout(&p.output().unwrap()), "hello");
}

/// Run by the test above in a process whose own stdin is a pipe, so a child that inherited it
/// would not see `/dev/null`.
#[test]
fn stdin_probe() {
    if std::env::var_os(PROBE_MARK).is_none() {
        return;
    }
    let own = std::fs::read_link("/proc/self/fd/0").unwrap();
    assert!(own.to_string_lossy().starts_with("pipe:"), "{own:?}");
    for kind in KINDS {
        let out = sh(kind, "readlink /proc/$$/fd/0").output().unwrap();
        assert_eq!(stdout(&out), "/dev/null\n", "{kind:?}");
    }
}

#[test]
fn a_child_that_exits_without_reading_its_stdin_still_returns() {
    let mut p = sh(Kind::Tool, "exit 4");
    p.stdin(vec![b'x'; 1 << 20]);
    assert_eq!(p.output().unwrap().status.code(), Some(4));
}

struct KillOnDrop(PathBuf);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = std::fs::read_to_string(&self.0)
            .ok()
            .and_then(|s| s.trim().parse::<libc::pid_t>().ok())
        {
            // SAFETY: kill(2) on a pid the test's own child recorded.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

#[test]
fn a_descendant_in_another_session_holding_stdin_does_not_hang_the_call() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let _cleanup = KillOnDrop(pidfile.clone());
    let mut p = sh(
        Kind::Tool,
        r#"exec 3<&0
           setsid sh -c 'echo $$ > "$0.tmp"; mv "$0.tmp" "$0"; exec sleep 600' "$0" <&3 &
           until [ -s "$0" ]; do :; done"#,
    );
    p.arg(&pidfile).stdin(vec![b'x'; 1 << 20]);
    assert!(p.output().unwrap().status.success());
}

#[test]
fn the_output_and_exit_status_come_back() {
    let out = sh(Kind::Tool, "echo out; echo err >&2; exit 3")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(out.stdout, b"out\n");
    assert_eq!(out.stderr, b"err\n");
}

#[test]
fn the_output_comes_back_when_a_descendant_holds_the_pipe() {
    let out = sh(Kind::Tool, "sleep 600 & echo hi; exit 3")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(out.stdout, b"hi\n");
}

#[test]
fn a_missing_binary_is_the_spawn_error() {
    let e = Proc::tool("ns-no-such-binary").output().unwrap_err();
    assert_eq!(e.kind(), ErrorKind::NotFound);
}

#[test]
fn every_kind_starts_its_children_with_its_default_timeout() {
    let want = [
        (Kind::GitLocal, 600),
        (Kind::GitNetwork, 600),
        (Kind::Harness { subscription: true }, 7200),
        (
            Kind::Harness {
                subscription: false,
            },
            7200,
        ),
        (Kind::Forge, 120),
        (Kind::Tool, 1800),
    ];
    assert_eq!(want.map(|(k, _)| k), KINDS);
    for (kind, secs) in want {
        let limit = Duration::from_secs(secs);
        assert_eq!(kind.timeout(), limit, "{kind:?}");
        assert_eq!(Proc::new(kind, "sh").timeout, limit, "{kind:?}");
    }
}

#[test]
fn a_child_that_ends_before_its_deadline_is_left_alone() {
    let mut p = sh(Kind::Tool, "echo done");
    p.timeout(Duration::from_secs(u64::from(u16::MAX)));
    assert_eq!(stdout(&p.output().unwrap()), "done\n");
}

#[test]
fn a_command_past_its_timeout_fails_naming_the_command() {
    let mut p = Proc::tool("sleep");
    p.arg("600").timeout(Duration::ZERO);
    let e = p.output().unwrap_err();
    assert_eq!(e.kind(), ErrorKind::TimedOut);
    assert_eq!(e.to_string(), "`sleep 600` timed out after 0s");
}

/// The deadline always applies: a hook that never holds does not keep the child running.
#[test]
fn the_deadline_fires_when_the_early_hook_never_does() {
    let mut p = sh(Kind::Tool, "sleep 600");
    p.timeout(Duration::ZERO).expire_when(|| false);
    assert_eq!(p.output().unwrap_err().kind(), ErrorKind::TimedOut);
}

/// A timeout kills the whole process group: once the test stops waiting for the child, the
/// write end of a FIFO held by the child and its background descendant reaches EOF.
#[test]
fn a_timeout_kills_the_process_group_descendants_included() {
    let dir = tempfile::tempdir().unwrap();
    let (fifo, reader) = held_fifo(dir.path());
    let pidfile = dir.path().join("pid");
    let mut p = sh(
        Kind::Tool,
        r#"exec 3>"$0"; sleep 600 & echo $! > "$1"; sleep 600"#,
    );
    p.arg(&fifo).arg(&pidfile);
    let ready = pidfile.clone();
    p.expire_when(move || has_line(&ready));
    assert_eq!(p.output().unwrap_err().kind(), ErrorKind::TimedOut);
    assert!(has_line(&pidfile), "killed before the descendant started");
    wait_for_eof(reader);
}

/// A FIFO in `dir` and its read end, opened before any writer so the open does not block.
fn held_fifo(dir: &Path) -> (PathBuf, File) {
    let fifo = dir.join("fifo");
    mkfifo(&fifo);
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
        .unwrap();
    (fifo, reader)
}

/// Block until every writer of the FIFO has closed it: every process holding it has exited.
fn wait_for_eof(mut reader: File) {
    blocking(&reader);
    let mut rest = Vec::new();
    reader.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty());
}

#[test]
fn a_descendant_left_running_after_a_normal_exit_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let (fifo, reader) = held_fifo(dir.path());
    let mut p = sh(Kind::Tool, r#"exec 3>"$0"; sleep 600 & exit 0"#);
    p.arg(&fifo);
    assert!(p.output().unwrap().status.success());
    wait_for_eof(reader);
}

fn blocking(f: &File) {
    use std::os::fd::AsRawFd;
    // SAFETY: fcntl(2) on a descriptor the File owns.
    unsafe {
        let flags = libc::fcntl(f.as_raw_fd(), libc::F_GETFL);
        assert_ne!(
            libc::fcntl(f.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK),
            -1
        );
    }
}

#[test]
fn a_timeout_leaves_a_concurrent_child_running() {
    let dir = tempfile::tempdir().unwrap();
    let (fifo, ready): (PathBuf, PathBuf) = (dir.path().join("go"), dir.path().join("ready"));
    mkfifo(&fifo);
    let mut other = sh(Kind::Tool, r#"echo up > "$0"; read x < "$1"; echo "$x""#);
    other.arg(&ready).arg(&fifo);
    let other = std::thread::spawn(move || other.output().unwrap());
    let mut p = Proc::tool("sleep");
    p.arg("600");
    let seen = ready.clone();
    p.expire_when(move || has_line(&seen));
    assert_eq!(p.output().unwrap_err().kind(), ErrorKind::TimedOut);
    File::options()
        .write(true)
        .open(&fifo)
        .unwrap()
        .write_all(b"survived\n")
        .unwrap();
    let out = other.join().unwrap();
    assert!(out.status.success());
    assert_eq!(stdout(&out), "survived\n");
}

#[test]
fn each_constructor_picks_its_kind() {
    let kinds = [
        (Proc::git_local().kind, Kind::GitLocal),
        (Proc::git_network().kind, Kind::GitNetwork),
        (
            Proc::harness("claude", true).kind,
            Kind::Harness { subscription: true },
        ),
        (
            Proc::harness("claude", false).kind,
            Kind::Harness {
                subscription: false,
            },
        ),
        (Proc::forge("gh").kind, Kind::Forge),
        (Proc::tool("sh").kind, Kind::Tool),
    ];
    for (got, want) in kinds {
        assert_eq!(got, want);
    }
}

#[test]
fn the_child_runs_in_the_directory_the_caller_names() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = sh(Kind::Tool, "pwd -P");
    p.current_dir(dir.path());
    let want = dir.path().canonicalize().unwrap();
    assert_eq!(
        stdout(&p.output().unwrap()).trim_end(),
        want.to_str().unwrap()
    );
}

/// What [`Proc::exec`] relies on: SIGINT and SIGTERM blocked before an exec stay blocked, and
/// one that lands in between stays pending for the new program.
#[test]
fn exec_holds_a_stop_pending_through_to_the_new_program() {
    use std::os::unix::process::CommandExt;
    let mut cmd = command("grep");
    cmd.args(["-E", "^(SigBlk|SigPnd|ShdPnd)", "/proc/self/status"]);
    // SAFETY: only pthread_sigmask(3) and kill(2) in the closure.
    unsafe {
        cmd.pre_exec(|| {
            crate::stop::block();
            libc::kill(libc::getpid(), libc::SIGTERM);
            Ok(())
        });
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let mask = |key: &str| {
        let line = text.lines().find(|l| l.starts_with(key)).unwrap();
        u64::from_str_radix(line.split_whitespace().nth(1).unwrap(), 16).unwrap()
    };
    let (int, term) = (1 << (libc::SIGINT - 1), 1 << (libc::SIGTERM - 1));
    assert_eq!(mask("SigBlk") & (int | term), int | term, "{text}");
    assert_eq!((mask("SigPnd") | mask("ShdPnd")) & term, term, "{text}");
}

#[test]
fn output_to_sends_both_streams_to_the_file_as_the_child_writes() {
    let mut log = tempfile::tempfile().unwrap();
    let mut p = sh(Kind::Tool, "echo out; echo err >&2; exit 3");
    let out = p.output_to(log.try_clone().unwrap()).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    let text = read_back(&mut log);
    assert_eq!(text, "out\nerr\n");
}

#[test]
fn a_stop_kills_the_child_and_fails_naming_it() {
    let mut p = Proc::tool("sleep");
    p.arg("600").stop_when(|| true);
    let e = p.output().unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Interrupted);
    assert_eq!(e.to_string(), "`sleep 600` stopped by a signal");
}

#[test]
fn a_stop_beats_an_expired_deadline() {
    let mut p = Proc::tool("sleep");
    p.arg("600").timeout(Duration::ZERO).stop_when(|| true);
    let e = p.output().unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Interrupted);
}

#[test]
fn a_child_that_ends_before_a_stop_is_left_alone() {
    let mut p = sh(Kind::Tool, "echo done");
    p.stop_when(|| false);
    assert_eq!(stdout(&p.output().unwrap()), "done\n");
}

#[test]
fn until_stop_ends_the_child_only_when_a_stop_is_requested() {
    let mut p = sh(Kind::Tool, "echo done");
    p.until_stop();
    assert_eq!(stdout(&p.output().unwrap()), "done\n");
}

#[test]
fn envs_reach_the_child() {
    let mut p = sh(Kind::Tool, r#"echo "$A $B""#);
    p.envs([("A", "1"), ("B", "2")]);
    assert_eq!(stdout(&p.output().unwrap()), "1 2\n");
}

/// A spawned child gets its kind's scrub and closed stdin, writes stdout to the file the caller
/// gives, and leads a session of its own.
#[test]
fn a_spawned_child_runs_scrubbed_in_its_own_session() {
    let mut log = tempfile::tempfile().unwrap();
    let vars = git::LOCATION_VARS.iter().map(|v| (*v, "/decoy"));
    let mut p = Proc::inheriting(Kind::Tool, "sh", vars);
    p.arg("-c")
        .arg(r#"read line; echo "read=$?"; env; ps -o sid= -p $$"#);
    let mut child = p.spawn(log.try_clone().unwrap()).unwrap();
    let pid = child.id();
    assert!(child.wait().unwrap().success());
    let text = read_back(&mut log);
    assert!(text.starts_with("read=1\n"), "{text}");
    for v in git::LOCATION_VARS {
        assert!(!text.contains(&format!("{v}=")), "passed {v}");
    }
    assert_eq!(text.lines().last().unwrap().trim(), pid.to_string());
}

#[test]
fn a_spawned_subscription_harness_never_sees_the_api_billing_variables() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("claude");
    crate::testutil::write_exe(&bin, "#!/bin/sh\nexec sh \"$@\"\n");
    let vars = billing::API_ENV.iter().map(|v| (*v, "decoy"));
    let mut p = Proc::inheriting(
        Kind::Harness { subscription: true },
        bin.to_str().unwrap(),
        vars,
    );
    p.arg("-c").arg("env");
    let mut log = tempfile::tempfile().unwrap();
    let mut child = p.spawn(log.try_clone().unwrap()).unwrap();
    assert!(child.wait().unwrap().success());
    let env = read_back(&mut log);
    for v in billing::API_ENV {
        assert!(!env.contains(&format!("{v}=")), "got {v}");
    }
}

const SPAWN_MARK: &str = "NS_PROC_SPAWN_STDERR_PROBE";

#[test]
fn a_spawned_child_writes_its_stderr_to_the_parents() {
    let out = crate::testutil::probe("proc::tests::spawn_stderr_probe", SPAWN_MARK, "1");
    let log = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("spawned-child-stderr\n"), "{err}");
    assert!(!log.contains("spawned-child-stderr"), "{log}");
}

#[test]
fn spawn_stderr_probe() {
    if std::env::var_os(SPAWN_MARK).is_none() {
        return;
    }
    let mut p = sh(Kind::Tool, "echo spawned-child-stderr >&2");
    let mut child = p.spawn(tempfile::tempfile().unwrap()).unwrap();
    assert!(child.wait().unwrap().success());
}

#[cfg(target_os = "linux")]
#[test]
fn a_spawned_child_gets_sigterm_when_the_thread_that_started_it_ends() {
    use std::os::unix::process::ExitStatusExt;
    let out = tempfile::tempfile().unwrap();
    let mut child =
        std::thread::spawn(move || sh(Kind::Tool, "exec sleep 600").spawn(out).unwrap())
            .join()
            .unwrap();
    let pid = child.id() as libc::pid_t;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(child.wait().unwrap()));
    let Ok(status) = rx.recv_timeout(Kind::Tool.timeout()) else {
        // The waiter hasn't reaped the child, so its pid is still ours to kill.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        panic!("the child outlived the thread that started it");
    };
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}

#[test]
fn polling_starts_at_a_millisecond_and_doubles_up_to_twenty() {
    let mut delay = FIRST_POLL;
    let mut seen = vec![delay];
    for _ in 0..7 {
        delay = next_poll(delay);
        seen.push(delay);
    }
    let ms: Vec<u128> = seen.iter().map(Duration::as_millis).collect();
    assert_eq!(ms, [1, 2, 4, 8, 16, 20, 20, 20]);
}

#[test]
fn scrub_billing_removes_every_codex_api_variable_and_no_anthropic_one() {
    let argv = ["codex".to_string()];
    let mut cmd = command("sh");
    cmd.arg("-c").arg("env");
    for v in billing::CODEX_API_ENV.iter().chain(billing::API_ENV) {
        cmd.env(v, "decoy");
    }
    scrub_billing(&mut cmd, &argv);
    let env = String::from_utf8_lossy(&cmd.output().unwrap().stdout).into_owned();
    for v in billing::CODEX_API_ENV {
        assert!(!env.contains(&format!("{v}=")), "kept {v}");
    }
    for v in billing::API_ENV
        .iter()
        .filter(|v| v.starts_with("ANTHROPIC"))
    {
        assert!(env.contains(&format!("{v}=decoy")), "removed {v}");
    }
}

#[test]
fn scrub_git_removes_every_git_location_variable() {
    let mut cmd = command("sh");
    cmd.arg("-c").arg("env");
    for v in git::LOCATION_VARS {
        cmd.env(v, "/decoy");
    }
    cmd.env("GIT_AUTHOR_NAME", "kept");
    scrub_git(&mut cmd);
    let env = String::from_utf8_lossy(&cmd.output().unwrap().stdout).into_owned();
    for v in git::LOCATION_VARS {
        assert!(!env.contains(&format!("{v}=")), "kept {v}");
    }
    assert!(env.contains("GIT_AUTHOR_NAME=kept"), "{env}");
}

const EXEC_MARK: &str = "NS_PROC_EXEC_PROBE";

fn run_exec_probe(mode: &str, envs: &[(&str, &str)]) -> String {
    let out = crate::testutil::probe_command("proc::tests::exec_probe", EXEC_MARK, mode)
        .envs(envs.iter().copied())
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn exec_probe() {
    let Ok(mode) = std::env::var(EXEC_MARK) else {
        return;
    };
    match mode.as_str() {
        "stopped" => {
            crate::stop::request(libc::SIGTERM);
            let e = Proc::tool("true").exec();
            println!("{:?}", e.kind());
        }
        "blocked" => {
            let e = Proc::tool("grep")
                .args(["-E", "^SigBlk", "/proc/self/status"])
                .exec();
            panic!("exec failed: {e}");
        }
        "billing" => {
            let bin = std::env::var("NS_PROC_EXEC_BIN").unwrap();
            let e = Proc::harness(bin, true).arg("-c").arg("env").exec();
            panic!("exec failed: {e}");
        }
        other => panic!("unknown mode {other}"),
    }
}

#[test]
fn exec_fails_as_interrupted_when_a_stop_was_already_requested() {
    let text = run_exec_probe("stopped", &[]);
    assert!(text.contains("... Interrupted\n"), "{text}");
}

#[test]
fn exec_runs_the_program_with_sigint_and_sigterm_blocked() {
    let text = run_exec_probe("blocked", &[]);
    let line = text.lines().find(|l| l.contains("SigBlk:")).unwrap();
    let mask = u64::from_str_radix(line.split_whitespace().last().unwrap(), 16).unwrap();
    let (int, term) = (1 << (libc::SIGINT - 1), 1 << (libc::SIGTERM - 1));
    assert_eq!(mask & (int | term), int | term, "{text}");
}

#[test]
fn exec_scrubs_the_api_billing_variables_for_a_subscription_harness() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("claude");
    crate::testutil::write_exe(&bin, "#!/bin/sh\nexec sh \"$@\"\n");
    let mut envs = vec![("NS_PROC_EXEC_BIN", bin.to_str().unwrap())];
    envs.extend(billing::API_ENV.iter().map(|v| (*v, "decoy")));
    let env = run_exec_probe("billing", &envs);
    assert!(env.contains("PATH="), "{env}");
    for v in billing::API_ENV {
        assert!(!env.contains(&format!("{v}=")), "kept {v}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn spawn_fails_when_the_parent_it_was_started_for_is_gone() {
    let not_me = std::process::id() as libc::pid_t + 1;
    let e = sh(Kind::Tool, "exit 0")
        .spawn_for(not_me, tempfile::tempfile().unwrap())
        .unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::ESRCH), "{e}");
}

const DIES_WITH_PARENT_MARK: &str = "NS_PROC_DIES_WITH_PARENT_PROBE";

#[cfg(target_os = "linux")]
#[test]
fn a_git_child_of_a_process_killed_by_a_signal_does_not_outlive_it() {
    let dir = tempfile::tempdir().unwrap();
    let (fifo, reader) = held_fifo(dir.path());
    let mut probe = crate::testutil::probe_command(
        "proc::tests::dies_with_parent_probe",
        DIES_WITH_PARENT_MARK,
        fifo.to_str().unwrap(),
    );
    let mut probe = probe.spawn().unwrap();
    // Opening for read blocks until the git child opens the FIFO for write.
    let mut ready = File::open(&fifo).unwrap();
    let mut seen = [0u8; 1];
    while seen[0] != b'\n' {
        assert_eq!(ready.read(&mut seen).unwrap(), 1, "the git child never ran");
    }
    drop(ready);
    probe.kill().unwrap();
    probe.wait().unwrap();
    wait_for_eof(reader);
}

/// Run by the test above: waits in `output()` on a fake `git` that holds the FIFO open.
#[test]
fn dies_with_parent_probe() {
    let Some(fifo) = std::env::var_os(DIES_WITH_PARENT_MARK) else {
        return;
    };
    let body = format!(
        "exec 3>'{}'\necho ready >&3\nexec sleep 600",
        fifo.to_string_lossy()
    );
    let _git = crate::testutil::fake_on_path("git", &body);
    for mut p in [Proc::git_local(), Proc::git_network()] {
        let _ = p.output();
    }
}

#[test]
fn stderr_to_sends_stderr_to_its_own_file_and_stdout_comes_back() {
    let mut err = tempfile::tempfile().unwrap();
    let mut p = sh(Kind::Tool, "echo out; echo err >&2");
    let out = p.stderr_to(err.try_clone().unwrap()).output().unwrap();
    assert_eq!(stdout(&out), "out\n");
    assert!(out.stderr.is_empty());
    assert_eq!(read_back(&mut err), "err\n");
}

#[test]
fn output_to_with_stderr_to_splits_the_streams() {
    let (mut log, mut err) = (tempfile::tempfile().unwrap(), tempfile::tempfile().unwrap());
    let mut p = sh(Kind::Tool, "echo out; echo err >&2");
    p.output_to(log.try_clone().unwrap())
        .stderr_to(err.try_clone().unwrap());
    p.output().unwrap();
    assert_eq!(read_back(&mut log), "out\n");
    assert_eq!(read_back(&mut err), "err\n");
}

#[test]
fn a_spawned_child_writes_stderr_to_the_file_stderr_to_names() {
    let (mut log, mut err) = (tempfile::tempfile().unwrap(), tempfile::tempfile().unwrap());
    let mut p = sh(Kind::Tool, "echo out; echo err >&2");
    p.stderr_to(err.try_clone().unwrap());
    let mut child = p.spawn(log.try_clone().unwrap()).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(read_back(&mut log), "out\n");
    assert_eq!(read_back(&mut err), "err\n");
}

#[test]
fn a_launched_program_runs_under_its_launcher_with_its_kinds_policy() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("claude");
    crate::testutil::write_exe(&bin, "#!/bin/sh\nexec sh \"$@\"\n");
    let launcher = ["env".to_string(), "LAUNCHED=1".to_string()];
    let mut p = Proc::launched(Kind::Harness { subscription: true }, &launcher, &bin);
    p.arg("-c")
        .arg("env")
        .env(billing::API_ENV[0], "set by the caller")
        .env("GIT_DIR", "/decoy");
    let env = stdout(&p.output().unwrap());
    assert!(env.contains("LAUNCHED=1\n"), "{env}");
    assert!(env.contains("GIT_DIR=/decoy\n"), "{env}");
    for v in billing::API_ENV {
        assert!(!env.contains(&format!("{v}=")), "got {v}");
    }
    assert_eq!(p.kind, Kind::Harness { subscription: true });
}

#[test]
fn launched_with_no_launcher_runs_the_program_itself() {
    let mut p = Proc::launched(Kind::Tool, &[], "sh");
    p.arg("-c").arg("echo $0");
    assert_eq!(stdout(&p.output().unwrap()), "sh\n");
}

/// The file whose line says the child of the SIGTERM test has set its trap.
static TRAPPED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

#[test]
fn a_stop_asks_the_group_to_end_before_it_kills_it() {
    let dir = tempfile::tempdir().unwrap();
    let (marker, trapped) = (dir.path().join("termed"), dir.path().join("trapped"));
    TRAPPED.set(trapped.clone()).unwrap();
    let mut p = sh(
        Kind::Tool,
        &format!(
            "trap 'echo term > {}; exit 0' TERM; echo up > {}; while :; do sleep 0.05; done",
            marker.display(),
            trapped.display()
        ),
    );
    p.stop_when(|| TRAPPED.get().is_some_and(|t| has_line(t)));
    assert_eq!(p.output().unwrap_err().kind(), ErrorKind::Interrupted);
    assert!(has_line(&marker), "the child never got SIGTERM");
}

#[test]
fn a_stop_kills_a_group_that_ignores_sigterm_after_the_grace() {
    let mut p = sh(Kind::Tool, "trap '' TERM; exec sleep 600");
    p.stop_when(|| true).grace(Duration::ZERO);
    assert_eq!(p.output().unwrap_err().kind(), ErrorKind::Interrupted);
}

#[test]
fn run_reports_the_status_and_how_long_the_child_ran() {
    let ended = sh(Kind::Tool, "exit 3").run().unwrap();
    assert_eq!(ended.status.and_then(|s| s.code()), Some(3));
    assert!(!ended.timed_out());
    assert!(ended.wall_s > 0.0);
}

#[test]
fn run_reports_a_timeout_as_no_status() {
    let mut p = sh(Kind::Tool, "exec sleep 600");
    let ended = p.timeout(Duration::ZERO).run().unwrap();
    assert!(ended.timed_out());
    assert!(ended.status.is_none());
}

#[test]
fn run_ends_the_child_when_a_stop_is_requested() {
    let mut p = Proc::tool("sleep");
    p.arg("600").stop_when(|| true);
    assert_eq!(p.run().unwrap_err().kind(), ErrorKind::Interrupted);
}

#[test]
fn run_fails_on_a_missing_binary_as_not_found() {
    let e = Proc::tool("/nonexistent/ns-proc-test").run().unwrap_err();
    assert_eq!(e.kind(), ErrorKind::NotFound);
}

#[test]
fn the_child_runs_in_its_own_session_and_group() {
    let out = sh(Kind::Tool, "ps -o sid=,pgid= -p $$").output().unwrap();
    let text = stdout(&out);
    let got: Vec<i32> = text
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    // SAFETY: getsid and getpgrp only read this process's ids.
    let (sid, pgid) = unsafe { (libc::getsid(0), libc::getpgrp()) };
    assert_eq!(got.len(), 2, "{text}");
    assert_ne!(got[0], sid, "shares the caller's session");
    assert_ne!(got[1], pgid, "shares the caller's process group");
}

#[test]
fn run_watches_for_a_stop_request_unless_the_caller_set_its_own() {
    let mut p = sh(Kind::Tool, "true");
    assert!(p.stopped.is_none());
    p.run().unwrap();
    assert!(p.stopped.is_some(), "run left the child deaf to a stop");
}
