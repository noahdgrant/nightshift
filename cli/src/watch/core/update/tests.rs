use super::*;
use serde_json::json;

fn set(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn paths(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// A night with nothing running, waiting or stopping.
fn idle() -> Idle {
    Idle::default()
}

#[test]
fn a_stopping_night_never_hands_off_or_syncs() {
    let both = Idle {
        draining: true,
        adopted: true,
        ..idle()
    };
    assert!(!ready_to_hand_off(&Idle {
        stopping: true,
        ..both
    }));
    assert_eq!(hand_off_step(&both, true), HandOffStep::DropDrain);
    assert_eq!(hand_off_step(&idle(), true), HandOffStep::Nothing);
}

#[test]
fn a_failed_commit_is_not_built_again_and_no_second_build_starts() {
    let touched = paths(&["cli/x"]);
    let failed = set(&["b"]);
    assert_eq!(to_build(false, &failed, Some("b"), Some(&touched)), None);
    assert_eq!(
        to_build(false, &failed, Some("c"), Some(&touched)),
        Some("c".into())
    );
    assert_eq!(
        to_build(true, &BTreeSet::new(), Some("c"), Some(&touched)),
        None
    );
}

#[test]
fn a_build_ends_on_a_stop_its_exit_or_its_ceiling() {
    let ok = Exit::Ended { success: true };
    assert_eq!(build_step(false, Exit::Running, 99, 100), BuildStep::Wait);
    assert_eq!(
        build_step(false, Exit::Running, 100, 100),
        BuildStep::TimedOut
    );
    assert_eq!(build_step(false, ok, 500, 100), BuildStep::Built);
    assert_eq!(
        build_step(false, Exit::Ended { success: false }, 0, 100),
        BuildStep::Failed
    );
    assert_eq!(
        build_step(false, Exit::Unreadable, 0, 100),
        BuildStep::Unreadable
    );
    for exit in [Exit::Running, ok] {
        assert_eq!(build_step(true, exit, 0, 100), BuildStep::Stop);
    }
}

#[test]
fn the_ceiling_takes_a_numeric_override_in_milliseconds() {
    assert_eq!(ceiling_ms(None, 60_000), 60_000);
    assert_eq!(ceiling_ms(Some("300"), 60_000), 300);
    assert_eq!(ceiling_ms(Some("soon"), 60_000), 60_000);
    assert_eq!(ceiling_ms(Some("-5"), 60_000), 60_000);
}

#[test]
fn a_checkout_that_stays_stuck_warns_once_per_reason() {
    assert!(warns_stale(None, "diverged"));
    assert!(!warns_stale(Some("diverged"), "diverged"));
    assert!(warns_stale(Some("diverged"), "dirty"));
}

#[test]
fn a_run_lock_holder_holds_its_issue_or_its_unit_ids_issue() {
    let h = json!({"pid": 1, "unit": "13-fix-uart", "issue": 13});
    assert!(holds(&h, 13));
    assert!(!holds(&h, 1));
    assert!(!holds(&h, 130));
    // A unit run without --issue names its issue only through the unit id.
    let by_unit = json!({"pid": 1, "unit": "13-fix-uart", "issue": null});
    assert!(holds(&by_unit, 13));
    assert!(!holds(&by_unit, 1));
    assert!(holds(&json!({"pid": 1, "unit": "13"}), 13));
    assert!(!holds(&json!({"pid": 1, "unit": "130-x"}), 13));
    // A holder that hasn't written its record yet is known by its lock's file name.
    assert!(holds(
        &json!({"pid": null, "unit": "99-x", "issue": null}),
        99
    ));
    assert!(!holds(
        &json!({"pid": null, "unit": "uart", "issue": null}),
        99
    ));
}

fn issue(number: u64, labels: &[&str]) -> Issue {
    Issue {
        number,
        title: format!("Fix {number}"),
        labels: paths(labels),
        body: String::new(),
        team: true,
    }
}

#[test]
fn issues_no_live_run_holds_are_requeued_lowest_first() {
    let label = "status:in-progress";
    let issues = vec![
        issue(9, &[label]),
        issue(4, &[label]),
        issue(7, &[label]),
        // Listed under the label, but it moved on since: left alone.
        issue(5, &["status:ready-for-agent"]),
    ];
    let holders = [json!({"pid": 1, "unit": "7-fix-7", "issue": 7})];
    let found = in_progress(issues, label, &holders);
    let stale: Vec<u64> = found.stale.iter().map(|i| i.number).collect();
    assert_eq!(stale, [4, 9]);
    assert_eq!(found.held.len(), 1);
    assert_eq!(found.held[0].0.number, 7);
    assert_eq!(found.held[0].1["pid"], 1);
}
