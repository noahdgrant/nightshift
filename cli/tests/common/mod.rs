//! The one way the integration tests start `ns`: in its own process group, bounded by a
//! timeout, with its process tree killed when the run ends. assert_cmd's own timeout kills only
//! the direct child and then waits on pipes a grandchild (the fake harness, fake `gh`, a
//! `sleep`) may hold open, so it can't bound a hang.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::assert::Assert;

/// How long one `ns` run may take before the test fails.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// How long a finished `ns` may take to close its output pipes before its stragglers are killed.
const POST_EXIT_GRACE: Duration = Duration::from_millis(500);

/// How long [`exits`] waits for a process to die.
pub const EXIT_WAIT: Duration = Duration::from_secs(5);

const TREE_VAR: &str = "NS_TEST_TREE";

static NEXT_TREE: AtomicU32 = AtomicU32::new(0);

/// `ns` built from this crate, bounded by [`TIMEOUT`].
pub fn ns() -> Ns {
    command(ns_path())
}

/// Where `ns` built from this crate is, for a script that a bounded `ns` runs to exec it.
pub fn ns_path() -> PathBuf {
    assert_cmd::cargo::cargo_bin("ns")
}

/// Any program, run with the same bounds as [`ns`].
pub fn command(program: impl AsRef<OsStr>) -> Ns {
    Ns {
        cmd: Command::new(program),
        stdin: Vec::new(),
        timeout: TIMEOUT,
    }
}

pub struct Ns {
    cmd: Command,
    stdin: Vec<u8>,
    timeout: Duration,
}

impl Ns {
    pub fn arg(&mut self, a: impl AsRef<OsStr>) -> &mut Self {
        self.cmd.arg(a);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.cmd.args(args);
        self
    }

    pub fn env(&mut self, k: impl AsRef<OsStr>, v: impl AsRef<OsStr>) -> &mut Self {
        self.cmd.env(k, v);
        self
    }

    pub fn env_remove(&mut self, k: impl AsRef<OsStr>) -> &mut Self {
        self.cmd.env_remove(k);
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.cmd.current_dir(dir);
        self
    }

    pub fn write_stdin(&mut self, input: impl Into<Vec<u8>>) -> &mut Self {
        self.stdin = input.into();
        self
    }

    pub fn timeout(&mut self, t: Duration) -> &mut Self {
        self.timeout = t;
        self
    }

    /// Start `ns` in the background with stdout discarded and stderr piped. The returned
    /// [`Group`] bounds its waits by this run's timeout.
    pub fn start(&mut self) -> Group {
        self.cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        Group::spawn(&mut self.cmd, self.timeout)
    }

    /// Start `ns` in the background with stdout and stderr piped, for a test that reads both.
    pub fn start_piped(&mut self) -> Group {
        self.cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Group::spawn(&mut self.cmd, self.timeout)
    }

    /// Run to the end and collect its output. Panics, naming the command, at the timeout.
    pub fn output(&mut self) -> Output {
        self.output_with_pid().0
    }

    /// [`Ns::output`], with the pid `ns` ran as.
    pub fn output_with_pid(&mut self) -> (Output, u32) {
        self.cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let shown = shown(&self.cmd);
        let timeout = self.timeout;
        let started = Instant::now();
        let (mut child, tag) = spawn_tagged(&mut self.cmd);
        let pid = child.id();
        let mut pipe = child.stdin.take().unwrap();
        let input = std::mem::take(&mut self.stdin);
        thread::spawn(move || pipe.write_all(&input));
        let stdout = drain(child.stdout.take().unwrap());
        let stderr = drain(child.stderr.take().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(child.wait());
        });
        let Ok(status) = rx.recv_timeout(timeout) else {
            kill_tree(Some(pid), &tag);
            panic!("{shown} timed out after {timeout:?}");
        };
        kill_tree(None, &tag);
        let collect = |rx: mpsc::Receiver<Vec<u8>>| {
            rx.recv_timeout(POST_EXIT_GRACE)
                .or_else(|_| {
                    kill_tree(None, &tag);
                    rx.recv_timeout(timeout.saturating_sub(started.elapsed()).max(POST_EXIT_GRACE))
                })
                .unwrap_or_else(|_| {
                    panic!("{shown} exited but a process it started still holds its output after {timeout:?}")
                })
        };
        let out = Output {
            status: status.unwrap(),
            stdout: collect(stdout),
            stderr: collect(stderr),
        };
        (out, pid)
    }

    #[track_caller]
    pub fn assert(&mut self) -> Assert {
        let out = self.output();
        Assert::new(out).append_context("command", shown(&self.cmd))
    }
}

fn shown(cmd: &Command) -> String {
    std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A child in its own process group. Waits on it are bounded, and dropping it kills its
/// process tree.
pub struct Group {
    child: Child,
    tag: String,
    shown: String,
    timeout: Duration,
    reaped: bool,
}

impl Group {
    pub fn spawn(cmd: &mut Command, timeout: Duration) -> Group {
        let (child, tag) = spawn_tagged(cmd);
        Group {
            child,
            tag,
            shown: shown(cmd),
            timeout,
            reaped: false,
        }
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn take_stderr(&mut self) -> ChildStderr {
        self.child.stderr.take().unwrap()
    }

    pub fn take_stdout(&mut self) -> ChildStdout {
        self.child.stdout.take().unwrap()
    }

    /// Kill the direct child only.
    pub fn kill(&mut self) {
        self.child.kill().unwrap();
    }

    /// Wait for the child to exit. Kills the tree and panics, naming the command, at the timeout.
    pub fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                self.reaped = true;
                return status;
            }
            self.expire_after(deadline, "to exit");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The next value on `rx`. Kills the tree and panics, naming the command, at the timeout.
    pub fn recv<T>(&self, rx: &mpsc::Receiver<T>) -> T {
        rx.recv_timeout(self.timeout).unwrap_or_else(|_| {
            self.kill_tree();
            panic!(
                "{} timed out after {:?} waiting for an event",
                self.shown, self.timeout
            )
        })
    }

    fn expire_after(&self, deadline: Instant, waiting_for: &str) {
        if Instant::now() >= deadline {
            self.kill_tree();
            panic!(
                "{} timed out after {:?} waiting {waiting_for}",
                self.shown, self.timeout
            );
        }
    }

    fn kill_tree(&self) {
        kill_tree((!self.reaped).then(|| self.child.id()), &self.tag);
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill_tree();
        if !self.reaped {
            let _ = self.child.wait();
        }
    }
}

fn spawn_tagged(cmd: &mut Command) -> (Child, String) {
    use std::os::unix::process::CommandExt;
    let tag = format!(
        "{}-{}",
        std::process::id(),
        NEXT_TREE.fetch_add(1, Ordering::Relaxed)
    );
    let child = cmd
        .env(TREE_VAR, &tag)
        .process_group(0)
        .spawn()
        .unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    (child, tag)
}

struct Proc {
    pid: u32,
    ppid: u32,
    pgid: u32,
}

fn processes() -> Vec<Proc> {
    let ps = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid="])
        .output()
        .expect("ps is required to kill a test's process tree");
    assert!(ps.status.success(), "ps failed: {ps:?}");
    String::from_utf8_lossy(&ps.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace().map(|n| n.parse().ok());
            Some(Proc {
                pid: f.next()??,
                ppid: f.next()??,
                pgid: f.next()??,
            })
        })
        .collect()
}

/// Pids whose environment carries `tag`, which finds a descendant reparented to init.
fn tagged(tag: &str) -> Vec<u32> {
    let entry = format!("{TREE_VAR}={tag}").into_bytes();
    std::fs::read_dir("/proc")
        .expect("/proc is required to kill a test's process tree")
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|p| {
            std::fs::read(format!("/proc/{p}/environ"))
                .is_ok_and(|env| env.split(|b| *b == 0).any(|e| e == entry))
        })
        .collect()
}

/// `root` and every process descended from it.
fn descendants(root: u32, procs: &[Proc]) -> BTreeSet<u32> {
    let mut tree = BTreeSet::from([root]);
    while let Some(p) = procs
        .iter()
        .find(|p| tree.contains(&p.ppid) && !tree.contains(&p.pid))
    {
        tree.insert(p.pid);
    }
    tree
}

/// Kill every process tagged `tag`, plus, while `running` is the still-unreaped child, it and
/// its descendants, and the process group of each. A reaped pid may belong to someone else by
/// now, so only the tag identifies what is left.
fn kill_tree(running: Option<u32>, tag: &str) {
    let procs = processes();
    let mut pids: BTreeSet<u32> = running
        .map(|root| descendants(root, &procs))
        .unwrap_or_default();
    pids.extend(tagged(tag));
    let mut groups: BTreeSet<u32> = procs
        .iter()
        .filter(|p| pids.contains(&p.pid))
        .map(|p| p.pgid)
        .collect();
    groups.extend(running);
    // SAFETY: getpgrp(2) has no failure mode.
    let own = unsafe { libc::getpgrp() } as u32;
    for g in groups.into_iter().filter(|g| *g > 1 && *g != own) {
        // SAFETY: kill(2) with a negative pid signals that process group; failures are ignored.
        unsafe {
            libc::kill(-(g as libc::pid_t), libc::SIGKILL);
        }
    }
    for p in pids
        .into_iter()
        .filter(|p| *p > 1 && *p != std::process::id())
    {
        // SAFETY: kill(2) on one pid; failures are ignored.
        unsafe {
            libc::kill(p as libc::pid_t, libc::SIGKILL);
        }
    }
}

/// Whether `pid` has exited, waiting up to [`EXIT_WAIT`] for it to. A zombie counts as exited.
pub fn exits(pid: &str) -> bool {
    let deadline = Instant::now() + EXIT_WAIT;
    loop {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout);
        if stat.trim().is_empty() || stat.trim().starts_with('Z') {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn drain(mut r: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}
