//! `ns status`: clap's definition and help text.

use clap::Args;

const STATUS_HELP: &str = "\
Examples:
  ns status
  ns status --since 2026-10-09T22:00:00Z
  ns status --json | jq -r '.ended.stuck[].unit'

Running: each unit a live ns run holds, with its last logged phase. Ended: each other unit by
the outcome of its last run that ended at or after --since: merged; PR for a human (done);
stuck (stuck or budget); paused; split; other (anything else, such as interrupted, shown only
when present). Next: the ready issues ns watch would take, in its order, read from the forge;
when the forge can't be read it says queue unavailable (null in --json) and gives the reason on
stderr. Entries hold only the fields ns log prints. --json prints one object with since,
running, ended and next.
Changes nothing.

Exit codes: 0 ok, 1 the run log or run locks could not be read, 2 usage error (bad --since, not
in a git repo).";

/// Summarise the night: units running now, units ended since --since by outcome, and what is next
#[derive(Args)]
#[command(after_help = STATUS_HELP)]
pub(crate) struct Status {
    /// Count units that ended at or after this UTC time (YYYY-MM-DDTHH:MM:SSZ, or YYYY-MM-DD for 00:00:00 UTC; default: 24 hours ago)
    #[arg(long, value_name = "TIME")]
    pub since: Option<String>,
    /// Print one JSON object instead of the summary
    #[arg(long)]
    pub json: bool,
}
