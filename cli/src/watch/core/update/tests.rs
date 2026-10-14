use super::*;
use serde_json::json;

fn set(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn paths(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `to_build` with no build running and nothing failed.
fn builds(to: Option<&str>, touched: Option<&[&str]>) -> Option<String> {
    let touched = touched.map(paths);
    to_build(false, &BTreeSet::new(), to, touched.as_deref())
}

/// A night with nothing running, waiting or stopping.
fn idle() -> Idle {
    Idle::default()
}

/// Origin has skill changes the checkout lacks: from `a1` to `b2`.
fn pending() -> (String, String) {
    ("a1".into(), "b2".into())
}

#[test]
fn watch_on_another_repo_never_drains_or_rebuilds() {
    assert_eq!(enabled(false, false, "abc123"), Enabled::Off);
    // With self-update off and no phase on codex, the fill never fetches origin to look.
    assert!(!checks_base(false, false));
    assert!(!ready_to_hand_off(&idle()));
}

#[test]
fn no_self_update_turns_it_off() {
    assert_eq!(enabled(true, true, "abc123"), Enabled::Off);
    assert!(!checks_base(false, false));
    assert_eq!(enabled(false, true, "abc123"), Enabled::From("abc123"));
    assert!(checks_base(true, false));
}

#[test]
fn changes_outside_cli_do_not_drain() {
    assert_eq!(
        builds(Some("b"), Some(&["docs/x.md", "cliff/x", "skills/cli/x"])),
        None
    );
    assert_eq!(builds(Some("b"), Some(&[])), None);
    assert_eq!(
        builds(Some("b"), Some(&["docs/x.md", "cli/x"])),
        Some("b".into())
    );
    assert_eq!(builds(Some("b"), Some(&["cli"])), Some("b".into()));
    // Nothing to build and nothing adopted: the night never stops starting units.
    assert!(!ready_to_hand_off(&idle()));
}

#[test]
fn an_ns_with_no_build_commit_or_one_the_repo_lacks_never_updates() {
    for stamp in ["unknown", ""] {
        assert_eq!(
            enabled(false, true, stamp),
            Enabled::NoBuildCommit,
            "{stamp:?}"
        );
    }
    assert_eq!(build_commit("abc123"), Some("abc123"));
    // A build commit the repo lacks: git can't read the range from it, so nothing builds.
    assert_eq!(builds(Some("b"), None), None);
    assert_eq!(builds(None, Some(&["cli/x"])), None);
}

#[test]
fn a_unit_paused_during_a_build_resumes_and_finishes_before_the_hand_off() {
    let adopted = Idle {
        adopted: true,
        ..idle()
    };
    for waiting in [
        Idle {
            paused: true,
            ..adopted
        },
        Idle {
            resuming: 1,
            ..adopted
        },
        Idle {
            running: 1,
            ..adopted
        },
    ] {
        assert!(!ready_to_hand_off(&waiting), "{waiting:?}");
    }
    assert!(ready_to_hand_off(&adopted));
    assert_eq!(hand_off_step(&adopted, false), HandOffStep::Exec);
}

#[test]
fn a_night_with_one_codex_phase_drains_before_it_syncs_a_skill_change() {
    // One phase of five reads the installed skills.
    let drain = drains_for_skills([false, false, true, false, false]);
    assert!(drain);
    assert!(!syncs_before_unit(drain));
    let to = skills_drain(Some(pending()), &BTreeSet::new());
    assert_eq!(to, Some(pending()));
    // The sync waits until the running unit ends, then runs once.
    let draining = Idle {
        draining: true,
        ..idle()
    };
    assert!(!ready_to_hand_off(&Idle {
        running: 1,
        ..draining
    }));
    assert!(ready_to_hand_off(&draining));
    assert_eq!(hand_off_step(&draining, false), HandOffStep::Sync);
    assert_eq!(skills_drain(Some(pending()), &set(&["b2"])), None);
}

#[test]
fn a_codex_night_syncs_skills_and_hands_off_to_a_new_build_once_its_units_end() {
    let both = Idle {
        draining: true,
        adopted: true,
        ..idle()
    };
    assert!(!ready_to_hand_off(&Idle { running: 1, ..both }));
    assert!(ready_to_hand_off(&both));
    // The sync comes first, then the hand-off on the next pass.
    assert_eq!(hand_off_step(&both, false), HandOffStep::Sync);
    let synced = Idle {
        draining: false,
        ..both
    };
    assert!(ready_to_hand_off(&synced));
    assert_eq!(hand_off_step(&synced, false), HandOffStep::Exec);
}

#[test]
fn a_claude_night_on_nightshift_does_not_drain_for_skills() {
    let drain = drains_for_skills([false; 5]);
    assert!(!drain);
    assert!(syncs_before_unit(drain));
    // An update and a skill change on origin: the fetch is for the update alone.
    assert!(checks_base(true, drain));
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
