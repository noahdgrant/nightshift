//! The hard rules after a phase, and the phase a red gate blames: moved from
//! `cli/tests/factory/run_runners_and_github_checks.rs`, `skills_sync_and_merge_gate.rs` and
//! `merge_conflicts_and_validate.rs`, where every assertion was a decision.

use super::super::sim::Sim;
use crate::run::Outcome;

/// The e2e `a_push_of_its_own_under_another_merge_is_still_stuck`: build pushes its own commit
/// to main, another unit's PR lands on top, and build rebases onto both.
#[test]
fn a_push_of_its_own_under_another_merge_is_still_stuck() {
    let mut s = Sim::new();
    s.queue("build", &["pass:commit+pushmain+landmain"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert!(r.reason.starts_with("default branch moved"), "{r:?}");
    assert!(
        r.reason.contains("is in no merged PR of another branch"),
        "{r:?}"
    );
    assert_eq!(r.calls(), ["triage", "build"]);
}

#[test]
fn a_merged_pr_from_this_units_branch_is_still_stuck() {
    let mut s = Sim::new();
    s.land_pr.head = format!("ns/{}", s.unit);
    s.queue("build", &["pass:commit+landmain"]);
    let r = s.run();
    assert!(
        r.reason.contains("is in no merged PR of another branch"),
        "{r:?}"
    );
}

#[test]
fn a_merge_github_cannot_attribute_is_stuck() {
    let mut s = Sim::new();
    s.forge_fails = Some("HTTP 502".into());
    s.queue("build", &["pass:commit+landmain"]);
    let r = s.run();
    assert!(r.reason.contains("cannot tell who made"), "{r:?}");
    assert_eq!(r.calls(), ["triage", "build"]);
}

/// Another unit's merge the build rebased onto is no breach: the run goes on, and the move is
/// logged with its PR.
#[test]
fn another_units_merge_the_build_rebased_onto_is_logged_and_the_run_goes_on() {
    let mut s = Sim::new();
    s.queue("build", &["pass:commit+landmain"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    let moved = r.events("default_moved_by_other_actor");
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(moved[0]["prs"], serde_json::json!([3]));
    assert!(!r.logged("breach"));
}

#[test]
fn a_pr_merged_by_a_phase_is_stuck() {
    let mut s = Sim::new();
    s.settings.merge_auto = true;
    s.queue("build", &["pass:commit"]);
    s.queue("ship", &["pass:merge"]);
    let r = s.run();
    assert!(r.reason.starts_with("PR merged by run"), "{r:?}");
    assert!(r.merged_at().is_empty());
}

#[test]
fn a_red_gate_after_a_verify_commit_goes_back_to_build_before_review() {
    let mut s = Sim::new();
    s.gate(&[Ok(()), Err("test ! -f broken"), Ok(())]);
    s.queue("build", &["pass:commit", "pass:commit"]);
    s.queue("verify", &["pass:commit", "pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "build", "verify", "review", "ship"]
    );
    assert!(r.runs[3].feedback.contains("test ! -f broken"));
    assert_eq!(
        r.gated(),
        [("build", true), ("verify", false), ("build", true)]
    );
}

/// A red gate naming a file review's commit changed blames review: a fix cycle fixes it, and
/// build, with one attempt, never runs again. Naming a file nobody changed blames no one, and
/// build is out of attempts.
#[test]
fn a_red_gate_naming_a_file_a_review_commit_changed_costs_build_no_attempt() {
    let run = |output: &str| {
        let mut s = Sim::new();
        s.max_attempts("build", 1);
        s.gate(&[Ok(()), Err(output)]);
        s.queue("build", &["pass:commit+touch=src/a.rs", "pass:commit"]);
        s.queue("review", &["pass:commit+touch=src/b.rs", "pass"]);
        s.queue("review-fix", &["pass:commit+next=review"]);
        s.run()
    };
    let r = run("error: src/b.rs:3:1: mismatched types");
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        &r.calls()[..5],
        ["triage", "build", "verify", "review", "review-fix"]
    );
    let r = run("error: src/c.rs:3:1: mismatched types");
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert!(
        r.reason.starts_with("build is out of attempts (1)"),
        "{r:?}"
    );
}
