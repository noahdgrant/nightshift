//! What the merge step reads to hold the merge while the default branch's CI is red
//! (docs/factory/merge.md, ADR-648); `core::merge` decides whether to wait.

use super::super::core::merge::MainHead;
use super::super::Ctx;
use crate::watch::{ci, forge};

/// Whether `unit` works an issue `ns watch` filed to fix the default branch's red CI, whose
/// unit name is the issue number and the slug of its title.
pub(super) fn fixes_main(unit: &str) -> bool {
    let prefix = super::super::slug(crate::watch::FIX_MAIN_TITLE);
    unit.split_once('-')
        .is_some_and(|(_, title)| title.starts_with(&prefix))
}

/// The default branch's CI at its head, `None` when the forge or its checks can't be read. A head
/// with no finished check, or with a check still running, is not red.
pub(super) fn main_head(ctx: &Ctx<'_>) -> Option<MainHead> {
    let forge = forge::open(&ctx.fac.queue, &ctx.worktree).ok()?;
    let head = forge.main_checks().ok()?;
    let running = head
        .checks
        .iter()
        .any(|c| c.outcome == ci::Outcome::Running);
    let main = ci::classify(head);
    Some(MainHead {
        red: main.state == ci::State::Red,
        running,
        sha: main.sha.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_unit_named_for_a_fix_main_issue_fixes_main() {
        assert!(fixes_main("100-fix-main-is-red-at-abcdef0-test"));
        assert!(!fixes_main("7-fix-the-thing"));
        assert!(!fixes_main("fix-main-is-red-at-abcdef0"));
        assert!(!fixes_main("100"));
    }
}
