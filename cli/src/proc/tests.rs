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

#[test]
fn the_git_kinds_run_git() {
    for mut p in [Proc::git_local(), Proc::git_network()] {
        p.arg("--version");
        assert!(stdout(&p.output().unwrap()).starts_with("git version"));
    }
}

#[test]
fn stdin_is_closed_unless_the_caller_writes_to_it() {
    for kind in KINDS {
        let out = sh(kind, "readlink /proc/$$/fd/0").output().unwrap();
        assert_eq!(stdout(&out), "/dev/null\n", "{kind:?}");
    }
    let mut p = sh(Kind::Tool, "cat");
    p.stdin(b"hello".to_vec());
    assert_eq!(stdout(&p.output().unwrap()), "hello");
}

#[test]
fn a_child_that_exits_without_reading_its_stdin_still_returns() {
    let mut p = sh(Kind::Tool, "exit 4");
    p.stdin(vec![b'x'; 1 << 20]);
    assert_eq!(p.output().unwrap().status.code(), Some(4));
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
fn every_kind_has_a_timeout_its_children_start_with() {
    for kind in KINDS {
        assert!(kind.timeout() > Duration::ZERO, "{kind:?}");
        assert_eq!(Proc::new(kind, "sh").timeout, kind.timeout(), "{kind:?}");
    }
}

#[test]
fn a_command_past_its_timeout_fails_naming_the_command() {
    let mut p = Proc::tool("sleep");
    p.arg("600").timeout(Duration::ZERO);
    let e = p.output().unwrap_err();
    assert_eq!(e.kind(), ErrorKind::TimedOut);
    assert!(
        e.to_string().starts_with("`sleep 600` timed out after "),
        "{e}"
    );
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
