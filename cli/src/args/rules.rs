//! `ns rules`: clap's definitions and help text.

use clap::{Args, Subcommand};

/// Run the repo's rule pack and fail only on violations not in its baseline
#[derive(Args)]
#[command(arg_required_else_help = true)]
pub(crate) struct Rules {
    #[command(subcommand)]
    pub command: RulesCmd,
}

const RULES_CHECK_HELP: &str = "\
Examples:
  ns rules check
  ns rules check --human
  ns rules check | jq .new

Spec: docs/factory/run.md (Rules). Runs each command in the `rules` row of docs/agents/stack.md
with sh -c at the top of the git checkout, e.g.
  | rules | `ruff check --output-format=concise .`, `./scripts/rules-json` | 10 s |
Each output line (stdout or stderr) that is a JSON object with \"file\" (and optionally
\"tool\", \"rule\", \"line\", \"message\", \"snippet\"), or reads file:line[:col]: message, is a
finding {tool,rule,file,line,message,snippet}. tool defaults to the command's first word,
snippet to the source line with whitespace collapsed. A finding matches a baseline entry in
.nightshift/rules-baseline.json on (tool, rule, file, snippet), so a moved line still matches.
A command that exits non-zero and prints no finding is an error, as is one that is killed,
exits 126 or 127, runs over 300 s (NS_RULES_TIMEOUT_MS) or prints over 8 MiB
(NS_RULES_MAX_OUTPUT_BYTES), whatever it printed first. No rules row: nothing runs.

Prints {ok,command,baseline,commands,findings,baselined,new,fixed}; fixed lists baseline
entries nothing matched (drop them with ns rules baseline).
Exit codes: 0 no new violations, 1 new violations or a command failed, 2 not in a git repo.";

const RULES_BASELINE_HELP: &str = "\
Examples:
  ns rules baseline
  ns rules baseline --dry-run
  ns rules baseline --allow-growth

Runs the rule pack as ns rules check does (see its --help) and writes every current violation
to .nightshift/rules-baseline.json, dropping entries that are gone. A violation the baseline
lacks, including every one when there is no baseline yet, is growth: without --allow-growth
it writes nothing and exits 1. A rerun with nothing added or dropped writes nothing. A command
that fails, is killed, times out or prints too much is an error, as in ns rules check.

Prints {ok,command,status,dry_run,baseline,commands,violations,added,dropped}; status is
written, planned, already-done or refused.
Exit codes: 0 ok, 1 refused growth or a command failed, 2 no rules row or not in a git repo.";

#[derive(Subcommand)]
pub(crate) enum RulesCmd {
    /// Fail on violations the baseline doesn't list; report the ones that are gone
    #[command(after_help = RULES_CHECK_HELP)]
    Check {
        /// Print one readable line per new or fixed violation instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Rewrite the baseline from the current violations; it may only shrink
    #[command(after_help = RULES_BASELINE_HELP)]
    Baseline {
        /// Let the baseline gain entries (a human decision: they become accepted debt)
        #[arg(long)]
        allow_growth: bool,
        /// Print what would change; write nothing
        #[arg(long)]
        dry_run: bool,
        /// Print one readable line per added or dropped violation instead of JSON
        #[arg(long)]
        human: bool,
    },
}
