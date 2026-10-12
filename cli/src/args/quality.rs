//! `ns quality`: clap's definitions and help text.

use std::path::PathBuf;

use clap::{Args, Subcommand};

const QUALITY_HELP: &str = "\
Examples:
  ns quality
  ns quality --since 2026-10-01
  ns quality --since 2026-10-09T21:04:00Z
  ns quality --since 2026-10-01 --until 2026-10-09
  ns quality --json | jq .first_pass
  ns quality import /tmp/unit-artifacts --dry-run

Spec: docs/FACTORY.md (Quality). Fetches origin's nightshift/quality branch (network) and reads
its records.jsonl plus <git-common-dir>/ns/quality-outbox.jsonl: one record per review attempt,
written by ns run when a unit ends. A unit with no record is read from .ns/<unit>/review.md,
review/cycle-*.md and history/review* in its linked worktree. Also reads
<git-common-dir>/ns/runs.jsonl. Changes nothing but the remote-tracking ref.
`records` says how many records came from the branch and the outbox, and why a fetch failed.
First-pass numbers come from a unit's oldest review artifact, the rest from its newest.
Prints {units,findings,first_pass,cycles_to_clean,leftovers,escapes,leftover_findings,
escape_findings,trend,run_log,gaps,per_unit,records}; per_unit[].source is record or worktree.
--since and --until are inclusive and UTC, and so is their echo in the output: a bare date
is 00:00:00 for --since and 23:59:59 for --until. trend and per_unit days are local (TZ).

Exit codes: 0 ok, 1 git failed, 2 usage error (bad --since or --until, --until before --since,
not in a git repo).";

/// Report how well units pass review: first-pass yield, findings per 100 lines, leftovers, escapes
#[derive(Args)]
#[command(after_help = QUALITY_HELP, args_conflicts_with_subcommands = true)]
pub(crate) struct Quality {
    #[command(subcommand)]
    pub command: Option<QualityCmd>,
    /// Only units whose newest review artifact was updated, and run-log events, at or after this UTC time (YYYY-MM-DDTHH:MM:SSZ, or YYYY-MM-DD for 00:00:00 UTC)
    #[arg(long, value_name = "TIME")]
    pub since: Option<String>,
    /// Only units whose newest review artifact was updated, and run-log events, at or before this UTC time (YYYY-MM-DDTHH:MM:SSZ, or YYYY-MM-DD for 23:59:59 UTC)
    #[arg(long, value_name = "TIME")]
    pub until: Option<String>,
    /// Print JSON (the default and only format)
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand)]
pub(crate) enum QualityCmd {
    /// Turn archived unit worktrees into quality records and push them to origin's nightshift/quality
    #[command(after_help = "\
Examples:
  ns quality import /tmp/unit-artifacts --dry-run
  ns quality import /tmp/unit-artifacts

<DIR> holds one directory per unit, laid out as in its worktree: <DIR>/<unit>/.ns/<unit>/review.md.
Units origin's branch (or the outbox) already has a record for are skipped, so a rerun is safe
and reports status already-done. Issue numbers come from the unit id, PRs from pr.md or the run
log, outcomes from <git-common-dir>/ns/runs.jsonl. A missing change size is measured with git in
this repo. --dry-run fetches the branch (network) to find what's recorded, and pushes nothing.

Prints {ok,status,units,records,imported,already_recorded,skipped,unparsed,branch,push}.
Exit codes: 0 pushed, planned or already-done; 1 nothing-to-import (no unit in <DIR> has a
review artifact), or outbox: the push failed and the records wait in the
outbox (the next ns run or import pushes them); 2 usage error.")]
    Import {
        /// Directory holding <unit>/.ns/<unit>/ for each archived unit
        dir: PathBuf,
        /// Print what would be imported; push nothing
        #[arg(long)]
        dry_run: bool,
    },
}
