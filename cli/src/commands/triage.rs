use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::auto::Streak;
use crate::forge::Forge;
use crate::harness::Harness;
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
            let waiting = auto::plan(&open, &Default::default()).triage;
            let count = if args.all {
                waiting.len()
            } else {
                args.max_units
            };
            waiting.iter().take(count).map(|u| u.number).collect()
        }
    };
    let failed = units_triage(
        forge.as_ref(),
        harness.as_ref(),
        &root,
        &units,
        Duration::from_secs(args.harness_timeout_seconds),
        &mut Streak::new(0),
    );
    if failed > 0 {
        bail!("{failed} of {} units were not triaged", units.len());
    }
    Ok(())
}

/// Triages each of `units`, printing one JSON line for each that was. A unit that cannot be
/// triaged is reported on stderr and left as it was. Returns how many could not be. A failed
/// session counts toward `streak`, and once it is reached no more units are triaged.
pub fn units_triage(
    forge: &dyn Forge,
    harness: &dyn Harness,
    root: &Path,
    units: &[u64],
    timeout: Duration,
    streak: &mut Streak,
) -> usize {
    let mut failed = 0;
    for unit in units {
        if streak.reached() {
            break;
        }
        match triage::run(forge, harness, root, *unit, timeout) {
            Ok(triaged) => {
                streak.record(*unit, false);
                println!(
                    "{}",
                    json!({
                        "unit": unit,
                        "route": triaged.route.name(),
                        "labels": triaged.labels,
                        "why": triaged.why,
                    })
                )
            }
            Err(e) => {
                eprintln!("ns: unit {unit} was not triaged: {e:#}");
                streak.record(*unit, harness::is_session_failure(&e));
                failed += 1;
            }
        }
    }
    failed
}
