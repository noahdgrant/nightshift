use super::super::locks::tests::MIN;
use super::*;
use crate::clock::Clock;
use crate::factory::Factory;

#[test]
fn other_merged_pr_names_only_a_merged_pr_of_another_branch() {
    let pr = |number: u64, merged: bool, head: &str| CommitPr {
        number,
        head: head.into(),
        merged,
    };
    let mine = "ns/7-fix";
    let check = |list: Vec<CommitPr>, own: Option<u64>| other_merged_pr(&list, own, mine);
    assert_eq!(check(vec![pr(3, true, "ns/3-b")], None), Some(3));
    assert_eq!(check(vec![pr(3, true, "ns/3-b")], Some(12)), Some(3));
    assert_eq!(check(vec![], None), None);
    assert_eq!(check(vec![pr(3, false, "ns/3-b")], None), None);
    assert_eq!(check(vec![pr(12, true, "ns/3-b")], Some(12)), None);
    assert_eq!(check(vec![pr(9, true, mine)], None), None);
    assert_eq!(
        check(vec![pr(3, true, "ns/3-b"), pr(12, false, "x")], Some(12)),
        None
    );
    assert_eq!(
        check(vec![pr(3, false, "a"), pr(4, true, "b")], Some(12)),
        Some(4)
    );
}

#[test]
fn a_budget_spent_during_a_lock_wait_stops_the_phase_and_frees_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let names = vec!["a".to_string()];
    let acquire = || {
        RunnerLocks::acquire(
            dir.path(),
            &names,
            "u",
            &Clock::pinned(1000),
            MIN,
            None,
            |_, _| {},
        )
        .unwrap()
        .held()
    };
    let held = recheck_budget(acquire(), Some(5.0), 4.0, "build");
    drop(held.ok().expect("under budget"));
    let Err(f) = recheck_budget(acquire(), Some(5.0), 5.0, "build") else {
        panic!("a spent budget kept the locks")
    };
    assert!(matches!(f.outcome, Outcome::Budget));
    assert_eq!(f.reason, "spent $5.00 of the $5.00 budget");
    assert_eq!(f.phase.as_deref(), Some("build"));
    acquire();
}

/// The phase `ledger` runs next on a unit with no artifacts.
fn next_phase(ledger: &mut Ledger) -> String {
    let dir = tempfile::tempdir().unwrap();
    match ledger
        .next(&read_state(dir.path(), "abc1234".into(), true, None))
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

#[test]
fn a_spent_budget_names_the_spend_and_the_limit() {
    assert_eq!(budget_spent(None, 99.0), None);
    assert_eq!(budget_spent(Some(5.0), 4.99), None);
    assert_eq!(
        budget_spent(Some(5.0), 5.0).as_deref(),
        Some("spent $5.00 of the $5.00 budget")
    );
}

fn ctx_in<'a>(fac: &'a Factory, worktree: &std::path::Path, common: &std::path::Path) -> Ctx<'a> {
    Ctx {
        unit: "u".into(),
        worktree: worktree.to_path_buf(),
        artifacts: worktree.to_path_buf(),
        common: common.to_path_buf(),
        fac,
        root: worktree.to_path_buf(),
        issue: None,
        issue_url: String::new(),
        gates: "auto".into(),
        base: "main".into(),
        gate: crate::run::GateSettings {
            commands: gate::Commands::default(),
            night: None,
        },
        env: crate::run::PhaseEnv {
            lock_dir: common.to_path_buf(),
            cap: None,
            ns: PathBuf::from("ns"),
            tmp: common.to_path_buf(),
        },
    }
}

#[test]
fn a_failed_ls_files_is_logged_and_matches_by_equality_only() {
    let dir = tempfile::tempdir().unwrap();
    let (wt, common) = (dir.path().join("wt"), dir.path().join("common"));
    fs::create_dir_all(&wt).unwrap();
    fs::create_dir_all(&common).unwrap();
    let fac = Factory::default();
    let ctx = ctx_in(&fac, &wt, &common);
    assert!(tracked_files(&ctx).is_empty());
    let log = fs::read_to_string(common.join("ns/runs.jsonl")).unwrap();
    assert!(log.contains("ls_files_failed"), "{log}");
    assert!(gate::same_file("src/main.rs", "src/main.rs", &wt, &[]));
}

#[test]
fn a_gate_path_two_crates_share_blames_neither_suspect() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().join("wt");
    let common = dir.path().join("common");
    fs::create_dir_all(&common).unwrap();
    for c in ["a", "b"] {
        fs::create_dir_all(wt.join(c).join("src")).unwrap();
        fs::write(wt.join(c).join("src/main.rs"), "1\n").unwrap();
    }
    let g = |args: &[&str]| git::run(&wt, args).unwrap();
    g(&["init", "-q", "-b", "main"]);
    g(&["config", "user.email", "t@example.com"]);
    g(&["config", "user.name", "t"]);
    g(&["config", "commit.gpgsign", "false"]);
    g(&["add", "-A"]);
    g(&["commit", "-q", "-m", "base"]);
    let from = g(&["rev-parse", "HEAD"]);
    fs::write(wt.join("a/src/main.rs"), "2\n").unwrap();
    g(&["add", "-A"]);
    g(&["commit", "-q", "-m", "build"]);
    let to = g(&["rev-parse", "HEAD"]);
    let mut ledger = Ledger::default();
    ledger.head_moved("build", &from, &to);
    let fac = Factory::default();
    let ctx = ctx_in(&fac, &wt, &common);
    assert_eq!(
        culprit(&ctx, &ledger, "src/main.rs:1:1: error: boom\n"),
        None
    );
    let hit = culprit(&ctx, &ledger, "a/src/main.rs:1:1: error: boom\n");
    assert_eq!(hit.map(|h| h.1), Some("build".to_string()));
}
