//! One file per command. A command's file holds its arguments and its `run`.

mod auto;
mod check;
mod install;
mod run;
mod triage;
mod version;
mod workspace;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Work through the repository's units unattended.
    Auto(auto::Args),
    /// Run the repository's checks on this workspace.
    Check(check::Args),
    /// Link skills into ns's skills directory.
    Install(install::Args),
    /// Move a unit through the states, printing each move.
    Run(run::Args),
    /// Print the version of ns.
    Version(version::Args),
    /// Decide what a unit needs next and record it on the forge.
    Triage(triage::Args),
    /// Create and remove a unit's workspace.
    Workspace(workspace::Args),
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Auto(args) => auto::run(args),
        Command::Check(args) => check::run(args),
        Command::Install(args) => install::run(args),
        Command::Run(args) => run::run(args),
        Command::Version(args) => version::run(args),
        Command::Triage(args) => triage::run(args),
        Command::Workspace(args) => workspace::run(args),
    }
}
