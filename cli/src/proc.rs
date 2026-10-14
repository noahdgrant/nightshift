//! The one way to start a child process. A caller picks a [`Kind`], which fixes the policy:
//! the environment scrub, a timeout that kills the whole process group, stdin closed unless
//! the caller writes to it, and no credential prompts for network git (#556).

use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use crate::{billing, git};

/// Remove inherited git location variables, so the command acts only on the repo it names.
/// [`Proc`] does this itself; this is for the sites that still build a `Command` until #581
/// moves them here.
pub fn scrub_git(cmd: &mut Command) -> &mut Command {
    for v in git::LOCATION_VARS {
        cmd.env_remove(v);
    }
    cmd
}

/// Remove the variables that would make `argv`'s harness bill an API instead of the
/// subscription. [`Proc`] does this for a subscription harness; this is for the sites that still
/// build a `Command` until #581 moves them here.
pub fn scrub_billing(cmd: &mut Command, argv: &[String]) {
    for k in billing::api_env(argv) {
        cmd.env_remove(k);
    }
}

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
    log: Option<File>,
    /// Whether to end the child now, as a stop does ([`Proc::until_stop`]).
    stopped: Option<fn() -> bool>,
    #[cfg(test)]
    expire_when: Option<Box<dyn Fn() -> bool + Send>>,
}

impl Proc {
    pub fn git_local() -> Self {
        Self::new(Kind::GitLocal, "git")
    }

    pub fn git_network() -> Self {
        Self::new(Kind::GitNetwork, "git")
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "harness sites move here in #581")
    )]
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
        scrub_git(&mut cmd);
        if kind == Kind::GitNetwork {
            cmd.env("GIT_TERMINAL_PROMPT", "0")
                .env("GCM_INTERACTIVE", "never");
        }
        Proc {
            kind,
            cmd,
            timeout: kind.timeout(),
            stdin: None,
            log: None,
            stopped: None,
            #[cfg(test)]
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

    pub fn envs<K, V>(&mut self, vars: impl IntoIterator<Item = (K, V)>) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.cmd.envs(vars);
        self
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "harness sites move here in #581")
    )]
    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.cmd.env_remove(key);
        self
    }

    /// Replace the kind's timeout for this call. [`Proc::output`] never runs without one.
    pub fn timeout(&mut self, limit: Duration) -> &mut Self {
        self.timeout = limit;
        self
    }

    /// Write `bytes` to the child's stdin, then close it. Without this, stdin is `/dev/null`.
    pub fn stdin(&mut self, bytes: Vec<u8>) -> &mut Self {
        self.stdin = Some(bytes);
        self
    }

    /// Send stdout and stderr to `log` as the child writes them; [`Proc::output`] then returns
    /// both empty.
    pub fn output_to(&mut self, log: File) -> &mut Self {
        self.log = Some(log);
        self
    }

    /// Kill the child's group as soon as a stop is requested ([`crate::stop`]), and fail with
    /// [`std::io::ErrorKind::Interrupted`].
    pub fn until_stop(&mut self) -> &mut Self {
        self.stopped = Some(|| crate::stop::requested().is_some());
        self
    }

    /// [`Proc::until_stop`] with `stopped` in place of a stop request, so a test can stop a child
    /// without stopping the test process.
    #[cfg(test)]
    fn stop_when(&mut self, stopped: fn() -> bool) -> &mut Self {
        self.stopped = Some(stopped);
        self
    }

    /// The environment changes made so far, as [`Command::get_envs`] lists them.
    #[cfg(test)]
    pub fn get_envs(&self) -> std::process::CommandEnvs<'_> {
        self.cmd.get_envs()
    }

    /// Also time out as soon as `expired` holds, so a test can end a child after an event; the
    /// deadline still applies.
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

    /// Run the child to its end in its own session and process group (on Linux it also gets
    /// SIGTERM if this process dies first), then kill whatever it
    /// left running there. Its output goes to temp files read after it exits, so a descendant
    /// holding a pipe open can't delay or drop it. A child past its timeout has its group
    /// killed and fails with [`std::io::ErrorKind::TimedOut`].
    pub fn output(&mut self) -> std::io::Result<Output> {
        self.scrub_billing();
        let captured = match self.log.take() {
            Some(log) => {
                self.cmd.stdout(log.try_clone()?).stderr(log);
                None
            }
            None => {
                let (out, err) = (tempfile::tempfile()?, tempfile::tempfile()?);
                self.cmd.stdout(out.try_clone()?).stderr(err.try_clone()?);
                Some((out, err))
            }
        };
        self.cmd.stdin(if self.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        new_session(&mut self.cmd);
        die_with(&mut self.cmd, std::process::id() as libc::pid_t);
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
        #[cfg(test)]
        let early = self.expire_when.as_deref();
        #[cfg(not(test))]
        let early = ();
        let waited = wait_until(&mut child, deadline, early, self.stopped);
        kill_group(child.id());
        let status = match waited? {
            Waited::Exited(status) => status,
            Waited::Stopped => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    format!("`{}` stopped by a signal", self.describe()),
                ))
            }
            Waited::TimedOut => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "`{}` timed out after {}s",
                        self.describe(),
                        self.timeout.as_secs_f32()
                    ),
                ))
            }
        };
        let read = |mut f: File| -> std::io::Result<Vec<u8>> {
            let mut buf = Vec::new();
            f.seek(SeekFrom::Start(0))?;
            f.read_to_end(&mut buf)?;
            Ok(buf)
        };
        let (stdout, stderr) = match captured {
            Some((out, err)) => (read(out)?, read(err)?),
            None => Default::default(),
        };
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }

    /// Start the child to run alongside the caller (a unit's `ns run`), with stdout to `out`,
    /// stderr inherited and stdin closed, in its own session and process group. The caller
    /// waits for it and ends it, so no timeout applies. On Linux it gets SIGTERM when the thread
    /// that started it ends, so it never outlives this process.
    pub fn spawn(&mut self, out: File) -> std::io::Result<Child> {
        self.spawn_for(std::process::id() as libc::pid_t, out)
    }

    /// [`Proc::spawn`] for the child of process `parent`, which is the caller, so the child
    /// gives up when that is not who started it.
    fn spawn_for(&mut self, parent: libc::pid_t, out: File) -> std::io::Result<Child> {
        self.scrub_billing();
        self.cmd
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(Stdio::inherit());
        new_session(&mut self.cmd);
        die_with(&mut self.cmd, parent);
        self.cmd.spawn()
    }

    /// Replace this process with the child; returns only on failure, which is
    /// [`std::io::ErrorKind::Interrupted`] when a stop was requested first. SIGINT and SIGTERM
    /// are left blocked from that check through the exec, so one that lands in between stays
    /// pending for the new program; on failure the caller unblocks them ([`crate::stop`]).
    #[cfg(unix)]
    pub fn exec(&mut self) -> std::io::Error {
        use std::os::unix::process::CommandExt;
        self.scrub_billing();
        // SAFETY: the closure only does pthread_sigmask(3) and an atomic load.
        unsafe {
            self.cmd.pre_exec(|| {
                crate::stop::block();
                match crate::stop::requested() {
                    Some(_) => Err(std::io::ErrorKind::Interrupted.into()),
                    None => Ok(()),
                }
            });
        }
        self.cmd.exec()
    }

    /// A subscription harness never sees the API billing variables, even ones the caller set.
    fn scrub_billing(&mut self) {
        if let Kind::Harness { subscription: true } = self.kind {
            let program = self.cmd.get_program().to_string_lossy().into_owned();
            scrub_billing(&mut self.cmd, &[program]);
        }
    }
}

/// Start the command in a new session, whose process group id is its pid.
fn new_session(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid(2) is async-signal-safe; the child is not yet a group leader, so
        // it gets a new session and a process group whose id is its pid.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

/// On Linux the child gets SIGTERM when the thread that started it ends, so a `ns` that dies
/// (a terminal Ctrl-C) takes its children with it even though they sit in sessions of their
/// own. It fails with `ESRCH` when `parent` is not who started it: that parent is already gone.
fn die_with(cmd: &mut Command, parent: libc::pid_t) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: prctl(2) and getppid(2) are async-signal-safe and touch only this child.
        // A parent that died before the prctl is seen by getppid, and the child gives up.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (cmd, parent);
}

/// The test-only way to end a wait before its deadline; nothing outside tests.
#[cfg(test)]
type Early<'a> = Option<&'a (dyn Fn() -> bool + Send)>;
#[cfg(not(test))]
type Early<'a> = ();

#[cfg(test)]
fn early_holds(early: Early) -> bool {
    early.is_some_and(|f| f())
}
#[cfg(not(test))]
fn early_holds(_: Early) -> bool {
    false
}

fn expired(deadline: Instant, early: Early) -> bool {
    Instant::now() >= deadline || early_holds(early)
}

enum Waited {
    Exited(ExitStatus),
    TimedOut,
    Stopped,
}

/// How the child ended: its exit, or its group killed once it is past `deadline` (or `early`
/// holds), or once `stopped` holds.
fn wait_until(
    child: &mut Child,
    deadline: Instant,
    early: Early,
    stopped: Option<fn() -> bool>,
) -> std::io::Result<Waited> {
    let mut delay = FIRST_POLL;
    loop {
        if let Some(s) = child.try_wait()? {
            return Ok(Waited::Exited(s));
        }
        let ended = if stopped.is_some_and(|f| f()) {
            Some(Waited::Stopped)
        } else if expired(deadline, early) {
            Some(Waited::TimedOut)
        } else {
            None
        };
        if let Some(ended) = ended {
            kill_group(child.id());
            child.wait()?;
            return Ok(ended);
        }
        std::thread::sleep(delay);
        delay = next_poll(delay);
    }
}

const FIRST_POLL: Duration = Duration::from_millis(1);
const LAST_POLL: Duration = Duration::from_millis(20);

/// The wait before the poll after one that waited `delay`: doubling, so a quick child is seen
/// within a millisecond or two and a long one is polled every 20 ms.
fn next_poll(delay: Duration) -> Duration {
    (delay * 2).min(LAST_POLL)
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
