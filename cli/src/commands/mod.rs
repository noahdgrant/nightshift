//! One file per command. A command's file holds its arguments and its `run`.

mod auto;
mod check;
mod define;
mod init;
mod plan;
mod run;
mod triage;
mod troubleshoot;
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
    /// Work out a spec for a vague unit with you, then plan it.
    Define(define::Args),
    /// Cut a unit into slices and make it ready for the agent.
    Plan(plan::Args),
    /// Pin the repository to this version of ns.
    Init(init::Args),
    /// Move a unit through the states, printing each move.
    Run(run::Args),
    /// Print the version of ns.
    Version(version::Args),
    /// Diagnose a bug nobody has reproduced and send the unit on.
    Troubleshoot(troubleshoot::Args),
    /// Decide what a unit needs next and record it on the forge.
    Triage(triage::Args),
    /// Create and remove a unit's workspace.
    Workspace(workspace::Args),
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Auto(args) => auto::run(args),
        Command::Check(args) => check::run(args),
        Command::Define(args) => define::run(args),
        Command::Init(args) => init::run(args),
        Command::Plan(args) => plan::run(args),
        Command::Run(args) => run::run(args),
        Command::Version(args) => version::run(args),
        Command::Triage(args) => triage::run(args),
        Command::Troubleshoot(args) => troubleshoot::run(args),
        Command::Workspace(args) => workspace::run(args),
    }
}
