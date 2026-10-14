use super::super::locks::tests::MIN;
use super::*;
use crate::clock::Clock;

#[test]
fn a_budget_spent_during_a_lock_wait_stops_the_phase_and_frees_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let names = vec!["a".to_string()];
    let acquire = || {
        RunnerLocks::acquire(
            dir.path(),
            &names,
            "u",
            &Clock::pinned(1000),
            MIN,
            None,
            |_, _| {},
        )
        .unwrap()
        .held()
    };
    let held = recheck_budget(acquire(), Some(5.0), 4.0, "build");
    drop(held.ok().expect("under budget"));
    let Err(f) = recheck_budget(acquire(), Some(5.0), 5.0, "build") else {
        panic!("a spent budget kept the locks")
    };
    assert!(matches!(f.outcome, Outcome::Budget));
    assert_eq!(f.reason, "spent $5.00 of the $5.00 budget");
    assert_eq!(f.phase.as_deref(), Some("build"));
    acquire();
}

#[test]
fn a_spent_budget_names_the_spend_and_the_limit() {
    assert_eq!(budget_spent(None, 99.0), None);
    assert_eq!(budget_spent(Some(5.0), 4.99), None);
    assert_eq!(
        budget_spent(Some(5.0), 5.0).as_deref(),
        Some("spent $5.00 of the $5.00 budget")
    );
}
