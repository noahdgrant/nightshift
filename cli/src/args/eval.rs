//! `ns eval`: clap's definition and help text.

use std::path::PathBuf;

use clap::Args;

use crate::eval;

const EVAL_HELP: &str = "\
Examples:
  ns eval --dry-run
  ns eval ns-tdd --case py-capacity-off-by-one --trials 1 --human
  ns eval ns-tdd --compare main --cases-only
  ns eval --changed-since origin/main --budget-usd 2
  ns eval ns-triage --triggers-only --no-write-results

Spec: docs/EVALS.md. Config: [eval] in the ns config (see ns ask --help for the path).
Each trial runs in a scratch dir under the system temp dir with HOME pointed at a
throwaway directory holding only the arm's skills. Prints JSON on stdout and writes
skills/<skill>/evals/results/<date>-<short-sha>.json. Raw transcripts and the
baseline cache live under [eval].transcripts.

Exit codes: 1 failure, 2 usage error, 3 eval harness not configured or bad config,
4 harness binary not on PATH.";

/// Measure skills: trigger evals and behaviour cases with and without the skill
#[derive(Args)]
#[command(after_help = EVAL_HELP)]
pub(crate) struct Eval {
    /// Skills to evaluate, e.g. ns-tdd (or tdd). Default: every skill with an evals/ dir
    pub skills: Vec<String>,
    /// Only this case id; repeat for several
    #[arg(long = "case", value_name = "ID")]
    pub cases: Vec<String>,
    /// Comma-separated arms: with, without (default: with,without)
    #[arg(long, value_name = "LIST", conflicts_with = "compare")]
    pub arms: Option<String>,
    /// Compare the skills against themselves at a git ref (arms: with, old)
    #[arg(long, value_name = "REF")]
    pub compare: Option<String>,
    /// Starting trials per arm (overrides [eval].trials)
    #[arg(long, value_name = "N")]
    pub trials: Option<u32>,
    /// Only skills with a file changed since this ref (committed, staged, unstaged or untracked)
    #[arg(long, value_name = "REF")]
    pub changed_since: Option<String>,
    /// Stop starting trials once the reported cost reaches this (overrides [eval].budget_usd)
    #[arg(long, value_name = "USD")]
    pub budget_usd: Option<f64>,
    /// Stop after this many harness runs (overrides [eval].max_runs)
    #[arg(long, value_name = "N")]
    pub max_runs: Option<u32>,
    /// Run only trigger evals
    #[arg(long, conflicts_with = "cases_only")]
    pub triggers_only: bool,
    /// Run only behaviour cases
    #[arg(long)]
    pub cases_only: bool,
    /// Print the plan as JSON; never calls a harness
    #[arg(long)]
    pub dry_run: bool,
    /// Ignore cached `without` baselines and rerun them
    #[arg(long)]
    pub no_cache: bool,
    /// Also write the result JSON to this file
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// Skills directory; fixtures are read from <its parent>/evals/fixtures
    #[arg(long, value_name = "DIR", default_value = "skills")]
    pub skills_dir: PathBuf,
    /// Don't write skills/<skill>/evals/results/<date>-<sha>.json
    #[arg(long)]
    pub no_write_results: bool,
    /// Also print a readable table to stderr
    #[arg(long)]
    pub human: bool,
    /// Eval harness name (overrides [eval].harness)
    #[arg(long)]
    pub harness: Option<String>,
    /// Model (overrides [eval].model)
    #[arg(long)]
    pub model: Option<String>,
    /// Seconds before a trigger run is killed
    #[arg(long, value_name = "SECS", default_value_t = eval::DEFAULT_TRIGGER_TIMEOUT_SECS)]
    pub trigger_timeout: u64,
}
