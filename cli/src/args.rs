//! The `ns` command line. Each command's clap definitions and help text live in its own module
//! (a command group shares one); this file holds the root and the `Cmd` enum naming them. `main`
//! parses it and dispatches each command to its module.

mod artifact;
mod ask;
mod check_markers;
mod clean;
mod doctor;
mod eval;
mod factory;
mod guard_hooks;
mod install;
mod lint;
mod log;
mod quality;
mod ratchet;
mod rules;
mod run;
mod score;
mod status;
mod transcripts;
mod upstream;
mod watch;
mod worktree;

use clap::{Parser, Subcommand};

pub(crate) use artifact::{Artifact, ArtifactCmd};
pub(crate) use ask::Ask;
pub(crate) use check_markers::CheckMarkers;
pub(crate) use clean::Clean;
pub(crate) use doctor::Doctor;
pub(crate) use eval::Eval;
pub(crate) use factory::{Factory, FactoryCmd};
pub(crate) use guard_hooks::GuardHooks;
pub(crate) use install::Install;
pub(crate) use lint::Lint;
pub(crate) use log::Log;
pub(crate) use quality::{Quality, QualityCmd};
pub(crate) use ratchet::{Ratchet, RatchetCmd};
pub(crate) use rules::{Rules, RulesCmd};
pub(crate) use run::Run;
pub(crate) use score::Score;
pub(crate) use status::Status;
pub(crate) use transcripts::{Transcripts, TranscriptsCmd};
pub(crate) use upstream::{Upstream, UpstreamCmd};
pub(crate) use watch::Watch;
pub(crate) use worktree::{Worktree, WorktreeCmd};

const ROOT_HELP: &str = "\
Examples:
  ns worktree new 142-uart-timeout
  ns worktree list
  echo 'Review this diff' | ns ask --role review.security
  ns lint skills
  ns check-markers
  ns artifact check .ns/142-uart-timeout/evidence.md --phase verify
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
    Worktree(Worktree),
    Ask(Ask),
    Lint(Lint),
    CheckMarkers(CheckMarkers),
    Artifact(Artifact),
    Install(Install),
    Eval(Eval),
    Factory(Factory),
    Score(Score),
    Run(Run),
    Watch(Watch),
    Clean(Clean),
    Log(Log),
    Status(Status),
    Quality(Quality),
    Ratchet(Ratchet),
    Rules(Rules),
    Transcripts(Transcripts),
    Upstream(Upstream),
    Doctor(Doctor),
    GuardHooks(GuardHooks),
}
