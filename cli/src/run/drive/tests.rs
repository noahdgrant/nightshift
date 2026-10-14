use super::*;
use crate::run::core::route::Decision;

/// The phase `ledger` runs next on a unit with no artifacts.
fn next_phase(ledger: &mut Ledger) -> String {
    let dir = tempfile::tempdir().unwrap();
    match ledger
        .next(&read_state(
            dir.path(),
            "abc1234".into(),
            true,
            None,
            Default::default(),
        ))
        .0
    {
        Decision::Run { phase, .. } => phase.to_string(),
        d => format!("{d:?}"),
    }
}

fn save(dir: &std::path::Path, ledger: &Ledger) {
    fs::write(ledger_path(dir), serde_json::to_vec(ledger).unwrap()).unwrap();
}

#[test]
fn a_saved_ledger_resumes_and_a_missing_one_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let resume = RunArgs::default();
    let (mut l, events) = start_ledger(&resume, dir.path(), "abc1234");
    assert_eq!((next_phase(&mut l).as_str(), events.len()), ("triage", 0));
    let mut ledger = Ledger::new(Some("review"));
    ledger.saved_at("abc1234");
    save(dir.path(), &ledger);
    let (mut l, events) = start_ledger(&resume, dir.path(), "abc1234");
    assert_eq!(next_phase(&mut l), "review");
    assert_eq!(events[0]["event"], "ledger_resumed");
    assert_eq!(events[0]["forced_dropped"], false);
    for args in [
        RunArgs {
            fresh: true,
            ..RunArgs::default()
        },
        RunArgs {
            from: Some("build".into()),
            ..RunArgs::default()
        },
    ] {
        let (mut l, events) = start_ledger(&args, dir.path(), "abc1234");
        assert_ne!(next_phase(&mut l), "review", "{args:?}");
        assert!(events.is_empty(), "{args:?}");
    }
}

#[test]
fn a_forced_decision_saved_at_another_head_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let mut ledger = Ledger::new(Some("review"));
    ledger.saved_at("abc1234");
    save(dir.path(), &ledger);
    let (mut l, events) = start_ledger(&RunArgs::default(), dir.path(), "def5678");
    assert_eq!(next_phase(&mut l), "triage");
    assert_eq!(events[0]["event"], "ledger_resumed");
    assert_eq!(events[0]["forced_dropped"], true);
    save(dir.path(), &Ledger::new(Some("review")));
    let (mut l, _) = start_ledger(&RunArgs::default(), dir.path(), "abc1234");
    assert_eq!(next_phase(&mut l), "triage", "saved at no recorded HEAD");
}

#[test]
fn a_ledger_with_nothing_forced_resumes_at_any_head() {
    let dir = tempfile::tempdir().unwrap();
    let mut ledger = Ledger::new(None);
    ledger.saved_at("abc1234");
    save(dir.path(), &ledger);
    let (_, events) = start_ledger(&RunArgs::default(), dir.path(), "def5678");
    assert_eq!(events[0]["forced_dropped"], false);
}

#[test]
fn an_unreadable_ledger_is_rederived_and_logged() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(ledger_path(dir.path())).unwrap();
    let (_, events) = start_ledger(&RunArgs::default(), dir.path(), "abc1234");
    assert_eq!(events[0]["event"], "state_rederived", "{events:?}");
}

#[test]
fn a_torn_ledger_is_rederived_and_logged() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(ledger_path(dir.path()), "{\"attempts\": {\"bui").unwrap();
    let (mut l, events) = start_ledger(&RunArgs::default(), dir.path(), "abc1234");
    assert_eq!(next_phase(&mut l), "triage");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["event"], "state_rederived");
    assert!(events[0]["error"].as_str().is_some_and(|e| !e.is_empty()));
}
