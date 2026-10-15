//! `ns doctor`: clap's definition and help text.

use clap::Args;

/// Report config path and validity, configured roles, harnesses on PATH, harness skill dirs, forge accounts, and free disk space
#[derive(Args)]
#[command(after_help = "\
Examples:
  ns doctor
  NS_CONFIG=./ns.toml ns doctor

skill_dirs.<harness>: the skill directory ns install --harness links into, and
ns_skills, how many ns-*/SKILL.md skills it holds (0 when it is missing).
forge.<name>: configured, token_resolved, the account the forge API returns for the token, and
the table it came from; for the forge origin is on, also repo, repo_readable and, for an SSH
origin, ssh: {dest, account} from ssh -T. warnings: an SSH account that differs from the
token's, and a set GH_TOKEN / GITLAB_TOKEN that overrides the config. The token itself is
never printed.
disk: [limits] disk_wait_gb and disk_stop_gb from this repo's .nightshift/nightshift.toml (the
defaults outside a repo), and each filesystem ns watch checks before it starts a unit (the one
holding the worktrees, and /mnt/c under WSL) with free_gb and state ok, wait or stop, or the
error reading it. warnings: a filesystem under a limit or unreadable.

Exit codes: 0 every check passed (ok is true), 1 a check failed (ok is false; problems says
which). Warnings never change the exit code.")]
pub(crate) struct Doctor {}
