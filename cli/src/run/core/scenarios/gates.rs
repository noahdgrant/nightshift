//! Where a gate verdict sends the unit: moved from `cli/tests/factory/run_rebases_and_gates.rs`
//! and `run_ratchets_and_worktrees.rs`, where every assertion was a decision. The sim pops one
//! verdict for each gate the core asks for and reuses none, so a test whose e2e version relies
//! on the shell reusing a verdict at the same HEAD stays end to end.

use std::collections::VecDeque;

use super::super::sim::Sim;
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
}
