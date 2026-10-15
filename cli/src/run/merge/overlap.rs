//! What the merge step reads to tell whether a PR behind its base must take the base in before
//! it merges: the files each side changed (docs/factory/merge.md, ADR-648). `core::merge`
//! decides, and logs why.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;

use super::super::core::merge::Overlap;
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

/// What the PR at `head` and the default branch at `tip` each changed since their merge base.
pub(super) fn read(wt: &Path, head: &str, tip: &str) -> Overlap {
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
        Ok(None) => Overlap::HasTip,
        Ok(Some((pr, base))) => Overlap::Files { pr, base },
        Err(e) => Overlap::Unreadable(format!("{e:#}")),
    }
}
