use std::fs;
use std::path::PathBuf;

use super::*;
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

pub(in crate::run) fn ctx_in<'a>(
    fac: &'a Factory,
    worktree: &std::path::Path,
    common: &std::path::Path,
) -> Ctx<'a> {
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
