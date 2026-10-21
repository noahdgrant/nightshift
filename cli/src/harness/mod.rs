//! Coding-agent harnesses behind one trait. Everything specific to one lives in its own file.

mod claude;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Result};

pub trait Harness {
    /// The version the harness reports. An error when it cannot be started.
    fn version_get(&self) -> Result<String>;

    /// Where, relative to a repository's top, this harness looks for skills.
    fn skills_dir(&self) -> &'static str;

    /// Makes the harness run `command` whenever a session starts in the repository at `root`, by
    /// a file meant to be committed. Does nothing when it already does. Returns whether it wrote.
    fn session_start_hook_install(&self, root: &Path, command: &str) -> Result<bool>;

    /// Runs one headless session and waits for it to finish.
    fn task_run(&self, task: &Task) -> Result<Reply>;

    /// Starts a session the user takes part in, on their terminal, that begins with `prompt`.
    /// Returns when the user ends it, and whether it ended normally.
    fn session_interact(&self, dir: &Path, prompt: &str) -> Result<bool>;
}

pub struct Task {
    /// Where the session works.
    pub dir: PathBuf,
    pub prompt: String,
    /// Whether the session may change files. Without it the session only reads.
    pub write: bool,
    /// The model to use. `None` takes the harness's own default.
    pub model: Option<String>,
    /// How long the session may take. A session still running then is killed and is an error.
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct Reply {
    pub text: String,
    /// Whether the session ended normally.
    pub success: bool,
}

/// The harnesses a repository can name in its manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Claude,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
        }
    }

    pub fn parse(name: &str) -> Result<Kind> {
        match name {
            "claude" => Ok(Kind::Claude),
            "codex" => bail!("the codex harness is not supported yet"),
            other => bail!("there is no harness named {other}"),
        }
    }
}

/// The harness `ns` uses. The only place one is chosen.
pub fn get() -> Box<dyn Harness> {
    of(Kind::Claude)
}

/// The harness of `kind`.
pub fn of(kind: Kind) -> Box<dyn Harness> {
    match kind {
        Kind::Claude => Box::new(claude::Claude::new(crate::os::get())),
    }
}
