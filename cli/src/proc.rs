//! The one way to start a child process. A caller picks a [`Kind`], which fixes the policy:
//! the environment scrub, a timeout that kills the whole process group, stdin closed unless
//! the caller writes to it, and no credential prompts for network git (#556).
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "call sites move here in #580 and #581")
)]

use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use crate::{billing, git};

const MINUTE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// git on a local repo.
    GitLocal,
    /// git that talks to a remote: it fails rather than asking for credentials (#495).
    GitNetwork,
    /// A model harness (claude, codex). Under `subscription` it never sees the variables that
    /// would bill an API instead.
    Harness { subscription: bool },
    /// A forge CLI (`gh`, `glab`) or a forge's `token_command`.
    Forge,
    /// Any other program: a build, a test run, a script.
    Tool,
}

impl Kind {
    /// How long a child of this kind runs before its process group is killed.
    pub fn timeout(self) -> Duration {
        match self {
            Kind::GitLocal | Kind::GitNetwork => 10 * MINUTE,
            Kind::Harness { .. } => 120 * MINUTE,
            Kind::Forge => 2 * MINUTE,
            Kind::Tool => 30 * MINUTE,
        }
    }
}

/// A child process to run under its kind's policy.
pub struct Proc {
    kind: Kind,
    cmd: Command,
    timeout: Duration,
    stdin: Option<Vec<u8>>,
    expire_when: Option<Box<dyn Fn() -> bool + Send>>,
}

impl Proc {
    pub fn git_local() -> Self {
        Self::new(Kind::GitLocal, "git")
    }

    pub fn git_network() -> Self {
        Self::new(Kind::GitNetwork, "git")
    }

    pub fn harness(program: impl AsRef<OsStr>, subscription: bool) -> Self {
        Self::new(Kind::Harness { subscription }, program)
    }

    pub fn forge(program: impl AsRef<OsStr>) -> Self {
        Self::new(Kind::Forge, program)
    }

    pub fn tool(program: impl AsRef<OsStr>) -> Self {
        Self::new(Kind::Tool, program)
    }

    /// The inherited git location variables are removed here, so one the caller sets with
    /// [`Proc::env`] still reaches the child (a scratch `GIT_INDEX_FILE`).
    fn new(kind: Kind, program: impl AsRef<OsStr>) -> Self {
        Self::with(kind, Command::new(program))
    }

    /// As [`Proc::new`], for a parent whose environment also holds `vars`.
    #[cfg(test)]
    fn inheriting<'a>(
        kind: Kind,
        program: &str,
        vars: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        let mut cmd = Command::new(program);
        cmd.envs(vars);
        Self::with(kind, cmd)
    }

    fn with(kind: Kind, mut cmd: Command) -> Self {
        git::scrub(&mut cmd);
        if kind == Kind::GitNetwork {
            cmd.env("GIT_TERMINAL_PROMPT", "0")
                .env("GCM_INTERACTIVE", "never");
        }
        Proc {
            kind,
            cmd,
            timeout: kind.timeout(),
            stdin: None,
            expire_when: None,
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.cmd.arg(arg);
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

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.cmd.current_dir(dir);
        self
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.cmd.env(key, value);
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.cmd.env_remove(key);
        self
    }

    /// Replace the kind's timeout for this call. There is no way to run without one.
    pub fn timeout(&mut self, limit: Duration) -> &mut Self {
        self.timeout = limit;
        self
    }

    /// Write `bytes` to the child's stdin, then close it. Without this, stdin is `/dev/null`.
    pub fn stdin(&mut self, bytes: Vec<u8>) -> &mut Self {
        self.stdin = Some(bytes);
        self
    }

    /// Time out as soon as `expired` holds, so a test can end a child after an event.
    #[cfg(test)]
    fn expire_when(&mut self, expired: impl Fn() -> bool + Send + 'static) -> &mut Self {
        self.expire_when = Some(Box::new(expired));
        self
    }

    /// `program arg...`, for an error.
    fn describe(&self) -> String {
        std::iter::once(self.cmd.get_program())
            .chain(self.cmd.get_args())
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Run the child to its end in its own session and process group, then kill whatever it
    /// left running there. Its output goes to temp files read after it exits, so a descendant
    /// holding a pipe open can't delay or drop it. A child past its timeout has its group
    /// killed and fails with [`std::io::ErrorKind::TimedOut`].
    pub fn output(&mut self) -> std::io::Result<Output> {
        if let Kind::Harness { subscription: true } = self.kind {
            let program = self.cmd.get_program().to_string_lossy().into_owned();
            billing::scrub(&mut self.cmd, &[program]);
        }
        let mut out = tempfile::tempfile()?;
        let mut err = tempfile::tempfile()?;
        self.cmd
            .stdout(out.try_clone()?)
            .stderr(err.try_clone()?)
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid(2) is async-signal-safe; the child is not yet a group leader, so
            // it gets a new session and a process group whose id is its pid.
            unsafe {
                self.cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let deadline = Instant::now() + self.timeout;
        let mut child = self.cmd.spawn()?;
        if let (Some(bytes), Some(mut pipe)) = (self.stdin.take(), child.stdin.take()) {
            // Never joined: a descendant in another session can hold the pipe's read end and
            // keep this write blocked after the group is killed. A child may also exit before
            // reading everything; its exit status tells the story.
            std::thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            });
        }
        let waited = wait_until(&mut child, deadline, self.expire_when.as_deref());
        kill_group(child.id());
        let status = waited?;
        let Some(status) = status else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "`{}` timed out after {}s",
                    self.describe(),
                    self.timeout.as_secs_f32()
                ),
            ));
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
}

fn expired(deadline: Instant, early: Option<&(dyn Fn() -> bool + Send)>) -> bool {
    early.map_or_else(|| Instant::now() >= deadline, |f| f())
}

/// The child's exit status, or `None` once it is past `deadline` (or `early` holds) and its
/// group has been killed.
fn wait_until(
    child: &mut Child,
    deadline: Instant,
    early: Option<&(dyn Fn() -> bool + Send)>,
) -> std::io::Result<Option<ExitStatus>> {
    loop {
        if let Some(s) = child.try_wait()? {
            return Ok(Some(s));
        }
        if expired(deadline, early) {
            kill_group(child.id());
            child.wait()?;
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    // SAFETY: kill(2) with a negative pid signals that process group; a group already gone
    // is not an error worth reporting.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

#[cfg(test)]
mod tests;
