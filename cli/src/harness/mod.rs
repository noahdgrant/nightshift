//! Coding-agent harnesses behind one trait. Everything specific to one lives in its own file.

mod claude;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Result};

/// Marks an error as the harness session itself failing (it did not start, finish or answer in
/// time), not a model answer `ns` cannot use.
#[derive(Debug)]
struct SessionFailed;

impl std::fmt::Display for SessionFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the harness session failed")
    }
}

/// Marks `error` as a failed session.
pub fn session_failed(error: anyhow::Error) -> anyhow::Error {
    error.context(SessionFailed)
}

/// Whether `error` was marked by [`session_failed`].
pub fn is_session_failure(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SessionFailed>().is_some()
}

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

    /// A saved session transcript as plain text: what the agent said, each tool call and the
    /// start of each tool result, in order.
    fn transcript_render(&self, transcript: &str) -> String;

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

/// How a batch of sessions runs.
pub struct Sessions<'a> {
    /// How long one session may take.
    pub timeout: Duration,
    /// The model each session uses. `None` takes the harness's own default.
    pub model: Option<&'a str>,
}

#[derive(Debug)]
pub struct Reply {
    pub text: String,
    /// Whether the session ended normally.
    pub success: bool,
    /// Everything the session printed, as the harness prints it: what was read and run, not only
    /// the final `text`.
    pub transcript: Vec<u8>,
    /// The transcripts of the sessions started again before this one, oldest first.
    pub earlier: Vec<Vec<u8>>,
}

/// The error of a session that ran out of time and was killed. It keeps what the session
/// produced up to then.
#[derive(Debug)]
pub struct TimedOut {
    pub seconds: u64,
    pub transcript: Vec<u8>,
    /// The transcripts of the sessions started again before this one, oldest first.
    pub earlier: Vec<Vec<u8>>,
}

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the harness did not finish within {} seconds",
            self.seconds
        )
    }
}

impl std::error::Error for TimedOut {}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marked_error_is_a_session_failure_even_with_more_context_on_top() {
        let marked = session_failed(anyhow::anyhow!("not logged in")).context("unit 4");
        assert!(is_session_failure(&marked));
        assert_eq!(
            format!("{marked:#}"),
            "unit 4: the harness session failed: not logged in"
        );
    }

    #[test]
    fn an_unmarked_error_is_not_a_session_failure() {
        assert!(!is_session_failure(&anyhow::anyhow!("no ROUTE line")));
    }
}
