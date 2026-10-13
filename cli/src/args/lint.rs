//! `ns lint`: clap's definition and help text.

use std::path::PathBuf;

use clap::Args;

/// Validate skills (frontmatter, names, upstream, relative links, features/*.md headings), and decisions when the parent has docs/decisions/; a repo-local root (.agents/skills) needs no ns- prefix
#[derive(Args)]
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
pub(crate) struct Lint {
    /// Skills directory
    #[arg(default_value = "skills")]
    pub path: PathBuf,
    /// Print one readable line per error instead of JSON
    #[arg(long)]
    pub human: bool,
}
