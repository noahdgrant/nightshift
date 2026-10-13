//! The subcommands of `ns quality`, `ratchet`, `rules`, `factory`, `transcripts`,
//! `upstream` and `worktree`: clap's definitions and help text for each.

use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum QualityCmd {
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

const RATCHET_CHECK_HELP: &str = "\
Examples:
  ns ratchet check
  ns ratchet check --human
  ns ratchet check | jq '.ratchets[] | select(.status == \"worse\")'
  ns ratchet check --factory path/to/factory

Spec: docs/FACTORY.md (Ratchets). Runs the command of each [ratchets.<name>] table in
nightshift.toml with sh -c at the top of the git checkout, e.g.
  [ratchets.flash]
  command = \"scripts/flash-size.sh\"   # prints one number on stdout
  better = \"lower\"                    # or \"higher\"
  tolerance = \"0.5%\"                  # of the recorded value, or a number; default 0
and compares what it printed with the value recorded in ratchets.json, next to
nightshift.toml. A value past the recorded one by more than the tolerance is worse. A ratchet
with no recorded value is untracked: reported, not failed, and nothing is recorded (that is
ns ratchet update). A command that exits non-zero, is killed, runs over 300 s
(NS_RATCHET_TIMEOUT_MS) or prints anything but one number is broken. Each command runs twice,
and two values further apart than the tolerance make the ratchet flaky; under ns run the
second run happens only on a ratchet's first check of the night. ns run's gate runs this
command whenever nightshift.toml defines a ratchet. Never edit ratchets.json or a
[ratchets.*] table to pass: loosening is a human decision.

Prints {ok,command,recorded,ratchets:[{name,command,better,tolerance,recorded,measured,runs,
limit,status,error}]}; status is ok, better, worse, untracked, broken or flaky.
Exit codes: 0 nothing worse, broken or flaky, 1 one was, 2 not in a git repo or a bad
nightshift.toml.";

const RATCHET_UPDATE_HELP: &str = "\
Examples:
  ns ratchet update
  ns ratchet update --dry-run
  ns ratchet update --allow-loosen

For a human or a scheduled job, never a unit. Measures each ratchet twice as ns ratchet check
does (see its --help) and records the values in ratchets.json, next to nightshift.toml: an
untracked ratchet is recorded, a better one tightened, an equal one unchanged. A worse one,
even within its tolerance, keeps its recorded value (kept) unless --allow-loosen records it
(loosened). Values of ratchets no longer defined are dropped. A broken or flaky ratchet
writes nothing and exits 1. A rerun with nothing to change writes nothing.

Prints {ok,command,status,dry_run,recorded,ratchets,dropped}; status is written, planned,
already-done or refused; each ratchet's status is recorded, tightened, unchanged, kept,
loosened, broken or flaky.
Exit codes: 0 ok, 1 a ratchet is broken or flaky, 2 no [ratchets.*] table, not in a git repo
or a bad nightshift.toml.";

#[derive(Subcommand)]
pub(crate) enum RatchetCmd {
    /// Fail when a measured value is worse than its recorded value plus tolerance
    #[command(after_help = RATCHET_CHECK_HELP)]
    Check {
        /// Factory directory holding nightshift.toml and ratchets.json (default: .nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
        /// Print JSON (the default)
        #[arg(long, conflicts_with = "human")]
        json: bool,
        /// Print one readable line per ratchet instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Record the current measurements in ratchets.json; it may only tighten
    #[command(after_help = RATCHET_UPDATE_HELP)]
    Update {
        /// Factory directory holding nightshift.toml and ratchets.json (default: .nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
        /// Record worse values too (a human decision: the ratchet loosens)
        #[arg(long)]
        allow_loosen: bool,
        /// Print what would change; write nothing
        #[arg(long)]
        dry_run: bool,
        /// Print JSON (the default)
        #[arg(long, conflicts_with = "human")]
        json: bool,
        /// Print one readable line per ratchet instead of JSON
        #[arg(long)]
        human: bool,
    },
}

const RULES_CHECK_HELP: &str = "\
Examples:
  ns rules check
  ns rules check --human
  ns rules check | jq .new

Spec: docs/FACTORY.md (Rules). Runs each command in the `rules` row of docs/agents/stack.md
with sh -c at the top of the git checkout, e.g.
  | rules | `ruff check --output-format=concise .`, `./scripts/rules-json` | 10 s |
Each output line (stdout or stderr) that is a JSON object with \"file\" (and optionally
\"tool\", \"rule\", \"line\", \"message\", \"snippet\"), or reads file:line[:col]: message, is a
finding {tool,rule,file,line,message,snippet}. tool defaults to the command's first word,
snippet to the source line with whitespace collapsed. A finding matches a baseline entry in
.nightshift/rules-baseline.json on (tool, rule, file, snippet), so a moved line still matches.
A command that exits non-zero and prints no finding is an error, as is one that is killed,
exits 126 or 127, runs over 300 s (NS_RULES_TIMEOUT_MS) or prints over 8 MiB
(NS_RULES_MAX_OUTPUT_BYTES), whatever it printed first. No rules row: nothing runs.

Prints {ok,command,baseline,commands,findings,baselined,new,fixed}; fixed lists baseline
entries nothing matched (drop them with ns rules baseline).
Exit codes: 0 no new violations, 1 new violations or a command failed, 2 not in a git repo.";

const RULES_BASELINE_HELP: &str = "\
Examples:
  ns rules baseline
  ns rules baseline --dry-run
  ns rules baseline --allow-growth

Runs the rule pack as ns rules check does (see its --help) and writes every current violation
to .nightshift/rules-baseline.json, dropping entries that are gone. A violation the baseline
lacks, including every one when there is no baseline yet, is growth: without --allow-growth
it writes nothing and exits 1. A rerun with nothing added or dropped writes nothing. A command
that fails, is killed, times out or prints too much is an error, as in ns rules check.

Prints {ok,command,status,dry_run,baseline,commands,violations,added,dropped}; status is
written, planned, already-done or refused.
Exit codes: 0 ok, 1 refused growth or a command failed, 2 no rules row or not in a git repo.";

#[derive(Subcommand)]
pub(crate) enum RulesCmd {
    /// Fail on violations the baseline doesn't list; report the ones that are gone
    #[command(after_help = RULES_CHECK_HELP)]
    Check {
        /// Print one readable line per new or fixed violation instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Rewrite the baseline from the current violations; it may only shrink
    #[command(after_help = RULES_BASELINE_HELP)]
    Baseline {
        /// Let the baseline gain entries (a human decision: they become accepted debt)
        #[arg(long)]
        allow_growth: bool,
        /// Print what would change; write nothing
        #[arg(long)]
        dry_run: bool,
        /// Print one readable line per added or dropped violation instead of JSON
        #[arg(long)]
        human: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum FactoryCmd {
    /// Parse nightshift.toml and runners/*.toml (unknown keys are errors) and check agents/<role>/agent.md
    #[command(after_help = "\
Examples:
  ns factory validate
  ns factory validate --factory path/to/factory

Prints {\"ok\",\"path\",\"name\",\"phases\",\"scorers\",\"supervise\",\"errors\",\"warnings\"};
supervise is {\"notify\",\"silence_minutes\"}, null when the file is missing or does not parse.
Exits 1 if any error.")]
    Validate {
        /// Directory holding nightshift.toml (default: <main-worktree>/.nightshift)
        #[arg(long, value_name = "DIR")]
        factory: Option<PathBuf>,
    },
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

/// Shared by the `ns upstream` subcommands' help: a literal, so `concat!` can use it.
macro_rules! upstream_cache_help {
    () => {
        "\
Each metadata.upstream entry is owner/repo@sha:path. The repo is fetched with git (network)
from https://github.com/owner/repo into a bare cache at <git-common-dir>/ns/upstream/owner/repo:
shallow the first time, deepened until it holds the recorded sha. git's url.<base>.insteadOf
redirects the fetch, e.g. to a mirror."
    };
}

#[derive(Subcommand)]
pub(crate) enum UpstreamCmd {
    /// Report, per skill and upstream entry, whether the upstream path changed since its sha
    #[command(after_help = concat!("\
Examples:
  ns upstream status
  ns upstream status --human
  ns upstream status | jq '.skills[] | select(any(.upstreams[]; .changed)) | .skill'

", upstream_cache_help!(), "

Prints {\"ok\",\"changed\",\"skills\":[{\"skill\",\"upstreams\":[{\"upstream\",\"head\",\"changed\"}]}]};
an entry that can't be checked has \"error\" instead of head and changed. Skills with no
metadata.upstream are left out. changed counts the changed entries.
Exit codes: 0 every entry checked, 1 an entry has an error, 2 no skills directory or not in a
git repo."))]
    Status {
        /// Skills directory
        #[arg(long, value_name = "DIR", default_value = "skills")]
        skills_dir: PathBuf,
        /// Print one readable line per upstream entry instead of JSON
        #[arg(long)]
        human: bool,
    },
    /// Print the upstream diff of a skill's upstream paths, from the recorded sha to HEAD
    #[command(after_help = concat!("\
Examples:
  ns upstream diff ns-tdd
  ns upstream diff tdd --human
  ns upstream diff ns-build | jq -r '.upstreams[].diff'

", upstream_cache_help!(), "

Prints {\"skill\",\"upstreams\":[{\"upstream\",\"head\",\"diff\"}]}, one entry per
metadata.upstream entry; diff is \"\" when the path is unchanged. --human prints each diff under
a \"=== <upstream> -> <head>\" line.
Exit codes: 0 ok, 1 a fetch or git failed, 2 unknown skill, a skill with no upstream, or not in a
git repo."))]
    Diff {
        /// Skill name, e.g. ns-tdd (or tdd)
        skill: String,
        /// Skills directory
        #[arg(long, value_name = "DIR", default_value = "skills")]
        skills_dir: PathBuf,
        /// Print the labelled diffs instead of JSON
        #[arg(long)]
        human: bool,
    },
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
