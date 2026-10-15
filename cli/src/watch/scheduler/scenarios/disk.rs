//! Free disk space: a slot waits while space is under `[limits] disk_wait_gb` and the night
//! stops under `disk_stop_gb` (docs/factory/watch.md, "Disk space").

use std::cell::Cell;
use std::rc::Rc;

use serde_json::json;

use super::{Sim, READY};
use crate::disk::GIB;
use crate::watch::core::fill::{DiskAct, Stopping};

/// A night with #2 and #3 ready, its worktree filesystem and the host drive reporting the GiB
/// `host` holds, the worktree one 500.
fn night(host: &Rc<Cell<u64>>) -> Sim {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.ready(3, "Fix b", &["type:fix"]);
    let host = Rc::clone(host);
    sim.free = Box::new(move || {
        vec![
            ("/w".into(), Ok(500 * GIB)),
            ("/mnt/c".into(), Ok(host.get() * GIB)),
        ]
    });
    sim
}

#[test]
fn watch_starts_a_unit_with_space_above_the_wait_limit() {
    let host = Rc::new(Cell::new(20));
    let mut sim = night(&host);
    sim.fill().unwrap();
    assert_eq!(sim.claimed, [2]);
    assert!(sim.disk_said.is_empty(), "{:?}", sim.disk_said);
}

#[test]
fn watch_waits_between_the_limits_and_starts_once_space_returns() {
    let host = Rc::new(Cell::new(15));
    let mut sim = night(&host);
    sim.fill().unwrap();
    sim.fill().unwrap();
    assert!(sim.claimed.is_empty());
    assert_eq!(sim.stopping, None);
    let low = DiskAct::Low {
        path: "/mnt/c".into(),
        free_gb: 15,
    };
    assert_eq!(sim.disk_said, [low, DiskAct::Wait]);
    assert_eq!(sim.labels(2), ["type:fix", READY]);
    host.set(40);
    sim.fill().unwrap();
    assert_eq!(sim.claimed, [2]);
}

#[test]
fn watch_stops_the_night_under_the_stop_limit() {
    let host = Rc::new(Cell::new(9));
    let mut sim = night(&host);
    let stopped = sim.run(|_, _| json!({"outcome": "done"})).unwrap();
    assert_eq!(stopped, "disk_critical");
    assert!(matches!(sim.stopping, Some(Stopping::All(_))));
    let critical = DiskAct::Critical {
        path: "/mnt/c".into(),
        free_gb: 9,
    };
    assert_eq!(sim.disk_said, [critical]);
    assert!(sim.claimed.is_empty());
    assert_eq!(sim.labels(2), ["type:fix", READY]);
}

#[test]
fn watch_stops_a_running_night_when_space_runs_out_between_units() {
    let host = Rc::new(Cell::new(100));
    let mut sim = night(&host);
    let stopped = sim
        .run(|_, _| {
            host.set(5);
            json!({"outcome": "done"})
        })
        .unwrap();
    assert_eq!(stopped, "disk_critical");
    assert_eq!(sim.claimed, [2]);
}

#[test]
fn watch_warns_once_of_a_free_space_it_cannot_read_and_starts_units() {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.ready(3, "Fix b", &["type:fix"]);
    sim.free = Box::new(|| vec![("/mnt/c".into(), Err("Input/output error".into()))]);
    sim.run(|_, _| json!({"outcome": "done"})).unwrap();
    assert_eq!(sim.claimed, [2, 3]);
    let unread = DiskAct::Unread {
        path: "/mnt/c".into(),
        error: "Input/output error".into(),
    };
    assert_eq!(sim.disk_said, [unread]);
}

#[test]
fn a_disk_wait_holds_the_fill_before_memory_is_checked() {
    let host = Rc::new(Cell::new(15));
    let mut sim = night(&host);
    sim.available_mb = Some(10);
    sim.fill().unwrap();
    assert!(sim.claimed.is_empty());
    assert!(!sim.memory_low);
    assert!(sim.said.is_empty(), "{:?}", sim.said);
    // Space returns and memory holds the slot instead.
    host.set(40);
    sim.fill().unwrap();
    assert!(sim.claimed.is_empty());
    assert!(sim.memory_low);
    sim.available_mb = Some(5000);
    sim.fill().unwrap();
    assert_eq!(sim.claimed, [2]);
}
