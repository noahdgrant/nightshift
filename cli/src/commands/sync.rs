use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{manifest, sync, vcs};

#[derive(ClapArgs)]
pub struct Args {}

/// Writes the skills of this `ns` where the harnesses of the checkout it is run in find them. Run again, it
/// changes only what differs.
pub fn run(_args: Args) -> Result<()> {
    let top = vcs::get().workspace_find(&std::env::current_dir()?)?;
    let pin = manifest::pin_read(&top)?;
    let synced = sync::run(&top, &pin.harness)?;
    let report: Vec<_> = synced
        .iter()
        .map(|s| {
            json!({ "harness": s.harness.name(), "dir": s.dir, "written": s.written, "removed": s.removed })
        })
        .collect();
    println!("{}", json!({ "synced": report }));
    Ok(())
}
