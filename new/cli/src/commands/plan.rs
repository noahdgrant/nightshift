use std::time::Duration;

use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{forge, harness, plan, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to plan: its number on the forge.
    unit: u64,
    /// How long the session may take before it is killed and the unit is left as it was.
    #[arg(long, default_value_t = 1800)]
    harness_timeout_seconds: u64,
}

/// Cuts the unit into slices, adds them to its description and makes it ready for the agent.
pub fn run(args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let slices = plan::run(
        forge::get().as_ref(),
        harness::get().as_ref(),
        &root,
        args.unit,
        Duration::from_secs(args.harness_timeout_seconds),
    )?;
    println!("{}", json!({ "unit": args.unit, "slices": slices }));
    Ok(())
}
