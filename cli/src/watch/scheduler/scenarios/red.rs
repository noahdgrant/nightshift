//! Red main under `[queue] fix_main_first`: which units start, what the night says once per
//! head, and when a fix unit that ends unfixed stops the night. Moved from
//! tests/factory/watch_fix_main.rs and tests/factory/watch_nights_and_splits.rs.

use serde_json::{json, Value};

use super::{by_issue, each, main_at, main_red, Said, Sim, MAIN_SHA};
use crate::watch::ci::{Outcome as CheckOutcome, State};
use crate::watch::Issue;

/// The fix issue's number: the first issue the fake files.
const FIX: u64 = 100;

/// A night with #2 ready, `cap` units at most, and main read as `main`.
fn night(cap: u32, main: crate::watch::ci::HeadChecks) -> Sim {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.max_units = Some(cap);
    sim.forge.main = main;
    sim
}

fn green(sha: &str) -> crate::watch::ci::HeadChecks {
    main_at(sha, CheckOutcome::Pass)
}

fn filed(sim: &Sim) -> usize {
    sim.forge.filed.borrow().len()
}

fn reds(sim: &Sim) -> Vec<(Option<String>, u64)> {
    sim.said
        .iter()
        .filter_map(|s| match s {
            Said::MainRed { sha, issue, .. } => Some((sha.clone(), *issue)),
            _ => None,
        })
        .collect()
}

fn greens(sim: &Sim) -> Vec<(Option<String>, State)> {
    sim.said
        .iter()
        .filter_map(|s| match s {
            Said::MainGreen { sha, state } => Some((sha.clone(), *state)),
            _ => None,
        })
        .collect()
}

fn unfixed(sim: &Sim) -> Vec<Said> {
    let un = |s: &&Said| matches!(s, Said::Unfixed { .. });
    sim.said.iter().filter(un).cloned().collect()
}

fn done(_: &Issue, _: &mut Sim) -> Value {
    json!({"outcome": "done"})
}

fn sha(c: char) -> String {
    c.to_string().repeat(40)
}

#[test]
fn watch_with_main_green_runs_the_queue_and_files_nothing() {
    let mut sim = night(1, green(MAIN_SHA));
    sim.run(done).unwrap();
    assert_eq!(by_issue(&sim.units), each(&[2], "done"));
    assert_eq!(filed(&sim), 0);
    assert!(sim.said.is_empty(), "{:?}", sim.said);
}

#[test]
fn watch_with_fix_main_first_off_runs_the_queue_while_main_is_red() {
    let mut sim = night(1, main_red());
    sim.fac.queue.fix_main_first = false;
    sim.run(done).unwrap();
    assert_eq!(by_issue(&sim.units), each(&[2], "done"));
    assert_eq!(filed(&sim), 0);
    assert!(reds(&sim).is_empty());
}

#[test]
fn watch_starts_a_fix_issue_it_just_filed_before_github_lists_it() {
    let mut sim = night(3, main_red());
    sim.forge.unlisted_filed = true;
    // A cap above one: a fill after the fix ends must reuse it, not file a second.
    sim.run(done).unwrap();
    assert_eq!(by_issue(&sim.units), each(&[FIX], "done"));
    assert_eq!(filed(&sim), 1);
}

#[test]
fn a_unit_running_when_main_turns_red_finishes_and_the_backlog_waits() {
    let mut sim = night(3, green(MAIN_SHA));
    sim.parallel = 2;
    sim.fill().unwrap();
    assert_eq!(sim.claimed, [2]);
    // Main is red before the backlog's next issue appears, so no fill sees one without the other.
    sim.forge.main = main_red();
    sim.ready(3, "Fix b", &["type:fix"]);
    sim.run(done).unwrap();
    assert_eq!(
        by_issue(&sim.units),
        [(2, "done".into()), (FIX, "done".into())]
    );
    assert_eq!(filed(&sim), 1);
    assert!(!sim.claimed.contains(&3), "{:?}", sim.claimed);
}

#[test]
fn a_closed_fix_issue_at_the_same_red_sha_is_not_filed_again() {
    let mut sim = night(3, main_red());
    sim.fill().unwrap();
    assert_eq!(sim.claimed, [FIX]);
    // The fix issue is closed, as by a person, once its unit has begun; main stays red.
    sim.forge.closed.insert(FIX);
    sim.run(done).unwrap();
    assert_eq!(by_issue(&sim.units), each(&[FIX], "done"));
    assert_eq!(filed(&sim), 1);
    let shut = sim.said.iter().filter(|s| **s == Said::Shut).count();
    assert_eq!(shut, 1, "{:?}", sim.said);
    assert!(!sim.claimed.contains(&2));
}

#[test]
fn a_unit_paused_before_main_turned_red_resumes_and_ends_as_it_would() {
    let mut sim = night(3, green(MAIN_SHA));
    sim.fill().unwrap();
    let reset = sim.now + 3600;
    sim.end(2, json!({"outcome": "paused", "reset_at": reset}))
        .unwrap();
    // Main turns red while the unit waits for its reset; it resumes and finishes beside the fix.
    sim.forge.main = main_red();
    sim.run(done).unwrap();
    assert_eq!(
        by_issue(&sim.units),
        [
            (2, "done".into()),
            (2, "paused".into()),
            (FIX, "done".into())
        ]
    );
}

#[test]
fn watch_reuses_the_open_fix_issue_instead_of_filing_another() {
    let mut sim = night(3, main_red());
    // Outside the team: never reused, since anyone may file a title.
    sim.ready(8, "fix: main is red at 1234567: lint", &[]);
    sim.forge.issues.last_mut().unwrap().team = false;
    sim.ready(
        9,
        "fix: main is red at 1234567: lint",
        &["priority:critical"],
    );
    sim.run(done).unwrap();
    assert_eq!(by_issue(&sim.units), each(&[9], "done"));
    assert_eq!(filed(&sim), 0);
    assert_eq!(reds(&sim), [(Some(MAIN_SHA.into()), 9)]);
}

#[test]
fn watch_reuses_a_parked_fix_issue_and_files_no_duplicate() {
    let mut sim = Sim::new();
    sim.max_units = Some(3);
    sim.forge.main = main_red();
    let labels = vec!["status:needs-info".to_string()];
    sim.forge.labels.borrow_mut().insert(9, labels.clone());
    sim.forge.issues.push(Issue {
        number: 9,
        title: "fix: main is red at 1234567: lint".into(),
        labels,
        body: String::new(),
        team: true,
    });
    sim.run(done).unwrap();
    assert!(sim.units.is_empty(), "{:?}", sim.units);
    assert_eq!(filed(&sim), 0);
    let warned = "fix issue #9 is open but the queue will not start it";
    assert!(
        sim.warnings.iter().any(|w| w.contains(warned)),
        "{:?}",
        sim.warnings
    );
    assert_eq!(reds(&sim), [(Some(MAIN_SHA.into()), 9)]);
}

#[test]
fn watch_resumes_the_queue_once_the_fix_merges_and_main_is_green() {
    let mut sim = night(3, main_red());
    let green_sha = "1111111111111111111111111111111111111111";
    // Merging the fix's PR turns main green.
    sim.run(|i, sim| {
        if i.number == FIX {
            sim.forge.main = green(green_sha);
        }
        json!({"outcome": "merged"})
    })
    .unwrap();
    assert_eq!(
        sim.units,
        [(FIX, "merged".to_string()), (2, "merged".to_string())]
    );
    let red = sim.said.first().cloned();
    assert_eq!(
        red,
        Some(Said::MainRed {
            sha: Some(MAIN_SHA.into()),
            failing: vec!["test".into()],
            issue: FIX,
        })
    );
    assert_eq!(reds(&sim).len(), 1);
    assert_eq!(greens(&sim), [(Some(green_sha.into()), State::Green)]);
}

#[test]
fn watch_logs_a_second_main_red_for_a_new_sha_and_again_after_green() {
    let mut sim = night(4, main_red());
    // Each merge moves main on: red at a new head, green, then red again. Merging closes the
    // fix issue, as the forge does with a closing PR.
    sim.run(|i, sim| {
        let next = match i.number {
            100 => main_at(&sha('2'), CheckOutcome::Fail),
            101 => green(&sha('3')),
            2 => main_at(&sha('4'), CheckOutcome::Fail),
            _ => return json!({"outcome": "merged"}),
        };
        sim.forge.main = next;
        sim.forge.closed.insert(i.number);
        json!({"outcome": "merged"})
    })
    .unwrap();
    let order: Vec<u64> = sim.units.iter().map(|u| u.0).collect();
    assert_eq!(order, [100, 101, 2, 102]);
    assert_eq!(
        reds(&sim),
        [
            (Some(MAIN_SHA.into()), 100),
            (Some(sha('2')), 101),
            (Some(sha('4')), 102)
        ]
    );
    assert_eq!(greens(&sim), [(Some(sha('3')), State::Green)]);
}

#[test]
fn watch_resumes_the_queue_when_main_is_pending_with_nothing_failing() {
    let mut sim = night(2, main_red());
    sim.run(|i, sim| {
        if i.number == FIX {
            sim.forge.main = main_at(MAIN_SHA, CheckOutcome::Running);
        }
        json!({"outcome": "merged"})
    })
    .unwrap();
    let order: Vec<u64> = sim.units.iter().map(|u| u.0).collect();
    assert_eq!(order, [FIX, 2]);
    assert_eq!(greens(&sim), [(Some(MAIN_SHA.into()), State::Pending)]);
}

/// A night with main red, #2 ready and the fix issue #100, its units ending as `result` says.
fn fix_ends(result: impl FnMut(&Issue, &mut Sim) -> Value) -> (Sim, String) {
    // A cap above one: a night that goes on would file or start more.
    let mut sim = night(3, main_red());
    let stopped = sim.run(result).unwrap();
    (sim, stopped)
}

/// The fix issue's unit ends `outcome` with main still red: the night stops.
fn stops_unfixed(outcome: &'static str, over: Option<&'static str>) {
    let (sim, stopped) = fix_ends(|_, sim| {
        sim.over = over;
        json!({"outcome": outcome})
    });
    assert_eq!(by_issue(&sim.units), each(&[FIX], outcome));
    assert_eq!(stopped, "main_red_unfixed");
    assert_eq!(
        unfixed(&sim),
        [Said::Unfixed {
            sha: Some(MAIN_SHA.into()),
            issue: FIX,
            outcome,
        }]
    );
    assert_eq!(filed(&sim), 1);
    assert!(!sim.claimed.contains(&2));
}

#[test]
fn a_fix_unit_that_ends_unfixed_stops_the_night() {
    stops_unfixed("stuck", None);
    stops_unfixed("split", None);
    // The budget spent by the fix unit: it ends budget.
    stops_unfixed("budget", Some("budget"));
}

#[test]
fn a_fix_unit_cut_short_by_until_stops_the_night_as_until() {
    let (sim, stopped) = fix_ends(|_, sim| {
        sim.over = Some("until");
        json!({"outcome": "budget"})
    });
    assert_eq!(by_issue(&sim.units), each(&[FIX], "budget"));
    assert_eq!(stopped, "until");
    assert!(unfixed(&sim).is_empty());
}

#[test]
fn a_fix_unit_that_ends_stuck_after_until_passed_still_logs_main_red_unfixed() {
    let mut sim = night(3, green(MAIN_SHA));
    sim.parallel = 2;
    sim.fill().unwrap();
    sim.forge.main = main_red();
    sim.fill().unwrap();
    assert_eq!(sim.claimed, [2, FIX]);
    // #2 waits for a held lock until --until and ends budget; the fix unit is still running.
    sim.over = Some("until");
    sim.end(2, json!({"outcome": "budget"})).unwrap();
    sim.end(FIX, json!({"outcome": "stuck"})).unwrap();
    assert_eq!(
        by_issue(&sim.units),
        [(2, "budget".into()), (FIX, "stuck".into())]
    );
    assert_eq!(sim.stopping.as_ref().unwrap().reason(), "until");
    let un = unfixed(&sim);
    assert_eq!(un.len(), 1, "{un:?}");
    assert!(matches!(
        un[0],
        Said::Unfixed {
            outcome: "stuck",
            ..
        }
    ));
}

#[test]
fn a_fix_unit_that_ends_unfixed_after_main_turned_green_stops_nothing() {
    let (sim, stopped) = fix_ends(|i, sim| {
        // Main turns green, by another hand, while the fix unit runs.
        if i.number == FIX {
            sim.forge.main = green(MAIN_SHA);
        }
        json!({"outcome": "stuck"})
    });
    assert_eq!(by_issue(&sim.units), each(&[2, FIX], "stuck"));
    assert_eq!(stopped, "queue empty");
    assert!(unfixed(&sim).is_empty());
}

#[test]
fn a_fix_unit_that_ends_unfixed_when_main_ci_cannot_be_read_stops_nothing() {
    let (sim, stopped) = fix_ends(|i, sim| {
        if i.number == FIX {
            sim.forge.fails.insert("main_checks");
        }
        json!({"outcome": "stuck"})
    });
    assert_eq!(by_issue(&sim.units), each(&[2, FIX], "stuck"));
    assert_eq!(stopped, "queue empty");
    assert!(unfixed(&sim).is_empty());
}

#[test]
fn a_fix_unit_the_harness_fails_does_not_stop_the_night() {
    // Every attempt exits non-zero at once with no output: the harness, not the work.
    let crash = json!({"exit": 1, "output_tokens": 0, "wall_s": 1.0});
    let (sim, stopped) = fix_ends(|_, _| json!({"outcome": "stuck", "phases": [crash, crash]}));
    assert_eq!(by_issue(&sim.units), each(&[FIX], "harness_failing"));
    assert_eq!(stopped, "queue empty");
    assert!(unfixed(&sim).is_empty());
}

#[test]
fn a_stuck_unit_that_is_not_the_fix_does_not_stop_the_night() {
    let mut sim = night(3, green(MAIN_SHA));
    sim.parallel = 2;
    sim.fill().unwrap();
    sim.forge.main = main_red();
    // #2 ends stuck after the fix issue is filed and started.
    sim.fill().unwrap();
    assert_eq!(sim.claimed, [2, FIX]);
    sim.end(2, json!({"outcome": "stuck"})).unwrap();
    let stopped = sim.run(done).unwrap();
    assert_eq!(
        by_issue(&sim.units),
        [(2, "stuck".into()), (FIX, "done".into())]
    );
    assert_eq!(stopped, "queue empty");
    assert!(unfixed(&sim).is_empty());
}

#[test]
fn a_unit_running_when_the_fix_unit_ends_unfixed_finishes_and_nothing_new_starts() {
    let mut sim = night(4, green(MAIN_SHA));
    sim.parallel = 2;
    sim.fill().unwrap();
    sim.forge.main = main_red();
    sim.ready(3, "Fix b", &["type:fix"]);
    sim.fill().unwrap();
    sim.end(FIX, json!({"outcome": "stuck"})).unwrap();
    let stopped = sim.run(done).unwrap();
    assert_eq!(
        by_issue(&sim.units),
        [(2, "done".into()), (FIX, "stuck".into())]
    );
    assert_eq!(stopped, "main_red_unfixed");
    assert_eq!(unfixed(&sim).len(), 1);
    assert!(!sim.claimed.contains(&3));
}
