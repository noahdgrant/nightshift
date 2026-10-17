use std::time::Duration;

use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{forge, harness, triage, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to triage: its number on the forge.
    unit: u64,
    /// How long the session may take before it is killed and the unit is left as it was.
    #[arg(long, default_value_t = 1800)]
    harness_timeout_seconds: u64,
}

/// Decides what the unit needs next, and records the decision as the unit's labels.
pub fn run(args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let triaged = triage::run(
        forge::get().as_ref(),
        harness::get().as_ref(),
        &root,
        args.unit,
        Duration::from_secs(args.harness_timeout_seconds),
    )?;
    println!(
        "{}",
        json!({
            "unit": args.unit,
            "route": triaged.route.name(),
            "labels": triaged.labels,
            "why": triaged.why,
        })
    );
    Ok(())
}
