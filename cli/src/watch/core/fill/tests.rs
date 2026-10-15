use std::path::PathBuf;

use super::*;
use crate::watch::core::red::Fix;

fn issue(n: u64) -> Issue {
    Issue {
        number: n,
        title: format!("Fix {n}"),
        labels: Vec::new(),
        body: String::new(),
        team: true,
    }
}

fn open(free: usize) -> Open<'static> {
    Open {
        kind: Kind::Full,
        paused: false,
        free,
        resume: 0,
        stopping: None,
        claims: Claims::default(),
        draining: false,
    }
}

#[test]
fn a_pause_starts_nothing_and_skips_the_housekeeping() {
    let o = Open {
        paused: true,
        resume: 2,
        ..open(2)
    };
    assert!(open_fill(&o).is_empty());
}

#[test]
fn a_full_fill_housekeeps_checks_the_base_and_triages_and_a_refill_only_triages() {
    use OpenAct::{CheckBase, CheckDisk, Housekeep, Triage};
    assert_eq!(
        open_fill(&open(1)),
        [Housekeep, CheckDisk, CheckBase, Triage]
    );
    let refill = Open {
        kind: Kind::Refill,
        ..open(1)
    };
    assert_eq!(open_fill(&refill), [CheckDisk, Triage]);
    // A drain under way starts nothing new and checks nothing again.
    let draining = Open {
        draining: true,
        ..open(1)
    };
    assert_eq!(open_fill(&draining), [Housekeep]);
}

#[test]
fn paused_units_resume_into_the_free_slots_first() {
    use OpenAct::{CheckBase, CheckDisk, Housekeep, Resume, Triage};
    let o = Open {
        resume: 3,
        ..open(2)
    };
    assert_eq!(open_fill(&o), [Housekeep, Resume, Resume]);
    let o = Open {
        resume: 1,
        ..open(2)
    };
    assert_eq!(
        open_fill(&o),
        [Housekeep, Resume, CheckDisk, CheckBase, Triage]
    );
    // --max-units stops claims, not resumes; a stop of everything stops both.
    let claims = Stopping::Claims("max_units".into());
    let o = Open {
        resume: 1,
        stopping: Some(&claims),
        ..open(2)
    };
    assert_eq!(open_fill(&o), [Housekeep, Resume]);
    let all = Stopping::All("until".into());
    let o = Open {
        resume: 1,
        stopping: Some(&all),
        ..open(2)
    };
    assert_eq!(open_fill(&o), [Housekeep]);
}

#[test]
fn no_claims_left_stops_the_fill_before_the_triage_pass() {
    let max = Open {
        claims: Claims {
            started: 2,
            max_units: Some(2),
            over: None,
        },
        ..open(1)
    };
    let stop = OpenAct::Stop(Stopping::Claims("max_units".into()));
    assert_eq!(open_fill(&max), [OpenAct::Housekeep, stop]);
    let until = Open {
        claims: Claims {
            over: Some("until"),
            ..Claims::default()
        },
        ..open(1)
    };
    let stop = OpenAct::Stop(Stopping::All("until".into()));
    assert_eq!(open_fill(&until), [OpenAct::Housekeep, stop]);
    // No slot free: nothing is asked.
    assert_eq!(open_fill(&Open { ..open(0) }), [OpenAct::Housekeep]);
}

const LIMITS: DiskLimits = DiskLimits {
    wait_gb: 20,
    stop_gb: 10,
};

/// `path` with `gib` GiB free.
fn free(path: &str, gib: u64) -> Free {
    (PathBuf::from(path), Ok(gib * GIB))
}

#[test]
fn space_above_the_wait_limit_holds_nothing() {
    let mut disk = Disk::default();
    assert_eq!(
        disk.check(vec![free("/w", 20), free("/mnt/c", 500)], LIMITS),
        []
    );
    assert!(!disk.waiting());
}

#[test]
fn space_between_the_limits_waits_said_once_until_it_returns() {
    let mut disk = Disk::default();
    let low = DiskAct::Low {
        path: "/mnt/c".into(),
        free_gb: 19,
    };
    // The least free of the filesystems decides; 19.5 GiB is under 20.
    let read = || vec![free("/w", 300), ("/mnt/c".into(), Ok(39 * GIB / 2))];
    assert_eq!(disk.check(read(), LIMITS), std::slice::from_ref(&low));
    assert!(disk.waiting());
    assert_eq!(disk.check(read(), LIMITS), [DiskAct::Wait]);
    assert_eq!(
        disk.check(vec![free("/w", 300), free("/mnt/c", 20)], LIMITS),
        []
    );
    assert!(!disk.waiting());
    // Low again later is said again.
    assert_eq!(disk.check(read(), LIMITS), [low]);
}

#[test]
fn space_under_the_stop_limit_stops_the_night() {
    let mut disk = Disk::default();
    let read = vec![free("/w", 15), ("/mnt/c".into(), Ok(10 * GIB - 1))];
    let critical = DiskAct::Critical {
        path: "/mnt/c".into(),
        free_gb: 9,
    };
    assert_eq!(disk.check(read, LIMITS), [critical]);
    // At exactly the stop limit the night only waits.
    assert!(matches!(
        disk.check(vec![free("/w", 10)], LIMITS)[..],
        [DiskAct::Low { .. }]
    ));
}

#[test]
fn a_filesystem_that_cannot_be_read_is_warned_of_once_and_holds_nothing() {
    let mut disk = Disk::default();
    let read = || vec![free("/w", 100), ("/mnt/c".into(), Err("gone".to_string()))];
    let unread = DiskAct::Unread {
        path: "/mnt/c".into(),
        error: "gone".into(),
    };
    assert_eq!(disk.check(read(), LIMITS), [unread]);
    assert_eq!(disk.check(read(), LIMITS), []);
    // The ones read still decide.
    let low = vec![free("/w", 12), ("/mnt/c".into(), Err("gone".to_string()))];
    assert!(matches!(disk.check(low, LIMITS)[..], [DiskAct::Low { .. }]));
}

#[test]
fn every_disk_act_but_an_unread_one_holds_the_fill() {
    let path = PathBuf::from("/w");
    let unread = DiskAct::Unread {
        path: path.clone(),
        error: "gone".into(),
    };
    assert!(!unread.holds());
    let low = DiskAct::Low {
        path: path.clone(),
        free_gb: 1,
    };
    let critical = DiskAct::Critical { path, free_gb: 1 };
    for act in [low, DiskAct::Wait, critical] {
        assert!(act.holds(), "{act:?}");
    }
}

#[test]
fn a_disk_check_holds_the_fill_unless_it_only_found_unread_space_and_a_critical_one_stops() {
    let path = PathBuf::from("/w");
    let unread = DiskAct::Unread {
        path: path.clone(),
        error: "gone".into(),
    };
    let low = DiskAct::Low {
        path: path.clone(),
        free_gb: 1,
    };
    let critical = DiskAct::Critical { path, free_gb: 1 };
    let go = DiskEffect {
        ok: true,
        stop: None,
    };
    let hold = DiskEffect {
        ok: false,
        stop: None,
    };
    let stop = DiskEffect {
        ok: false,
        stop: Some(DISK_CRITICAL),
    };
    assert_eq!(disk_effect(&[]), go);
    assert_eq!(disk_effect(std::slice::from_ref(&unread)), go);
    assert_eq!(disk_effect(&[unread.clone(), low.clone()]), hold);
    assert_eq!(disk_effect(&[DiskAct::Wait]), hold);
    assert_eq!(disk_effect(&[unread, critical]), stop);
}

#[test]
fn zero_limits_never_wait_or_stop() {
    let off = DiskLimits {
        wait_gb: 0,
        stop_gb: 0,
    };
    assert_eq!(Disk::default().check(vec![free("/w", 0)], off), []);
}

/// What a fill with `free` slots plans on `ready`, nothing running, with everything it needs read.
fn plan(ready: &[Issue], gate: &Gate, taken: &[u64], free: usize, claims: Claims) -> Vec<FillAct> {
    let taken: BTreeSet<u64> = taken.iter().copied().collect();
    let input = FillInput {
        ready,
        gate,
        taken: &taken,
        free,
        claims,
        holds: &Holds::default(),
    };
    let overlaps = Overlaps {
        running: Some(Vec::new()),
        rule: None,
        briefs: ready.iter().map(|i| (i.number, Vec::new())).collect(),
    };
    plan_fill(&input, &overlaps, 0).unwrap()
}

fn claimed(acts: &[FillAct]) -> Vec<u64> {
    acts.iter()
        .filter_map(|a| match a {
            FillAct::Claim(i) => Some(i.number),
            _ => None,
        })
        .collect()
}

#[test]
fn the_queue_starts_in_order_into_the_free_slots_and_never_twice() {
    let ready = [issue(2), issue(3), issue(2), issue(4), issue(5)];
    let acts = plan(&ready, &Gate::Open, &[4], 3, Claims::default());
    assert_eq!(claimed(&acts), [2, 3, 5]);
}

#[test]
fn max_units_stops_claims_mid_fill() {
    let ready = [issue(2), issue(3), issue(4)];
    let claims = Claims {
        started: 1,
        max_units: Some(2),
        over: None,
    };
    let acts = plan(&ready, &Gate::Open, &[], 3, claims);
    let stop = FillAct::Stop(Stopping::Claims("max_units".into()));
    assert_eq!(acts, [FillAct::Claim(issue(2)), stop]);
}

#[test]
fn red_main_starts_only_its_fix_issue_or_nothing() {
    let ready = [issue(2), issue(5)];
    let listed = Gate::Only(Fix::Listed(issue(5)));
    assert_eq!(
        claimed(&plan(&ready, &listed, &[], 2, Claims::default())),
        [5]
    );
    let unlisted = Gate::Only(Fix::Listed(issue(9)));
    assert!(plan(&ready, &unlisted, &[], 2, Claims::default()).is_empty());
    let filed = Gate::Only(Fix::JustFiled(issue(100)));
    assert_eq!(
        claimed(&plan(&ready, &filed, &[], 2, Claims::default())),
        [100]
    );
    // The fix unit already running is not started twice.
    assert!(plan(&ready, &filed, &[100], 2, Claims::default()).is_empty());
    assert!(plan(&ready, &Gate::Shut, &[], 2, Claims::default()).is_empty());
}

#[test]
fn the_pick_asks_for_what_it_lacks_one_read_at_a_time() {
    let ready = [issue(2), issue(3)];
    let taken = BTreeSet::new();
    let input = FillInput {
        ready: &ready,
        gate: &Gate::Open,
        taken: &taken,
        free: 2,
        claims: Claims::default(),
        holds: &Holds::default(),
    };
    let mut overlaps = Overlaps::default();
    assert_eq!(plan_fill(&input, &overlaps, 0), Err(Need::Running));
    overlaps.running = Some(vec![("1-a".into(), vec!["cli/**".into()])]);
    assert_eq!(plan_fill(&input, &overlaps, 0), Err(Need::Rule));
    overlaps.rule = Some(Overlap::new(vec!["cli/a.rs".into()], &[]));
    assert_eq!(plan_fill(&input, &overlaps, 0), Err(Need::Brief(2)));
    overlaps.briefs.insert(2, vec!["cli/a.rs".into()]);
    assert_eq!(plan_fill(&input, &overlaps, 0), Err(Need::Brief(3)));
    overlaps.briefs.insert(3, vec!["docs/x.md".into()]);
    let acts = plan_fill(&input, &overlaps, 0).unwrap();
    let held = FillAct::Held {
        issue: 2,
        overlaps: vec!["1-a".into()],
    };
    assert_eq!(acts, [held, FillAct::Claim(issue(3))]);
}
