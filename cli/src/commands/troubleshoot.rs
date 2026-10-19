use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::forge::Forge;
use crate::harness::Harness;
use crate::vcs::Vcs;
use crate::{forge, harness, troubleshoot, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to troubleshoot: its number on the forge.
    unit: u64,
    /// How long the session may take before it is killed and the unit is left as it was.
    #[arg(long, default_value_t = 3600)]
    harness_timeout_seconds: u64,
}

/// Diagnoses a bug nobody has reproduced, adds the diagnosis to the unit and sends it on.
pub fn run(args: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let timeout = Duration::from_secs(args.harness_timeout_seconds);
    let failed = units_troubleshoot(
        forge::get().as_ref(),
        harness::get().as_ref(),
        vcs.as_ref(),
        &root,
        &[args.unit],
        timeout,
    );
    if failed > 0 {
        bail!("unit {} was not troubleshot", args.unit);
    }
    Ok(())
}

/// Troubleshoots each of `units`, printing one JSON line for each that was. A unit that cannot be
/// is reported on stderr and left as it was. Returns how many could not be.
pub fn units_troubleshoot(
    forge: &dyn Forge,
    harness: &dyn Harness,
    vcs: &dyn Vcs,
    root: &Path,
    units: &[u64],
    timeout: Duration,
) -> usize {
    let mut failed = 0;
    for unit in units {
        match troubleshoot::run(forge, harness, vcs, root, *unit, timeout) {
            Ok(diagnosis) => println!(
                "{}",
                json!({ "unit": unit, "route": diagnosis.route.name() })
            ),
            Err(e) => {
                eprintln!("ns: unit {unit} was not troubleshot: {e:#}");
                failed += 1;
            }
        }
    }
    failed
}
