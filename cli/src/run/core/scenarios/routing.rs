//! Which phase runs next, and what it costs: moved from `cli/tests/factory/run_phases_and_skills.rs`,
//! `run_rebases_and_gates.rs` and `run_ratchets_and_worktrees.rs`, where every assertion was a
//! decision.

use super::super::sim::Sim;
use crate::run::Outcome;

/// Build 1 commits and another actor moves main; verify gets five attempts, so only build's cap
/// can end the unit. The e2e `base_moves_during_build`, whose `pass:script` is `pass:commit+movemain`.
fn base_moves_during_build() -> Sim {
    let mut s = Sim::new();
    s.max_attempts("verify", 5);
    s
}

#[test]
fn verify_fail_goes_back_to_build_with_feedback() {
    let mut s = Sim::new();
    s.queue("build", &["pass:commit", "pass:commit"]);
    s.queue("verify", &["fail", "pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done);
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "build", "verify", "review", "ship"]
    );
    let b = &r.runs[3];
    assert_eq!(b.attempt, 2);
    assert!(b.feedback.contains("verify said fail"), "{}", b.feedback);
}

#[test]
fn a_ship_no_go_for_base_movement_goes_to_build_at_no_attempts_cost() {
    let mut s = base_moves_during_build();
    s.max_attempts("ship", 5);
    s.queue(
        "build",
        &["pass:commit+movemain", "pass:commit", "pass:commit"],
    );
    s.queue("verify", &["pass", "fail", "pass"]);
    s.queue("ship", &["fail:base moved", "pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        &r.calls()[..6],
        ["triage", "build", "verify", "review", "ship", "build"]
    );
    let rb = r.rebases();
    assert_eq!(rb.len(), 1, "{rb:?}");
    assert_eq!(rb[0].why, "pr.md failed: base moved");
}

#[test]
fn the_ship_after_a_free_rebase_build_is_not_free() {
    let mut s = base_moves_during_build();
    s.queue("build", &["pass:commit+movemain", "pass:commit"]);
    s.queue("verify", &["fail:base moved", "pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "build", "verify", "review", "ship"]
    );
    assert!(!r.logged("ship_after_base_moved"));
    assert!(!r.runs.iter().any(|run| run.follows_base));
}

#[test]
fn a_free_rebase_build_that_fails_leaves_the_unit_stuck_at_the_normal_cap() {
    let mut s = base_moves_during_build();
    s.queue("build", &["pass:commit+movemain", "fail", "pass:commit"]);
    s.queue("verify", &["fail:base moved"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert_eq!(r.reason, "build is out of attempts (2)", "{r:?}");
    assert_eq!(r.phase, Some("build"));
    assert_eq!(r.calls(), ["triage", "build", "verify", "build"]);
}

#[test]
fn a_review_no_go_for_base_movement_goes_to_build_at_no_attempts_cost() {
    let mut s = base_moves_during_build();
    s.max_attempts("review", 5);
    s.queue(
        "build",
        &["pass:commit+movemain", "pass:commit", "pass:commit"],
    );
    s.queue("verify", &["pass", "fail", "pass"]);
    s.queue("review", &["fail:base moved", "pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        &r.calls()[..6],
        ["triage", "build", "verify", "review", "build", "verify"]
    );
    let rb = r.rebases();
    assert_eq!(rb.len(), 1, "{rb:?}");
    assert_eq!(rb[0].why, "review.md failed");
}

#[test]
fn a_free_rebase_build_that_times_out_leaves_the_unit_stuck_at_the_normal_cap() {
    let mut s = base_moves_during_build();
    s.queue(
        "build",
        &["pass:commit+movemain", "blocked:sleep", "pass:commit"],
    );
    s.queue("verify", &["fail:base moved"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert!(
        r.reason.starts_with("build is out of attempts (2)"),
        "{r:?}"
    );
    assert_eq!(r.calls(), ["triage", "build", "verify", "build"]);
}

#[test]
fn a_free_rebase_build_that_writes_no_artifact_leaves_the_unit_stuck_at_the_normal_cap() {
    let mut s = base_moves_during_build();
    s.queue("build", &["pass:commit+movemain", "none", "pass:commit"]);
    s.queue("verify", &["fail:base moved"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert!(
        r.reason.starts_with("build is out of attempts (2)"),
        "{r:?}"
    );
    assert_eq!(r.calls(), ["triage", "build", "verify", "build"]);
}

#[test]
fn a_build_that_fails_saying_base_moved_still_spends_an_attempt() {
    let mut s = Sim::new();
    s.queue("build", &["fail:base moved", "fail:base moved"]);
    let r = s.run();
    assert_eq!(r.reason, "build is out of attempts (2)", "{r:?}");
    assert_eq!(r.calls(), ["triage", "build", "build"]);
    assert!(r.rebases().is_empty());
}

#[test]
fn without_an_origin_a_base_moved_no_go_spends_an_attempt() {
    let mut s = Sim::new();
    s.remote = None;
    s.queue("build", &["pass:commit", "pass:commit"]);
    s.queue("verify", &["fail:base moved", "fail"]);
    let r = s.run();
    assert!(r.reason.contains("out of attempts"), "{r:?}");
    assert!(r.rebases().is_empty());
}

#[test]
fn attempt_limit_gives_stuck() {
    let mut s = Sim::new();
    s.queue("build", &["pass:commit", "pass:commit", "pass:commit"]);
    s.queue("verify", &["fail", "fail"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert!(r.reason.contains("out of attempts"), "{r:?}");
    assert_eq!(r.calls(), ["triage", "build", "verify", "build", "verify"]);
}

#[test]
fn triage_without_brief_is_stuck() {
    let mut s = Sim::new();
    s.queue("triage", &["none"]);
    let r = s.run();
    assert!(r.reason.contains("no brief"), "{r:?}");
    assert_eq!(r.calls(), ["triage"]);
}

#[test]
fn review_commit_makes_evidence_stale_and_reverifies() {
    let mut s = Sim::new();
    s.queue("build", &["pass:commit"]);
    s.queue("review", &["pass:commit"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done);
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "review", "verify", "ship"]
    );
}

#[test]
fn blocked_artifact_is_stuck() {
    let mut s = Sim::new();
    s.queue("build", &["pass:commit"]);
    s.queue("verify", &["blocked:needs hardware"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert!(r.reason.contains("evidence.md is blocked"), "{}", r.reason);
    assert_eq!(r.artifact, Some("evidence.md"));
}
