use std::time::Duration;

use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{auto, forge, harness, triage, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to triage: its number on the forge. Without it, the units nobody has triaged.
    unit: Option<u64>,
    /// How many untriaged units to triage, oldest and highest priority first.
    #[arg(long, default_value_t = 1, conflicts_with_all = ["unit", "all"])]
    max_units: usize,
    /// Triage every untriaged unit.
    #[arg(long, conflicts_with = "unit")]
    all: bool,
    /// How long the session may take before it is killed and the unit is left as it was.
    #[arg(long, default_value_t = 1800)]
    harness_timeout_seconds: u64,
}

/// Decides what each unit needs next, and records the decision as the unit's labels. A unit that
/// cannot be triaged is reported and left as it was, and the others go on.
pub fn run(args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let (forge, harness) = (forge::get(), harness::get());
    let units: Vec<u64> = match args.unit {
        Some(unit) => vec![unit],
        None => {
            let open = forge.unit_list(&root)?;
            let waiting = auto::plan(&open).triage;
            let count = if args.all {
                waiting.len()
            } else {
                args.max_units
            };
            waiting.iter().take(count).map(|u| u.number).collect()
        }
    };
    let mut failed = 0;
    for unit in &units {
        let triaged = triage::run(
            forge.as_ref(),
            harness.as_ref(),
            &root,
            *unit,
            Duration::from_secs(args.harness_timeout_seconds),
        );
        match triaged {
            Ok(triaged) => println!(
                "{}",
                json!({
                    "unit": unit,
                    "route": triaged.route.name(),
                    "labels": triaged.labels,
                    "why": triaged.why,
                })
            ),
            Err(e) => {
                eprintln!("ns: unit {unit} was not triaged: {e:#}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {} units were not triaged", units.len());
    }
    Ok(())
}
