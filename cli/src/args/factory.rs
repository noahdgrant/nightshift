//! `ns factory`: clap's definitions and help text.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// Check the factory definition (.nightshift/nightshift.toml and agents/<role>/agent.md)
#[derive(Args)]
#[command(arg_required_else_help = true)]
pub(crate) struct Factory {
    #[command(subcommand)]
    pub command: FactoryCmd,
}

#[derive(Subcommand)]
pub(crate) enum FactoryCmd {
    /// Parse nightshift.toml and runners/*.toml (unknown keys are errors) and check agents/<role>/agent.md
    #[command(after_help = "\
Examples:
  ns factory validate
  ns factory validate --factory path/to/factory

Prints {\"ok\",\"path\",\"name\",\"phases\",\"scorers\",\"supervise\",\"errors\",\"warnings\"};
supervise is {\"notify\",\"silence_minutes\"}, null when the file is missing or does not parse.
Exits 1 if any error.")]
    Validate {
        /// Directory holding nightshift.toml (default: <main-worktree>/.nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
    },
}
