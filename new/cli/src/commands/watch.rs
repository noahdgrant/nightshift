use std::time::Duration;

use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{forge, harness, triage, vcs, watch};

#[derive(ClapArgs)]
pub struct Args {
    /// Print what would be triaged and run, in order, and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// Triage the units that need it, then stop.
    #[arg(long)]
    triage_only: bool,
    /// With --triage-only: how many units to triage.
    #[arg(long, default_value_t = 1)]
    limit: usize,
    /// How long a triage session may take before it is killed and the unit is left as it was.
    #[arg(long, default_value_t = 1800)]
    harness_timeout_seconds: u64,
}

/// Looks at the repository's open units and says what is next, or triages some of them.
pub fn run(args: Args) -> Result<()> {
    if args.dry_run == args.triage_only {
        bail!("ns watch needs exactly one of --dry-run and --triage-only so far");
    }
    let here = std::env::current_dir()?;
    let root = vcs::get().root_find(&here)?;
    let forge = forge::get();
    let open = forge.unit_list(&root)?;
    let plan = watch::plan(&open);
    if args.dry_run {
        let list = |units: &[&forge::Unit]| -> Vec<_> {
            units
                .iter()
                .map(|u| json!({ "unit": u.number, "title": u.title }))
                .collect()
        };
        println!(
            "{}",
            json!({
                "queue": list(&plan.queue),
                "triage": list(&plan.triage),
                "skipped": plan.skipped.iter()
                    .map(|(u, why)| json!({ "unit": u.number, "why": why }))
                    .collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    let harness = harness::get();
    let timeout = Duration::from_secs(args.harness_timeout_seconds);
    let mut failed = 0;
    for unit in plan.triage.iter().take(args.limit) {
        match triage::run(
            forge.as_ref(),
            harness.as_ref(),
            &root,
            unit.number,
            timeout,
        ) {
            Ok(t) => println!(
                "{}",
                json!({ "unit": unit.number, "route": t.route.name(), "labels": t.labels })
            ),
            Err(e) => {
                failed += 1;
                eprintln!("ns: triage of unit {} went wrong: {e:#}", unit.number);
                println!(
                    "{}",
                    json!({ "unit": unit.number, "error": format!("{e:#}") })
                );
            }
        }
    }
    if failed > 0 {
        bail!("{failed} unit(s) could not be triaged");
    }
    Ok(())
}
