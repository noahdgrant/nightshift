use std::collections::HashSet;
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::{Local, NaiveTime};
use clap::Args as ClapArgs;
use serde_json::json;

use super::{run as unit_run, triage};
use crate::{auto, forge, harness, lock, until, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// Print what would be triaged and run, in order, and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// How many ready units to run. Without it, every one that is ready.
    #[arg(long)]
    max_units: Option<usize>,
    /// Start no unit after this time of day, like 06:30. A unit already running finishes. If
    /// the time has passed today, it means tomorrow.
    #[arg(long, value_parser = until::time_parse)]
    until: Option<NaiveTime>,
    #[command(flatten)]
    options: unit_run::Options,
}

/// Triages every unit nobody has triaged, then runs the ready units one at a time, each
/// unattended, until none is left or `--max-units` have been run. A unit that is stuck or goes
/// wrong is left for a human by `ns run`, and the next unit goes on.
pub fn run(args: Args) -> Result<()> {
    let here = std::env::current_dir()?;
    let vcs = vcs::get();
    let root = vcs.root_find(&here)?;
    let forge = forge::get();
    let open = forge.unit_list(&root)?;
    let plan = auto::plan(&open);
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

    // Two at once would run the same units twice.
    let _lock = lock::lock_take(
        &vcs.state_dir_find(&root)?,
        "auto.lock",
        "another ns auto is running in this repository",
    )?;
    let harness = harness::get();
    let waiting: Vec<u64> = plan.triage.iter().map(|u| u.number).collect();
    let mut failed = triage::units_triage(
        forge.as_ref(),
        harness.as_ref(),
        &root,
        &waiting,
        Duration::from_secs(args.options.harness_timeout_seconds),
    );
    let deadline = args
        .until
        .map(|time| until::deadline_find(Local::now(), time));
    // A unit is run once, even when it is still ready afterwards.
    let mut run_already = HashSet::new();
    while args.max_units.is_none_or(|max| run_already.len() < max) {
        if deadline.is_some_and(|at| Local::now() >= at) {
            println!("{}", json!({ "stopped": "until" }));
            break;
        }
        // Read again: triage and the units just run change what is ready and what is blocked.
        let open = forge.unit_list(&root)?;
        let plan = auto::plan(&open);
        let Some(unit) = plan
            .queue
            .iter()
            .map(|u| u.number)
            .find(|n| !run_already.contains(n))
        else {
            break;
        };
        run_already.insert(unit);
        let ended = unit_run::run(unit_run::Args::unattended(unit, args.options.clone()));
        if let Err(e) = &ended {
            eprintln!("ns: unit {unit}: {e:#}");
            failed += 1;
        }
        let status: Vec<String> = forge
            .unit_get(&root, unit)
            .map(|u| u.labels)
            .unwrap_or_default()
            .into_iter()
            .filter(|l| l.starts_with(auto::STATE_PREFIX))
            .collect();
        println!(
            "{}",
            json!({ "unit": unit, "ok": ended.is_ok(), "status": status })
        );
    }
    if failed > 0 {
        bail!("{failed} units were not triaged or did not finish");
    }
    Ok(())
}
