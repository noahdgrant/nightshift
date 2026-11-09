use std::time::Duration;

use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{checks, os, rubric, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// How long the checks may take before they are killed and count as failed.
    #[arg(long, default_value_t = 1800)]
    timeout_seconds: u64,
}

/// Runs the repository's checks on the workspace this is run in. What they print goes to stderr.
pub fn run(args: Args) -> Result<()> {
    let here = std::env::current_dir()?;
    let vcs = vcs::get();
    let root = vcs.root_find(&here)?;
    let problems = rubric::repo_read(&root).1;
    if !problems.is_empty() {
        eprintln!("{}", problems.join("\n"));
        println!(
            "{}",
            json!({ "ok": false, "configured": checks::command_find(&root)?.is_some(), "cached": false })
        );
        bail!("the rubric files are not valid");
    }
    let Some(report) = checks::run(
        vcs.as_ref(),
        os::get().as_ref(),
        &root,
        &here,
        true,
        Duration::from_secs(args.timeout_seconds),
    )?
    else {
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
