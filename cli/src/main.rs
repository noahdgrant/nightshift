//! `ns`: the Nightshift CLI.

mod ask;
mod billing;
mod clean;
mod clock;
mod config;
mod doctor;
mod error;
mod eval;
mod factory;
mod forge;
mod frontmatter;
mod gate;
mod git;
mod install;
mod lint;
mod markers;
mod memcap;
mod quality;
mod review_md;
mod run;
mod skills_sync;
mod stats;
mod stop;
#[cfg(test)]
mod testutil;
mod watch;
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
  ns check-markers
  ns install --dry-run
  ns doctor
  ns eval ns-tdd --dry-run
  ns factory validate
  ns run --issue 142 --dry-run
  ns watch --until 06:30
  ns clean --dry-run
  ns quality --since 2026-10-01

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

    /// Check human-review markers in tracked files: every start has an end, none nested
    #[command(after_help = "\
Examples:
  ns check-markers
  ns check-markers path/to/repo --human

A region runs from a line containing `ns:human-review start` (optionally `: <reason>`)
through a line containing `ns:human-review end`, in any comment syntax. Checks every file
`git ls-files` lists under the path. Prints {\"ok\",\"files\",\"errors\":[{\"file\",\"line\",\"message\"}]}.
Exit codes: 0 balanced, 1 an unbalanced or nested marker, 2 not a directory in a git repo.")]
    CheckMarkers {
        /// Directory inside a git repository
        #[arg(default_value = ".")]
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

    /// Check the factory definition (.nightshift/nightshift.toml and agents/<role>/agent.md)
    #[command(arg_required_else_help = true)]
    Factory {
        #[command(subcommand)]
        command: FactoryCmd,
    },

    /// Drive one unit through triage, build, verify, review and ship, unattended
    #[command(after_help = RUN_HELP)]
    Run {
        /// Unit id, e.g. 142-uart-timeout (default with --issue: <n>-<slug of the title>)
        unit_id: Option<String>,
        /// The unit's GitHub issue number
        #[arg(long, value_name = "N")]
        issue: Option<u64>,
        /// Start with this phase instead of the state table's pick
        #[arg(long, value_name = "PHASE")]
        from: Option<String>,
        /// Gate policy passed to every phase prompt: stop or auto (default: nightshift.toml gates)
        #[arg(long, value_name = "POLICY")]
        gates: Option<String>,
        /// Base for a new worktree (default: the repo's default branch)
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        /// Print the next decision and the rendered prompt; run nothing
        #[arg(long)]
        dry_run: bool,
        /// Directory holding nightshift.toml (default: <main-worktree>/.nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
        /// Set by ns watch: the night directory with its hold, spend and facts
        #[arg(long, value_name = "DIR", hide = true)]
        night: Option<PathBuf>,
    },

    /// Pull ready issues from GitHub and run them, up to --parallel at once
    #[command(after_help = WATCH_HELP)]
    Watch {
        /// Take one unit, then stop
        #[arg(long)]
        once: bool,
        /// Start no new unit after this local time (HH:MM)
        #[arg(long, value_name = "HH:MM")]
        until: Option<String>,
        /// Units per invocation (default: [limits] max_units, else no cap)
        #[arg(long, value_name = "N")]
        max_units: Option<u32>,
        /// Units running at once, each its own ns run (default: [limits] parallel, else 1)
        #[arg(long, value_name = "N")]
        parallel: Option<u32>,
        /// Print the ordered queue with skip reasons; change nothing
        #[arg(long)]
        dry_run: bool,
        /// Directory holding nightshift.toml (default: <main-worktree>/.nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
    },

    /// Remove unit worktrees and ns/<unit> branches whose issue is closed and whose PR merged
    #[command(after_help = CLEAN_HELP)]
    Clean {
        /// Print what would be removed and why the rest stay; remove and record nothing
        #[arg(long)]
        dry_run: bool,
    },

    /// Report how well units pass review: first-pass yield, findings per 100 lines, leftovers, escapes
    #[command(after_help = QUALITY_HELP, args_conflicts_with_subcommands = true)]
    Quality {
        #[command(subcommand)]
        command: Option<QualityCmd>,
        /// Only units whose newest review artifact was updated, and run-log events, at or after this UTC time (YYYY-MM-DDTHH:MM:SSZ, or YYYY-MM-DD for 00:00:00 UTC)
        #[arg(long, value_name = "TIME")]
        since: Option<String>,
        /// Only units whose newest review artifact was updated, and run-log events, at or before this UTC time (YYYY-MM-DDTHH:MM:SSZ, or YYYY-MM-DD for 23:59:59 UTC)
        #[arg(long, value_name = "TIME")]
        until: Option<String>,
        /// Print JSON (the default and only format)
        #[arg(long)]
        json: bool,
    },

    /// Report config path and validity, configured roles, harnesses on PATH, and forge accounts
    #[command(after_help = "\
Examples:
  ns doctor
  NS_CONFIG=./ns.toml ns doctor

forge.<name>: configured, token_resolved, and for github the account gh api user returns.
The token itself is never printed.")]
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

const RUN_HELP: &str = "\
Examples:
  ns run --issue 142
  ns run 142-uart-timeout --dry-run
  ns run 142-uart-timeout --from verify

Spec: docs/FACTORY.md. Prints {unit,outcome,phase,reason,pr,cost_usd,phases}. Events go to
<git-common-dir>/ns/runs.jsonl, harness transcripts to <git-common-dir>/ns/transcripts/.
A unit that ends merged, done or stuck pushes its review quality records to origin's
nightshift/quality branch (network); a failed push leaves them in
<git-common-dir>/ns/quality-outbox.jsonl for the next run and never fails the unit.
After its merge step merges the PR and the records are saved, it removes the unit's worktree and
local ns/<unit> branch, unless it has uncommitted tracked changes or commits the PR lacks (see
ns clean --help); `cleanup` in the output says which.

Forge tokens: [forge.github] / [forge.gitlab] in the ns config (see ns ask --help for the path)
are resolved once and exported as GH_TOKEN / GITLAB_TOKEN to every child; a set variable wins.
  [forge.github]
  token_command = \"gh auth token --user <account>\"   # or: token_env = \"MY_GH_TOKEN\"

Exit codes: 0 done, merged or split (triage split the issue into child issues), 1 stuck,
2 usage or config error (incl. missing subscription login, harness not on PATH, forge token
that does not resolve), 3 budget, 4 paused on a usage limit, 5 another ns run holds this unit's
run lock (runs of other units go ahead). A phase whose runner names a lock another
run holds waits for it instead ([runners] lock_dir in the ns config shares locks across repos),
and so does the merge step for the repo's merge lock.";

const WATCH_HELP: &str = "\
Examples:
  ns watch --dry-run
  ns watch --once
  ns watch --until 06:30 --max-units 3
  ns watch --until 06:30 --parallel 3

Lists open issues labelled [queue] ready_label with gh, drops blocked ones and ones with an
open PR that closes them (not one from their own unit branch), sorts by [queue] order, and runs
each as a child ns run --issue, up to --parallel at once, every one on the config and factory
definition read at start. A usage limit in one unit pauses the others at their next phase,
and the budget counts what every unit spent. Forge tokens as in ns run --help.
--until is local time (TZ). Times printed for people, until and reset_at, are local with
their offset (2026-10-09T06:30:00-04:00).
At start, in-progress issues no live ns run holds go back to the ready label (requeued).
At start and between units, worktrees of units a human merged since are removed as ns clean
removes them (cleaned; --dry-run lists them as clean).
SIGINT or SIGTERM ends every running phase and returns each unit's issue to the ready label.
Prints {units,requeued,cleaned,stopped,until,cost_usd,error}. Exit codes as ns run's errors: 2 usage,
5 lock held (another ns run or ns watch); 130 or 143 when stopped by SIGINT or SIGTERM.";

const CLEAN_HELP: &str = "\
Examples:
  ns clean --dry-run
  ns clean

Spec: docs/FACTORY.md (Cleanup). For each worktree on a branch ns/<unit>, reads the issue
the unit id starts with and the PR in .ns/<unit>/pr.md with gh (network). A unit goes when the
issue is closed and the PR merged, closes the issue and comes from ns/<unit>. Its quality record
is written first (a push to origin's nightshift/quality, or the outbox when that fails); then the
worktree, untracked files and .ns/ included, and the local branch are removed. A worktree stays,
with its reason in `kept`, while its issue is open, it has uncommitted changes to tracked files,
its HEAD is not on ns/<unit>, its branch holds changes the merged PR head does not (whitespace
counts), its record could not be saved or a review artifact read, it is the current directory,
or a live ns run or another cleanup holds the unit's run lock (nothing is recorded for it). A PR head missing
locally is fetched from refs/pull/<n>/head. --dry-run writes nothing but may fetch.
ns run cleans its own unit after its merge step; ns watch cleans at start and between units.

Prints {ok,command,dry_run,status,planned|removed,kept,records}; status is planned, cleaned or
nothing-to-clean, so a rerun is safe.
Exit codes: 0 ok, 1 a removal failed (its kept entry has \"error\": true), 2 usage error.";

const QUALITY_HELP: &str = "\
Examples:
  ns quality
  ns quality --since 2026-10-01
  ns quality --since 2026-10-09T21:04:00Z
  ns quality --since 2026-10-01 --until 2026-10-09
  ns quality --json | jq .first_pass
  ns quality import /tmp/unit-artifacts --dry-run

Spec: docs/FACTORY.md (Quality). Fetches origin's nightshift/quality branch (network) and reads
its records.jsonl plus <git-common-dir>/ns/quality-outbox.jsonl: one record per review attempt,
written by ns run when a unit ends. A unit with no record is read from .ns/<unit>/review.md,
review/cycle-*.md and history/review* in its linked worktree. Also reads
<git-common-dir>/ns/runs.jsonl. Changes nothing but the remote-tracking ref.
`records` says how many records came from the branch and the outbox, and why a fetch failed.
First-pass numbers come from a unit's oldest review artifact, the rest from its newest.
Prints {units,findings,first_pass,cycles_to_clean,leftovers,escapes,leftover_findings,
escape_findings,trend,run_log,gaps,per_unit,records}; per_unit[].source is record or worktree.
--since and --until are inclusive and UTC, and so is their echo in the output: a bare date
is 00:00:00 for --since and 23:59:59 for --until. trend and per_unit days are local (TZ).

Exit codes: 0 ok, 1 git failed, 2 usage error (bad --since or --until, --until before --since,
not in a git repo).";

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
enum QualityCmd {
    /// Turn archived unit worktrees into quality records and push them to origin's nightshift/quality
    #[command(after_help = "\
Examples:
  ns quality import /tmp/unit-artifacts --dry-run
  ns quality import /tmp/unit-artifacts

<DIR> holds one directory per unit, laid out as in its worktree: <DIR>/<unit>/.ns/<unit>/review.md.
Units origin's branch (or the outbox) already has a record for are skipped, so a rerun is safe
and reports status already-done. Issue numbers come from the unit id, PRs from pr.md or the run
log, outcomes from <git-common-dir>/ns/runs.jsonl. A missing change size is measured with git in
this repo. --dry-run fetches the branch (network) to find what's recorded, and pushes nothing.

Prints {ok,status,units,records,imported,already_recorded,skipped,unparsed,branch,push}.
Exit codes: 0 pushed, planned or already-done; 1 nothing-to-import (no unit in <DIR> has a
review artifact), or outbox: the push failed and the records wait in the
outbox (the next ns run or import pushes them); 2 usage error.")]
    Import {
        /// Directory holding <unit>/.ns/<unit>/ for each archived unit
        dir: PathBuf,
        /// Print what would be imported; push nothing
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum FactoryCmd {
    /// Parse nightshift.toml and runners/*.toml (unknown keys are errors) and check agents/<role>/agent.md
    #[command(after_help = "\
Examples:
  ns factory validate
  ns factory validate --factory path/to/factory

Prints {\"ok\",\"path\",\"phases\",\"errors\"}. Exits 1 if any error.")]
    Validate {
        /// Directory holding nightshift.toml (default: <main-worktree>/.nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
    },
}

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
        Cmd::CheckMarkers { path, human } => return markers::cli(&path, human),
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
        Cmd::Factory { command } => match command {
            FactoryCmd::Validate { factory } => return factory::validate(factory.as_deref()),
        },
        Cmd::Run {
            unit_id,
            issue,
            from,
            gates,
            base,
            dry_run,
            factory,
            night,
        } => {
            return run::cli(run::RunArgs {
                unit: unit_id,
                issue,
                from,
                gates,
                dry_run,
                factory,
                base,
                triage_only: false,
                night,
            })
        }
        Cmd::Watch {
            once,
            until,
            max_units,
            parallel,
            dry_run,
            factory,
        } => {
            return watch::run(watch::WatchArgs {
                once,
                until,
                max_units,
                parallel,
                dry_run,
                factory,
            })
        }
        Cmd::Quality {
            command: Some(QualityCmd::Import { dir, dry_run }),
            ..
        } => return quality::import(&dir, dry_run),
        Cmd::Quality {
            command: None,
            since,
            until,
            json: _,
        } => return quality::cli(since, until),
        Cmd::Clean { dry_run } => return clean::cli(dry_run),
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
