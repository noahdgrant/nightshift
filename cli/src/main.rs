//! `ns`: the Nightshift CLI.

mod ask;
mod config;
mod doctor;
mod error;
mod eval;
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
  ns worktree new 142-uart-timeout
  ns worktree list
  echo 'Review this diff' | ns ask --role review.security
  ns lint skills
  ns install --dry-run
  ns doctor
  ns eval ns-tdd --dry-run

Exit codes: 0 ok, 1 failure, 2 usage error, 3 role not configured, 4 harness missing,
5 no write command for `ns ask --write`.
Run `ns <command> --help` for details on one command.";

#[derive(Parser)]
#[command(
    name = "ns",
    version,
    about = "Nightshift CLI: worktrees, headless harness calls, skill lint and install",
    after_help = ROOT_HELP,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create, set up, list and remove per-unit worktrees (branch ns/<unit-id>)
    #[command(
        after_help = "Examples:\n  ns worktree new 142-uart-timeout\n  ns worktree setup 142-uart-timeout\n  ns worktree list\n  ns worktree remove 142-uart-timeout --dry-run",
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

    /// Validate ns-*/SKILL.md skills (frontmatter, names, upstream, relative links)
    #[command(after_help = "\
Examples:
  ns lint
  ns lint path/to/skills
  ns lint skills --human

Prints {\"ok\",\"skills\",\"errors\":[{\"skill\",\"file\",\"message\"}]}. Exits 1 if any error.")]
    Lint {
        /// Skills directory
        #[arg(default_value = "skills")]
        path: PathBuf,
        /// Print one readable line per error instead of JSON
        #[arg(long)]
        human: bool,
    },

    /// Symlink skills/ns-* into harness skill directories
    #[command(after_help = "\
Examples:
  ns install --dry-run
  ns install --source ~/src/nightshift/skills
  ns install --target ~/.agents/skills --target ~/.cursor/skills

Default targets: ~/.agents/skills and ~/.claude/skills (created if missing).
Stale ns-* symlinks are replaced. Real directories in the way are reported as
conflicts and left alone; the command then exits 1. Safe to re-run.")]
    Install {
        /// Directory holding the ns-* skill directories (default: ./skills if present, else the checkout ns was built from)
        #[arg(long, value_name = "DIR")]
        source: Option<PathBuf>,
        /// Target skill directory; repeat for several (default: ~/.agents/skills, ~/.claude/skills)
        #[arg(long, value_name = "DIR")]
        target: Vec<PathBuf>,
        /// Report the actions without touching the filesystem
        #[arg(long)]
        dry_run: bool,
    },

    /// Measure skills: trigger evals and behaviour cases with and without the skill
    #[command(after_help = EVAL_HELP)]
    Eval {
        /// Skills to evaluate, e.g. ns-tdd (or tdd). Default: every skill with an evals/ dir
        skills: Vec<String>,
        /// Only this case id; repeat for several
        #[arg(long = "case", value_name = "ID")]
        cases: Vec<String>,
        /// Comma-separated arms: with, without (default: with,without)
        #[arg(long, value_name = "LIST", conflicts_with = "compare")]
        arms: Option<String>,
        /// Compare the skills against themselves at a git ref (arms: with, old)
        #[arg(long, value_name = "REF")]
        compare: Option<String>,
        /// Starting trials per arm (overrides [eval].trials)
        #[arg(long, value_name = "N")]
        trials: Option<u32>,
        /// Only skills with a file changed since this ref (committed, staged, unstaged or untracked)
        #[arg(long, value_name = "REF")]
        changed_since: Option<String>,
        /// Stop starting trials once the reported cost reaches this (overrides [eval].budget_usd)
        #[arg(long, value_name = "USD")]
        budget_usd: Option<f64>,
        /// Stop after this many harness runs (overrides [eval].max_runs)
        #[arg(long, value_name = "N")]
        max_runs: Option<u32>,
        /// Run only trigger evals
        #[arg(long, conflicts_with = "cases_only")]
        triggers_only: bool,
        /// Run only behaviour cases
        #[arg(long)]
        cases_only: bool,
        /// Print the plan as JSON; never calls a harness
        #[arg(long)]
        dry_run: bool,
        /// Ignore cached `without` baselines and rerun them
        #[arg(long)]
        no_cache: bool,
        /// Also write the result JSON to this file
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Skills directory; fixtures are read from <its parent>/evals/fixtures
        #[arg(long, value_name = "DIR", default_value = "skills")]
        skills_dir: PathBuf,
        /// Don't write skills/<skill>/evals/results/<date>-<sha>.json
        #[arg(long)]
        no_write_results: bool,
        /// Also print a readable table to stderr
        #[arg(long)]
        human: bool,
        /// Eval harness name (overrides [eval].harness)
        #[arg(long)]
        harness: Option<String>,
        /// Model (overrides [eval].model)
        #[arg(long)]
        model: Option<String>,
        /// Seconds before a trigger run is killed
        #[arg(long, value_name = "SECS", default_value_t = eval::DEFAULT_TRIGGER_TIMEOUT_SECS)]
        trigger_timeout: u64,
    },

    /// Report config path and validity, configured roles, and harnesses on PATH
    #[command(after_help = "\
Examples:
  ns doctor
  NS_CONFIG=./ns.toml ns doctor")]
    Doctor,
}

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

const EVAL_HELP: &str = "\
Examples:
  ns eval --dry-run
  ns eval ns-tdd --case py-capacity-off-by-one --trials 1 --human
  ns eval ns-tdd --compare main --cases-only
  ns eval --changed-since origin/main --budget-usd 2
  ns eval ns-triage --triggers-only --no-write-results

Spec: docs/EVALS.md. Config: [eval] in the ns config (see ns ask --help for the path).
Each trial runs in a scratch dir under the system temp dir with HOME pointed at a
throwaway directory holding only the arm's skills. Prints JSON on stdout and writes
skills/<skill>/evals/results/<date>-<short-sha>.json. Raw transcripts and the
baseline cache live under [eval].transcripts.

Exit codes: 1 failure, 2 usage error, 3 eval harness not configured or bad config,
4 harness binary not on PATH.";

#[derive(Subcommand)]
enum WorktreeCmd {
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
command with sh -c in the worktree, stopping at the first failure. Prints
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

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Cmd::Worktree { command } => match command {
            WorktreeCmd::New {
                unit_id,
                base,
                repo,
                no_setup,
            } => return worktree::new(&unit_id, base.as_deref(), repo.as_deref(), no_setup),
            WorktreeCmd::Setup { unit_id, repo } => {
                return worktree::setup(&unit_id, repo.as_deref())
            }
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
                    "error: {} conflict(s): real files or unrelated symlinks are in the way; move them, then re-run:\n  ns install --dry-run",
                    report.conflicts
                );
                return Ok(ExitCode::from(1));
            }
        }
        Cmd::Eval {
            skills,
            cases,
            arms,
            compare,
            trials,
            changed_since,
            budget_usd,
            max_runs,
            triggers_only,
            cases_only,
            dry_run,
            no_cache,
            out,
            skills_dir,
            no_write_results,
            human,
            harness,
            model,
            trigger_timeout,
        } => {
            return eval::run(eval::Args {
                skills,
                cases,
                arms,
                compare,
                trials,
                changed_since,
                budget_usd,
                max_runs,
                triggers_only,
                cases_only,
                dry_run,
                no_cache,
                out,
                skills_dir,
                no_write_results,
                human,
                harness,
                model,
                trigger_timeout_secs: trigger_timeout,
            })
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
            if let Some(ns) = e.downcast_ref::<SfError>() {
                eprintln!("error: {}", ns.message);
                if let Some(h) = &ns.hint {
                    for line in h.lines() {
                        if line.is_empty() {
                            eprintln!();
                        } else {
                            eprintln!("  {line}");
                        }
                    }
                }
                ExitCode::from(ns.code as u8)
            } else {
                eprintln!("error: {e:#}");
                ExitCode::from(1)
            }
        }
    }
}
