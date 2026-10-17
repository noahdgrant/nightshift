//! Runs a child process with a time limit. What it started stops with it: the child leads its own
//! process group, and a child that runs out of time takes its group down with it.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

pub struct Finished {
    /// Whether the child exited with success. `None` when it ran out of time and was killed.
    pub success: Option<bool>,
    /// What the child printed to a piped stdout, up to the end or the kill.
    pub stdout: Vec<u8>,
}

/// Starts `cmd` and waits for it, at most `timeout`. A piped stdin is fed `input` and closed; a
/// piped stdout is collected.
pub fn run(mut cmd: Command, input: &[u8], timeout: Duration) -> Result<Finished> {
    cmd.process_group(0);
    let mut child = cmd.spawn().context("cannot start the process")?;
    let feed = child.stdin.take().map(|mut stdin| {
        let input = input.to_vec();
        // A child that exits early closes the pipe, which is not this thread's problem.
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        })
    });
    let collect = child.stdout.take().map(|mut stdout| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        })
    });
    let success = wait(&mut child, timeout)?;
    if let Some(feed) = feed {
        let _ = feed.join();
    }
    let stdout = collect
        .map(|c| c.join().unwrap_or_default())
        .unwrap_or_default();
    Ok(Finished { success, stdout })
}

fn wait(child: &mut Child, timeout: Duration) -> Result<Option<bool>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("cannot wait for the process")? {
            return Ok(Some(status.success()));
        }
        if started.elapsed() >= timeout {
            kill_group(child.id());
            child.wait().context("cannot wait for the process")?;
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(20));
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
        let done = run(sh("cat; exit 3"), b"hi", Duration::from_secs(30)).unwrap();
        assert_eq!(done.success, Some(false));
        assert_eq!(done.stdout, b"hi");
    }

    #[test]
    fn a_process_out_of_time_is_killed_with_what_it_started() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("grandchild-lived");
        // The grandchild would write the marker after the kill, if it survived.
        let script = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let done = run(sh(&script), b"", Duration::from_millis(100)).unwrap();
        assert_eq!(done.success, None);
        thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists());
    }
}
