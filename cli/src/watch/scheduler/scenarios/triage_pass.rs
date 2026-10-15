//! The triage pass: which candidates it runs, in what order beside the units, and when it stops.
//! Moved from tests/factory/triage_pass_and_memory_cap.rs and tests/factory/watch_fix_main.rs.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::{main_at, main_red, Sim, MAIN_SHA, READY};
use crate::watch::ci::Outcome as CheckOutcome;
use crate::watch::forge::fake::FakeForge;
use crate::watch::{triage, Issue};

const NEEDS_TRIAGE: &str = "status:needs-triage";

/// A triage-only run that writes no brief: it ends done, the issue left as it was.
fn none() -> Value {
    json!({"outcome": "done"})
}

/// A triage-only run whose triage moves issue `n` off the triage label onto `labels`, as
/// `triage_sets` does in the end-to-end harness.
fn sets(forge: &FakeForge, n: u64, labels: &[&str]) -> Value {
    let mut all = forge.labels.borrow_mut();
    let ls = all.entry(n).or_default();
    ls.retain(|l| l != NEEDS_TRIAGE);
    ls.extend(labels.iter().map(|l| l.to_string()));
    none()
}

/// A run whose every attempt failed at once: the harness, not the work.
fn crash() -> Value {
    let attempt = json!({"exit": 1, "output_tokens": 0, "wall_s": 1.0});
    json!({"outcome": "stuck", "reason": "exited 1", "phases": [attempt]})
}

/// Every triage-only run writes no brief.
fn all_none(sim: &mut Sim) {
    sim.triage = Some(Box::new(|_, _| Ok(none())));
}

/// A unit that ends done.
fn done(_: &Issue, _: &mut Sim) -> Value {
    json!({"outcome": "done"})
}

/// A unit that ends done after filing each of `filed` as needs-triage, as a review files an
/// escape, when it runs issue `by`.
fn files(by: u64, filed: &'static [(u64, &'static str)]) -> impl FnMut(&Issue, &mut Sim) -> Value {
    move |i, sim| {
        if i.number == by {
            for (n, title) in filed {
                sim.untriaged(*n, title, &[NEEDS_TRIAGE]);
            }
        }
        json!({"outcome": "done"})
    }
}

fn numbers(pairs: &[(u64, String)]) -> Vec<u64> {
    pairs.iter().map(|p| p.0).collect()
}

fn outcomes(pairs: &[(u64, String)]) -> Vec<String> {
    pairs.iter().map(|p| p.1.clone()).collect()
}

fn triage_runs(sim: &Sim) -> usize {
    sim.timeline
        .iter()
        .filter(|t| t.starts_with("triage "))
        .count()
}

#[test]
fn triage_takes_priority_labels_first_highest_first_then_issue_numbers() {
    let mut sim = Sim::new();
    // The queue's category order (`type:fix` first) plays no part in triage order.
    sim.untriaged(2, "No priority, docs", &[NEEDS_TRIAGE, "type:docs"]);
    sim.untriaged(3, "No priority, fix", &[NEEDS_TRIAGE, "type:fix"]);
    sim.untriaged(4, "Low", &[NEEDS_TRIAGE, "priority:low"]);
    sim.untriaged(
        5,
        "High docs",
        &[NEEDS_TRIAGE, "priority:high", "type:docs"],
    );
    sim.untriaged(6, "High fix", &[NEEDS_TRIAGE, "priority:high", "type:fix"]);
    sim.untriaged(8, "Medium", &[NEEDS_TRIAGE, "priority:medium"]);
    let listed = triage::candidates(&sim.forge, &sim.fac, &BTreeSet::new()).unwrap();
    let order: Vec<u64> = listed.issues.iter().map(|(i, _)| i.number).collect();
    assert_eq!(order, [5, 6, 8, 4, 2, 3]);
    all_none(&mut sim);
    let stopped = sim.run(done).unwrap();
    assert_eq!(numbers(&sim.triaged()), [5, 6, 8, 4, 2, 3]);
    assert_eq!(stopped, "queue empty");
}

#[test]
fn issues_filed_since_the_last_pass_are_triaged_before_the_backlog() {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.ready(3, "Fix b", &["type:fix"]);
    // A backlog candidate that outranks the follow-up on priority.
    sim.untriaged(4, "Old", &[NEEDS_TRIAGE, "priority:high"]);
    all_none(&mut sim);
    sim.run(files(2, &[(9, "Follow-up")])).unwrap();
    // #4 waits while units are ready; #9 is new before unit 3, so it goes first.
    assert_eq!(numbers(&sim.triaged()), [9, 4]);
    assert_eq!(sim.claimed, [2, 3]);
    let order = [
        "claim 2", "end 2", "triage 9", "claim 3", "end 3", "triage 4",
    ];
    assert_eq!(sim.timeline, order);
    assert_eq!(triage_runs(&sim), 2);
}

#[test]
fn every_new_issue_is_triaged_even_with_a_ready_queue() {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.ready(3, "Fix b", &["type:fix"]);
    all_none(&mut sim);
    // Unit 2's ship files two follow-ups; both are new at the next pass.
    sim.run(files(2, &[(8, "First"), (9, "Second")])).unwrap();
    assert_eq!(numbers(&sim.triaged()), [8, 9]);
    assert_eq!(sim.claimed, [2, 3]);
    assert_eq!(
        sim.timeline[1..5],
        ["end 2", "triage 8", "triage 9", "claim 3"]
    );
}

#[test]
fn the_backlog_stops_once_an_issue_is_ready_and_goes_on_at_the_next_pass() {
    let mut sim = Sim::new();
    sim.untriaged(5, "A", &[NEEDS_TRIAGE, "type:fix"]);
    sim.untriaged(6, "B", &[NEEDS_TRIAGE]);
    sim.untriaged(8, "C", &[NEEDS_TRIAGE]);
    sim.triage = Some(Box::new(|n, forge| {
        Ok(match n {
            5 => sets(forge, 5, &[READY]),
            _ => none(),
        })
    }));
    let stopped = sim.run(done).unwrap();
    assert_eq!(numbers(&sim.triaged()), [5, 6, 8]);
    assert_eq!(sim.claimed, [5]);
    assert_eq!(stopped, "queue empty");
    // #5 came back ready, so the pass stopped and unit 5 ran before #6 and #8 were triaged.
    let order = ["triage 5", "claim 5", "end 5", "triage 6", "triage 8"];
    assert_eq!(sim.timeline, order);
}

#[test]
fn a_blocked_ready_issue_does_not_stop_the_backlog() {
    let mut sim = Sim::new();
    sim.ready(2, "Blocked", &["type:fix"]);
    sim.forge.issues.last_mut().unwrap().body = "Blocked by: #7".into();
    sim.untriaged(5, "A", &[NEEDS_TRIAGE]);
    sim.untriaged(6, "B", &[NEEDS_TRIAGE]);
    all_none(&mut sim);
    sim.run(done).unwrap();
    assert_eq!(numbers(&sim.triaged()), [5, 6]);
    assert!(sim.units.is_empty(), "{:?}", sim.units);
}

#[test]
fn each_candidate_is_triaged_once_a_night() {
    let mut sim = Sim::new();
    sim.untriaged(5, "Unclear", &[NEEDS_TRIAGE, "priority:high"]);
    sim.untriaged(6, "Clear", &[NEEDS_TRIAGE]);
    sim.triage = Some(Box::new(|n, forge| {
        Ok(match n {
            6 => sets(forge, 6, &[READY]),
            _ => none(),
        })
    }));
    // Unit 6's ship files #9, which triage leaves untriaged too.
    sim.run(files(6, &[(9, "Follow-up")])).unwrap();
    // #5 and #9 stay untriaged and are candidates at every later pass, but run once each.
    assert_eq!(numbers(&sim.triaged()), [5, 6, 9]);
    assert_eq!(sim.claimed, [6]);
    assert_eq!(triage_runs(&sim), 3);
    assert_eq!(sim.labels(5), [NEEDS_TRIAGE, "priority:high"]);
    assert_eq!(sim.labels(9), [NEEDS_TRIAGE]);
}

#[test]
fn watch_triages_a_custom_triage_label() {
    let mut sim = Sim::new();
    sim.fac.queue.triage_label = "needs-triage".into();
    sim.untriaged(5, "Custom", &["needs-triage"]);
    sim.untriaged(6, "Default label only", &[NEEDS_TRIAGE]);
    let listed = triage::candidates(&sim.forge, &sim.fac, &BTreeSet::new()).unwrap();
    let found: Vec<(u64, &str)> = listed
        .issues
        .iter()
        .map(|(i, why)| (i.number, why.as_str()))
        .collect();
    assert_eq!(found, [(5, "needs-triage")]);
}

#[test]
fn a_newly_ready_issue_sorts_in_by_priority() {
    let mut sim = Sim::new();
    sim.ready(2, "Low fix", &["type:fix", "priority:low"]);
    sim.ready(3, "Low fix too", &["type:fix", "priority:low"]);
    sim.max_units = Some(2);
    sim.triage = Some(Box::new(|n, forge| {
        Ok(sets(forge, n, &[READY, "priority:high"]))
    }));
    let stopped = sim.run(files(2, &[(9, "Escape")])).unwrap();
    // Triage-only runs are not units: max_units counts units 2 and 9 only.
    assert_eq!(numbers(&sim.triaged()), [9]);
    assert_eq!(sim.claimed, [2, 9]);
    assert_eq!(stopped, "max_units");
    assert_eq!(sim.labels(3), ["type:fix", "priority:low", READY]);
}

#[test]
fn a_triage_that_leaves_the_issue_untriaged_is_not_retried_that_night() {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.ready(3, "Fix b", &["type:fix"]);
    sim.untriaged(5, "Unclear", &[NEEDS_TRIAGE]);
    all_none(&mut sim);
    sim.run(done).unwrap();
    assert_eq!(sim.triaged(), [(5, "done".to_string())]);
    assert_eq!(sim.night.triage.records[0]["state"], json!([NEEDS_TRIAGE]));
    assert_eq!(sim.claimed, [2, 3]);
    assert_eq!(triage_runs(&sim), 1);
    assert_eq!(sim.labels(5), [NEEDS_TRIAGE]);
}

#[test]
fn a_paused_triage_sleeps_and_retries_the_same_issue() {
    let mut sim = Sim::new();
    sim.untriaged(5, "Escape", &[NEEDS_TRIAGE]);
    let reset = sim.now + 3600;
    sim.deadline = Some(sim.now + 10 * 3600);
    let mut runs = 0;
    sim.triage = Some(Box::new(move |n, forge| {
        runs += 1;
        Ok(match runs {
            1 => json!({"outcome": "paused", "reset_at": reset}),
            _ => sets(forge, n, &[READY]),
        })
    }));
    sim.run(done).unwrap();
    assert_eq!(
        sim.triaged(),
        [(5, "paused".to_string()), (5, "done".to_string())]
    );
    let reset_at = crate::clock::local_iso(reset);
    assert_eq!(sim.night.triage.records[0]["reset_at"], reset_at.as_str());
    assert_eq!(sim.units, [(5, "done".to_string())]);
}

#[test]
fn a_triage_paused_past_until_ends_the_night_and_touches_no_label() {
    let mut sim = Sim::new();
    sim.untriaged(5, "Escape", &[NEEDS_TRIAGE]);
    sim.untriaged(6, "Next", &[NEEDS_TRIAGE]);
    let reset = sim.now + 8 * 3600;
    sim.deadline = Some(sim.now + 3600);
    sim.triage = Some(Box::new(move |_, _| {
        Ok(json!({"outcome": "paused", "reset_at": reset}))
    }));
    let stopped = sim.run(done).unwrap();
    assert_eq!(stopped, "usage limit resets after --until");
    assert_eq!(outcomes(&sim.triaged()), ["paused"]);
    assert!(sim.units.is_empty(), "{:?}", sim.units);
    assert_eq!(sim.labels(5), [NEEDS_TRIAGE]);
    assert_eq!(sim.labels(6), [NEEDS_TRIAGE]);
}

#[test]
fn triage_runs_that_keep_failing_instantly_stop_the_night() {
    let mut sim = Sim::new();
    for n in [5, 6, 8] {
        sim.untriaged(n, "A", &[NEEDS_TRIAGE]);
    }
    sim.triage = Some(Box::new(|_, _| Ok(crash())));
    let stopped = sim.run(done).unwrap();
    assert_eq!(stopped, "harness failing");
    assert_eq!(
        outcomes(&sim.triaged()),
        ["harness_failing", "harness_failing"]
    );
    assert_eq!(sim.night.triage.records[0]["reason"], "exited 1");
    assert!(sim.units.is_empty(), "{:?}", sim.units);
    for n in [5, 6, 8] {
        assert_eq!(sim.labels(n), [NEEDS_TRIAGE]);
    }
}

#[test]
fn a_triage_that_works_resets_the_harness_failure_count() {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.untriaged(5, "A", &[NEEDS_TRIAGE]);
    sim.untriaged(6, "B", &[NEEDS_TRIAGE]);
    // Unit 2 crashes, 5 works, then 6 crashes.
    sim.triage = Some(Box::new(|n, _| Ok(if n == 5 { none() } else { crash() })));
    let stopped = sim.run(|_, _| crash()).unwrap();
    assert_eq!(sim.units, [(2, "harness_failing".to_string())]);
    assert_eq!(outcomes(&sim.triaged()), ["done", "harness_failing"]);
    assert_eq!(stopped, "queue empty");
}

#[test]
fn a_failing_triage_and_a_failing_unit_share_the_harness_breaker() {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.untriaged(5, "A", &[NEEDS_TRIAGE]);
    sim.untriaged(6, "B", &[NEEDS_TRIAGE]);
    sim.triage = Some(Box::new(|_, _| Ok(crash())));
    let stopped = sim.run(|_, _| crash()).unwrap();
    assert_eq!(stopped, "harness failing");
    assert_eq!(sim.units, [(2, "harness_failing".to_string())]);
    assert_eq!(sim.triaged(), [(5, "harness_failing".to_string())]);
}

#[test]
fn an_unreadable_state_after_triage_is_recorded_as_null() {
    let mut sim = Sim::new();
    sim.untriaged(5, "Escape", &[NEEDS_TRIAGE]);
    sim.forge.fails.insert("labels");
    all_none(&mut sim);
    let stopped = sim.run(done).unwrap();
    assert_eq!(outcomes(&sim.triaged()), ["done"]);
    assert!(sim.night.triage.records[0]["state"].is_null());
    assert_eq!(stopped, "queue empty");
}

#[test]
fn the_budget_stops_the_triage_pass() {
    let mut sim = Sim::new();
    sim.fac.limits.budget_usd = Some(0.4);
    sim.untriaged(5, "A", &[NEEDS_TRIAGE]);
    sim.untriaged(6, "B", &[NEEDS_TRIAGE]);
    sim.triage = Some(Box::new(|_, _| {
        Ok(json!({"outcome": "done", "cost_usd": 0.5}))
    }));
    let stopped = sim.run(done).unwrap();
    assert_eq!(stopped, "budget");
    assert_eq!(numbers(&sim.triaged()), [5]);
    assert!(sim.units.is_empty(), "{:?}", sim.units);
    assert_eq!(sim.labels(6), [NEEDS_TRIAGE]);
}

#[test]
fn a_triage_run_that_errors_is_recorded_and_the_night_goes_on() {
    let mut sim = Sim::new();
    sim.untriaged(5, "Unreadable", &[NEEDS_TRIAGE]);
    sim.untriaged(6, "Fine", &[NEEDS_TRIAGE]);
    let mut started = 0;
    sim.triage = Some(Box::new(move |n, _| {
        if n == 5 {
            anyhow::bail!("cannot read issue #5: no such issue");
        }
        started += 1;
        assert_eq!(started, 1, "one triage-only run started");
        Ok(none())
    }));
    sim.run(done).unwrap();
    assert_eq!(numbers(&sim.triaged()), [5, 6]);
    assert_eq!(outcomes(&sim.triaged()), ["error", "done"]);
    let reason = sim.night.triage.records[0]["reason"].as_str().unwrap();
    assert!(reason.contains("cannot read issue #5"), "{reason}");
}

#[test]
fn the_same_start_error_twice_turns_the_triage_pass_off() {
    let mut sim = Sim::new();
    for n in [5, 6, 8] {
        sim.untriaged(n, "A", &[NEEDS_TRIAGE]);
    }
    // A global failure: no subscription login, whichever issue the run is for.
    sim.triage = Some(Box::new(|_, _| anyhow::bail!("no subscription login")));
    let stopped = sim.run(done).unwrap();
    assert_eq!(numbers(&sim.triaged()), [5, 6]);
    assert_eq!(outcomes(&sim.triaged()), ["error", "error"]);
    let recs = &sim.night.triage.records;
    assert_eq!(recs[0]["reason"], recs[1]["reason"]);
    assert_eq!(stopped, "queue empty");
}

#[test]
fn different_start_errors_keep_the_triage_pass_on() {
    let mut sim = Sim::new();
    for n in [5, 6, 8] {
        sim.untriaged(n, "A", &[NEEDS_TRIAGE]);
    }
    sim.triage = Some(Box::new(|n, _| match n {
        5 | 6 => anyhow::bail!("cannot read issue #{n}"),
        _ => Ok(none()),
    }));
    sim.run(done).unwrap();
    assert_eq!(outcomes(&sim.triaged()), ["error", "error", "done"]);
}

#[test]
fn a_follow_up_filed_by_a_unit_is_triaged_before_the_next_unit() {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.ready(3, "Fix b", &["type:fix"]);
    all_none(&mut sim);
    // Unit 2's ship files follow-up #9 as needs-triage, as review files an escape.
    sim.run(files(2, &[(9, "Follow-up")])).unwrap();
    assert_eq!(numbers(&sim.triaged()), [9]);
    assert_eq!(sim.claimed, [2, 3]);
    assert_eq!(sim.timeline[1..4], ["end 2", "triage 9", "claim 3"]);
}

#[test]
fn watch_with_main_red_runs_no_triage_only_run() {
    let mut sim = Sim::new();
    sim.untriaged(5, "Fix e", &[NEEDS_TRIAGE, "type:fix"]);
    sim.forge.main = main_red();
    sim.max_units = Some(3);
    all_none(&mut sim);
    sim.run(done).unwrap();
    assert_eq!(triage_runs(&sim), 0);
    assert!(sim.triaged().is_empty());
    assert_eq!(sim.units, [(100, "done".to_string())]);
}

#[test]
fn watch_stops_triaging_when_main_turns_red_after_a_triage_only_run() {
    let mut sim = Sim::new();
    sim.untriaged(5, "Fix e", &[NEEDS_TRIAGE, "type:fix"]);
    sim.untriaged(6, "Fix f", &[NEEDS_TRIAGE, "type:fix"]);
    sim.forge.main = main_at(MAIN_SHA, CheckOutcome::Pass);
    sim.max_units = Some(3);
    // #5's triage-only run edits its issue, and main is red from then on.
    sim.triage = Some(Box::new(|n, forge| {
        *forge.moved_main.borrow_mut() = Some(main_red());
        Ok(sets(forge, n, &["status:needs-info"]))
    }));
    sim.run(done).unwrap();
    assert_eq!(triage_runs(&sim), 1);
    assert_eq!(numbers(&sim.triaged()), [5]);
    assert_eq!(sim.units, [(100, "done".to_string())]);
    assert_eq!(sim.forge.filed.borrow().len(), 1);
}
