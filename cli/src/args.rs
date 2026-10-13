//! The `ns` command line: clap's definitions of every command, flag and help text. `main`
//! parses it and dispatches each command to its module.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::eval;
use crate::install;

mod subcommands;
pub(crate) use subcommands::{
    FactoryCmd, QualityCmd, RatchetCmd, RulesCmd, TranscriptsCmd, UpstreamCmd, WorktreeCmd,
};

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
  ns score 142-uart-timeout --dry-run
  ns run --issue 142 --dry-run
  ns watch --until 06:30
  ns clean --dry-run
  ns quality --since 2026-10-01
  ns log -f --unit 142-uart-timeout
  ns transcripts list --new --mark
  ns upstream status --human

Exit codes: 0 ok, 1 failure, 2 usage error, 3 role not configured, 4 harness missing,
5 no write command for `ns ask --write`.
Run `ns <command> --help` for details on one command.";

/// `ns --version`: the package version and the commit it was built from (`build.rs`).
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("NS_BUILD_COMMIT"),
    ")"
);

#[derive(Parser)]
#[command(
    name = "ns",
    version = VERSION,
    about = "Nightshift CLI: worktrees, headless harness calls, skill lint and install",
    after_help = ROOT_HELP,
    arg_required_else_help = true
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
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

    /// Validate skills (frontmatter, names, upstream, relative links, features/*.md headings), and decisions when the parent has docs/decisions/; a repo-local root (.agents/skills) needs no ns- prefix
    #[command(after_help = "\
Examples:
  ns lint
  ns lint path/to/skills
  ns lint skills --human
  ns lint .agents/skills

When the skills directory's parent has docs/decisions/, also checks that each decision file
has a unique frontmatter id, that its amends: and amended_by: targets exist, and that every
D<n> or ADR-<n>[a-z] cited in skills/, cli/src/, docs/ or AGENTS.md has a file. These errors
have skill \"decisions\".

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
    Install {
        /// Directory holding the ns-* skill directories (default: ./skills if present, else the checkout ns was built from)
        #[arg(long, value_name = "DIR")]
        source: Option<PathBuf>,
        /// Link into this harness's skill directory; repeat for several (default: agents, claude)
        #[arg(long, value_name = "HARNESS")]
        harness: Vec<install::Harness>,
        /// Also link into this skill directory; repeat for several
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

    /// Grade a finished unit with the factory's scorers and append the results to the run ledger
    #[command(after_help = SCORE_HELP)]
    Score {
        /// Unit id, e.g. 142-uart-timeout
        unit_id: String,
        /// Only this scorer (the file stem of scorers/<name>.md; default: every scorer)
        #[arg(long, value_name = "NAME")]
        scorer: Option<String>,
        /// Directory holding nightshift.toml (default: <main-worktree>/.nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
        /// Print each scorer's prompt as JSON; calls no judge and writes no ledger line
        #[arg(long)]
        dry_run: bool,
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
        /// CPU share for every phase and gate (default: none; see CPU share below)
        #[arg(long, value_name = "N", value_parser = crate::cpus::parse)]
        cpus: Option<u32>,
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
        /// On the nightshift source repo, never drain and rebuild ns when cli/ changes
        #[arg(long)]
        no_self_update: bool,
        /// The night so far, from the ns that handed off to this one (internal)
        #[arg(long, value_name = "FILE", hide = true)]
        resume_night: Option<PathBuf>,
    },

    /// Remove unit worktrees and ns/<unit> branches whose issue is closed and whose PR merged
    #[command(after_help = CLEAN_HELP)]
    Clean {
        /// Print what would be removed and why the rest stay; remove and record nothing
        #[arg(long)]
        dry_run: bool,
    },

    /// Print the run log, one line per event; -f follows new events as runs append them
    #[command(after_help = LOG_HELP)]
    Log {
        /// Only this unit's events
        #[arg(long, value_name = "UNIT_ID")]
        unit: Option<String>,
        /// Only events at or after this UTC time (YYYY-MM-DDTHH:MM:SSZ, or YYYY-MM-DD for 00:00:00 UTC)
        #[arg(long, value_name = "TIME")]
        since: Option<String>,
        /// Keep running and print events as they are appended, like tail -f
        #[arg(short, long)]
        follow: bool,
        /// Print one JSON object per event instead of a line
        #[arg(long)]
        json: bool,
        /// Print the assistant messages and tool calls of the newest phase transcript instead
        #[arg(long, conflicts_with = "since")]
        phase_output: bool,
    },

    /// Summarise the night: units running now, units ended since --since by outcome, and what is next
    #[command(after_help = STATUS_HELP)]
    Status {
        /// Count units that ended at or after this UTC time (YYYY-MM-DDTHH:MM:SSZ, or YYYY-MM-DD for 00:00:00 UTC; default: 24 hours ago)
        #[arg(long, value_name = "TIME")]
        since: Option<String>,
        /// Print one JSON object instead of the summary
        #[arg(long)]
        json: bool,
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

    /// Hold measured values (binary size, coverage) to the values recorded in ratchets.json
    #[command(arg_required_else_help = true)]
    Ratchet {
        #[command(subcommand)]
        command: RatchetCmd,
    },

    /// Run the repo's rule pack and fail only on violations not in its baseline
    #[command(arg_required_else_help = true)]
    Rules {
        #[command(subcommand)]
        command: RulesCmd,
    },

    /// List and read Claude Code session transcripts; track which were processed
    #[command(
        after_help = "Examples:\n  ns transcripts list --since 2026-10-01\n  ns transcripts list --new --mark\n  ns transcripts read aaaa1111 --compact",
        arg_required_else_help = true
    )]
    Transcripts {
        #[command(subcommand)]
        command: TranscriptsCmd,
    },

    /// Compare adapted skills with the upstream path each records in metadata.upstream
    #[command(
        after_help = "Examples:\n  ns upstream status\n  ns upstream status --human\n  ns upstream diff ns-tdd",
        arg_required_else_help = true
    )]
    Upstream {
        #[command(subcommand)]
        command: UpstreamCmd,
    },

    /// Report config path and validity, configured roles, harnesses on PATH, harness skill dirs, and forge accounts
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
never printed.")]
    Doctor,

    /// The PreToolUse hook ns run gives claude phases: deny a git hook bypass (internal)
    #[command(hide = true)]
    GuardHooks,
}

const SCORE_HELP: &str = "\
Examples:
  ns score 142-uart-timeout
  ns score 142-uart-timeout --scorer evidence
  ns score 142-uart-timeout --dry-run

Spec: docs/FACTORY.md (Scoring). Builds a prompt per scorer from its rubric and labels, every
*.md in .ns/<unit>/ (the unit's worktree, else the main checkout) and the PR outcome: the PR in
pr.md read with gh (network, --dry-run too), whether a commit on the default branch reverts its
merge commit, and its review count. Runs the scorer's role read-only through `ns ask` and takes
the reply's last `label: <value>` line. Appends {unit,scorer,label,score,pass} per scorer to
<common git dir>/ns/scores.jsonl only when every judge named a known label.

Prints {ok,unit,ledger,scores}; with --dry-run {ok,dry_run,unit,prompts:[{scorer,role,prompt}]}.
Exit codes: 0 scored, 1 a judge named no known label, a scorer is invalid or missing, or gh or
the harness failed, 2 usage error, 3 role not configured, 4 harness missing.";

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
  ns run --issue 142 --cpus 4

Spec: docs/FACTORY.md. Prints {unit,outcome,phase,reason,pr,cost_usd,phases}. Events go to
<git-common-dir>/ns/runs.jsonl, harness transcripts to <git-common-dir>/ns/transcripts/.
A unit that ends merged, done or stuck pushes its review quality records to origin's
nightshift/quality branch (network); a failed push leaves them in
<git-common-dir>/ns/quality-outbox.jsonl for the next run and never fails the unit.
After its merge step merges the PR and the records are saved, it removes the unit's worktree and
local ns/<unit> branch, unless it has uncommitted tracked changes or commits the PR lacks (see
ns clean --help); `cleanup` in the output says which.

CPU share: --cpus N sets NS_CPUS, RUST_TEST_THREADS and CARGO_BUILD_JOBS to N for every phase
and gate, each unless already set. ns watch passes --cpus max(1, cores / --parallel). The start
event in runs.jsonl records each value and whether it was computed or inherited.

Forge tokens: [forge.github] / [forge.gitlab] in the ns config (see ns ask --help for the path)
are resolved once and exported as GH_TOKEN / GITLAB_TOKEN to every child; a set variable wins,
with a warning. Owner and repo tables, matched on origin's URL, override the default; the most
specific wins. At start the token must read the repo, else exit 2 naming the account and table.
  [forge.github]
  token_command = \"gh auth token --user <account>\"   # or: token_env = \"MY_GH_TOKEN\"
  [forge.github.owners.\"example-org\"]
  token_command = \"gh auth token --user work-account\"

Exit codes: 0 done, merged or split (triage split the issue into child issues), 1 stuck,
2 usage or config error (incl. missing subscription login, harness not on PATH, forge token
that does not resolve or cannot read the repo), 3 budget, 4 paused on a usage limit, 5 another ns run holds this unit's
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
On the nightshift source repo, when origin's default branch gains cli/ commits after the one
ns was built from, it starts no new unit, waits for the running ones, builds the new ns and
execs it with the night's state; --no-self-update turns that off.
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

const STATUS_HELP: &str = "\
Examples:
  ns status
  ns status --since 2026-10-09T22:00:00Z
  ns status --json | jq -r '.ended.stuck[].unit'

Running: each unit a live ns run holds, with its last logged phase. Ended: each other unit by
the outcome of its last run that ended at or after --since: merged; PR for a human (done);
stuck (stuck or budget); paused; split; other (anything else, such as interrupted, shown only
when present). Next: the ready issues ns watch would take, in its order, read from the forge;
when the forge can't be read it says queue unavailable (null in --json) and gives the reason on
stderr. Entries hold only the fields ns log prints. --json prints one object with since,
running, ended and next.
Changes nothing.

Exit codes: 0 ok, 1 the run log or run locks could not be read, 2 usage error (bad --since, not
in a git repo).";

const LOG_HELP: &str = "\
Examples:
  ns log
  ns log -f
  ns log --unit 142-uart-timeout --since 2026-10-09
  ns log --json | jq -r 'select(.event == \"end\") | .reason'
  ns log --phase-output --unit 142-uart-timeout -f

Reads <git-common-dir>/ns/runs.jsonl, which ns run and ns watch append to. A line per event:
time, event, #issue, phase, attempt, status, duration, outcome, PR link and reason, leaving out
what the event lacks. The PR link comes from origin's URL; without one it is pr #<n>.
--json prints one object per event with only ts, event, unit, issue, phase, attempt, status,
wall_s, outcome, pr, pr_url and reason; nothing else in the log is printed. --since is UTC and
inclusive, and drops events with no time. -f prints the log so far, then polls it every
0.5 s until stopped (Ctrl-C). Lines that are not UTF-8 JSON objects are skipped; without -f
their count is on stderr.
--phase-output reads instead the newest transcript under <git-common-dir>/ns/transcripts/<unit>/
(of the unit the run log names last without --unit): its last 20 assistant messages and tool calls, one line each, text
past 200 characters cut short and known credential shapes replaced by [redacted]. With --json
each is {\"kind\":\"assistant\",\"text\"} or {\"kind\":\"tool\",\"name\",\"input\"}. -f follows that
transcript, and moves on to the next phase's. It does not take --since.
Changes nothing.

Exit codes: 0 ok, 1 the log or transcript could not be read or there is no transcript yet, 2 usage
error (bad --since or --unit, not in a git repo).";

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
