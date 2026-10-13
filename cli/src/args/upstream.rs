//! `ns upstream`: clap's definitions and help text.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// Compare adapted skills with the upstream path each records in metadata.upstream
#[derive(Args)]
#[command(
    after_help = "Examples:\n  ns upstream status\n  ns upstream status --human\n  ns upstream diff ns-tdd",
    arg_required_else_help = true
)]
pub(crate) struct Upstream {
    #[command(subcommand)]
    pub command: UpstreamCmd,
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
