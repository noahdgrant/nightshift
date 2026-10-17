use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{auto, forge, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// Print what would be triaged and run, in order, and change nothing.
    #[arg(long)]
    dry_run: bool,
}

/// Looks at the repository's open units and says what is next.
pub fn run(args: Args) -> Result<()> {
    if !args.dry_run {
        bail!("ns auto only supports --dry-run so far");
    }
    let here = std::env::current_dir()?;
    let root = vcs::get().root_find(&here)?;
    let open = forge::get().unit_list(&root)?;
    let plan = auto::plan(&open);
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
    Ok(())
}
