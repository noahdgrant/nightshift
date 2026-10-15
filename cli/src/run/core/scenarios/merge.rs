//! The merge step's decisions, driven through the sim's fake GitHub: moved from
//! `cli/tests/factory/merge_conflicts_and_validate.rs`, `merge_gate_and_labels.rs`,
//! `merge_update_skip.rs` and `run_runners_and_github_checks.rs`, where every assertion was a
//! decision: the outcome, the phases, their feedback, the merge step's actions (the e2e tests'
//! `gh` calls) and the events the core logs.

use serde_json::{json, Value};

use super::super::merge::Ask;
use super::super::sim::{check, Report, Sim};
use crate::factory::LocalGate;
use crate::run::Outcome;

/// The head the fake GitHub's update-branch gives the PR.
const UPDATED_HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

/// The e2e `AUTO`: `merge.policy = "auto"`, a build that commits.
fn auto() -> Sim {
    let mut s = Sim::new();
    s.settings.merge_auto = true;
    s.queue("build", &["pass:commit"]);
    s
}

/// `auto()` under `[gate] local = "build-only"`, with the e2e `remote_queue`'s two committing
/// builds.
fn remote_queue() -> Sim {
    let mut s = auto();
    s.settings.local_gate = LocalGate::BuildOnly;
    s.queue("build", &["pass:commit"]);
    s
}

/// The e2e `behind`: ship pushes, then another actor lands a commit on main that changes one of
/// the PR's files when `overlaps`; GitHub reports the PR `BEHIND`.
fn behind(overlaps: bool) -> Sim {
    let mut s = auto();
    s.queue("ship", &["pass:movemain"]);
    s.merge.merge_state = "BEHIND";
    s.merge.moves_overlap = overlaps;
    s
}

fn sha(n: u32) -> String {
    format!("m{n:06}")
}

/// The merge step's CI waits, updates and merges, in order, as the e2e tests' `gh` call lines.
fn steps(r: &Report) -> Vec<String> {
    r.asks
        .iter()
        .filter_map(|a| match a {
            Ask::WatchChecks { .. } => Some("pr checks 12 --watch".to_string()),
            Ask::UpdateBranch => Some("pr update-branch 12".into()),
            Ask::Merge { head } => Some(format!("pr merge 12 --squash --match-head-commit {head}")),
            _ => None,
        })
        .collect()
}

fn count_polls(r: &Report) -> Vec<&str> {
    r.asks
        .iter()
        .filter_map(|a| match a {
            Ask::CountChecks { sha } => Some(sha.as_str()),
            _ => None,
        })
        .collect()
}

fn position(r: &Report, want: impl Fn(&Ask) -> bool) -> Option<usize> {
    r.asks.iter().position(want)
}

#[test]
fn auto_merge_conflict_goes_back_to_build() {
    let mut s = auto();
    s.queue("build", &["pass:commit"]);
    s.merge.merge_state = "BEHIND";
    s.merge.update_error = Some("gh pr update-branch 12 failed: ".into());
    let r = s.run();
    assert!(r.runs[5].feedback.contains("rebase onto main"), "{r:?}");
    // The first conflict's rebuild is free. Main stays where it was, so the later ones use
    // build's two attempts and the unit ends stuck instead of looping.
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(r.reason, "build is out of attempts (2)");
    assert!(r.merged_at().is_empty());
    let rb = r.rebases();
    assert_eq!(rb.len(), 1, "{rb:?}");
    assert_eq!(rb[0].why, "PR #12 conflicts with main");
}

#[test]
fn a_lagging_head_after_update_branch_is_waited_for() {
    let mut s = auto();
    s.merge.merge_states = ["BEHIND"].into();
    s.merge.update_lag = 2;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(r.asked(&Ask::ViewHead), 3);
    assert_eq!(r.events("update_branch").len(), 1);
    let polls = count_polls(&r);
    assert!(!polls.is_empty());
    assert!(polls.iter().all(|p| *p == UPDATED_HEAD), "{polls:?}");
    assert_eq!(r.merged_at(), [UPDATED_HEAD]);
}

#[test]
fn a_head_that_never_changes_after_update_branch_needs_a_human_merge() {
    let mut s = auto();
    s.merge.merge_states = ["BEHIND"].into();
    s.merge.update_lag = u32::MAX;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.reason,
        "PR #12 head c000001 did not change after update-branch within 3 min; needs a human merge"
    );
    assert_eq!(r.asked(&Ask::ViewHead), 9);
    assert!(r.events("update_branch").is_empty());
    assert!(count_polls(&r).is_empty());
    assert!(r.merged_at().is_empty());
}

#[test]
fn a_dirty_view_while_waiting_for_the_head_goes_back_to_build() {
    let mut s = auto();
    s.queue("build", &["pass:commit"]);
    s.merge.merge_state = "BEHIND";
    s.merge.head_state = "DIRTY";
    s.merge.update_lag = u32::MAX;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert!(r.runs[5].feedback.contains("rebase onto main"), "{r:?}");
    assert!(r.merged_at().is_empty());
}

#[test]
fn a_failed_update_branch_logs_its_cause() {
    let mut s = auto();
    s.queue("build", &["pass:commit"]);
    s.merge.merge_state = "BEHIND";
    s.merge.update_error = Some("GraphQL: merge conflict between base and head".into());
    let r = s.run();
    let failed = r.events("update_branch_failed");
    // The first failure's rebuild is a free rebase; the next two use build's attempts.
    assert_eq!(failed.len(), 3, "{failed:?}");
    let err = failed[0]["error"].as_str().unwrap();
    assert!(
        err.contains("merge conflict between base and head"),
        "{err}"
    );
    assert!(failed[0].get("workflow_scope").is_none(), "{failed:?}");
    assert!(r.merged_at().is_empty());
}

#[test]
fn a_pr_still_behind_after_update_branch_logs_its_merge_state() {
    let mut s = auto();
    s.merge.merge_states = ["BEHIND"].into();
    s.merge.head_state = "BEHIND";
    s.merge.update_lag = u32::MAX;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    let failed = r.events("update_branch_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["merge_state"], "BEHIND", "{failed:?}");
    let err = failed[0]["error"].as_str().unwrap();
    assert!(err.contains("did not change"), "{err}");
}

#[test]
fn ci_failure_rebuilds_then_merges_on_the_same_pr() {
    let mut s = auto();
    s.queue("build", &["pass:commit"]);
    s.merge.checks = [vec![check("ci", "fail")], vec![check("ci", "pass")]].into();
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "review", "ship", "build", "verify", "review", "ship"]
    );
    let p = &r.runs[5].feedback;
    assert!(p.contains("CI failed on PR #12: ci (fail)"), "{p}");
    assert!(p.contains("assert 1 == 2"), "{p}");
    let ci: Vec<(Value, Value)> = r
        .logs
        .iter()
        .filter(|ev| ev["event"] == "ci_failed" || ev["event"] == "ci_passed")
        .map(|ev| (ev["event"].clone(), ev["where"].clone()))
        .collect();
    assert_eq!(
        ci,
        [
            (json!("ci_failed"), json!("github")),
            (json!("ci_passed"), json!("github"))
        ]
    );
}

#[test]
fn checks_that_register_late_are_watched_then_merged() {
    let mut s = auto();
    s.merge.register_after = [2, u32::MAX];
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    let watch = position(&r, |a| matches!(a, Ask::WatchChecks { .. })).unwrap();
    let third = r
        .asks
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, Ask::CountChecks { .. }))
        .nth(2)
        .expect("3 polls")
        .0;
    assert!(third < watch, "{:?}", r.asks);
    assert_eq!(count_polls(&r).len(), 3);
}

#[test]
fn no_checks_registered_within_the_timeout_needs_a_human_merge() {
    let mut s = auto();
    s.merge.register_after = [u32::MAX, u32::MAX];
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.reason,
        "no CI checks registered within 3 min on PR #12; needs a human merge"
    );
    assert_eq!(r.asked(&Ask::WatchChecks { minutes: 0 }), 0);
    assert!(r.merged_at().is_empty());
    assert_eq!(count_polls(&r).len(), 9);
}

#[test]
fn a_shorter_register_timeout_polls_on_the_capped_schedule() {
    let mut s = auto();
    s.merge.register_minutes = 1;
    s.merge.register_after = [u32::MAX, u32::MAX];
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.reason,
        "no CI checks registered within 1 min on PR #12; needs a human merge"
    );
    assert_eq!(count_polls(&r).len(), 5);
}

#[test]
fn status_only_ci_registers_and_is_watched() {
    let mut s = auto();
    s.merge.register_after = [u32::MAX, 1];
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(r.asked(&Ask::WatchChecks { minutes: 0 }), 1);
    assert_eq!(count_polls(&r).len(), 2);
}

#[test]
fn a_failing_check_query_is_reported_not_called_no_ci() {
    let mut s = auto();
    s.merge.count_errors[0] = Some("gh api failed: HTTP 403: rate limit exceeded".into());
    s.merge.register_after[1] = u32::MAX;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert!(
        r.reason
            .starts_with("could not query CI checks on PR #12: "),
        "{}",
        r.reason
    );
    assert!(r.reason.contains("rate limit exceeded"), "{}", r.reason);
    assert!(r.reason.ends_with("; needs a human merge"), "{}", r.reason);
    assert_eq!(r.asked(&Ask::WatchChecks { minutes: 0 }), 0);
}

#[test]
fn zero_check_runs_and_a_failing_status_query_is_reported_not_called_no_ci() {
    let mut s = auto();
    s.merge.register_after[0] = u32::MAX;
    s.merge.count_errors[1] = Some("gh api failed: HTTP 500: server error".into());
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert!(
        r.reason
            .starts_with("could not query CI checks on PR #12: "),
        "{}",
        r.reason
    );
    assert!(r.reason.contains("server error"), "{}", r.reason);
    assert_eq!(r.asked(&Ask::WatchChecks { minutes: 0 }), 0);
}

#[test]
fn a_status_with_checks_proceeds_though_the_check_runs_query_fails() {
    let mut s = auto();
    s.merge.count_errors[0] = Some("gh api failed: HTTP 403: rate limit exceeded".into());
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(r.asked(&Ask::WatchChecks { minutes: 0 }), 1);
    assert_eq!(r.merged_at().len(), 1);
}

#[test]
fn human_review_files_need_a_human_merge() {
    let mut s = auto();
    s.merge.review_hits = vec![".github/workflows/ci.yml".into()];
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done);
    assert!(r
        .reason
        .starts_with("changes files that need human review; needs a human merge"));
    assert!(r.merged_at().is_empty());
}

#[test]
fn human_policy_never_merges() {
    let mut s = Sim::new();
    s.queue("build", &["pass:commit"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done);
    assert!(r.asks.is_empty(), "{:?}", r.asks);
}

#[test]
fn a_main_that_moves_during_ci_updates_the_pr_and_waits_for_ci_again_before_merging() {
    let mut s = auto();
    s.merge.moves_on_watch = 1;
    s.merge.merge_states = ["CLEAN", "BEHIND"].into();
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    let moves = r.events("merge_base_moved");
    assert_eq!(moves.len(), 1, "{moves:?}");
    let m = moves[0];
    assert_eq!(
        (&m["pr"], &m["from"], &m["to"], &m["attempt"]),
        (&json!(12), &json!(sha(0)), &json!(sha(1)), &json!(1)),
        "{m}"
    );
    assert_eq!(
        steps(&r),
        [
            "pr checks 12 --watch".to_string(),
            "pr update-branch 12".into(),
            "pr checks 12 --watch".into(),
            format!("pr merge 12 --squash --match-head-commit {UPDATED_HEAD}"),
        ]
    );
}

#[test]
fn a_main_unreadable_before_ci_counts_as_moved_once_it_can_be_read() {
    let mut s = auto();
    s.merge.tip_fails = 1;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    let moves = r.events("merge_base_moved");
    assert_eq!(moves.len(), 1, "{moves:?}");
    assert_eq!(moves[0]["from"], Value::Null, "{moves:?}");
    assert_eq!(moves[0]["to"], json!(sha(0)), "{moves:?}");
    assert_eq!(r.asked(&Ask::WatchChecks { minutes: 0 }), 2);
}

/// A main that moves during every CI wait: past `BASE_MOVE_CAP` moves the merge lock is held
/// through the update and CI wait, and the move past `BASE_MOVE_HARD_CAP` ends the unit stuck.
#[test]
fn a_main_that_keeps_moving_takes_the_lock_once_at_the_cap_and_stops_past_the_hard_cap() {
    let mut s = auto();
    s.merge.moves_on_watch = u32::MAX;
    s.merge.merge_states = ["CLEAN"].into();
    s.merge.merge_state = "BEHIND";
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(
        r.reason,
        "the default branch moved 10 times during this PR's CI, the last 5 with the merge lock held (cap 10)"
    );
    let attempts: Vec<&Value> = r
        .events("merge_base_moved")
        .iter()
        .map(|m| &m["attempt"])
        .collect();
    assert_eq!(
        attempts,
        (1..=11)
            .map(|a| json!(a))
            .collect::<Vec<_>>()
            .iter()
            .collect::<Vec<_>>()
    );
    let held = r.events("merge_lock_held_through_ci");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["moves"], 5);
    assert_eq!(r.asked(&Ask::WatchChecks { minutes: 0 }), 11);
    assert_eq!(r.asked(&Ask::TakeLock), 6);
    assert!(r.merged_at().is_empty());
}

#[test]
fn a_pr_head_that_moves_after_ci_passed_needs_a_human_merge() {
    let mut s = auto();
    s.merge.head_after_ci = Some(UPDATED_HEAD.into());
    let r = s.run();
    assert_eq!((r.outcome, r.needs_human), (Outcome::Done, true), "{r:?}");
    assert_eq!(
        r.reason,
        format!("PR #12 head moved from c000001 to {UPDATED_HEAD} after CI passed on it; needs a human merge")
    );
    assert!(r.merged_at().is_empty());
}

#[test]
fn with_the_local_queue_off_a_base_that_moved_in_other_files_skips_the_update() {
    let mut s = behind(false);
    s.merge.local_queue = false;
    s.gate(&[]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(r.asked(&Ask::UpdateBranch), 0);
    assert_eq!(r.events("merge_update_skipped").len(), 1);
}

#[test]
fn with_the_local_queue_on_a_behind_pr_gates_the_merge_result_and_checks_no_overlap() {
    let mut s = behind(false);
    s.gate(&[]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(r.asked(&Ask::UpdateBranch), 0);
    assert!(r.events("merge_update_skipped").is_empty());
    assert!(r.events("merge_update_needed").is_empty());
}

const MAIN_TIP: &str = "abcdef0123456789abcdef0123456789abcdef01";

/// `behind(false)` under build-only, as the e2e `behind` with `BUILD_ONLY` and `LAND_DISJOINT`.
fn behind_build_only() -> Sim {
    let mut s = behind(false);
    s.settings.local_gate = LocalGate::BuildOnly;
    s
}

#[test]
fn a_red_main_holds_the_merge_until_it_goes_green() {
    let mut s = behind_build_only();
    s.merge.main_red_polls = 2;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    let held = r.events("merge_held_main_red");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(
        (&held[0]["pr"], &held[0]["sha"]),
        (&json!(12), &json!(MAIN_TIP))
    );
    let queries: Vec<usize> = (0..r.asks.len())
        .filter(|i| r.asks[*i] == Ask::MainChecks)
        .collect();
    assert_eq!(queries.len(), 3, "{:?}", r.asks);
    let merge = position(&r, |a| matches!(a, Ask::Merge { .. })).unwrap();
    assert!(queries[2] < merge, "{:?}", r.asks);
}

#[test]
fn a_main_still_red_after_the_ci_timeout_ends_the_unit_stuck() {
    let mut s = behind_build_only();
    s.merge.main_red_polls = u32::MAX;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(
        r.reason,
        format!("main's CI is red at {MAIN_TIP}; not merging until it is fixed")
    );
    assert!(r.merged_at().is_empty());
    assert_eq!(r.events("merge_held_main_red").len(), 1);
}

/// The local merge queue gated the merge with the old base, so a move in other files still
/// sends the PR round to gate the merge with the new one.
#[test]
fn with_the_local_queue_on_a_base_that_moves_in_other_files_under_the_lock_goes_round() {
    let mut s = behind(false);
    s.gate(&[]);
    s.merge.moves_on_watch = 1;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    let moved = r.events("merge_base_moved");
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(moved[0]["to"], json!(sha(2)), "{moved:?}");
    assert!(r.events("merge_update_skipped").is_empty());
}

#[test]
fn with_the_local_queue_on_a_red_main_holds_nothing() {
    let mut s = behind(false);
    s.gate(&[]);
    s.merge.main_red_polls = u32::MAX;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert!(r.events("merge_held_main_red").is_empty());
    assert_eq!(r.asked(&Ask::MainChecks), 0);
}

#[test]
fn under_build_only_a_clean_pr_missing_a_disjoint_landing_skips_the_update() {
    let mut s = behind_build_only();
    s.merge.merge_state = "CLEAN";
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(r.asked(&Ask::UpdateBranch), 0);
    assert_eq!(r.events("merge_update_skipped").len(), 1);
    assert!(r.events("merge_update_needed").is_empty());
}

#[test]
fn auto_merge_updates_a_branch_behind_main_first() {
    let mut s = auto();
    s.merge.merge_state = "BEHIND";
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    let upd = position(&r, |a| *a == Ask::UpdateBranch).expect("update-branch");
    let watch = position(&r, |a| matches!(a, Ask::WatchChecks { .. })).unwrap();
    let merge = position(&r, |a| matches!(a, Ask::Merge { .. })).unwrap();
    assert!(upd < watch && watch < merge, "{:?}", r.asks);
    let vias: Vec<&Value> = r
        .events("update_branch")
        .iter()
        .map(|e| &e["via"])
        .collect();
    assert_eq!(vias, [&json!("api")]);
}

#[test]
fn build_only_does_not_update_a_pr_head_that_holds_main() {
    let r = remote_queue().run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(r.asked(&Ask::UpdateBranch), 0);
    assert_eq!(r.asked(&Ask::WatchChecks { minutes: 0 }), 1);
}

#[test]
fn build_only_sends_a_conflict_with_main_back_to_ship() {
    for dirty in [true, false] {
        let mut s = remote_queue();
        s.remote = Some(("main".into(), 1));
        if dirty {
            s.merge.merge_state = "DIRTY";
        } else {
            s.merge.update_error = Some("gh pr update-branch 12 failed: ".into());
        }
        let r = s.run();
        assert_eq!(r.outcome, Outcome::Stuck, "{dirty}: {r:?}");
        assert_eq!(r.reason, "ship is out of attempts (2)", "{dirty}: {r:?}");
        // The first conflict's reship is free. Main stays where it was, so the next one uses
        // ship's second attempt and the unit ends stuck instead of looping.
        assert_eq!(
            r.calls(),
            ["triage", "build", "verify", "review", "ship", "ship", "ship"],
            "{dirty}"
        );
        let p = &r.runs[5].feedback;
        assert!(
            p.contains("PR #12 conflicts with main: rebase onto main and resolve conflicts"),
            "{dirty}: {p}"
        );
        assert!(
            p.contains(&format!(
                "merge in the newest origin/main ({}). This ship costs no attempt.",
                sha(1)
            )),
            "{dirty}: {p}"
        );
        assert!(!r.runs[6].feedback.contains("costs no attempt"), "{dirty}");
        let rb = r.rebases();
        assert_eq!(rb.len(), 1, "{dirty}: {rb:?}");
        assert_eq!(
            (rb[0].phase, rb[0].why.as_str()),
            ("ship", "the PR conflicts with its base"),
            "{dirty}"
        );
        assert!(r.merged_at().is_empty(), "{dirty}");
        assert!(
            r.events("conflict").iter().any(|ev| ev["pr"] == 12),
            "{dirty}"
        );
    }
}

/// `remote_queue()` with the PR conflicting with main, and main's moves in no file of the PR's.
fn dirty_remote_queue() -> Sim {
    let mut s = remote_queue();
    s.merge.merge_state = "DIRTY";
    s.merge.moves_overlap = false;
    s
}

#[test]
fn build_only_ships_that_follow_a_moved_base_cost_ship_no_attempts() {
    let mut s = dirty_remote_queue();
    // The e2e `ship_brings_in_main(e, 2)`: each scripted ship commits and clears the conflict,
    // except the second, where another actor moves main and the PR conflicts again.
    s.queue(
        "ship",
        &[
            "pass",
            "pass:commit+clean",
            "pass:movemain+dirty",
            "pass:commit+clean",
            "pass",
        ],
    );
    s.queue("review", &["pass", "pass:commit", "pass:commit"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(
        r.calls(),
        [
            "triage", "build", "verify", "review", "ship", "ship", "verify", "review", "verify",
            "ship", "ship", "verify", "review", "verify", "ship"
        ]
    );
    assert_eq!(r.events("ship_after_base_moved").len(), 2);
    assert_eq!(r.rebases().len(), 2);
}

#[test]
fn build_only_ships_that_fail_after_a_moved_base_still_run_out_of_attempts() {
    let mut s = dirty_remote_queue();
    s.queue("ship", &["pass", "fail:review", "fail:review"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(r.reason, "ship is out of attempts (2)", "{r:?}");
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "review", "ship", "ship", "review"]
    );
}

#[test]
fn build_only_a_failing_ship_after_a_moved_base_gives_its_free_attempt_back() {
    let mut s = dirty_remote_queue();
    s.queue("ship", &["pass", "pass:commit+clean", "fail:review"]);
    s.queue("review", &["pass", "pass:commit"]);
    let r = s.run();
    assert_eq!(r.reason, "ship is out of attempts (2)", "{r:?}");
    assert_eq!(
        r.calls(),
        [
            "triage", "build", "verify", "review", "ship", "ship", "verify", "review", "verify",
            "ship", "review"
        ]
    );
}

#[test]
fn build_only_a_build_after_a_moved_base_makes_the_next_ship_cost_an_attempt() {
    let mut s = dirty_remote_queue();
    s.queue("ship", &["pass", "pass:commit+clean", "pass"]);
    s.queue("review", &["pass", "fail"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Merged, "{r:?}");
    assert_eq!(
        r.calls(),
        [
            "triage", "build", "verify", "review", "ship", "ship", "verify", "review", "build",
            "verify", "review", "ship"
        ]
    );
    assert!(r.events("ship_after_base_moved").is_empty());
}

#[test]
fn build_only_a_reship_that_changes_nothing_does_not_make_the_next_reship_a_chain_ship() {
    let mut s = dirty_remote_queue();
    s.queue(
        "ship",
        &[
            "pass:movemain",
            "pass:movemain",
            "pass:movemain",
            "fail:review",
            "fail:review",
        ],
    );
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(r.reason, "ship is out of attempts (2)", "{r:?}");
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "review", "ship", "ship", "ship", "ship", "review"]
    );
    assert!(!r.logged("ship_after_base_moved"));
}

#[test]
fn build_only_stops_reshipping_for_a_base_that_moves_on_every_check() {
    let mut s = dirty_remote_queue();
    s.queue("ship", &["pass:movemain"; 7]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(
        r.reason,
        "ship was sent back 5 times because the base moved (cap 5)"
    );
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "review", "ship", "ship", "ship", "ship", "ship", "ship"]
    );
    assert_eq!(r.rebases().len(), 5);
    let capped = r.events("reship_cap");
    assert_eq!(capped.len(), 1, "{capped:?}");
    assert_eq!(
        (&capped[0]["reships"], &capped[0]["cap"]),
        (&json!(5), &json!(5))
    );
    assert!(r.merged_at().is_empty());
}

#[test]
fn build_only_a_reship_earns_no_credit_when_origin_default_cannot_be_read() {
    let mut s = dirty_remote_queue();
    s.queue("review", &["pass:noorigin"]);
    s.queue("ship", &["pass", "fail:review", "fail:review"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(r.reason, "ship is out of attempts (2)", "{r:?}");
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "review", "ship", "ship", "review"]
    );
    assert!(!r.logged("ship_after_base_moved"));
    assert!(r.rebases().is_empty());
}

#[test]
fn build_only_needs_a_human_merge_when_main_cannot_be_fetched() {
    let mut s = remote_queue();
    s.queue("ship", &["pass:noorigin"]);
    let r = s.run();
    assert!(r.needs_human, "{r:?}");
    assert!(
        r.reason
            .starts_with("cannot fetch main to update PR #12: git fetch origin main failed"),
        "{}",
        r.reason
    );
    assert!(r.merged_at().is_empty());
}

#[test]
fn build_only_waits_for_github_checks_at_most_ci_timeout_minutes() {
    let mut s = remote_queue();
    s.merge.ci_minutes = 0;
    s.merge.ci_times_out = true;
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(r.reason, "CI on PR #12 did not finish within 0 min");
    assert!(r.merged_at().is_empty());
}
