//! `ns ask`: clap's definition and help text.

use std::path::PathBuf;

use clap::Args;

const ASK_HELP: &str = "\
Examples:
  ns ask --role review.security --prompt-file prompt.md
  git diff main | ns ask --role review.correctness
  ns ask --role build --write --cwd ../myrepo.worktrees/142-uart-timeout --prompt-file brief.md
  ns ask --role verify --dry-run

Config: $NS_CONFIG, else $XDG_CONFIG_HOME/nightshift/config.toml, else ~/.config/nightshift/config.toml.
  [roles.default]
  harness = \"claude\"
  [roles.\"review.security\"]
  harness = \"codex\"
  model = \"gpt-5-codex\"

Read-only by default. --write switches to the harness's command_write (built-in:
claude --permission-mode acceptEdits, codex --sandbox workspace-write).

Exit codes: 1 harness failed, 3 role not configured, 4 harness binary not on PATH,
5 --write given but the harness has no command_write.";

/// Run the harness configured for a role headless; prompt on stdin, answer on stdout
#[derive(Args)]
#[command(after_help = ASK_HELP)]
pub(crate) struct Ask {
    /// Dotted role name, e.g. review.security. Falls back to review, then default
    #[arg(long)]
    pub role: String,
    /// Read the prompt from this file instead of stdin
    #[arg(long, value_name = "FILE")]
    pub prompt_file: Option<PathBuf>,
    /// Let the harness edit files and run commands (uses the harness's command_write)
    #[arg(long)]
    pub write: bool,
    /// Run the harness in this directory, e.g. a unit's worktree (default: current directory)
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,
    /// Print the resolved command as JSON without running it
    #[arg(long)]
    pub dry_run: bool,
}
