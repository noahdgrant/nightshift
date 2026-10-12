//! `ns ratchet`: clap's definitions and help text.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// Hold measured values (binary size, coverage) to the values recorded in ratchets.json
#[derive(Args)]
#[command(arg_required_else_help = true)]
pub(crate) struct Ratchet {
    #[command(subcommand)]
    pub command: RatchetCmd,
}

const RATCHET_CHECK_HELP: &str = "\
Examples:
  ns ratchet check
  ns ratchet check --human
  ns ratchet check | jq '.ratchets[] | select(.status == \"worse\")'
  ns ratchet check --factory path/to/factory

Spec: docs/FACTORY.md (Ratchets). Runs the command of each [ratchets.<name>] table in
nightshift.toml with sh -c at the top of the git checkout, e.g.
  [ratchets.flash]
  command = \"scripts/flash-size.sh\"   # prints one number on stdout
  better = \"lower\"                    # or \"higher\"
  tolerance = \"0.5%\"                  # of the recorded value, or a number; default 0
and compares what it printed with the value recorded in ratchets.json, next to
nightshift.toml. A value past the recorded one by more than the tolerance is worse. A ratchet
with no recorded value is untracked: reported, not failed, and nothing is recorded (that is
ns ratchet update). A command that exits non-zero, is killed, runs over 300 s
(NS_RATCHET_TIMEOUT_MS) or prints anything but one number is broken. Each command runs twice,
and two values further apart than the tolerance make the ratchet flaky; under ns run the
second run happens only on a ratchet's first check of the night. ns run's gate runs this
command whenever nightshift.toml defines a ratchet. Never edit ratchets.json or a
[ratchets.*] table to pass: loosening is a human decision.

Prints {ok,command,recorded,ratchets:[{name,command,better,tolerance,recorded,measured,runs,
limit,status,error}]}; status is ok, better, worse, untracked, broken or flaky.
Exit codes: 0 nothing worse, broken or flaky, 1 one was, 2 not in a git repo or a bad
nightshift.toml.";

const RATCHET_UPDATE_HELP: &str = "\
Examples:
  ns ratchet update
  ns ratchet update --dry-run
  ns ratchet update --allow-loosen

For a human or a scheduled job, never a unit. Measures each ratchet twice as ns ratchet check
does (see its --help) and records the values in ratchets.json, next to nightshift.toml: an
untracked ratchet is recorded, a better one tightened, an equal one unchanged. A worse one,
even within its tolerance, keeps its recorded value (kept) unless --allow-loosen records it
(loosened). Values of ratchets no longer defined are dropped. A broken or flaky ratchet
writes nothing and exits 1. A rerun with nothing to change writes nothing.

Prints {ok,command,status,dry_run,recorded,ratchets,dropped}; status is written, planned,
already-done or refused; each ratchet's status is recorded, tightened, unchanged, kept,
loosened, broken or flaky.
Exit codes: 0 ok, 1 a ratchet is broken or flaky, 2 no [ratchets.*] table, not in a git repo
or a bad nightshift.toml.";

#[derive(Subcommand)]
pub(crate) enum RatchetCmd {
    /// Fail when a measured value is worse than its recorded value plus tolerance
    #[command(after_help = RATCHET_CHECK_HELP)]
    Check {
        /// Factory directory holding nightshift.toml and ratchets.json (default: .nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
        /// Print JSON (the default)
        #[arg(long, conflicts_with = "human")]
        json: bool,
        /// Print one readable line per ratchet instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Record the current measurements in ratchets.json; it may only tighten
    #[command(after_help = RATCHET_UPDATE_HELP)]
    Update {
        /// Factory directory holding nightshift.toml and ratchets.json (default: .nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
        /// Record worse values too (a human decision: the ratchet loosens)
        #[arg(long)]
        allow_loosen: bool,
        /// Print what would change; write nothing
        #[arg(long)]
        dry_run: bool,
        /// Print JSON (the default)
        #[arg(long, conflicts_with = "human")]
        json: bool,
        /// Print one readable line per ratchet instead of JSON
        #[arg(long)]
        human: bool,
    },
}
