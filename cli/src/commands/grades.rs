use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::Args as ClapArgs;

use crate::runlog::RunLog;
use crate::states::retrospective::GRADES_FILE;
use crate::transcripts::Transcripts;
use crate::{grades, harness, manifest, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// How many ISO weeks back to show, the current one included.
    #[arg(long, default_value_t = 8)]
    weeks: u32,
    /// Print one JSON line for each rubric and period, model, wait, cycle and unmerged-token line instead of the tables.
    #[arg(long)]
    json: bool,
}

/// Prints each rubric's pass rate and number of grades per ISO week, per `ns` version and per
/// model of the unit's last `build` session, from `grades.jsonl`. Changes nothing and starts no session.
pub fn run(args: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let level = manifest::retrospective_share_read(&root)?;
    let events = RunLog::sibling_open(&*vcs, &root, GRADES_FILE)?.events()?;
    let entries = grades::parse(&events);
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let runs = RunLog::open(&*vcs, &root)?.events()?;
    let harness = harness::get();
    let transcripts = Transcripts::open(&*vcs, &root)?;
    // The tokens of a unit's kept sessions; `None` when none is kept or none says its tokens.
    let kept = |unit: &str| {
        let texts: Vec<String> = transcripts
            .sessions(unit)
            .ok()?
            .iter()
            .filter_map(|s| std::fs::read(&s.path).ok())
            .map(|raw| String::from_utf8_lossy(&raw).into_owned())
            .collect();
        grades::kept_tokens(&*harness, &texts)
    };
    let report = grades::report(&entries, now, args.weeks)
        .with_waits(&runs, now, args.weeks)
        .with_cycles(&runs, &entries, &kept, now, args.weeks, level);
    if report.is_empty() {
        if !args.json {
            if entries.is_empty() {
                println!("There are no grades yet.");
            } else {
                println!("There are no grades in the last {} weeks.", args.weeks);
            }
        }
    } else if args.json {
        for line in report.json_lines() {
            println!("{line}");
        }
    } else {
        print!("{}", report.render());
    }
    Ok(())
}
