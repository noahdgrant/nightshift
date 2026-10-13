//! `ns score`: clap's definition and help text.

use std::path::PathBuf;

use clap::Args;

const SCORE_HELP: &str = "\
Examples:
  ns score 142-uart-timeout
  ns score 142-uart-timeout --scorer evidence
  ns score 142-uart-timeout --dry-run

Spec: docs/factory/scorers.md (Scoring). Builds a prompt per scorer from its rubric and labels, every
*.md in .ns/<unit>/ (the unit's worktree, else the main checkout) and the PR outcome: the PR in
pr.md read with gh (network, --dry-run too), whether a commit on the default branch reverts its
merge commit, and its review count. Runs the scorer's role read-only through `ns ask` and takes
the reply's last `label: <value>` line. Appends {unit,scorer,label,score,pass} per scorer to
<common git dir>/ns/scores.jsonl only when every judge named a known label.

Prints {ok,unit,ledger,scores}; with --dry-run {ok,dry_run,unit,prompts:[{scorer,role,prompt}]}.
Exit codes: 0 scored, 1 a judge named no known label, a scorer is invalid or missing, or gh or
the harness failed, 2 usage error, 3 role not configured, 4 harness missing.";

/// Grade a finished unit with the factory's scorers and append the results to the run ledger
#[derive(Args)]
#[command(after_help = SCORE_HELP)]
pub(crate) struct Score {
    /// Unit id, e.g. 142-uart-timeout
    pub unit_id: String,
    /// Only this scorer (the file stem of scorers/<name>.md; default: every scorer)
    #[arg(long, value_name = "NAME")]
    pub scorer: Option<String>,
    /// Directory holding nightshift.toml (default: <main-worktree>/.nightshift)
    #[arg(long, value_name = "DIR")]
    pub factory: Option<PathBuf>,
    /// Print each scorer's prompt as JSON; calls no judge and writes no ledger line
    #[arg(long)]
    pub dry_run: bool,
}
