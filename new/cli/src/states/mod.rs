//! What each state does. A state passes by returning `Ok` and fails by returning `Err`.

mod build;
mod setup;
pub mod ship;
mod verdict;
mod verify;

use std::path::PathBuf;

use anyhow::Result;

use crate::forge::Forge;
use crate::harness::Harness;
use crate::state::{Outcome, State};
use crate::vcs::Vcs;
use crate::workspace::Workspace;

/// The artifact `build` leaves with a snapshot of the workspace as it ended the build.
const BUILT_SNAPSHOT: &str = "built.snapshot";

/// Everything a state works with.
pub struct Context {
    pub vcs: Box<dyn Vcs>,
    pub forge: Box<dyn Forge>,
    pub harness: Box<dyn Harness>,
    /// Where `ns` was started, inside the repository.
    pub from: PathBuf,
    pub unit: u64,
    /// Set by `setup`.
    pub workspace: Option<Workspace>,
    pub ship: ship::Options,
}

/// Runs one state. `Err` means the state itself went wrong, which `Outcome::Redo` repeats.
pub fn run(ctx: &mut Context, state: State) -> Result<Outcome> {
    match state {
        State::Setup => setup::run(ctx),
        State::Build => build::run(ctx),
        State::Verify => verify::run(ctx),
        State::Ship => ship::run(ctx),
        _ => Ok(Outcome::Pass),
    }
}
