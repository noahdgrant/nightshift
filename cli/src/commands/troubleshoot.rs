use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::auto::Streak;
use crate::forge::Forge;
use crate::harness::{Harness, Sessions};
use crate::manifest::{self, Place};
use crate::vcs::Vcs;
use crate::{forge, harness, troubleshoot, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to troubleshoot: its number on the forge.
    unit: u64,
    /// How long the session may take before it is killed and the unit is left as it was.
    #[arg(long, default_value_t = 3600)]
    harness_timeout_seconds: u64,
    /// The model to use, instead of the one `[model]` in `.nightshift/nightshift.toml` names.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    model: Option<String>,
}

/// Diagnoses a bug nobody has reproduced, adds the diagnosis to the unit and sends it on.
pub fn run(args: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let timeout = Duration::from_secs(args.harness_timeout_seconds);
    // Read here so a bad value stops `ns` before it runs anything.
    let model = manifest::model_read(&root, Place::Troubleshoot, args.model.as_deref())?;
    let failed = units_troubleshoot(
        forge::get().as_ref(),
        harness::get().as_ref(),
        vcs.as_ref(),
        &root,
        &[args.unit],
        &Sessions {
            timeout,
            model: model.as_deref(),
        },
        &mut Streak::new(0),
    );
    if failed > 0 {
        bail!("unit {} was not troubleshot", args.unit);
    }
    Ok(())
}

/// Troubleshoots each of `units`, printing one JSON line for each that was. A unit that cannot be
/// is reported on stderr and left as it was. Returns how many could not be. A failed session
/// counts toward `streak`, and once it is reached no more units are troubleshot.
pub fn units_troubleshoot(
    forge: &dyn Forge,
    harness: &dyn Harness,
    vcs: &dyn Vcs,
    root: &Path,
    units: &[u64],
    limits: &Sessions,
    streak: &mut Streak,
) -> usize {
    let mut failed = 0;
    for unit in units {
        if streak.reached() {
            break;
        }
        match troubleshoot::run(
            forge,
            harness,
            vcs,
            root,
            *unit,
            limits.timeout,
            limits.model,
        ) {
            Ok(diagnosis) => {
                streak.record(*unit, false);
                println!(
                    "{}",
                    json!({ "unit": unit, "route": diagnosis.route.name() })
                )
            }
            Err(e) => {
                eprintln!("ns: unit {unit} was not troubleshot: {e:#}");
                streak.record(*unit, harness::is_session_failure(&e));
                failed += 1;
            }
        }
    }
    failed
}
