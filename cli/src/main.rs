//! `sf`: the Software Factory CLI.

mod ask;
mod config;
mod doctor;
mod error;
mod frontmatter;
mod git;
mod install;
mod lint;
mod which;
mod worktree;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use error::SfError;

const ROOT_HELP: &str = "\
Examples:
  sf worktree new 142-uart-timeout
  sf worktree list
  echo 'Review this diff' | sf ask --role review.security
  sf lint skills
  sf install --dry-run
  sf doctor

Exit codes: 0 ok, 1 failure, 2 usage error, 3 role not configured, 4 harness missing,
5 no write command for `sf ask --write`.
Run `sf <command> --help` for details on one command.";

#[derive(Parser)]
#[command(
    name = "sf",
    version,
    about = "Software Factory CLI: worktrees, headless harness calls, skill lint and install",
    after_help = ROOT_HELP,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create, list and remove per-unit worktrees (branch sf/<unit-id>)
    #[command(
        after_help = "Examples:\n  sf worktree new 142-uart-timeout\n  sf worktree list\n  sf worktree remove 142-uart-timeout --dry-run",
        arg_required_else_help = true
    )]
    Worktree {
        #[command(subcommand)]
        command: WorktreeCmd,
    },

    /// Run the harness configured for a role headless; prompt on stdin, answer on stdout
    #[command(after_help = ASK_HELP)]
    Ask {
        /// Dotted role name, e.g. review.security. Falls back to review, then default
        #[arg(long)]
        role: String,
        /// Read the prompt from this file instead of stdin
        #[arg(long, value_name = "FILE")]
        prompt_file: Option<PathBuf>,
        /// Let the harness edit files and run commands (uses the harness's command_write)
        #[arg(long)]
        write: bool,
        /// Run the harness in this directory, e.g. a unit's worktree (default: current directory)
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
        /// Print the resolved command as JSON without running it
        #[arg(long)]
        dry_run: bool,
    },

    /// Validate sf-*/SKILL.md skills (frontmatter, names, upstream, relative links)
    #[command(after_help = "\
Examples:
  sf lint
  sf lint path/to/skills
  sf lint skills --human

Prints {\"ok\",\"skills\",\"errors\":[{\"skill\",\"file\",\"message\"}]}. Exits 1 if any error.")]
    Lint {
        /// Skills directory
        #[arg(default_value = "skills")]
        path: PathBuf,
        /// Print one readable line per error instead of JSON
        #[arg(long)]
        human: bool,
    },

    /// Symlink skills/sf-* into harness skill directories
    #[command(after_help = "\
Examples:
  sf install --dry-run
  sf install --source ~/src/software-factory/skills
  sf install --target ~/.agents/skills --target ~/.cursor/skills

Default targets: ~/.agents/skills and ~/.claude/skills (created if missing).
Stale sf-* symlinks are replaced. Real directories in the way are reported as
conflicts and left alone; the command then exits 1. Safe to re-run.")]
    Install {
        /// Directory holding the sf-* skill directories (default: ./skills if present, else the checkout sf was built from)
        #[arg(long, value_name = "DIR")]
        source: Option<PathBuf>,
        /// Target skill directory; repeat for several (default: ~/.agents/skills, ~/.claude/skills)
        #[arg(long, value_name = "DIR")]
        target: Vec<PathBuf>,
        /// Report the actions without touching the filesystem
        #[arg(long)]
        dry_run: bool,
    },

    /// Report config path and validity, configured roles, and harnesses on PATH
    #[command(after_help = "\
Examples:
  sf doctor
  SF_CONFIG=./sf.toml sf doctor")]
    Doctor,
}

const ASK_HELP: &str = "\
Examples:
  sf ask --role review.security --prompt-file prompt.md
  git diff main | sf ask --role review.correctness
  sf ask --role build --write --cwd ../myrepo.worktrees/142-uart-timeout --prompt-file brief.md
  sf ask --role verify --dry-run

Config: $SF_CONFIG, else $XDG_CONFIG_HOME/sf/config.toml, else ~/.config/sf/config.toml.
  [roles.default]
  harness = \"claude\"
  [roles.\"review.security\"]
  harness = \"codex\"
  model = \"gpt-5-codex\"

Read-only by default. --write switches to the harness's command_write (built-in:
claude --permission-mode acceptEdits, codex --sandbox workspace-write).

Exit codes: 1 harness failed, 3 role not configured, 4 harness binary not on PATH,
5 --write given but the harness has no command_write.";

#[derive(Subcommand)]
enum WorktreeCmd {
    /// Create (or reuse) the worktree for a unit and its .sf/<unit-id>/ artifact folder
    #[command(after_help = "\
Examples:
  sf worktree new 142-uart-timeout
  sf worktree new uart-timeout --base develop
  sf worktree new 142-uart-timeout --repo ~/src/firmware

Creates <repo>/../<repo-name>.worktrees/<unit-id> on branch sf/<unit-id> and
prints {\"unit\",\"path\",\"branch\",\"artifacts\"}. Re-running prints the same JSON.")]
    New {
        /// Unit id: [a-z0-9][a-z0-9-]*, e.g. 142-uart-timeout
        unit_id: String,
        /// Branch or commit to start from (default: the repo's default branch, else HEAD)
        #[arg(long)]
        base: Option<String>,
        /// Any path inside the repo (default: current directory)
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
    },
    /// List sf worktrees as JSON, with the latest artifact status
    #[command(after_help = "\
Examples:
  sf worktree list
  sf worktree list --repo ~/src/firmware

Each entry: {\"unit\",\"path\",\"branch\",\"artifacts\",\"status\"}; status is null or
{\"file\",\"phase\",\"status\",\"updated\"} from the newest artifact frontmatter.")]
    List {
        /// Any path inside the repo (default: current directory)
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
    },
    /// Remove a unit's worktree; the sf/<unit-id> branch is kept
    #[command(after_help = "\
Examples:
  sf worktree remove 142-uart-timeout --dry-run
  sf worktree remove 142-uart-timeout
  sf worktree remove 142-uart-timeout --force

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

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Cmd::Worktree { command } => match command {
            WorktreeCmd::New {
                unit_id,
                base,
                repo,
            } => worktree::new(&unit_id, base.as_deref(), repo.as_deref())?,
            WorktreeCmd::List { repo } => worktree::list(repo.as_deref())?,
            WorktreeCmd::Remove {
                unit_id,
                dry_run,
                force,
                repo,
            } => worktree::remove(&unit_id, dry_run, force, repo.as_deref())?,
        },
        Cmd::Ask {
            role,
            prompt_file,
            write,
            cwd,
            dry_run,
        } => ask::run(ask::AskArgs {
            role: &role,
            prompt_file: prompt_file.as_deref(),
            write,
            cwd: cwd.as_deref(),
            dry_run,
        })?,
        Cmd::Lint { path, human } => {
            let report = lint::lint_dir(&path)?;
            if human {
                for e in &report.errors {
                    println!("{}: {}: {}", e.file, e.skill, e.message);
                }
                println!(
                    "{} skills checked, {} errors",
                    report.skills,
                    report.errors.len()
                );
            } else {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            if !report.ok {
                return Ok(ExitCode::from(1));
            }
        }
        Cmd::Install {
            source,
            target,
            dry_run,
        } => {
            let targets = if target.is_empty() {
                install::default_targets()
            } else {
                target
            };
            let source = source.unwrap_or_else(install::default_source);
            let report = install::run(&source, &targets, dry_run)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.conflicts > 0 {
                eprintln!(
                    "error: {} conflict(s): real files or unrelated symlinks are in the way; move them, then re-run:\n  sf install --dry-run",
                    report.conflicts
                );
                return Ok(ExitCode::from(1));
            }
        }
        Cmd::Doctor => doctor::run()?,
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            if let Some(sf) = e.downcast_ref::<SfError>() {
                eprintln!("error: {}", sf.message);
                if let Some(h) = &sf.hint {
                    for line in h.lines() {
                        if line.is_empty() {
                            eprintln!();
                        } else {
                            eprintln!("  {line}");
                        }
                    }
                }
                ExitCode::from(sf.code as u8)
            } else {
                eprintln!("error: {e:#}");
                ExitCode::from(1)
            }
        }
    }
}
