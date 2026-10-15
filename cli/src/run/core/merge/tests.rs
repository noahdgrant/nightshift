use serde_json::json;

use super::*;

/// Drive a fresh step under `cfg` through `facts`, each at time 0, and return the acts.
fn drive(cfg: MergeConfig, facts: Vec<MergeFact>) -> Vec<MergeAct> {
    let mut s = MergeState::new(cfg);
    facts.into_iter().map(|f| next(&mut s, f, 0)).collect()
}

/// The facts that bring a step to the start of its first round, PR #12 at `c1`.
fn opened() -> Vec<MergeFact> {
    vec![
        MergeFact::Start,
        MergeFact::Pr {
            ship: true,
            number: Some(12),
        },
        MergeFact::View {
            state: Some("OPEN".into()),
            head: "c1".into(),
        },
        MergeFact::Currency {
            head: "c1".into(),
            current: true,
            review_passes: true,
        },
        MergeFact::Default(None),
    ]
}

#[test]
fn workflow_refusal_needs_both_the_refusal_and_a_workflow() {
    assert!(workflow_refusal(
        "gh pr update-branch 12 failed: GraphQL: refusing to allow an OAuth App to create or update workflow `.github/workflows/ci.yml` without `workflow` scope"
    ));
    assert!(workflow_refusal(
        "refusing to allow a GitHub App to create or update workflow `.github/workflows/ci.yml` without `workflows` permission"
    ));
    assert!(!workflow_refusal("refusing to allow a merge: conflict"));
    assert!(!workflow_refusal("workflow run failed: conflict"));
}

/// The PR is opened in order: pr.md, the forge's view, currency, the default branch; each
/// failure ends the step with its reason.
#[test]
fn an_unmergeable_pr_ends_the_step_before_any_round() {
    let end =
        |facts: Vec<MergeFact>| match &drive(MergeConfig::default(), facts).last().unwrap().ask {
            Ask::End(MergeEnd::Finish {
                outcome, reason, ..
            }) => (*outcome, reason.clone()),
            a => panic!("{a:?}"),
        };
    let mut f = opened();
    f[1] = MergeFact::Pr {
        ship: false,
        number: None,
    };
    assert_eq!(end(f[..2].to_vec()), (Outcome::Done, "no pr.md".into()));
    let mut f = opened();
    f[2] = MergeFact::View {
        state: Some("MERGED".into()),
        head: String::new(),
    };
    assert_eq!(
        end(f[..3].to_vec()),
        (
            Outcome::Stuck,
            "PR merged by run: #12 was merged before ns run's merge step".into()
        )
    );
    let mut f = opened();
    f[3] = MergeFact::Currency {
        head: "c2".into(),
        current: false,
        review_passes: true,
    };
    assert_eq!(
        end(f[..4].to_vec()),
        (
            Outcome::Done,
            "PR head c1 is not current with HEAD c2; needs a human merge".into()
        )
    );
    let acts = drive(MergeConfig::default(), opened());
    assert_eq!(acts.last().unwrap().ask, Ask::FetchTip);
}

#[test]
fn an_overlap_says_whether_the_pr_takes_the_base_in_and_why() {
    let files = |pr: &[&str], base: &[&str]| Overlap::Files {
        pr: pr.iter().map(|s| s.to_string()).collect(),
        base: base.iter().map(|s| s.to_string()).collect(),
    };
    let (ev, need) = needs_update(12, "h", "t", &files(&["a"], &["b"]));
    assert!(!need);
    assert_eq!(
        ev,
        json!({"event": "merge_update_skipped", "pr": 12, "base": "t", "pr_files": ["a"], "base_files": ["b"]})
    );
    let (ev, need) = needs_update(12, "h", "t", &files(&["a", "b"], &["b"]));
    assert!(need);
    assert_eq!(ev["overlap"], json!(["b"]));
    let (ev, need) = needs_update(12, "h", "t", &Overlap::HasTip);
    assert!(need);
    assert_eq!(ev["reason"], "t is already in the PR head h");
    let (ev, need) = needs_update(12, "h", "t", &Overlap::Unreadable("no base".into()));
    assert!(need);
    assert_eq!(
        (ev["overlap"].clone(), ev["error"].clone()),
        (json!(null), json!("no base"))
    );
}

/// Polls back off from 5 s, doubling to at most 30 s, and stop at the deadline.
#[test]
fn a_poll_backs_off_to_its_deadline() {
    let mut s = at_round(MergeConfig {
        register_minutes: 3,
        ..MergeConfig::default()
    });
    let act = next(&mut s, MergeFact::Tip(Ok("t".into())), 0);
    assert_eq!((act.ask, act.poll), (Ask::MergeState, 0));
    let mut now = 0;
    let mut sleeps = vec![];
    loop {
        let act = next(&mut s, MergeFact::MergeState(Some("UNKNOWN".into())), now);
        match act.ask {
            Ask::Sleep(t) => {
                sleeps.push(t);
                now = t;
                let again = next(&mut s, MergeFact::Done, now);
                assert_eq!(again.ask, Ask::MergeState);
                assert_eq!(again.poll as usize, sleeps.len());
            }
            _ => break,
        }
    }
    assert_eq!(sleeps, [5, 15, 35, 65, 95, 125, 155, 180]);
}

/// A step at its first round, past the opening facts, under `cfg`.
fn at_round(cfg: MergeConfig) -> MergeState {
    let mut s = MergeState::new(cfg);
    for f in opened() {
        next(&mut s, f, 0);
    }
    s
}

/// CI ends with `checks` on a clean PR whose checks registered at once.
fn ci_ends(s: &mut MergeState, checks: Vec<Check>) -> MergeAct {
    next(s, MergeFact::Tip(Ok("t".into())), 0);
    next(s, MergeFact::MergeState(Some("CLEAN".into())), 0);
    next(s, MergeFact::CheckCounts([Ok(1), Ok(0)]), 0);
    next(s, MergeFact::Watched { timed_out: false }, 0);
    next(s, MergeFact::Checks(checks), 0)
}

/// A failed check with no run link asks for no log; a lock wait that gives up ends the step as
/// budget; a fact out of turn ends it stuck.
#[test]
fn a_lock_given_up_is_budget_and_a_fact_out_of_turn_is_a_bug() {
    let unheld = || MergeConfig {
        fixes_main: true,
        ..MergeConfig::default()
    };
    let mut s = at_round(unheld());
    let act = ci_ends(&mut s, vec![Check::default()]);
    assert_eq!(act.ask, Ask::FailedLogs(vec![]));
    let mut s = at_round(unheld());
    let pass = Check {
        bucket: Some("pass".into()),
        ..Check::default()
    };
    assert_eq!(ci_ends(&mut s, vec![pass]).ask, Ask::TakeLock);
    let act = next(
        &mut s,
        MergeFact::Locked(Some("gave up at --until".into())),
        0,
    );
    assert_eq!(
        act.ask,
        Ask::End(MergeEnd::Finish {
            outcome: Outcome::Budget,
            reason: "gave up at --until".into(),
            needs_human: false
        })
    );
    let act = drive(MergeConfig::default(), vec![MergeFact::Done]);
    assert!(
        matches!(&act[0].ask, Ask::End(MergeEnd::Finish { outcome: Outcome::Stuck, reason, .. }) if reason.contains("this is a bug in ns")),
        "{act:?}"
    );
}
