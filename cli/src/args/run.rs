//! `ns run`: clap's definition and help text.

use std::path::PathBuf;

use clap::Args;

const RUN_HELP: &str = "\
Examples:
  ns run --issue 142
  ns run 142-uart-timeout --dry-run
  ns run 142-uart-timeout --from verify
  ns run --issue 142 --cpus 4

Attempts, credits and a pending forced phase are kept in .ns/<unit>/state.json, so a restart
resumes them; --fresh, --from or a missing or unreadable file starts a new ledger.

Spec: docs/factory/run.md. Prints {unit,outcome,phase,reason,pr,cost_usd,phases}. Events go to
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

/// Drive one unit through triage, build, verify, review and ship, unattended
#[derive(Args)]
#[command(after_help = RUN_HELP)]
pub(crate) struct Run {
    /// Unit id, e.g. 142-uart-timeout (default with --issue: <n>-<slug of the title>)
    pub unit_id: Option<String>,
    /// The unit's GitHub issue number
    #[arg(long, value_name = "N")]
    pub issue: Option<u64>,
    /// Start with this phase instead of the state table's pick
    #[arg(long, value_name = "PHASE")]
    pub from: Option<String>,
    /// Gate policy passed to every phase prompt: stop or auto (default: nightshift.toml gates)
    #[arg(long, value_name = "POLICY")]
    pub gates: Option<String>,
    /// Base for a new worktree (default: the repo's default branch)
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,
    /// Print the next decision and the rendered prompt; run nothing
    #[arg(long)]
    pub dry_run: bool,
    /// Directory holding nightshift.toml (default: <main-worktree>/.nightshift)
    #[arg(long, value_name = "DIR")]
    pub factory: Option<PathBuf>,
    /// Set by ns watch: the night directory with its hold, spend and facts
    #[arg(long, value_name = "DIR", hide = true)]
    pub night: Option<PathBuf>,
    /// CPU share for every phase and gate (default: none; see CPU share below)
    #[arg(long, value_name = "N", value_parser = crate::cpus::parse)]
    pub cpus: Option<u32>,
    /// Forget the attempts and credits .ns/<unit>/state.json kept, e.g. after fixing an artifact by hand
    #[arg(long)]
    pub fresh: bool,
}
