//! `ns install`: clap's definition and help text.

use std::path::PathBuf;

use clap::Args;

use crate::install;

/// Symlink skills/ns-* into harness skill directories
#[derive(Args)]
#[command(after_help = "\
Examples:
  ns install --dry-run
  ns install --source ~/src/nightshift/skills
  ns install --harness claude --harness cursor
  ns install --harness codex --target ~/work/skills

Harness skill directories, as each harness documents them:
  claude  ~/.claude/skills
  codex   ~/.agents/skills
  cursor  ~/.cursor/skills
  agents  ~/.agents/skills (shared; Codex and Cursor read it)

Targets are every --harness directory plus every --target directory. With
neither flag: --harness agents --harness claude. Directories are created if missing.
Stale ns-* symlinks are replaced. Real directories in the way are reported as
conflicts and left alone; the command then exits 1. Safe to re-run.")]
pub(crate) struct Install {
    /// Directory holding the ns-* skill directories (default: ./skills if present, else the checkout ns was built from)
    #[arg(long, value_name = "DIR")]
    pub source: Option<PathBuf>,
    /// Link into this harness's skill directory; repeat for several (default: agents, claude)
    #[arg(long, value_name = "HARNESS")]
    pub harness: Vec<install::Harness>,
    /// Also link into this skill directory; repeat for several
    #[arg(long, value_name = "DIR")]
    pub target: Vec<PathBuf>,
    /// Report the actions without touching the filesystem
    #[arg(long)]
    pub dry_run: bool,
}
