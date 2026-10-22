use anyhow::{bail, Result};
use clap::Args as ClapArgs;

use crate::state::State;
use crate::transcripts::{self, Transcripts};
use crate::{harness, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit whose sessions to show.
    unit: u64,
    /// Show only the sessions of this state.
    #[arg(long)]
    state: Option<String>,
    /// Show only the most recent session.
    #[arg(long)]
    last: bool,
}

/// Prints what each harness session of the unit said and ran, oldest first. Changes nothing.
pub fn run(args: Args) -> Result<()> {
    let state = match &args.state {
        None => None,
        Some(name) => match State::parse(name) {
            Some(state) => Some(state),
            None => bail!("there is no state named {name}"),
        },
    };
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let kept = Transcripts::open(vcs.as_ref(), &root)?;
    let sessions = transcripts::select(kept.sessions(&args.unit.to_string())?, state, args.last);
    let harness = harness::get();
    for session in sessions {
        let raw = std::fs::read(&session.path)?;
        println!(
            "== {} {} ({}) ==\n{}",
            session.state,
            session.n,
            session.model.as_deref().unwrap_or("harness default model"),
            harness.transcript_render(&String::from_utf8_lossy(&raw))
        );
    }
    Ok(())
}
