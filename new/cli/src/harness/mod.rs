//! Coding-agent harnesses behind one trait. Everything specific to one lives in its own file.

mod claude;

use std::path::PathBuf;

use anyhow::Result;

pub trait Harness {
    /// Runs one headless session and waits for it to finish.
    fn task_run(&self, task: &Task) -> Result<Reply>;
}

pub struct Task {
    /// Where the session works.
    pub dir: PathBuf,
    pub prompt: String,
    /// Whether the session may change files. Without it the session only reads.
    pub write: bool,
    /// The model to use. `None` takes the harness's own default.
    pub model: Option<String>,
}

pub struct Reply {
    pub text: String,
    /// Whether the session ended normally.
    pub success: bool,
}

/// The harness `ns` uses. The only place one is chosen.
pub fn get() -> Box<dyn Harness> {
    Box::new(claude::Claude::new())
}
