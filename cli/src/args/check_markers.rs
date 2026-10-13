//! `ns check-markers`: clap's definition and help text.

use std::path::PathBuf;

use clap::Args;

/// Check human-review markers in tracked files: every start has an end, none nested
#[derive(Args)]
#[command(after_help = "\
Examples:
  ns check-markers
  ns check-markers path/to/repo --human

A region runs from a line containing `ns:human-review start` (optionally `: <reason>`)
through a line containing `ns:human-review end`, in any comment syntax. Checks every file
`git ls-files` lists under the path. Prints {\"ok\",\"files\",\"errors\":[{\"file\",\"line\",\"message\"}]}.
Exit codes: 0 balanced, 1 an unbalanced or nested marker, 2 not a directory in a git repo.")]
pub(crate) struct CheckMarkers {
    /// Directory inside a git repository
    #[arg(default_value = ".")]
    pub path: PathBuf,
    /// Print one readable line per error instead of JSON
    #[arg(long)]
    pub human: bool,
}
