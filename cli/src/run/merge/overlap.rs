//! Whether a PR behind its base must take the base in before it merges: only when the commits
//! it lacks change one of its files (docs/factory/merge.md, ADR-648).

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use serde_json::json;

use super::super::Ctx;
use crate::git::{self, same_sha};

/// The paths changed from `from` to `to`, a rename as both of its paths.
fn changed_files(wt: &Path, from: &str, to: &str) -> Result<BTreeSet<String>> {
    Ok(
        git::run(wt, &["diff", "--name-only", "--no-renames", from, to])?
            .lines()
            .map(String::from)
            .collect(),
    )
}

/// Whether PR #`n` at `head` must take in `tip` before it merges: the commits it lacks change one
/// of its files, or either list can't be read. Logs which, and the lists when it need not.
pub(super) fn needs_update(ctx: &Ctx<'_>, n: u64, head: &str, tip: &str) -> bool {
    let wt = &ctx.worktree;
    let lists = git::run(wt, &["merge-base", head, tip]).and_then(|mb| {
        if same_sha(&mb, tip) {
            return Ok(None);
        }
        Ok(Some((
            changed_files(wt, &mb, head)?,
            changed_files(wt, &mb, tip)?,
        )))
    });
    match lists {
        Ok(None) => ctx.log(json!({
            "event": "merge_update_needed",
            "pr": n,
            "overlap": null,
            "reason": format!("{tip} is already in the PR head {head}"),
        })),
        Ok(Some((pr_files, base_files))) => {
            let overlap: Vec<&String> = pr_files.intersection(&base_files).collect();
            if overlap.is_empty() {
                ctx.log(json!({
                    "event": "merge_update_skipped",
                    "pr": n,
                    "base": tip,
                    "pr_files": pr_files,
                    "base_files": base_files,
                }));
                return false;
            }
            ctx.log(json!({"event": "merge_update_needed", "pr": n, "overlap": overlap}));
        }
        Err(e) => ctx.log(json!({
            "event": "merge_update_needed",
            "pr": n,
            "overlap": null,
            "error": format!("{e:#}"),
        })),
    }
    true
}
