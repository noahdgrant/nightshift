//! The operating system behind one trait: processes, the shell, links and where the user's files
//! live. Everything specific to one lives in its own file.

mod linux;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;

pub trait Os {
    /// Starts `cmd` and waits for it, at most `timeout`. A piped stdin is fed `input` and closed;
    /// a piped stdout is collected. A child out of time is killed with everything it started.
    fn process_run(&self, cmd: Command, input: &[u8], timeout: Duration) -> Result<Finished>;

    /// A command that runs `script` in the system's shell.
    fn shell_command(&self, script: &str) -> Command;

    /// Makes `link` point at `target`.
    fn file_link(&self, target: &Path, link: &Path) -> Result<()>;

    /// Where the user's own files for `ns` live, when nothing says otherwise.
    fn config_dir_find(&self) -> Result<PathBuf>;
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
