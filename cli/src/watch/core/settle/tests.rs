use serde_json::json;

use super::*;
use SettleAct::{Close, Comment, FlagPr, Label, Pause, Record, StopAll, Unfixed, UnflagPr};

fn result(outcome: &str, extra: &serde_json::Value) -> RunResult {
    let mut v = json!({"unit": "2-fix-a", "outcome": outcome, "reason": "why"});
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    RunResult::from_json(v).unwrap()
}

fn record(outcome: &'static str) -> SettleAct {
    Record {
        outcome,
        reset_at: None,
        finished: true,
    }
}

fn label(event: Event, defer: bool) -> SettleAct {
    Label { event, defer }
}

#[test]
fn each_outcome_earns_its_label_and_record() {
    let f = SettleFacts::default();
    let merged = result("merged", &json!({"pr": 7}));
    assert_eq!(
        settle(&merged, f),
        [
            label(Event::Merged, true),
            Close,
            UnflagPr(7),
            record("merged")
        ]
    );
    let split = result("split", &json!({}));
    assert_eq!(
        settle(&split, f),
        [label(Event::Split, false), record("split")]
    );
    let budget = result("budget", &json!({}));
    let acts = settle(&budget, f);
    assert_eq!(
        acts,
        [
            label(Event::Budget, false),
            StopAll("budget".into()),
            record("budget")
        ]
    );
}

#[test]
fn a_clean_done_waits_on_a_human_merge_and_flags_its_pr() {
    let done = result("done", &json!({"pr": 7}));
    let acts = settle(&done, SettleFacts::default());
    let body = "nightshift unit `2-fix-a`: merge.policy is \"human\", so ns never merges; needs a human merge";
    assert_eq!(
        acts,
        [
            label(Event::AwaitingHumanMerge, false),
            FlagPr {
                pr: 7,
                body: body.into()
            },
            record("done"),
        ]
    );
    // No PR, nothing flagged.
    let acts = settle(&result("done", &json!({})), SettleFacts::default());
    assert_eq!(
        acts,
        [label(Event::AwaitingHumanMerge, false), record("done")]
    );
}

#[test]
fn a_stuck_unit_goes_to_a_human_with_a_comment() {
    let stuck = result("stuck", &json!({"artifact": "/w/.ns/2-fix-a/verify.md"}));
    let acts = settle(&stuck, SettleFacts::default());
    let text = "nightshift got stuck on unit `2-fix-a`: why\n\nLast artifact: `.ns/2-fix-a/verify.md` on branch `ns/2-fix-a`";
    assert_eq!(
        acts,
        [
            label(Event::NeedsHuman, true),
            Comment(text.into()),
            record("stuck")
        ]
    );
}

#[test]
fn a_stuck_comment_lists_the_open_findings_of_a_review() {
    let review =
        "\n## Critical\n### C1. Off by one\n- Location: `cli/src/a.rs:3`\n- Status: open\n";
    let stuck = result("stuck", &json!({"artifact": "/w/.ns/2-fix-a/review.md"}));
    let text = stuck_comment(&stuck, Some(review));
    let quoted = "Open findings in `review.md`:\n```text\nC1. Off by one (cli/src/a.rs:3)\n```";
    assert!(text.contains(quoted), "{text}");
    // Another artifact quotes none, whatever text is passed.
    let other = result("stuck", &json!({"artifact": "/w/.ns/2-fix-a/verify.md"}));
    assert!(!stuck_comment(&other, Some(review)).contains("Open findings"));
    assert!(stuck_comment(&result("stuck", &json!({})), None).ends_with("Last artifact: none"));
}

#[test]
fn a_harness_failure_requeues_and_the_second_stops_the_night() {
    let stuck = result("stuck", &json!({}));
    let f = SettleFacts {
        failing: true,
        harness_fails: 1,
        ..SettleFacts::default()
    };
    let first = [
        label(Event::HarnessFailing, false),
        record("harness_failing"),
    ];
    assert_eq!(settle(&stuck, f), first);
    let f = SettleFacts {
        harness_fails: 2,
        ..f
    };
    let acts = settle(&stuck, f);
    assert_eq!(acts[1], StopAll("harness failing".into()));
}

#[test]
fn a_stop_interrupts_all_but_a_clean_end() {
    let f = SettleFacts {
        stopping: true,
        ..SettleFacts::default()
    };
    let interrupted = [
        label(Event::Interrupted, false),
        Record {
            outcome: "interrupted",
            reset_at: None,
            finished: false,
        },
    ];
    for o in ["stuck", "budget", "paused"] {
        assert_eq!(settle(&result(o, &json!({})), f), interrupted, "{o}");
    }
    let merged = settle(&result("merged", &json!({})), f);
    assert_eq!(merged[0], label(Event::Merged, true));
}

#[test]
fn a_paused_unit_is_recorded_then_pauses_every_unit() {
    let f = SettleFacts {
        resume_at: 900,
        ..SettleFacts::default()
    };
    let acts = settle(&result("paused", &json!({})), f);
    let rec = Record {
        outcome: "paused",
        reset_at: Some(900),
        finished: false,
    };
    assert_eq!(acts, [rec, Pause(900)]);
}

#[test]
fn the_fix_unit_ending_unfixed_stops_unless_until_or_the_harness_ended_it() {
    let f = SettleFacts {
        unfixed: true,
        ..SettleFacts::default()
    };
    let stuck = result("stuck", &json!({}));
    assert_eq!(settle(&stuck, f)[0], Unfixed);
    assert!(checks_main(&stuck, &f));
    let until = SettleFacts {
        over: Some("until"),
        ..f
    };
    let budget = result("budget", &json!({}));
    assert!(!checks_main(&budget, &until));
    assert!(!settle(&budget, until).contains(&Unfixed));
    assert_eq!(settle(&budget, until)[1], StopAll("until".into()));
    // Stuck after --until passed is still unfixed: only budget is cut short.
    assert_eq!(settle(&stuck, until)[0], Unfixed);
    let harness = SettleFacts { failing: true, ..f };
    assert!(!checks_main(&stuck, &harness));
    assert!(!settle(&stuck, harness).contains(&Unfixed));
}
