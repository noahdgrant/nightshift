//! `ns worktree`: clap's definitions and help text.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// Create, set up, list and remove per-unit worktrees (branch ns/<unit-id>)
#[derive(Args)]
#[command(
    after_help = "Examples:\n  ns worktree new 142-uart-timeout\n  ns worktree setup 142-uart-timeout\n  ns worktree list\n  ns worktree remove 142-uart-timeout --dry-run",
    arg_required_else_help = true
)]
pub(crate) struct Worktree {
    #[command(subcommand)]
    pub command: WorktreeCmd,
}

#[derive(Subcommand)]
pub(crate) enum WorktreeCmd {
    /// Create (or reuse) the worktree for a unit and its .ns/<unit-id>/ artifact folder
    #[command(after_help = "\
Examples:
  ns worktree new 142-uart-timeout
  ns worktree new uart-timeout --base develop
  ns worktree new 142-uart-timeout --repo ~/src/firmware

Creates <repo>/../<repo-name>.worktrees/<unit-id> on branch ns/<unit-id> and
prints {\"unit\",\"path\",\"branch\",\"artifacts\",\"setup\"}. Re-running prints the same JSON.

A new worktree runs the [worktree] setup commands from <main-root>/.nightshift/nightshift.toml
(env NS_UNIT, NS_WORKTREE, NS_MAIN_ROOT); \"setup\" lists {\"run\",\"exit\"} per command.
An existing worktree whose setup never finished (stopped, crashed, failed) runs them again;
one whose setup finished prints \"setup\":[].
A failing command exits 1 and leaves the worktree; retry with ns worktree setup <unit-id>.")]
    New {
        /// Unit id: [a-z0-9][a-z0-9-]*, e.g. 142-uart-timeout
        unit_id: String,
        /// Branch or commit to start from (default: the repo's default branch, else HEAD)
        #[arg(long)]
        base: Option<String>,
        /// Any path inside the repo (default: current directory)
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Skip the [worktree] setup commands from .nightshift/nightshift.toml
        #[arg(long)]
        no_setup: bool,
    },
    /// Re-run the [worktree] setup commands in a unit's existing worktree
    #[command(after_help = "\
Examples:
  ns worktree setup 142-uart-timeout
  ns worktree setup 142-uart-timeout --repo ~/src/firmware

Reads [worktree] setup from <main-root>/.nightshift/nightshift.toml and runs each
command with sh -c in the worktree, stopping at the first failure. It runs them even
when an earlier setup finished, and records setup as finished only when all pass. Prints
{\"unit\",\"path\",\"setup\":[{\"run\",\"exit\"}]}; exits 1 if a command fails.")]
    Setup {
        unit_id: String,
        /// Any path inside the repo (default: current directory)
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
    },
    /// List ns worktrees as JSON, with the latest artifact status
    #[command(after_help = "\
Examples:
  ns worktree list
  ns worktree list --repo ~/src/firmware

Each entry: {\"unit\",\"path\",\"branch\",\"artifacts\",\"status\"}; status is null or
{\"file\",\"phase\",\"status\",\"updated\"} from the newest artifact frontmatter.")]
    List {
        /// Any path inside the repo (default: current directory)
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
    },
    /// Remove a unit's worktree; the ns/<unit-id> branch is kept
    #[command(after_help = "\
Examples:
  ns worktree remove 142-uart-timeout --dry-run
  ns worktree remove 142-uart-timeout
  ns worktree remove 142-uart-timeout --force

Refuses a worktree with uncommitted changes unless --force. Removing a unit with
no worktree is a no-op.")]
    Remove {
        unit_id: String,
        /// Show what would be removed without removing it
        #[arg(long)]
        dry_run: bool,
        /// Remove even if the worktree has uncommitted changes (they are lost)
        #[arg(long)]
        force: bool,
        /// Any path inside the repo (default: current directory)
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
    },
}
