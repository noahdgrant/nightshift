//! Parallel units: claims into free slots, never twice, under `--max-units`, and a usage limit's
//! pause across every unit, and only team issues. Moved from tests/factory/cleanup_and_cpus.rs
//! and tests/factory/watch_nights_and_splits.rs.

use serde_json::json;

use super::{by_issue, each, Sim, READY};
use crate::watch::Issue;

/// A night with #2 and #3 ready, `parallel` units at once.
fn two_ready(parallel: usize) -> Sim {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.ready(3, "Fix b", &["type:fix"]);
    sim.parallel = parallel;
    sim
}

fn done(_: &Issue, _: &mut Sim) -> serde_json::Value {
    json!({"outcome": "done"})
}

#[test]
fn watch_parallel_never_takes_an_issue_twice() {
    let mut sim = two_ready(2);
    // GitHub keeps listing a claimed issue as ready for a while.
    sim.forge.live = false;
    sim.fill().unwrap();
    // Unit 2 ended and its slot refills from a queue that still lists #3, mid build.
    sim.end(2, json!({"outcome": "done"})).unwrap();
    sim.fill().unwrap();
    sim.run(done).unwrap();
    assert_eq!(by_issue(&sim.units), each(&[2, 3], "done"));
    assert_eq!(sim.claimed, [2, 3]);
}

#[test]
fn watch_parallel_once_and_max_units_cap_the_units_started() {
    let mut sim = two_ready(3);
    sim.ready(4, "Fix c", &["type:fix"]);
    sim.max_units = Some(2);
    let stopped = sim.run(done).unwrap();
    assert_eq!(by_issue(&sim.units), each(&[2, 3], "done"));
    assert_eq!(stopped, "max_units");
    assert_eq!(sim.labels(4), ["type:fix", READY]);
    // The next night, `--once`, on the same tracker.
    let mut next = Sim::new();
    next.forge = std::mem::take(&mut sim.forge);
    next.parallel = 3;
    next.max_units = Some(1);
    next.run(done).unwrap();
    assert_eq!(by_issue(&next.units), each(&[4], "done"));
}

#[test]
fn watch_parallel_resumes_at_the_later_of_two_resets() {
    let mut sim = two_ready(2);
    sim.deadline = Some(sim.now + 10 * 3600);
    // The later reset comes first: a pause must not shrink to the earlier one that follows.
    let (later, earlier) = (sim.now + 7200, sim.now + 3600);
    sim.fill().unwrap();
    sim.end(2, json!({"outcome": "paused", "reset_at": later}))
        .unwrap();
    sim.end(3, json!({"outcome": "paused", "reset_at": earlier}))
        .unwrap();
    assert_eq!(sim.resets, [(2, later), (3, earlier)]);
    assert_eq!(sim.paused_until, Some(later));
    sim.run(done).unwrap();
    assert_eq!(sim.now, later);
    assert_eq!(by_issue(&sim.units[2..]), each(&[2, 3], "done"));
}

#[test]
fn watch_parallel_gives_every_paused_unit_back_when_the_reset_is_past_until() {
    let mut sim = two_ready(2);
    sim.deadline = Some(sim.now + 3600);
    let reset = sim.now + 8 * 3600;
    sim.fill().unwrap();
    sim.end(2, json!({"outcome": "paused", "reset_at": reset}))
        .unwrap();
    // Unit 3 pauses at its next phase, held by the night's pause.
    let held = json!({"outcome": "paused", "reset_at": reset, "held": true});
    let stopped = sim.run(|_, _| held.clone()).unwrap();
    assert_eq!(stopped, "usage limit resets after --until");
    assert_eq!(by_issue(&sim.units), each(&[2, 3], "paused"));
    for n in [2, 3] {
        assert_eq!(sim.labels(n), ["type:fix", READY]);
    }
}

#[test]
fn watch_never_runs_a_non_team_issue() {
    let mut sim = Sim::new();
    sim.ready(2, "Stranger", &["type:fix"]);
    sim.forge.issues.last_mut().unwrap().team = false;
    let stopped = sim.run(done).unwrap();
    assert_eq!(stopped, "queue empty");
    assert!(sim.units.is_empty(), "{:?}", sim.units);
    assert!(sim.claimed.is_empty());
    assert_eq!(sim.labels(2), ["type:fix", READY]);
}
