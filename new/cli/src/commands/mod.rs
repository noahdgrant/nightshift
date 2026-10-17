//! One file per command. A command's file holds its arguments and its `run`.

mod run;
mod version;
mod workspace;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Move a unit through the states, printing each move.
    Run(run::Args),
    /// Print the version of ns.
    Version(version::Args),
    /// Create and remove a unit's workspace.
    Workspace(workspace::Args),
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Run(args) => run::run(args),
        Command::Version(args) => version::run(args),
        Command::Workspace(args) => workspace::run(args),
    }
}
