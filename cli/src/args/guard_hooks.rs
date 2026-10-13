//! `ns guard-hooks`: clap's definition.

use clap::Args;

/// The PreToolUse hook ns run gives claude phases: deny a git hook bypass (internal)
#[derive(Args)]
#[command(hide = true)]
pub(crate) struct GuardHooks {}
