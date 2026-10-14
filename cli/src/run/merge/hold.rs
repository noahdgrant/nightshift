//! Hold the merge while the default branch's CI is red (docs/factory/merge.md, ADR-648).

use anyhow::Result;
use serde_json::json;

use super::super::{finish, Ctx, Outcome};
use super::{MergeStep, REGISTER_BACKOFF_MAX, REGISTER_BACKOFF_START};
use crate::clock::Clock;
use crate::watch::{ci, forge};

/// Whether `unit` works an issue `ns watch` filed to fix the default branch's red CI, whose
/// unit name is the issue number and the slug of its title.
fn fixes_main(unit: &str) -> bool {
    let prefix = super::super::slug(crate::watch::FIX_MAIN_TITLE);
    unit.split_once('-')
        .is_some_and(|(_, title)| title.starts_with(&prefix))
}

/// Wait while the default branch's CI is red, before the merge lock: `None` once it isn't, or
/// can't be read, else how the merge step ends after `ci_timeout_minutes`. A head with no
/// finished check, or with a check still running, is not red. The unit fixing main is never held.
pub(super) fn hold_while_main_red(
    ctx: &Ctx<'_>,
    n: u64,
    clock: &Clock,
) -> Result<Option<MergeStep>> {
    if fixes_main(&ctx.unit) {
        return Ok(None);
    }
    let Ok(forge) = forge::open(&ctx.fac.queue, &ctx.worktree) else {
        return Ok(None);
    };
    let deadline = clock.now() + (ctx.fac.merge.ci_timeout_minutes * 60) as i64;
    let mut wait = REGISTER_BACKOFF_START;
    let mut polls = 0;
    let mut logged = None;
    loop {
        clock.check_polls(polls)?;
        polls += 1;
        let Ok(head) = forge.main_checks() else {
            return Ok(None);
        };
        let running = head
            .checks
            .iter()
            .any(|c| c.outcome == ci::Outcome::Running);
        let main = ci::classify(head);
        if main.state != ci::State::Red || running {
            return Ok(None);
        }
        let sha = main.sha.unwrap_or_default();
        if logged.as_ref() != Some(&sha) {
            ctx.log(json!({"event": "merge_held_main_red", "pr": n, "sha": sha}));
            logged = Some(sha.clone());
        }
        let now = clock.now();
        if now >= deadline {
            return Ok(Some(MergeStep::Finish(finish(
                Outcome::Stuck,
                format!("main's CI is red at {sha}; not merging until it is fixed"),
                Some("merge"),
            ))));
        }
        clock.sleep_until((now + wait).min(deadline))?;
        wait = (wait * 2).min(REGISTER_BACKOFF_MAX);
    }
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
