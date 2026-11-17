//! Linux. A child leads its own process group, and one that runs out of time takes its group
//! down with it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Local};

use super::{systemd, Finished, Installed, Os, Runner, RunnerState, Streamed};

pub struct Linux;

static CLOCK_READS: AtomicU64 = AtomicU64::new(0);

/// The time now. With `NS_CLOCK_TICK_SECONDS` set, which only a test sets, each read of the clock
/// reads that many seconds later than the one before, so a test reaches a time of day without
/// waiting for it.
fn clock_read(tick: Option<&str>) -> DateTime<Local> {
    let seconds = tick.and_then(|t| t.parse::<i64>().ok()).unwrap_or(0);
    let reads = CLOCK_READS.fetch_add(1, Ordering::Relaxed) as i64;
    Local::now() + chrono::Duration::seconds(seconds * reads)
}

impl Os for Linux {
    fn process_stream(
        &self,
        cmd: Command,
        input: &[u8],
        timeout: Duration,
        tick: Duration,
        on: &mut dyn FnMut(Streamed),
    ) -> Result<Finished> {
        run(cmd, input, timeout, tick, on)
    }

    fn process_interact(&self, mut cmd: Command) -> Result<bool> {
        // Not a group of its own: a group that is not in front of the terminal is stopped when it
        // reads from it.
        Ok(cmd.status().context("cannot start the process")?.success())
    }

    fn process_replace(&self, mut cmd: Command) -> anyhow::Error {
        anyhow::Error::new(cmd.exec()).context("cannot start the process")
    }

    fn shell_command(&self, script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    fn time_now(&self) -> DateTime<Local> {
        clock_read(std::env::var("NS_CLOCK_TICK_SECONDS").ok().as_deref())
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }

    fn runner_install(&self, runner: &Runner) -> Result<Installed> {
        systemd::install(&systemd::dir_find()?, runner, &mut systemd::ctl)
    }

    fn runner_remove(&self, root: &Path) -> Result<bool> {
        systemd::remove(&systemd::dir_find()?, root, &mut systemd::ctl)
    }

    fn runner_check(&self, runner: &Runner) -> Result<RunnerState> {
        systemd::check(&systemd::dir_find()?, runner)
    }
}

fn run(
    mut cmd: Command,
    input: &[u8],
    timeout: Duration,
    tick: Duration,
    on: &mut dyn FnMut(Streamed),
) -> Result<Finished> {
    cmd.process_group(0);
    let mut child = cmd.spawn().context("cannot start the process")?;
    let feed = child.stdin.take().map(|mut stdin| {
        let input = input.to_vec();
        // A child that exits early closes the pipe, which is not this thread's problem.
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        })
    });
    // The reader sends each line as it arrives; the loop below hands it on.
    let (lines, arrived) = mpsc::channel::<Vec<u8>>();
    let reader = child.stdout.take().map(|stdout| {
        thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                match stdout.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if lines.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        })
    });
    // A callback that panics unwinds out of here, and the child must not outlive it.
    let mut group = GroupGuard(Some(child.id()));
    let mut stdout = Vec::new();
    let mut take = |line: Vec<u8>, on: &mut dyn FnMut(Streamed)| {
        on(Streamed::Line(&line));
        stdout.extend(line);
    };
    let started = Instant::now();
    let mut next_tick = tick;
    let mut open = reader.is_some();
    let success = loop {
        if let Some(status) = child.try_wait().context("cannot wait for the process")? {
            break Some(status.success());
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            kill_group(child.id());
            child.wait().context("cannot wait for the process")?;
            break None;
        }
        if !tick.is_zero() && elapsed >= next_tick {
            on(Streamed::Tick(elapsed));
            next_tick += tick;
        }
        if open {
            match arrived.recv_timeout(Duration::from_millis(20)) {
                Ok(line) => take(line, on),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => open = false,
            }
        } else {
            thread::sleep(Duration::from_millis(20));
        }
    };
    group.0 = None;
    if let Some(feed) = feed {
        let _ = feed.join();
    }
    // What the child printed last is still in the channel until the reader reaches the end.
    for line in arrived {
        take(line, on);
    }
    if let Some(reader) = reader {
        let _ = reader.join();
    }
    Ok(Finished { success, stdout })
}

/// Kills the child's process group when dropped, unless the child is already gone.
struct GroupGuard(Option<u32>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            kill_group(pid);
        }
    }
}

/// Kills the process group led by `pid`, which this module started and which is its own group.
fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{pid}")])
        .status();
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_clock_ticks_only_when_a_tick_is_set() {
        let hours = |tick| (clock_read(tick) - Local::now()).num_hours();
        assert_eq!(hours(None), 0);
        assert_eq!(hours(Some("junk")), 0);
        let first = clock_read(Some("3600"));
        let second = clock_read(Some("3600"));
        assert!(second - first >= chrono::Duration::minutes(59));
    }

    use std::process::Stdio;

    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        cmd
    }

    #[test]
    fn a_process_that_finishes_gives_its_output() {
        let _spawn = crate::testutil::spawning();
        let done = Linux
            .process_run(sh("cat; exit 3"), b"hi", Duration::from_secs(30))
            .unwrap();
        assert_eq!(done.success, Some(false));
        assert_eq!(done.stdout, b"hi");
    }

    #[test]
    fn a_line_reaches_the_callback_before_the_child_exits() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("child-went-on");
        let script = format!(
            "echo first; sleep 1; touch {}; echo second",
            marker.display()
        );
        let mut seen = Vec::new();
        let mut early = None;
        let done = Linux
            .process_stream(
                sh(&script),
                b"",
                Duration::from_secs(30),
                Duration::ZERO,
                &mut |event| {
                    if let Streamed::Line(line) = event {
                        early.get_or_insert(!marker.exists());
                        seen.push(String::from_utf8_lossy(line).into_owned());
                    }
                },
            )
            .unwrap();
        assert_eq!(early, Some(true));
        assert_eq!(seen, ["first\n", "second\n"]);
        assert_eq!(done.success, Some(true));
        assert_eq!(done.stdout, b"first\nsecond\n");
    }

    #[test]
    fn a_silent_child_gets_a_tick_each_interval_until_it_ends() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let go = tmp.path().join("go");
        // The child ends when the callback has seen two ticks, so the test waits on that event and
        // not on the clock.
        let script = format!("while [ ! -e {} ]; do sleep 0.05; done", go.display());
        let mut ticks = Vec::new();
        Linux
            .process_stream(
                sh(&script),
                b"",
                Duration::from_secs(30),
                Duration::from_millis(100),
                &mut |event| {
                    if let Streamed::Tick(elapsed) = event {
                        ticks.push(elapsed);
                        if ticks.len() == 2 {
                            std::fs::write(&go, "").unwrap();
                        }
                    }
                },
            )
            .unwrap();
        assert_eq!(ticks.len(), 2);
        assert!(ticks[0] >= Duration::from_millis(100));
        assert!(ticks[1] >= ticks[0]);
    }

    #[test]
    fn a_process_out_of_time_is_killed_with_what_it_started() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("grandchild-lived");
        // The grandchild would write the marker after the kill, if it survived.
        let script = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let done = Linux
            .process_run(sh(&script), b"", Duration::from_millis(100))
            .unwrap();
        assert_eq!(done.success, None);
        thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists());
    }

    #[test]
    fn a_callback_that_panics_does_not_leave_the_child_running() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("child-lived");
        let script = format!("echo hi; sleep 1; touch {}", marker.display());
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = Linux.process_stream(
                sh(&script),
                b"",
                Duration::from_secs(30),
                Duration::ZERO,
                &mut |_| panic!("stdout is closed"),
            );
        }));
        assert!(outcome.is_err());
        thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists());
    }
}
