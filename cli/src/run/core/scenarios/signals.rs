//! Phases and gates killed by a signal: moved from `cli/tests/factory/run_signals_and_cleanup.rs`,
//! where every assertion was a decision.

use super::super::sim::Sim;
use crate::run::Outcome;

#[test]
fn a_phase_killed_on_every_attempt_is_stuck_naming_the_signal() {
    let mut s = Sim::new();
    s.max_attempts("build", 1);
    s.queue("build", &["pass:commit+kill"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Stuck);
    assert_eq!(
        r.reason,
        "build is out of attempts (1): the last attempt was killed by SIGKILL"
    );
}

#[test]
fn a_killed_rebuild_keeps_the_red_gate_feedback() {
    let mut s = Sim::new();
    s.max_attempts("build", 3);
    s.gate(&[Err("boom")]);
    s.queue("build", &["pass:commit", "pass:commit+kill", "pass:commit"]);
    let r = s.run();
    assert_eq!(r.outcome, Outcome::Done, "{r:?}");
    let b = &r.runs[3];
    assert_eq!((b.phase, b.attempt), ("build", 3));
    assert_eq!(b.why, "was killed by SIGKILL with the CI gate still red");
    assert!(b.feedback.contains("boom"), "{}", b.feedback);
}

#[test]
fn a_gate_still_red_after_a_silent_build_still_names_the_signal() {
    let mut s = Sim::new();
    s.gate(&[Err("was killed by SIGKILL"); 2]);
    s.queue("build", &["pass:commit", "none"]);
    let r = s.run();
    assert_eq!(
        r.reason,
        "build is out of attempts (2): the CI gate was killed by SIGKILL"
    );
    assert_eq!(
        r.runs[2].why,
        "the CI gate was killed by SIGKILL after build"
    );
    assert_eq!(r.gates.len(), 1, "{:?}", r.gates);
}
