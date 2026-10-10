//! The one way the integration tests start `ns`: in its own process group, bounded by a
//! timeout, with its process tree killed when the run ends. assert_cmd's own timeout kills only
//! the direct child and then waits on pipes a grandchild (the fake harness, fake `gh`, a
//! `sleep`) may hold open, so it can't bound a hang.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use assert_cmd::assert::Assert;

/// How long one `ns` run may take before the test fails.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// `ns` built from this crate, bounded by [`TIMEOUT`].
pub fn ns() -> Ns {
    Ns {
        cmd: Command::new(assert_cmd::cargo::cargo_bin("ns")),
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

    /// Start `ns` in the background with stdout discarded and stderr piped.
    pub fn start(&mut self) -> Group {
        self.cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        Group(spawn_in_group(&mut self.cmd))
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
        let mut child = spawn_in_group(&mut self.cmd);
        let pid = child.id();
        let mut pipe = child.stdin.take().unwrap();
        let input = std::mem::take(&mut self.stdin);
        // ns may exit before reading everything; its exit status tells the story.
        thread::spawn(move || pipe.write_all(&input));
        let stdout = drain(child.stdout.take().unwrap());
        let stderr = drain(child.stderr.take().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(child.wait());
        });
        let Ok(status) = rx.recv_timeout(self.timeout) else {
            kill_tree(pid);
            panic!("{:?} timed out after {:?}", self.cmd, self.timeout);
        };
        // Reap anything ns left running in its group so the pipes close.
        kill_tree(pid);
        let out = Output {
            status: status.unwrap(),
            stdout: stdout.recv().unwrap(),
            stderr: stderr.recv().unwrap(),
        };
        (out, pid)
    }

    #[track_caller]
    pub fn assert(&mut self) -> Assert {
        let out = self.output();
        Assert::new(out).append_context("command", format!("{:?}", self.cmd))
    }
}

/// A child in its own process group. Dropping it kills its process tree.
pub struct Group(pub Child);

impl Drop for Group {
    fn drop(&mut self) {
        kill_tree(self.0.id());
        let _ = self.0.wait();
    }
}

pub fn spawn_in_group(cmd: &mut Command) -> Child {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0)
        .spawn()
        .unwrap_or_else(|e| panic!("{cmd:?}: {e}"))
}

/// Kill the process group led by `pid` and the group of every process descended from it, which
/// reaches a phase `ns run` starts in a session of its own.
fn kill_tree(pid: u32) {
    let ps = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid="])
        .output()
        .unwrap();
    let rows: Vec<[u32; 3]> = String::from_utf8_lossy(&ps.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace().map(|n| n.parse().ok());
            Some([f.next()??, f.next()??, f.next()??])
        })
        .collect();
    let mut tree = BTreeSet::from([pid]);
    let mut groups = BTreeSet::from([pid]);
    while let Some([child, _, group]) = rows
        .iter()
        .find(|[child, parent, _]| tree.contains(parent) && !tree.contains(child))
    {
        tree.insert(*child);
        groups.insert(*group);
    }
    for g in groups {
        // SAFETY: kill(2) with a negative pid signals that process group; failures are ignored.
        unsafe {
            libc::kill(-(g as libc::pid_t), libc::SIGKILL);
        }
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
