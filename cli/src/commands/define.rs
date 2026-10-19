use std::time::Duration;

use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{define, forge, harness, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to define: its number on the forge.
    unit: u64,
    /// How long the planning session may take before it is killed.
    #[arg(long, default_value_t = 1800)]
    harness_timeout_seconds: u64,
}

/// Works out a spec for the unit with you, records it, plans it and makes it ready for the agent.
pub fn run(args: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let slices = define::run(
        forge::get().as_ref(),
        harness::get().as_ref(),
        vcs.as_ref(),
        &root,
        args.unit,
        Duration::from_secs(args.harness_timeout_seconds),
    )?;
    println!("{}", json!({ "unit": args.unit, "slices": slices }));
    Ok(())
}
