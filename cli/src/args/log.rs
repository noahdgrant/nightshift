//! `ns log`: clap's definition and help text.

use clap::Args;

const LOG_HELP: &str = "\
Examples:
  ns log
  ns log -f
  ns log --unit 142-uart-timeout --since 2026-10-09
  ns log --json | jq -r 'select(.event == \"end\") | .reason'
  ns log --phase-output --unit 142-uart-timeout -f

Reads <git-common-dir>/ns/runs.jsonl, which ns run and ns watch append to. A line per event:
time, event, #issue, phase, attempt, status, duration, outcome, PR link and reason, leaving out
what the event lacks. The PR link comes from origin's URL; without one it is pr #<n>.
--json prints one object per event with only ts, event, unit, issue, phase, attempt, status,
wall_s, outcome, pr, pr_url and reason; nothing else in the log is printed. --since is UTC and
inclusive, and drops events with no time. -f prints the log so far, then polls it every
0.5 s until stopped (Ctrl-C). Lines that are not UTF-8 JSON objects are skipped; without -f
their count is on stderr.
--phase-output reads instead the newest transcript under <git-common-dir>/ns/transcripts/<unit>/
(of the unit the run log names last without --unit): its last 20 assistant messages and tool calls, one line each, text
past 200 characters cut short and known credential shapes replaced by [redacted]. With --json
each is {\"kind\":\"assistant\",\"text\"} or {\"kind\":\"tool\",\"name\",\"input\"}. -f follows that
transcript, and moves on to the next phase's. It does not take --since.
Changes nothing.

Exit codes: 0 ok, 1 the log or transcript could not be read or there is no transcript yet, 2 usage
error (bad --since or --unit, not in a git repo).";

/// Print the run log, one line per event; -f follows new events as runs append them
#[derive(Args)]
#[command(after_help = LOG_HELP)]
pub(crate) struct Log {
    /// Only this unit's events
    #[arg(long, value_name = "UNIT_ID")]
    pub unit: Option<String>,
    /// Only events at or after this UTC time (YYYY-MM-DDTHH:MM:SSZ, or YYYY-MM-DD for 00:00:00 UTC)
    #[arg(long, value_name = "TIME")]
    pub since: Option<String>,
    /// Keep running and print events as they are appended, like tail -f
    #[arg(short, long)]
    pub follow: bool,
    /// Print one JSON object per event instead of a line
    #[arg(long)]
    pub json: bool,
    /// Print the assistant messages and tool calls of the newest phase transcript instead
    #[arg(long, conflicts_with = "since")]
    pub phase_output: bool,
}
