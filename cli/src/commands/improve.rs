use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::Args as ClapArgs;

use crate::harness::Sessions;
use crate::improve::{self, Mode, Request};
use crate::runlog::RunLog;
use crate::states::retrospective::GRADES_FILE;
use crate::{forge, harness, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// How many days back to read grades from.
    #[arg(long, default_value_t = 7)]
    since: u32,
    /// Print the units it would file, and file none.
    #[arg(long)]
    dry_run: bool,
    /// How long the session may take before it is killed.
    #[arg(long, default_value_t = 1800)]
    harness_timeout_seconds: u64,
    /// The model to use, instead of the harness's own.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    model: Option<String>,
}

/// Files a unit for each cause that at least 3 different units failed on, from the grades of the
/// last `--since` days. Changes nothing else, and `ns auto` never runs it.
pub fn run(args: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let events = RunLog::sibling_open(&*vcs, &root, GRADES_FILE)?.events()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let failures = improve::failures_parse(&events, now - i64::from(args.since) * 86_400);
    let mode = if args.dry_run {
        Mode::DryRun
    } else {
        Mode::File
    };
    improve::run(
        forge::get().as_ref(),
        harness::get().as_ref(),
        &root,
        &Request {
            failures: &failures,
            mode,
            sessions: &Sessions {
                timeout: Duration::from_secs(args.harness_timeout_seconds),
                model: args.model.as_deref(),
            },
        },
    )
}
