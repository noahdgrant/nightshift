//! The review loop `ns run` drives (#560): each review pass and each fix cycle is its own phase
//! run, a pass that leaves findings open (`next: review`) starts a fix cycle, and the cycle
//! limits end the loop stuck for a human.

use super::super::sim::Sim;
use crate::factory::Cycles;
use crate::run::Outcome;

/// A pass that leaves `ids` open for a fix cycle.
fn open(ids: &str) -> String {
    format!("fail:next=review+find={ids}")
}

#[test]
fn a_review_with_open_importants_runs_a_fix_cycle_then_a_second_pass() {
    let mut s = Sim::new();
    s.queue("review", &[&open("I1,I2"), "pass"]);
    s.queue("review-fix", &["pass:commit+next=review"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.calls(),
        [
            "triage",
            "build",
            "verify",
            "review",
            "review-fix",
            "verify",
            "review",
            "ship"
        ]
    );
    let fix = &r.runs[4];
    assert_eq!(fix.why, "review.md has findings open: fix cycle 1 (full)");
    assert!(
        fix.feedback.contains("Fix cycle 1 (full)"),
        "{}",
        fix.feedback
    );
    assert!(fix.feedback.contains("I1, I2"), "{}", fix.feedback);
    let pass = &r.runs[6];
    assert_eq!(pass.why, "fix cycle 1 ran: review pass 1");
    assert!(
        pass.feedback
            .contains("Review pass 1, after fix cycle 1 (full)"),
        "{}",
        pass.feedback
    );
    assert!(
        pass.feedback.contains("handed (I1, I2)"),
        "{}",
        pass.feedback
    );
}

#[test]
fn a_review_pass_that_finds_only_suggestions_goes_to_ship() {
    let mut s = Sim::new();
    s.queue("review", &["pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done);
    assert!(!r.calls().contains(&"review-fix"), "{:?}", r.calls());
}

#[test]
fn a_fix_cycle_with_no_commits_still_runs_the_next_pass_once() {
    let mut s = Sim::new();
    s.queue("review", &[&open("I1"), "pass"]);
    s.queue("review-fix", &["pass:next=review"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.calls(),
        [
            "triage",
            "build",
            "verify",
            "review",
            "review-fix",
            "review",
            "ship"
        ]
    );
}

#[test]
fn the_cycle_limit_ends_the_loop_stuck_with_the_open_findings() {
    let mut s = Sim::new();
    let i1 = open("I1");
    s.queue("review", &[i1.as_str(); 6]);
    s.queue("review-fix", &["pass:commit+next=review"; 6]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(
        r.reason,
        "review.md has findings open after 3 fix cycles (limit 3 full, 2 fix-only): I1"
    );
    let calls = r.calls();
    assert_eq!(
        calls.iter().filter(|p| **p == "review-fix").count(),
        3,
        "{calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|p| **p == "review").count(),
        4,
        "{calls:?}"
    );
}

#[test]
fn fix_only_cycles_hand_over_only_findings_with_fewer_than_two_attempts() {
    let mut s = Sim::new();
    let (i1, late) = (open("I1"), open("I1,I2"));
    s.queue("review", &[&i1, &i1, &i1, &late, &late, &late]);
    s.queue("review-fix", &["pass:commit+next=review"; 6]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(
        r.reason,
        "review.md has findings open after 5 fix cycles (limit 3 full, 2 fix-only): I1, I2"
    );
    let fixes: Vec<_> = r.runs.iter().filter(|x| x.phase == "review-fix").collect();
    assert_eq!(fixes.len(), 5);
    for (f, n) in fixes[3..].iter().zip([4, 5]) {
        assert_eq!(
            f.why,
            format!("review.md has findings open: fix cycle {n} (fix-only)")
        );
        assert!(
            f.feedback.contains("findings from review.md: I2."),
            "{}",
            f.feedback
        );
    }
    let last_pass = r.runs.iter().rfind(|x| x.phase == "review").unwrap();
    assert!(
        last_pass.feedback.contains("This pass is fix-only"),
        "{}",
        last_pass.feedback
    );
}

#[test]
fn the_cycle_limits_come_from_the_settings() {
    let mut s = Sim::new();
    s.settings.cycles = Cycles {
        full: 1,
        fix_only: 0,
    };
    s.queue("review", &[&open("C1"), &open("C1")]);
    s.queue("review-fix", &["pass:commit+next=review"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(
        r.reason,
        "review.md has findings open after 1 fix cycles (limit 1 full, 0 fix-only): C1"
    );
}

#[test]
fn review_passes_and_fix_cycles_spend_no_attempts_but_a_timed_out_fix_retried_does() {
    let mut s = Sim::new();
    s.queue("review", &[&open("I1"), &open("I1"), "pass"]);
    s.queue(
        "review-fix",
        &[
            "pass:commit+next=review",
            "none:sleep",
            "pass:commit+next=review",
        ],
    );
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    let fixes: Vec<u32> = r
        .runs
        .iter()
        .filter(|x| x.phase == "review-fix")
        .map(|x| x.attempt)
        .collect();
    assert_eq!(fixes, [1, 2, 3]);

    let mut s = Sim::new();
    s.queue("review", &[&open("I1")]);
    s.queue("review-fix", &["none:sleep"; 3]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(r.phase, Some("review-fix"));
    assert!(
        r.reason.starts_with("review-fix is out of attempts (2)"),
        "{}",
        r.reason
    );
    let fixes = r.calls().iter().filter(|p| **p == "review-fix").count();
    assert_eq!(
        fixes,
        3,
        "the first fix after a pass is free: {:?}",
        r.calls()
    );
}

#[test]
fn a_rerun_resumes_mid_loop_from_the_artifacts_and_the_record() {
    let mut s = Sim::new();
    s.max_attempts("verify", 2);
    s.queue("review", &[&open("I1"), "pass"]);
    s.queue("review-fix", &["pass:commit+next=review"]);
    s.queue("verify", &["pass", "crash", "crash"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(r.calls().last(), Some(&"verify"));
    s.queue("verify", &["pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(r.calls(), ["verify", "review", "ship"]);
    assert_eq!(r.runs[1].why, "fix cycle 1 ran: review pass 1");
}

#[test]
fn a_rerun_at_the_cycle_limit_stays_stuck_without_running_a_phase() {
    let mut s = Sim::new();
    s.settings.cycles = Cycles {
        full: 1,
        fix_only: 0,
    };
    s.queue("review", &[&open("I1"), &open("I1")]);
    s.queue("review-fix", &["pass:commit+next=review"]);
    assert_eq!(s.run().outcome, Outcome::Stuck);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert!(r.calls().is_empty(), "{:?}", r.calls());
}

#[test]
fn a_fix_cycle_that_names_build_or_a_human_goes_there() {
    let mut s = Sim::new();
    s.queue("review", &[&open("I1"), "pass"]);
    s.queue("review-fix", &["fail:next=build"]);
    s.queue("build", &["pass:commit", "pass:commit"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.calls(),
        [
            "triage",
            "build",
            "verify",
            "review",
            "review-fix",
            "build",
            "verify",
            "review",
            "ship"
        ]
    );

    let mut s = Sim::new();
    s.queue("review", &[&open("I1")]);
    s.queue("review-fix", &["blocked:next=human"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert!(r.reason.starts_with("fix.md is blocked"), "{}", r.reason);
}

#[test]
fn a_fix_cycle_cut_short_after_it_committed_is_retried_as_the_same_cycle() {
    let mut s = Sim::new();
    s.queue("review", &[&open("I1"), "pass"]);
    s.queue(
        "review-fix",
        &["pass:commit+sleep", "pass:commit+next=review"],
    );
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.calls(),
        [
            "triage",
            "build",
            "verify",
            "review",
            "review-fix",
            "verify",
            "review-fix",
            "verify",
            "review",
            "ship"
        ]
    );
    let fixes: Vec<_> = r.runs.iter().filter(|x| x.phase == "review-fix").collect();
    assert_eq!(
        fixes[1].why,
        "review.md has findings open: fix cycle 1 (full)"
    );
    assert_eq!(r.runs[8].why, "fix cycle 1 ran: review pass 1");
}

#[test]
fn a_finding_with_two_fix_attempts_is_not_handed_to_a_fix_only_cycle() {
    let mut s = Sim::new();
    let (i1, both) = (open("I1"), open("I1,I2"));
    s.queue("review", &[&i1, &both, &both, &both]);
    s.queue("review-fix", &["pass:commit+next=review"; 4]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(
        r.reason,
        "review.md has findings open after 3 fix cycles (limit 3 full, 2 fix-only): I1, I2"
    );
}

#[test]
fn a_fix_cycle_with_no_ids_to_hand_over_fixes_what_review_md_left_open() {
    let mut s = Sim::new();
    s.queue("review", &["fail:next=review", "pass"]);
    s.queue("review-fix", &["pass:commit+next=review"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    let fix = r.runs.iter().find(|x| x.phase == "review-fix").unwrap();
    assert!(
        fix.feedback
            .contains("the open Critical and Important findings in changed code"),
        "{}",
        fix.feedback
    );
}

#[test]
fn a_failed_fix_cycle_that_names_a_human_stops_the_unit() {
    let mut s = Sim::new();
    s.queue("review", &[&open("I1")]);
    s.queue("review-fix", &["fail:next=human"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(
        r.reason,
        "fix.md needs a human: review-fix said fail next=human"
    );
}

#[test]
fn a_passing_review_goes_to_ship_whatever_its_next_says() {
    let mut s = Sim::new();
    s.queue("review", &["pass:next=review"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert!(!r.calls().contains(&"review-fix"), "{:?}", r.calls());
}
