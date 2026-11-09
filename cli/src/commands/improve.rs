use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::Args as ClapArgs;

use crate::manifest::{self, Place};
use crate::runlog::RunLog;
use crate::states::retrospective::GRADES_FILE;
use crate::{forge, harness, improve, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// How many days back to read the grades that did not pass.
    #[arg(long, default_value_t = 7)]
    since: u32,
    /// Print the units it would file instead of filing them.
    #[arg(long)]
    dry_run: bool,
    /// How long the session may take before it is killed and nothing is filed.
    #[arg(long, default_value_t = 1800)]
    harness_timeout_seconds: u64,
    /// The model to use, instead of the one `[model]` in `.nightshift/nightshift.toml` names.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    model: Option<String>,
}

/// Files units that fix the causes that at least 3 units failed on the same rubric, from the
/// grades of the last `--since` days. It changes no file, and `ns auto` never runs it.
pub fn run(args: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    // Read here so a bad value stops `ns` before it runs anything. The session is read-only like
    // triage's, so it uses triage's model.
    let model = manifest::model_read(&root, Place::Triage, args.model.as_deref())?;
    let events = RunLog::sibling_open(&*vcs, &root, GRADES_FILE)?.events()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let lines = improve::run(
        forge::get().as_ref(),
        harness::get().as_ref(),
        &root,
        &crate::grades::failures_since(&events, now, args.since),
        args.dry_run,
        Duration::from_secs(args.harness_timeout_seconds),
        model.as_deref(),
    )?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}
