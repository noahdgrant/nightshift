//! `ns clean`: clap's definition and help text.

use clap::Args;

const CLEAN_HELP: &str = "\
Examples:
  ns clean --dry-run
  ns clean

Spec: docs/FACTORY.md (Cleanup). For each worktree on a branch ns/<unit>, reads the issue
the unit id starts with and the PR in .ns/<unit>/pr.md with gh (network). A unit goes when the
issue is closed and the PR merged, closes the issue and comes from ns/<unit>. Its quality record
is written first (a push to origin's nightshift/quality, or the outbox when that fails); then the
worktree, untracked files and .ns/ included, and the local branch are removed. A worktree stays,
with its reason in `kept`, while its issue is open, it has uncommitted changes to tracked files,
its HEAD is not on ns/<unit>, its branch holds changes the merged PR head does not (whitespace
counts), its record could not be saved or a review artifact read, it is the current directory,
or a live ns run or another cleanup holds the unit's run lock (nothing is recorded for it). A PR head missing
locally is fetched from refs/pull/<n>/head. --dry-run writes nothing but may fetch.
ns run cleans its own unit after its merge step; ns watch cleans at start and between units.

Prints {ok,command,dry_run,status,planned|removed,kept,records}; status is planned, cleaned or
nothing-to-clean, so a rerun is safe.
Exit codes: 0 ok, 1 a removal failed (its kept entry has \"error\": true), 2 usage error.";

/// Remove unit worktrees and ns/<unit> branches whose issue is closed and whose PR merged
#[derive(Args)]
#[command(after_help = CLEAN_HELP)]
pub(crate) struct Clean {
    /// Print what would be removed and why the rest stay; remove and record nothing
    #[arg(long)]
    pub dry_run: bool,
}
