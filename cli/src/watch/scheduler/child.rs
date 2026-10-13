//! A unit's `ns run` as a child process: starting it in its own process group, signalling that
//! group, and reading the signal that stopped it.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};

use anyhow::{Context, Result};

use crate::clock;
use crate::watch::{snapshot::Snapshot, unit_id, Issue};

#[cfg(unix)]
pub(super) const SIGTERM: i32 = libc::SIGTERM;
#[cfg(not(unix))]
pub(super) const SIGTERM: i32 = 15;

/// Start `ns run <unit> --issue <n> --cpus <cpus>` on the night's snapshot, its result written to
/// `result`. It gets its own process group, so a terminal's Ctrl-C reaches only `ns watch`, which
/// passes it on once; and on Linux it gets SIGTERM if `ns watch` dies, so no unit outlives its
/// night. The kernel sends that signal when the thread that spawned the child ends, so units must
/// be started from the thread that runs the whole night, as the scheduler does.
pub(super) fn spawn(
    snap: &Snapshot,
    issue: &Issue,
    base: Option<String>,
    cpus: usize,
    clock: &clock::Clock,
    result: &Path,
) -> Result<Child> {
    // The binary this night runs, even if a newer one was installed over it since.
    let exe = if cfg!(target_os = "linux") {
        PathBuf::from("/proc/self/exe")
    } else {
        std::env::current_exe().context("cannot find the ns binary")?
    };
    let out =
        File::create(result).with_context(|| format!("cannot create {}", result.display()))?;
    let mut cmd = Command::new(exe);
    // Naming the unit here spares `ns run` reading the title back through `gh`, which a GitLab
    // project has no answer for.
    cmd.args(["run", &unit_id(issue), "--issue", &issue.number.to_string()])
        .arg("--night")
        .arg(snap.night().path())
        .arg("--factory")
        .arg(snap.factory())
        .args(["--cpus", &cpus.to_string()])
        .env("NS_CONFIG", snap.config())
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(Stdio::inherit());
    if let Some(b) = base {
        cmd.args(["--base", &b]);
    }
    // The unit starts at the night's time, which a pinned clock's sleeps have moved on.
    if let Some(t) = clock.pinned_at() {
        cmd.env("NS_NOW", t.to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        #[cfg(target_os = "linux")]
        {
            let watch = std::process::id() as libc::pid_t;
            // SAFETY: prctl(2) and getppid(2) are async-signal-safe and touch only this child.
            // A parent that died before the prctl is seen by getppid, and the child gives up.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != watch {
                        return Err(std::io::Error::other("ns watch is gone"));
                    }
                    Ok(())
                });
            }
        }
    }
    cmd.spawn()
        .with_context(|| format!("cannot start ns run --issue {}", issue.number))
}
/// Signal a unit's process group: its `ns run` and what runs in that group, a setup command or
/// a `gh` call, as a signal to the whole of `ns watch`'s group would. Its phases, each in a
/// session of its own, are ended by the `ns run`.
#[cfg(unix)]
pub(super) fn signal(child: &Child, sig: i32) {
    // SAFETY: kill(2) on the process group of a child this process started and has not reaped:
    // the child leads the group, so the group id is still its own.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), sig);
    }
}

#[cfg(not(unix))]
pub(super) fn signal(_: &Child, _: i32) {}

/// The SIGINT or SIGTERM that stopped a unit: its exit code 128 + the signal, or the signal
/// that killed it.
pub(super) fn stop_signal(status: &ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .map(|c| c - 128)
            .or(status.signal())
            .filter(|s| [libc::SIGINT, libc::SIGTERM].contains(s))
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_unit_stopped_by_sigint_or_sigterm_names_the_signal() {
        use std::os::unix::process::ExitStatusExt;
        let exited = |c: i32| ExitStatus::from_raw(c << 8);
        assert_eq!(stop_signal(&exited(143)), Some(libc::SIGTERM));
        assert_eq!(stop_signal(&exited(130)), Some(libc::SIGINT));
        assert_eq!(stop_signal(&exited(0)), None);
        assert_eq!(stop_signal(&exited(1)), None);
        assert_eq!(stop_signal(&exited(137)), None);
        assert_eq!(
            stop_signal(&ExitStatus::from_raw(libc::SIGTERM)),
            Some(libc::SIGTERM)
        );
        assert_eq!(stop_signal(&ExitStatus::from_raw(libc::SIGKILL)), None);
    }
}
