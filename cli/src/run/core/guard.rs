//! Guard decisions over plain data: who moved the default branch during a phase (another actor's
//! merged PRs, or this unit's push), and the breach a phase that merged a PR makes. The shell
//! (`run/post_phase.rs`) reads the remote, the ancestry and the forge's PRs for each commit, and
//! logs and stops on what these return.

use serde_json::{json, Value};

use crate::watch::forge::CommitPr;

/// The most commits one move of the default branch is checked for, so a long push is still
/// decided with a bounded number of forge queries.
pub const MOVE_CHECK_MAX: usize = 50;

/// Why the forge could not say which PRs hold a commit.
#[derive(Debug, Clone, PartialEq)]
pub enum Unread {
    /// The forge could not be opened.
    NoForge(String),
    /// The query for this commit failed.
    Commit(String),
}

/// The default branch moved to a commit the unit's HEAD holds. That is another actor's move only
/// when every commit it added along the first-parent line (`commits`, newest first, at most
/// [`MOVE_CHECK_MAX`] + 1, or why they could not be listed) came from a merged PR that isn't
/// `own_pr` and doesn't come from `own_branch`: the unit holds the tip because it rebased onto
/// it. `prs_of` reads the PRs that hold one commit; it is called in order, and not past the first
/// commit that decides. Returns those PRs, or why the move counts as this unit's.
pub fn moved_by_others(
    commits: Result<Vec<String>, String>,
    own_pr: Option<u64>,
    own_branch: &str,
    mut prs_of: impl FnMut(&str) -> Result<Vec<CommitPr>, Unread>,
) -> Result<Vec<u64>, String> {
    let commits = commits.map_err(|e| format!("cannot list the commits it added: {e}"))?;
    if commits.is_empty() {
        return Err("it added no commit".into());
    }
    if commits.len() > MOVE_CHECK_MAX {
        return Err(format!("it added more than {MOVE_CHECK_MAX} commits"));
    }
    let mut prs = vec![];
    for c in &commits {
        let pulls = prs_of(c).map_err(|e| match e {
            Unread::NoForge(e) => format!("cannot tell who made it: {e}"),
            Unread::Commit(e) => format!("cannot tell who made {c}: {e}"),
        })?;
        let n = other_merged_pr(&pulls, own_pr, own_branch)
            .ok_or_else(|| format!("{c} is in no merged PR of another branch"))?;
        if !prs.contains(&n) {
            prs.push(n);
        }
    }
    Ok(prs)
}

/// A merged PR among the PRs that hold a commit, when none of them is `own_pr` or comes from
/// `own_branch`.
pub fn other_merged_pr(pulls: &[CommitPr], own_pr: Option<u64>, own_branch: &str) -> Option<u64> {
    let own = |p: &CommitPr| Some(p.number) == own_pr || p.head == own_branch;
    if pulls.iter().any(own) {
        return None;
    }
    pulls.iter().find(|p| p.merged).map(|p| p.number)
}

/// The default branch `branch` moved from `from` to `now` during `phase`. `attributed` is `None`
/// when the unit's HEAD doesn't hold `now`, so the unit can't have pushed it; else what
/// [`moved_by_others`] found. Returns the run-log event, and why the unit stops when it does.
pub fn branch_moved(
    phase: &str,
    (branch, from, now): (&str, &str, &str),
    attributed: Option<Result<Vec<u64>, String>>,
) -> (Value, Option<String>) {
    match attributed.unwrap_or(Ok(vec![])) {
        Ok(prs) => (
            json!({"event": "default_moved_by_other_actor", "phase": phase, "from": from, "to": now, "prs": prs}),
            None,
        ),
        Err(why) => (
            json!({"event": "breach", "rule": "default branch moved", "phase": phase, "from": from, "to": now, "why": why}),
            Some(format!("default branch moved: {branch} went from {from} to {now}, which this unit holds and may have pushed: {why}")),
        ),
    }
}

/// The PR #`n` that pr.md names was merged during `phase`: the run-log event and the breach.
pub fn pr_merged(phase: &str, n: u64) -> (Value, String) {
    (
        json!({"event": "breach", "rule": "PR merged by run", "phase": phase, "pr": n}),
        format!("PR merged by run: #{n} was merged during the {phase} phase, outside ns run's merge step"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u64, merged: bool, head: &str) -> CommitPr {
        CommitPr {
            number,
            head: head.into(),
            merged,
        }
    }

    #[test]
    fn other_merged_pr_names_only_a_merged_pr_of_another_branch() {
        let mine = "ns/7-fix";
        let check = |list: Vec<CommitPr>, own: Option<u64>| other_merged_pr(&list, own, mine);
        assert_eq!(check(vec![pr(3, true, "ns/3-b")], None), Some(3));
        assert_eq!(check(vec![pr(3, true, "ns/3-b")], Some(12)), Some(3));
        assert_eq!(check(vec![], None), None);
        assert_eq!(check(vec![pr(3, false, "ns/3-b")], None), None);
        assert_eq!(check(vec![pr(12, true, "ns/3-b")], Some(12)), None);
        assert_eq!(check(vec![pr(9, true, mine)], None), None);
        assert_eq!(
            check(vec![pr(3, true, "ns/3-b"), pr(12, false, "x")], Some(12)),
            None
        );
        assert_eq!(
            check(vec![pr(3, false, "a"), pr(4, true, "b")], Some(12)),
            Some(4)
        );
    }

    /// Each commit is looked up in order, stopping at the first that is the unit's; a PR that
    /// holds several commits is named once.
    #[test]
    fn a_move_is_another_actors_only_when_every_commit_is_another_branchs_merged_pr() {
        let commits = |n: usize| Ok((0..n).map(|i| format!("c{i}")).collect::<Vec<_>>());
        let mut asked = vec![];
        let by = moved_by_others(commits(3), None, "ns/7-x", |c| {
            asked.push(c.to_string());
            Ok(vec![pr(if c == "c2" { 4 } else { 3 }, true, "ns/3-b")])
        });
        assert_eq!(by, Ok(vec![3, 4]));
        assert_eq!(asked, ["c0", "c1", "c2"]);
        let mut asked = 0;
        let mine = moved_by_others(commits(3), Some(12), "ns/7-x", |_| {
            asked += 1;
            Ok(vec![pr(12, true, "ns/7-x")])
        });
        assert_eq!(mine, Err("c0 is in no merged PR of another branch".into()));
        assert_eq!(asked, 1);
        let none = |_: &str| -> Result<Vec<CommitPr>, Unread> { panic!("no lookup") };
        assert_eq!(
            moved_by_others(commits(0), None, "b", none),
            Err("it added no commit".into())
        );
        assert_eq!(
            moved_by_others(commits(51), None, "b", none),
            Err("it added more than 50 commits".into())
        );
        assert_eq!(
            moved_by_others(Err("boom".into()), None, "b", none),
            Err("cannot list the commits it added: boom".into())
        );
        let forge = |_: &str| Err(Unread::NoForge("no token".into()));
        assert_eq!(
            moved_by_others(commits(1), None, "b", forge),
            Err("cannot tell who made it: no token".into())
        );
        let query = |_: &str| Err(Unread::Commit("HTTP 502".into()));
        assert_eq!(
            moved_by_others(commits(1), None, "b", query),
            Err("cannot tell who made c0: HTTP 502".into())
        );
    }

    #[test]
    fn a_move_the_unit_does_not_hold_or_others_made_is_logged_and_one_it_may_have_made_breaches() {
        let at = ("main", "a1", "b2");
        let (ev, breach) = branch_moved("build", at, None);
        assert_eq!(ev["event"], "default_moved_by_other_actor");
        assert_eq!(ev["prs"], json!([]));
        assert_eq!(breach, None);
        let (ev, breach) = branch_moved("build", at, Some(Ok(vec![3])));
        assert_eq!((ev["prs"].clone(), breach), (json!([3]), None));
        let (ev, breach) = branch_moved("ship", at, Some(Err("it added no commit".into())));
        assert_eq!(ev["event"], "breach");
        assert_eq!(ev["why"], "it added no commit");
        assert_eq!(
            breach.as_deref(),
            Some("default branch moved: main went from a1 to b2, which this unit holds and may have pushed: it added no commit")
        );
    }
}
