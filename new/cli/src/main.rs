//! `ns`: the Nightshift CLI.

mod artifact;
mod checks;
mod commands;
mod forge;
mod harness;
mod skill;
mod state;
mod states;
mod vcs;
mod workspace;

use clap::Parser;

#[derive(Parser)]
#[command(name = "ns", version)]
struct Cli {
    #[command(subcommand)]
    command: commands::Command,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = commands::run(cli.command) {
        eprintln!("ns: {e:#}");
        std::process::exit(1);
    }
}
