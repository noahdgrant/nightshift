//! Which gates run and where their verdicts send the unit: moved from
//! `cli/tests/factory/run_rebases_and_gates.rs` and `run_ratchets_and_worktrees.rs`, where every
//! assertion was a decision. The sim pops one verdict for each gate it runs, and reuses a verdict
//! at the same HEAD as `gate::Gate::after` does.

use std::collections::VecDeque;

use super::super::sim::Sim;
use crate::factory::LocalGate;
use crate::run::Outcome;

/// How many verdicts [`gates`] queues; the ones past those given are green.
const QUEUED: usize = 100;

/// The gate verdicts in order: `""` is green, any other text a red gate's output. Every gate
/// after them is green.
fn gates(verdicts: &[&str]) -> VecDeque<Result<(), String>> {
    let mut q: VecDeque<_> = verdicts
        .iter()
        .map(|v| {
            if v.is_empty() {
                Ok(())
            } else {
                Err(v.to_string())
            }
        })
        .collect();
    q.resize(QUEUED, Ok(()));
    q
}

/// A gate that is always red with `output`.
fn always_red(output: &str) -> VecDeque<Result<(), String>> {
    vec![Err(output.to_string()); QUEUED].into()
}

/// How many gates the core asked for: the e2e tests' `gate_events(&e).len()`.
fn gates_run(s: &Sim) -> usize {
    QUEUED - s.gates.as_ref().map_or(QUEUED, VecDeque::len)
}

/// Build 1 commits and another actor moves main; verify gets five attempts. The same helper as
/// `routing.rs`'s.
fn base_moves_during_build() -> Sim {
    let mut s = Sim::new();
    s.max_attempts("verify", 5);
    s
}

#[test]
fn a_red_gate_goes_back_to_build_with_its_output_tail() {
    let mut s = Sim::new();
    s.gates = Some(gates(&["boom from the gate", ""]));
    s.queue("build", &["pass:commit", "pass:commit"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done);
    assert_eq!(
        r.calls(),
        ["triage", "build", "build", "verify", "review", "ship"]
    );
    let b = &r.runs[2];
    assert_eq!(b.attempt, 2);
    assert!(b.feedback.contains("boom from the gate"), "{}", b.feedback);
    assert_eq!(gates_run(&s), 2);
}

#[test]
fn a_gate_red_until_attempts_run_out_is_stuck() {
    let mut s = Sim::new();
    s.gates = Some(always_red("exit 1"));
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert_eq!(r.reason, "build is out of attempts (2)");
    assert_eq!(r.calls(), ["triage", "build", "build"]);
    assert_eq!(gates_run(&s), 2);
}

/// In the e2e version the gates after the rebase builds move main; here the rebase build moves
/// it, which the gate after it sees the same way.
#[test]
fn a_red_gate_after_a_rebase_with_main_moved_since_rebases_once_more() {
    let mut s = base_moves_during_build();
    s.gates = Some(gates(&["", "boom", "boom"]));
    s.queue(
        "build",
        &[
            "pass:commit+movemain",
            "pass:commit+movemain",
            "pass:commit",
            "pass:commit",
        ],
    );
    s.queue("verify", &["fail:base moved", "pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "build", "build", "build", "verify", "review", "ship"]
    );
    let rb = r.rebases();
    assert_eq!(rb.len(), 2, "one extra rebase, never a second: {rb:?}");
    assert_eq!(rb[1].attempt, 3);
    assert_eq!(
        rb[1].why,
        "the CI gate failed after a rebase and main moved since"
    );
    let p = &r.runs[4].feedback;
    assert!(p.contains("boom"), "{p}");
    assert!(p.contains("rebase onto the newest origin/main"), "{p}");
    // The third red spent build's second attempt.
    assert!(!r.runs[5].feedback.contains("rebase onto the newest"));
}

#[test]
fn a_red_gate_after_a_rebase_with_main_still_spends_an_attempt() {
    let mut s = base_moves_during_build();
    s.gates = Some(gates(&["", "boom"]));
    s.queue(
        "build",
        &["pass:commit+movemain", "pass:commit", "pass:commit"],
    );
    s.queue("verify", &["fail:base moved", "fail"]);
    let r = s.run();
    assert_eq!(r.reason, "build is out of attempts (2)", "{r:?}");
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "build", "build", "verify"]
    );
    assert_eq!(r.rebases().len(), 1);
}

#[test]
fn a_red_gate_with_a_silent_rebuild_never_reaches_verify() {
    let mut s = Sim::new();
    s.gates = Some(always_red("exit 1"));
    s.queue("build", &["pass:commit", "none"]);
    let r = s.run();
    assert_eq!(r.reason, "build is out of attempts (2)");
    assert_eq!(r.calls(), ["triage", "build", "build"]);
}

#[test]
fn a_red_gate_with_a_committing_rebuild_that_writes_no_artifact_never_reaches_verify() {
    let mut s = Sim::new();
    s.max_attempts("build", 3);
    s.gates = Some(always_red("exit 1"));
    s.queue("build", &["pass:commit", "none:commit", "none:commit"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert!(!r.calls().contains(&"verify"), "{:?}", r.calls());
    assert_eq!(
        gates_run(&s),
        3,
        "a commit since the red verdict runs the gate again"
    );
}

#[test]
fn a_rebuild_that_leaves_head_alone_skips_a_green_gate() {
    let mut s = Sim::new();
    s.gate(&[]);
    s.queue("build", &["pass:commit", "pass"]);
    s.queue("verify", &["fail", "pass"]);
    let r = s.run();
    assert_eq!(&r.calls()[..4], ["triage", "build", "verify", "build"]);
    assert_eq!(r.gates.len(), 1, "{:?}", r.gates);
}

#[test]
fn a_red_gate_is_not_skipped_when_the_rebuild_writes_no_artifact() {
    let mut s = Sim::new();
    s.max_attempts("build", 3);
    s.gate(&[Err("boom")]);
    s.queue("build", &["pass:commit", "none", "pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done);
    assert_eq!(
        r.calls(),
        ["triage", "build", "build", "build", "verify", "review", "ship"]
    );
    assert!(
        r.runs[3].feedback.contains("boom"),
        "{}",
        r.runs[3].feedback
    );
    assert_eq!(r.runs[2].why, "the CI gate failed after build");
    assert_eq!(r.runs[3].why, "the CI gate is still red");
    assert_eq!(r.gated(), [("build", false), ("build", true)]);
}

#[test]
fn a_build_that_did_not_pass_runs_no_gate() {
    let mut s = Sim::new();
    s.max_attempts("build", 5);
    s.gate(&[]);
    s.queue("build", &["none", "fail", "blocked:no"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert_eq!(r.calls(), ["triage", "build", "build", "build"]);
    assert!(r.gates.is_empty(), "{:?}", r.gates);
}

#[test]
fn a_review_that_commits_runs_the_gate_again() {
    let mut s = Sim::new();
    s.gate(&[]);
    s.queue("build", &["pass:commit"]);
    s.queue("review", &["pass:commit"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done);
    assert_eq!(
        r.calls(),
        ["triage", "build", "verify", "review", "verify", "ship"]
    );
    assert_eq!(r.gated(), [("build", true), ("review", true)]);
    assert!(r.gates.iter().all(|g| g.full), "{:?}", r.gates);
}

/// Under `build-only` a review commit runs the fast gate, and its green does not cover the full
/// gate a rebuild at the same HEAD runs.
#[test]
fn under_build_only_the_fast_gate_after_a_review_commit_leaves_the_full_one_to_run() {
    let mut s = Sim::new();
    s.settings.local_gate = LocalGate::BuildOnly;
    s.gate(&[]);
    s.queue("build", &["pass:commit", "pass"]);
    s.queue("review", &["pass:commit", "pass"]);
    s.queue("verify", &["pass", "fail", "pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    let g: Vec<_> = r.gates.iter().map(|g| (g.phase, g.full)).collect();
    assert_eq!(g, [("build", true), ("review", false), ("build", true)]);
}

/// What a gate prints when it fails on the file review's commit changed.
const ON_REVIEWS_FILE: &str = "error: src/b.rs:3:1: mismatched types";

/// A red gate on a file review's commit changed runs a fix cycle with the gate's output, then
/// the loop's next review pass, and never build.
#[test]
fn a_red_gate_on_a_review_commit_runs_a_fix_cycle_then_a_review_pass() {
    let mut s = Sim::new();
    s.gate(&[Ok(()), Err(ON_REVIEWS_FILE)]);
    s.queue("build", &["pass:commit+touch=src/a.rs"]);
    s.queue("review", &["pass:commit+touch=src/b.rs", "pass"]);
    s.queue("review-fix", &["pass:commit+touch=src/b.rs+next=review"]);
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
    assert!(fix.feedback.contains(ON_REVIEWS_FILE), "{}", fix.feedback);
    assert_eq!(
        fix.why,
        "the CI gate failed after review: fix cycle 1 (full)"
    );
    assert_eq!(r.runs[6].why, "fix cycle 1 ran: review pass 1");
    assert_eq!(
        r.gated(),
        [("build", true), ("review", false), ("review-fix", true)]
    );
}

/// A gate-driven fix cycle cut short is retried as the same cycle, with the gate's output.
#[test]
fn a_gate_driven_fix_cycle_cut_short_is_retried_with_the_gate_s_output() {
    for cut in ["pass:sleep", "pass:kill"] {
        let mut s = Sim::new();
        s.gate(&[Ok(()), Err(ON_REVIEWS_FILE)]);
        s.queue("build", &["pass:commit+touch=src/a.rs"]);
        s.queue("review", &["pass:commit+touch=src/b.rs", "pass"]);
        s.queue(
            "review-fix",
            &[cut, "pass:commit+touch=src/b.rs+next=review"],
        );
        let r = s.run();
        assert_eq!(r.outcome, Outcome::Done, "{cut}: {r:?}");
        assert_eq!(
            r.calls(),
            [
                "triage",
                "build",
                "verify",
                "review",
                "review-fix",
                "review-fix",
                "verify",
                "review",
                "ship"
            ],
            "{cut}"
        );
        let retry = &r.runs[5];
        assert!(
            retry.feedback.contains(ON_REVIEWS_FILE),
            "{}",
            retry.feedback
        );
        assert!(retry.why.ends_with("fix cycle 1 (full)"), "{}", retry.why);
    }
}

/// A gate that stays red on review's file runs fix cycles until the loop's limit, then stops the
/// unit as a spent review loop does.
#[test]
fn a_gate_red_on_review_commits_until_the_cycles_run_out_spends_the_loop() {
    let mut s = Sim::new();
    let mut q = always_red(ON_REVIEWS_FILE);
    q[0] = Ok(());
    s.gates = Some(q);
    s.queue("build", &["pass:commit+touch=src/a.rs"]);
    s.queue("review", &["pass:commit+touch=src/b.rs"]);
    s.queue("review-fix", &["pass:commit+touch=src/b.rs+next=review"; 6]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck, "{r:?}");
    assert_eq!(
        r.reason,
        "the CI gate failed after review-fix, and the review loop has used its 5 fix cycles \
         (limit 3 full, 2 fix-only)"
    );
    assert_eq!(r.artifact, Some("review.md"));
    let calls = r.calls();
    assert_eq!(
        calls.iter().filter(|p| **p == "review-fix").count(),
        5,
        "{calls:?}"
    );
    assert!(!calls[2..].contains(&"build"), "{calls:?}");
}

#[test]
fn a_timed_out_review_that_moved_head_still_runs_the_gate() {
    let mut s = Sim::new();
    s.gate(&[]);
    s.queue("build", &["pass:commit"]);
    s.queue("review", &["blocked:commit+sleep", "pass"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    assert_eq!(r.gates.len(), 2, "{:?}", r.gates);
    assert_eq!((r.gates[1].phase, r.gates[1].attempt), ("review", 1));
}

/// A `phase` that fails after a commit still runs the gate after it.
fn fails_after_moving_head(phase: &'static str) {
    let mut s = Sim::new();
    s.gate(&[]);
    s.queue("build", &["pass:commit"]);
    s.queue(phase, &["fail:commit"]);
    let r = s.run();
    assert_eq!(r.gates.len(), 2, "{:?}", r.gates);
    assert_eq!(r.gates[1].phase, phase);
}

#[test]
fn a_review_that_fails_after_moving_head_still_runs_the_gate() {
    fails_after_moving_head("review");
}

#[test]
fn a_verify_that_fails_after_moving_head_still_runs_the_gate() {
    fails_after_moving_head("verify");
}

#[test]
fn a_ship_that_fails_after_moving_head_still_runs_the_gate() {
    fails_after_moving_head("ship");
}

/// A gate that fails its ratchets on every attempt leaves them as the reason.
#[test]
fn a_gate_red_on_its_ratchets_until_attempts_run_out_names_them() {
    let mut s = Sim::new();
    s.gates = Some(always_red("ratchets: flash 104 > 100"));
    let r = s.run();
    assert_eq!(
        r.reason,
        "build is out of attempts (2): the CI gate failed on flash 104 > 100"
    );
    assert_eq!(
        r.runs[2].why,
        "the CI gate failed on flash 104 > 100 after build"
    );
}
