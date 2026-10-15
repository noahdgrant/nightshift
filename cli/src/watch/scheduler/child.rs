//! A unit's `ns run` as a child process: starting it in its own process group, signalling that
//! group, and reading the signal that stopped it.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};

use anyhow::{Context, Result};

use crate::clock;
use crate::memory;
use crate::proc::{Kind, Proc};
use crate::watch::{snapshot::Snapshot, unit_id, Issue};

#[cfg(unix)]
pub(super) const SIGTERM: i32 = libc::SIGTERM;
#[cfg(not(unix))]
pub(super) const SIGTERM: i32 = 15;

/// What a unit's `ns run` starts with, whichever binary runs it.
pub(super) struct Start<'a> {
    pub snap: &'a Snapshot,
    pub issue: &'a Issue,
    pub cpus: usize,
    pub clock: &'a clock::Clock,
    /// Where the run's result goes.
    pub result: &'a Path,
    /// The unit's own slice under `ns.slice`, when the units share a cap (`crate::memory`).
    pub slice: Option<&'a str>,
}

/// Start `ns run <unit> --issue <n> --cpus <cpus>` on the night's snapshot, its result written to
/// `result`, with `exe`, a newer `ns` a self-update built, else the binary this night runs; in a
/// systemd scope in the unit's slice when it has one. It gets its own process group, so a terminal's Ctrl-C reaches only `ns watch`, which
/// passes it on once; and on Linux it gets SIGTERM if `ns watch` dies, so no unit outlives its
/// night. The kernel sends that signal when the thread that spawned the child ends, so units must
/// be started from the thread that runs the whole night, as the scheduler does.
pub(super) fn spawn(s: &Start, base: Option<String>, exe: Option<&Path>) -> Result<Child> {
    let (snap, issue, result) = (s.snap, s.issue, s.result);
    let exe = night_exe(exe)?;
    let out =
        File::create(result).with_context(|| format!("cannot create {}", result.display()))?;
    let mut cmd = command(s.slice, exe);
    // Naming the unit here spares `ns run` reading the title back through `gh`, which a GitLab
    // project has no answer for.
    cmd.args(["run", &unit_id(issue), "--issue", &issue.number.to_string()])
        .arg("--night")
        .arg(snap.night().path())
        .arg("--factory")
        .arg(snap.factory())
        .args(["--cpus", &s.cpus.to_string()])
        .env("NS_CONFIG", snap.config());
    if let Some(b) = base {
        cmd.args(["--base", &b]);
    }
    // The unit starts at the night's time, which a pinned clock's sleeps have moved on.
    if let Some(t) = s.clock.pinned_at() {
        cmd.env("NS_NOW", t.to_string());
    }
    cmd.spawn(out)
        .with_context(|| format!("cannot start ns run --issue {}", issue.number))
}
/// `exe`, else the binary this night runs, even if a newer one was installed over it since. On
/// Linux that is `/proc/<pid>/exe` of `ns watch` itself, not `/proc/self/exe`: under a slice the
/// unit is exec'd by `systemd-run`, where `/proc/self` names `systemd-run`.
fn night_exe(exe: Option<&Path>) -> Result<PathBuf> {
    Ok(match exe {
        Some(e) => e.to_path_buf(),
        None if cfg!(target_os = "linux") => {
            PathBuf::from(format!("/proc/{}/exe", std::process::id()))
        }
        None => std::env::current_exe().context("cannot find the ns binary")?,
    })
}

/// The command that runs `exe`: in a systemd scope in `slice`, which the run is told of, when
/// there is one.
fn command(slice: Option<&str>, exe: PathBuf) -> Proc {
    let Some(slice) = slice else {
        return Proc::tool(exe);
    };
    let mut cmd = Proc::launched(Kind::Tool, &memory::launcher(slice), exe);
    cmd.env(memory::SLICE_ENV, slice);
    cmd
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

    #[test]
    fn a_unit_with_a_slice_runs_in_a_scope_there_and_is_told_its_name() {
        let plain = command(None, "/bin/ns".into());
        assert_eq!(plain.argv(), ["/bin/ns"]);
        let scoped = command(Some("ns-7x1.slice"), "/bin/ns".into());
        assert_eq!(
            scoped.argv(),
            [
                "systemd-run",
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--slice=ns-7x1.slice",
                "--",
                "/bin/ns"
            ]
        );
        let told: Vec<_> = scoped.get_envs().collect();
        assert!(
            told.contains(&(
                std::ffi::OsStr::new(memory::SLICE_ENV),
                Some(std::ffi::OsStr::new("ns-7x1.slice"))
            )),
            "{told:?}"
        );
        assert!(!plain.get_envs().any(|(k, _)| k == memory::SLICE_ENV));
    }

    #[cfg(unix)]
    #[test]
    fn the_night_binary_is_this_one_when_another_process_execs_it() {
        let exe = night_exe(None).unwrap();
        let out = Proc::tool("sh")
            .args(["-c", r#"exec "$0" --list"#])
            .arg(&exe)
            .output()
            .unwrap();
        let listed = String::from_utf8_lossy(&out.stdout);
        assert!(
            listed.contains("the_night_binary_is_this_one_when_another_process_execs_it"),
            "{} ran something else: {listed} {}",
            exe.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        let given = Path::new("/bin/ns");
        assert_eq!(night_exe(Some(given)).unwrap(), given);
    }

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
