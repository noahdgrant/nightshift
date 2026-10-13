//! `ns watch`: clap's definition and help text.

use std::path::PathBuf;

use clap::Args;

const WATCH_HELP: &str = "\
Examples:
  ns watch --dry-run
  ns watch --once
  ns watch --until 06:30 --max-units 3
  ns watch --until 06:30 --parallel 3

Lists open issues labelled [queue] ready_label with gh, drops blocked ones and ones with an
open PR that closes them (not one from their own unit branch), sorts by [queue] order, and runs
each as a child ns run --issue, up to --parallel at once, every one on the config and factory
definition read at start. A usage limit in one unit pauses the others at their next phase,
and the budget counts what every unit spent. Forge tokens as in ns run --help.
--until is local time (TZ). Times printed for people, until and reset_at, are local with
their offset (2026-10-09T06:30:00-04:00).
At start, in-progress issues no live ns run holds go back to the ready label (requeued).
At start and between units, worktrees of units a human merged since are removed as ns clean
removes them (cleaned; --dry-run lists them as clean).
SIGINT or SIGTERM ends every running phase and returns each unit's issue to the ready label.
On the nightshift source repo, when origin's default branch gains cli/ commits after the one
ns was built from, it starts no new unit, waits for the running ones, builds the new ns and
execs it with the night's state; --no-self-update turns that off.
Prints {units,requeued,cleaned,stopped,until,cost_usd,error}. Exit codes as ns run's errors: 2 usage,
5 lock held (another ns run or ns watch); 130 or 143 when stopped by SIGINT or SIGTERM.";

/// Pull ready issues from GitHub and run them, up to --parallel at once
#[derive(Args)]
#[command(after_help = WATCH_HELP)]
pub(crate) struct Watch {
    /// Take one unit, then stop
    #[arg(long)]
    pub once: bool,
    /// Start no new unit after this local time (HH:MM)
    #[arg(long, value_name = "HH:MM")]
    pub until: Option<String>,
    /// Units per invocation (default: [limits] max_units, else no cap)
    #[arg(long, value_name = "N")]
    pub max_units: Option<u32>,
    /// Units running at once, each its own ns run (default: [limits] parallel, else 1)
    #[arg(long, value_name = "N")]
    pub parallel: Option<u32>,
    /// Print the ordered queue with skip reasons; change nothing
    #[arg(long)]
    pub dry_run: bool,
    /// Directory holding nightshift.toml (default: <main-worktree>/.nightshift)
    #[arg(long, value_name = "DIR")]
    pub factory: Option<PathBuf>,
    /// On the nightshift source repo, never drain and rebuild ns when cli/ changes
    #[arg(long)]
    pub no_self_update: bool,
    /// The night so far, from the ns that handed off to this one (internal)
    #[arg(long, value_name = "FILE", hide = true)]
    pub resume_night: Option<PathBuf>,
}
