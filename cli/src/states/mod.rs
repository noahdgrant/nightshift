//! What each state does. A state passes by returning `Ok` and fails by returning `Err`.

mod build;
mod check;
pub mod clean_up;
pub mod review;
mod rounds;
mod setup;
pub mod ship;
pub mod verdict;
mod verify;

pub use verify::unproven_note;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use crate::forge::Forge;
use crate::harness::{Harness, Reply, Task, TimedOut};
use crate::os::Os;
use crate::state::{Outcome, State};
use crate::transcripts::Transcripts;
use crate::vcs::Vcs;
use crate::workspace::Workspace;

/// The artifact `build` leaves with a snapshot of the workspace as it ended the build.
pub(crate) const BUILT_SNAPSHOT: &str = "built.snapshot";

/// Everything a state works with.
pub struct Context {
    pub vcs: Box<dyn Vcs>,
    pub forge: Box<dyn Forge>,
    pub harness: Box<dyn Harness>,
    pub os: Box<dyn Os>,
    /// Where `ns` was started, inside the repository.
    pub from: PathBuf,
    pub unit: u64,
    /// Set by `setup`.
    pub workspace: Option<Workspace>,
    pub ship: ship::Options,
    /// How long a harness session may take.
    pub harness_timeout: Duration,
    /// How long the repository's checks may take.
    pub checks_timeout: Duration,
}

/// Runs one state. `Err` means the state itself went wrong, which `Outcome::Redo` repeats.
pub fn run(ctx: &mut Context, state: State) -> Result<Outcome> {
    match state {
        State::Setup => setup::run(ctx),
        State::Build => build::run(ctx),
        State::Verify => verify::run(ctx),
        State::Review => review::run(ctx),
        State::Ship => ship::run(ctx),
        State::CleanUp => clean_up::run(ctx),
        _ => Ok(Outcome::Pass),
    }
}

/// Runs a headless session of `state` and keeps its transcript, also when the session ran out of
/// time, which is when it is most wanted.
pub(crate) fn session_run(ctx: &Context, state: State, task: &Task) -> Result<Reply> {
    let root = ctx.vcs.root_find(&ctx.from)?;
    let kept = Transcripts::open(ctx.vcs.as_ref(), &root)?;
    let unit = ctx.unit.to_string();
    match ctx.harness.task_run(task) {
        Ok(reply) => {
            for raw in reply.earlier.iter().chain([&reply.transcript]) {
                kept.keep(&unit, state, raw)?;
            }
            Ok(reply)
        }
        Err(e) => {
            if let Some(timed_out) = e.downcast_ref::<TimedOut>() {
                for raw in timed_out.earlier.iter().chain([&timed_out.transcript]) {
                    kept.keep(&unit, state, raw)?;
                }
            }
            Err(e)
        }
    }
}
