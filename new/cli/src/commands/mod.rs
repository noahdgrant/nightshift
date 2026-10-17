//! One file per command. A command's file holds its arguments and its `run`.

mod check;
mod install;
mod run;
mod version;
mod watch;
mod workspace;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Run the repository's checks on this workspace.
    Check(check::Args),
    /// Link skills into ns's skills directory.
    Install(install::Args),
    /// Move a unit through the states, printing each move.
    Run(run::Args),
    /// Print the version of ns.
    Version(version::Args),
    /// Pick the next units to triage and run.
    Watch(watch::Args),
    /// Create and remove a unit's workspace.
    Workspace(workspace::Args),
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Check(args) => check::run(args),
        Command::Install(args) => install::run(args),
        Command::Run(args) => run::run(args),
        Command::Version(args) => version::run(args),
        Command::Watch(args) => watch::run(args),
        Command::Workspace(args) => workspace::run(args),
    }
}
