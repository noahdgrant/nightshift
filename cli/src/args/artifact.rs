//! `ns artifact`: clap's definitions and help text.

use std::path::PathBuf;

use clap::builder::PossibleValuesParser;
use clap::{Args, Subcommand};

/// Check a phase artifact's ns-artifact/2 frontmatter
#[derive(Args)]
#[command(after_help = "\
Examples:
  ns artifact check .ns/142-uart-timeout/evidence.md --phase verify
  ns artifact check .ns/$NS_UNIT/review.md --phase review | jq -r '.errors[]'

Run `ns artifact check --help` for the schema and exit codes.")]
pub(crate) struct Artifact {
    #[command(subcommand)]
    pub command: ArtifactCmd,
}

#[derive(Subcommand)]
pub(crate) enum ArtifactCmd {
    /// Validate an artifact's frontmatter against ns-artifact/2 and print what routing will read
    #[command(after_help = "\
Examples:
  ns artifact check .ns/142-uart-timeout/evidence.md --phase verify
  ns artifact check .ns/$NS_UNIT/brief.md --phase triage
  ns artifact check .ns/$NS_UNIT/pr.md | jq -r '.errors[]'

The frontmatter opens the file and holds schema: ns-artifact/2, unit, phase, status, cause,
next, reason, sha (every phase but triage), updated, and as the phase needs them delta_of,
findings (review), pr (ship), base and touches (triage). Unknown keys are ignored. Rules:
status pass has cause none; fail and blocked need a cause, a next and a reason; split is
triage's only; a passed triage names next. A file with no schema key is ns-artifact/1 and
fails the check. A build artifact also fails on a `file:` line that names a file the unit's
diff touches (from brief.md's base, else origin/HEAD, to HEAD) unless it carries an
`unrelated: <reason>` or `too-big: <lines>` tag. Spec: skills/ns-contract/SKILL.md (Units of work and artifacts).
Prints {ok,file,schema,phase,status,cause,next,errors:[...]}, the values as written when invalid.
Exit codes: 0 valid, 1 invalid (each error says what to change), 2 usage error or unreadable file.")]
    Check {
        /// The artifact, such as .ns/<unit>/evidence.md
        file: PathBuf,
        /// The phase that wrote it; the artifact's `phase` must match
        #[arg(long, value_parser = PossibleValuesParser::new(crate::artifact::Phase::NAMES))]
        phase: Option<String>,
    },
}
