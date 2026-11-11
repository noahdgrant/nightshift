//! The operating system behind one trait: processes, the shell, links and where the user's files
//! live. Everything specific to one lives in its own file.

mod linux;
mod systemd;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local, NaiveTime};

pub trait Os {
    /// Starts `cmd` and waits for it, at most `timeout`. A piped stdin is fed `input` and closed;
    /// a piped stdout is collected. A child out of time is killed with everything it started.
    fn process_run(&self, cmd: Command, input: &[u8], timeout: Duration) -> Result<Finished>;

    /// Starts `cmd` on the user's terminal and waits for it. The user works with it, so it has no
    /// time limit. Returns whether it exited with success.
    fn process_interact(&self, cmd: Command) -> Result<bool>;

    /// Replaces this process with `cmd`, which keeps its environment, arguments and terminal.
    /// Returns only the reason it could not.
    fn process_replace(&self, cmd: Command) -> anyhow::Error;

    /// A command that runs `script` in the system's shell.
    fn shell_command(&self, script: &str) -> Command;

    /// The time of day by the clock on the wall.
    fn time_now(&self) -> DateTime<Local>;

    /// Blocks for `duration`.
    fn sleep(&self, duration: Duration);

    /// Writes what starts `ns auto` for `runner.root` on a schedule, and turns the schedule on.
    /// Changes nothing that is already as `runner` says.
    fn runner_install(&self, runner: &Runner) -> Result<Installed>;

    /// Turns off and deletes the runner of the repository at `root`. Returns whether there was one.
    fn runner_remove(&self, root: &Path) -> Result<bool>;

    /// Whether the installed runner is the one `runner` describes.
    fn runner_check(&self, runner: &Runner) -> Result<RunnerState>;
}

/// What a runner is told about the repository it starts `ns` for.
pub struct Runner {
    /// The repository's main checkout, where `ns auto` runs.
    pub root: PathBuf,
    /// The `ns` that starts the night: the one that installed the runner.
    pub ns: PathBuf,
    /// The `PATH` the night's tools (`git`, the forge's CLI, the harness) are found by.
    pub path: String,
    /// When the night starts.
    pub start: NaiveTime,
    /// The time of day `ns auto` stops starting units.
    pub until: NaiveTime,
}

#[derive(Debug)]
pub struct Installed {
    /// The files that make up the runner.
    pub files: Vec<PathBuf>,
    /// Whether any of them was written, because it was new or different.
    pub changed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunnerState {
    NotInstalled,
    /// Installed, and it matches the settings and the running `ns`.
    Current,
    /// Installed, and it differs from what `ns runner install` would write now.
    Differs,
}

pub struct Finished {
    /// Whether the child exited with success. `None` when it ran out of time and was killed.
    pub success: Option<bool>,
    /// What the child printed to a piped stdout, up to the end or the kill.
    pub stdout: Vec<u8>,
}

/// The operating system `ns` runs on. The only place one is chosen.
pub fn get() -> Box<dyn Os> {
    Box::new(linux::Linux)
}
