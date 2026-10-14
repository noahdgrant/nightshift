use super::*;

fn empty() -> State {
    State {
        arts: BTreeMap::new(),
        head: "abc1234".into(),
        has_issue: true,
        unit_diff: None,
    }
}

fn admit(l: &mut Ledger, phase: &'static str) -> Result<Admitted, Denied> {
    l.admit(phase, String::new(), "why".into(), None, None, 2)
}

fn ended(l: &mut Ledger, a: &Admitted, status: Option<&str>) {
    l.phase_ended(
        a,
        &PhaseResult {
            status: status.map(str::to_string),
            ..PhaseResult::default()
        },
    );
}

fn remote(sha: &str) -> Option<(String, String)> {
    Some(("main".into(), sha.into()))
}

#[test]
fn a_phase_gets_max_attempts_then_the_reason_names_the_last_failure() {
    let mut l = Ledger::new(None);
    for _ in 0..2 {
        let a = admit(&mut l, "build").unwrap();
        l.phase_ended(
            &a,
            &PhaseResult {
                timed_out: true,
                timeout_text: "timed out after 90 min".into(),
                ..PhaseResult::default()
            },
        );
    }
    assert_eq!(
        admit(&mut l, "build").unwrap_err().reason,
        "build is out of attempts (2): the last attempt timed out after 90 min; raise its timeout_minutes or split the unit"
    );
}

#[test]
fn a_clean_attempt_clears_the_last_failure() {
    let mut l = Ledger::new(None);
    let a = admit(&mut l, "build").unwrap();
    l.phase_ended(
        &a,
        &PhaseResult {
            killed: Some("killed by the memory cap".into()),
            ..PhaseResult::default()
        },
    );
    let a = admit(&mut l, "build").unwrap();
    ended(&mut l, &a, Some("fail"));
    assert_eq!(
        admit(&mut l, "build").unwrap_err().reason,
        "build is out of attempts (2)"
    );
}

#[test]
fn the_first_verify_or_review_after_a_pass_is_free() {
    for phase in ["verify", "review"] {
        let mut l = Ledger::new(None);
        let a = admit(&mut l, phase).unwrap();
        ended(&mut l, &a, Some("pass"));
        let a = admit(&mut l, phase).unwrap();
        assert_eq!(a.attempt, 2);
        ended(&mut l, &a, Some("fail"));
        let a = admit(&mut l, phase).unwrap();
        assert_eq!(a.attempt, 3, "{phase}: the rerun after a pass cost nothing");
        ended(&mut l, &a, Some("fail"));
        assert!(admit(&mut l, phase).is_err(), "{phase}");
    }
}

#[test]
fn a_ship_rerun_after_a_pass_still_costs_an_attempt() {
    let mut l = Ledger::new(None);
    for _ in 0..2 {
        let a = admit(&mut l, "ship").unwrap();
        ended(&mut l, &a, Some("pass"));
    }
    assert!(admit(&mut l, "ship").is_err());
}

#[test]
fn a_rebase_onto_a_new_base_is_free_and_onto_the_same_base_is_not() {
    let mut l = Ledger::new(None);
    for _ in 0..2 {
        let a = admit(&mut l, "build").unwrap();
        ended(&mut l, &a, Some("fail"));
    }
    let free = |l: &mut Ledger, sha: &str| {
        l.admit(
            "build",
            String::new(),
            "base moved".into(),
            Some(Rebase::BaseMoved),
            remote(sha),
            2,
        )
    };
    let a = free(&mut l, "s1").unwrap();
    assert_eq!(
        a.rebase,
        Some((Rebase::BaseMoved, "main".into(), "s1".into()))
    );
    ended(&mut l, &a, Some("pass"));
    let a = free(&mut l, "s2").unwrap();
    ended(&mut l, &a, Some("pass"));
    assert!(
        free(&mut l, "s2").is_err(),
        "a second rebase onto the same sha used an attempt"
    );
}

#[test]
fn a_free_rebase_that_does_not_pass_gives_its_credit_back() {
    let mut l = Ledger::new(None);
    let a = admit(&mut l, "build").unwrap();
    ended(&mut l, &a, Some("fail"));
    let a = l
        .admit(
            "build",
            String::new(),
            "base moved".into(),
            Some(Rebase::BaseMoved),
            remote("s1"),
            2,
        )
        .unwrap();
    ended(&mut l, &a, Some("fail"));
    assert!(admit(&mut l, "build").is_err());
}

#[test]
fn a_rebase_with_no_remote_is_charged() {
    let mut l = Ledger::new(None);
    let a = l
        .admit(
            "build",
            String::new(),
            "base moved".into(),
            Some(Rebase::BaseMoved),
            None,
            2,
        )
        .unwrap();
    assert_eq!(a.rebase, None);
}

#[test]
fn a_lock_give_up_counts_the_attempt_returns_credits_and_reruns_the_decision() {
    let mut l = Ledger::new(None);
    let a = l
        .admit(
            "build",
            "fb".into(),
            "base moved".into(),
            Some(Rebase::BaseMoved),
            remote("s1"),
            2,
        )
        .unwrap();
    l.lock_gave_up(a, "waited 90 min for lock bench");
    let (d, r) = l.next(&empty());
    assert_eq!(d, run("build", "fb", "base moved"));
    assert_eq!(r, Some(Rebase::BaseMoved));
    let a = l
        .admit(
            "build",
            "fb".into(),
            "base moved".into(),
            r,
            remote("s1"),
            2,
        )
        .unwrap();
    assert_eq!(a.attempt, 2);
    assert!(
        a.rebase.is_some(),
        "the credit came back, so s1 is new again"
    );
}

#[test]
fn a_ship_after_a_free_ships_reruns_is_free() {
    let mut l = Ledger::new(None);
    let a = l
        .admit(
            "ship",
            String::new(),
            "conflict".into(),
            Some(Rebase::BaseMoved),
            remote("s1"),
            1,
        )
        .unwrap();
    ended(&mut l, &a, Some("pass"));
    let a = l
        .admit("ship", String::new(), "stale".into(), None, None, 1)
        .unwrap();
    assert!(a.follows_base);
}

#[test]
fn a_build_between_ships_cancels_the_free_ship() {
    let mut l = Ledger::new(None);
    let a = l
        .admit(
            "ship",
            String::new(),
            "conflict".into(),
            Some(Rebase::BaseMoved),
            remote("s1"),
            1,
        )
        .unwrap();
    ended(&mut l, &a, Some("pass"));
    admit(&mut l, "build").unwrap();
    let a = l
        .admit("ship", String::new(), "stale".into(), None, None, 2)
        .unwrap();
    assert!(!a.follows_base);
}

#[test]
fn the_merge_step_sends_ship_back_up_to_the_cap() {
    let mut l = Ledger::new(None);
    for _ in 0..RESHIP_CAP {
        l.merge_back(MergeBack::Reship("conflict".into())).unwrap();
        let (d, r) = l.next(&empty());
        assert_eq!(d, run("ship", "conflict", "the PR conflicts with its base"));
        assert_eq!(r, Some(Rebase::BaseMoved));
    }
    assert_eq!(
        l.merge_back(MergeBack::Reship("conflict".into()))
            .unwrap_err(),
        "ship was sent back 5 times because the base moved (cap 5)"
    );
}

#[test]
fn a_red_merge_step_rebuilds_and_records_why_it_would_stick() {
    let mut l = Ledger::new(None);
    l.merge_back(MergeBack::Rebuild {
        feedback: "CI red".into(),
        reason: "the PR's checks failed".into(),
        stuck_reason: Some("CI failed on test x".into()),
    })
    .unwrap();
    assert_eq!(
        l.next(&empty()),
        (run("build", "CI red", "the PR's checks failed"), None)
    );
    for _ in 0..2 {
        let a = admit(&mut l, "build").unwrap();
        ended(&mut l, &a, None);
    }
    assert_eq!(
        admit(&mut l, "build").unwrap_err().reason,
        "build is out of attempts (2)",
        "a clean build since the red merge step cleared its reason"
    );
}

#[test]
fn a_red_gate_after_a_rebase_onto_a_base_that_moved_again_is_one_more_free_rebase() {
    let mut l = Ledger::new(None);
    let a = l
        .admit(
            "build",
            String::new(),
            "base moved".into(),
            Some(Rebase::BaseMoved),
            remote("s1"),
            2,
        )
        .unwrap();
    ended(&mut l, &a, Some("pass"));
    l.gate_red(&a, "red", "the CI gate failed".into(), None, true);
    let (d, r) = l.next(&empty());
    assert_eq!(
        d,
        run(
            "build",
            "red",
            "the CI gate failed after a rebase and main moved since"
        )
    );
    assert_eq!(r, Some(Rebase::MovedSinceRebase));
    l.gate_red(&a, "red", "the CI gate failed".into(), None, false);
    assert_eq!(
        l.next(&empty()),
        (run("build", "red", "the CI gate failed"), None)
    );
}

#[test]
fn from_forces_the_first_phase_and_then_the_table_decides() {
    let mut l = Ledger::new(Some("review"));
    assert_eq!(l.next(&empty()), (run("review", "", "--from"), None));
    assert_eq!(l.next(&empty()), (run("triage", "", "no brief.md"), None));
}

#[test]
fn a_ledger_survives_a_round_trip_through_json_mid_run() {
    let mut l = Ledger::new(None);
    let a = l
        .admit(
            "ship",
            String::new(),
            "conflict".into(),
            Some(Rebase::BaseMoved),
            remote("s1"),
            2,
        )
        .unwrap();
    ended(&mut l, &a, Some("pass"));
    l.merge_back(MergeBack::Reship("again".into())).unwrap();
    let back: Ledger = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
    assert_eq!(back, l);
    let mut back = back;
    assert_eq!(
        back.next(&empty()),
        (
            run("ship", "again", "the PR conflicts with its base"),
            Some(Rebase::BaseMoved)
        )
    );
}
