//! `ns`: the Nightshift CLI.

mod artifact;
mod auto;
mod brief;
mod checks;
mod commands;
mod config;
mod define;
mod dispatch;
mod forge;
mod harness;
mod labels;
mod lock;
mod manifest;
mod merge;
mod os;
mod plan;
mod retry;
mod runlog;
mod skill;
mod state;
mod states;
mod status;
mod sync;
mod triage;
mod troubleshoot;
mod until;
mod vcs;
mod versions;
mod workspace;

/// Tests that write a program and run it, or start any process, hold this guard. A process
/// started while another thread has a program open for writing keeps that file busy until it
/// execs, and running the program then fails with "text file busy".
#[cfg(test)]
mod testutil {
    use std::sync::{Mutex, MutexGuard};

    static SPAWNING: Mutex<()> = Mutex::new(());

    pub fn spawning() -> MutexGuard<'static, ()> {
        SPAWNING.lock().unwrap_or_else(|e| e.into_inner())
    }
}

use clap::Parser;

#[derive(Parser)]
#[command(name = "ns", version)]
struct Cli {
    #[command(subcommand)]
    command: commands::Command,
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if let Err(e) = dispatch::run(&args).and_then(|()| commands::run(Cli::parse().command)) {
        eprintln!("ns: {e:#}");
        std::process::exit(1);
    }
}
