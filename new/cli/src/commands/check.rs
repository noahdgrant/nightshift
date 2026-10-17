use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{checks, vcs};

#[derive(ClapArgs)]
pub struct Args {}

/// Runs the repository's checks on the workspace this is run in. What they print goes to stderr.
pub fn run(_args: Args) -> Result<()> {
    let here = std::env::current_dir()?;
    let vcs = vcs::get();
    let root = vcs.root_find(&here)?;
    // Outside a unit's workspace there is no start to read the checks from; use the current commit.
    let base = vcs
        .workspace_base_find(&here)
        .unwrap_or_else(|_| "HEAD".into());
    let Some(report) = checks::run(vcs.as_ref(), &root, &here, &base, true)? else {
        println!("{}", json!({ "ok": true, "configured": false }));
        return Ok(());
    };
    println!(
        "{}",
        json!({ "ok": report.passed, "configured": true, "cached": report.cached })
    );
    if !report.passed {
        bail!("the checks failed");
    }
    Ok(())
}
