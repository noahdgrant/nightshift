//! `ns transcripts`: clap's definitions and help text.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// List and read Claude Code session transcripts; track which were processed
#[derive(Args)]
#[command(
    after_help = "Examples:\n  ns transcripts list --since 2026-10-01\n  ns transcripts list --new --mark\n  ns transcripts read aaaa1111 --compact",
    arg_required_else_help = true
)]
pub(crate) struct Transcripts {
    #[command(subcommand)]
    pub command: TranscriptsCmd,
}

#[derive(Subcommand)]
pub(crate) enum TranscriptsCmd {
    /// List sessions as JSON: id, source, date, project, size, mtime, path
    #[command(after_help = "\
Examples:
  ns transcripts list
  ns transcripts list --since 2026-10-01 --project ~/zephyrproject/zephyr
  ns transcripts list --new
  ns transcripts list --new --mark

Reads $CLAUDE_CONFIG_DIR/projects (default ~/.claude/projects). Prints
{\"index\",\"marked\",\"sessions\"}, oldest first.

--new keeps sessions whose size or mtime differ from the index at
$XDG_DATA_HOME/nightshift/transcripts-index.json (default ~/.local/share/...).
--mark records the listed sessions there as processed; re-running is safe.")]
    List {
        /// Only sessions that started on or after this date (YYYY-MM-DD, UTC)
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// Only sessions run in this project directory
        #[arg(long, value_name = "PATH")]
        project: Option<PathBuf>,
        /// Only sessions new or changed since the last --mark
        #[arg(long)]
        new: bool,
        /// Record the listed sessions in the index as processed
        #[arg(long)]
        mark: bool,
    },
    /// Print one session's prompts, assistant text, tool calls and errors as JSON
    #[command(after_help = "\
Examples:
  ns transcripts read aaaa1111-0000-4000-8000-000000000001
  ns transcripts read aaaa1111 --compact

Prints {\"id\",\"source\",\"date\",\"project\",\"path\",\"entries\"}. Each entry is
{\"kind\":\"user\"|\"assistant\"|\"output\"|\"error\",\"text\"} or {\"kind\":\"tool\",\"name\",\"input\"}.
Text and tool input over 2000 characters end in \"…[truncated N chars]\"; --compact cuts at 200,
tool output included. Thinking and injected meta messages are left out.
Exit codes: 1 no session matches, 2 the id prefix matches several.")]
    Read {
        /// Session id, or a unique prefix of one
        id: String,
        /// Cut text and tool output at 200 characters instead of 2000
        #[arg(long)]
        compact: bool,
    },
}
