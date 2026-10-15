use super::*;
use crate::run::core::route::{
    base_moved, decide, first_line, rebases_for_base, Carried, Decision,
};
use crate::testutil::{commit_file, g, rebased_unit, Rebased};

fn art(status: &str, sha: &str) -> Art {
    Art {
        status: status.into(),
        sha: Some(sha.into()),
        pr: None,
        body: format!("{status} body"),
        invalid: None,
        next: None,
        findings: vec![],
    }
}

fn state(arts: &[(&'static str, Art)]) -> State {
    State {
        arts: arts.iter().cloned().collect(),
        head: "abc1234".into(),
        has_issue: true,
        ..State::default()
    }
}

fn phase_of(d: &Decision) -> &str {
    match d {
        Decision::Run { phase, .. } => phase,
        Decision::Done => "done",
        Decision::Stuck(_) | Decision::LoopSpent(_) => "stuck",
        Decision::Split(_) => "split",
    }
}

fn next(arts: &[(&'static str, Art)]) -> String {
    phase_of(&decide(&state(arts))).to_string()
}

#[test]
fn stuck_reason_is_the_first_body_line() {
    let blocked = |body: &str| {
        let mut a = art("blocked", "abc1234");
        a.body = body.into();
        match decide(&state(&[("verify", a)])) {
            Decision::Stuck(r) => r,
            _ => panic!("not stuck"),
        }
    };
    assert_eq!(
        blocked("\n  Needs a board on the bench.  \nlater text\n"),
        "evidence.md is blocked: Needs a board on the bench."
    );
    // A heading marker is dropped and its text kept: the first line is the blocker even
    // when it is written as a heading, and later body text is never used.
    assert_eq!(
        blocked("## Needs hardware\n\nlater text\n"),
        "evidence.md is blocked: Needs hardware"
    );
    let long = blocked(&"x".repeat(300));
    assert_eq!(long, format!("evidence.md is blocked: {}", "x".repeat(200)));
}

#[test]
fn a_split_brief_ends_the_unit_with_its_first_line() {
    let mut brief = art("split", "");
    brief.body = "\nSplit into #8 and #9.\nIssue: https://github.com/o/r/issues/7\n".into();
    assert_eq!(
        decide(&state(&[("triage", brief.clone())])),
        Decision::Split("brief.md is split: Split into #8 and #9.".into())
    );
    // The children replaced the unit, so an artifact a later phase left behind changes
    // nothing.
    let later = ("build", art("blocked", "abc1234"));
    assert_eq!(next(&[("triage", brief), later]), "split");
}

#[test]
fn a_split_at_the_cap_is_blocked_and_stuck_on_its_reason() {
    let mut brief = art("blocked", "");
    brief.body =
        "Six behaviours, over the split cap of 5: a human must split it.\nIssue: u\n".into();
    assert_eq!(
        decide(&state(&[("triage", brief)])),
        Decision::Stuck(
            "brief.md is blocked: Six behaviours, over the split cap of 5: a human must split it."
                .into()
        )
    );
}

#[test]
fn only_a_passed_build_sent_back_for_a_moved_base_rebases() {
    let moved = |status: &str| {
        let mut a = art(status, "abc1234");
        a.body = "Base moved: rebase onto main\n".into();
        a
    };
    let brief = ("triage", art("pass", ""));
    let rebases = |arts: &[(&'static str, Art)]| {
        let s = state(arts);
        rebases_for_base(&s, &decide(&s))
    };
    let built = ("build", art("pass", "abc1234"));
    assert!(rebases(&[
        brief.clone(),
        built.clone(),
        ("verify", moved("fail"))
    ]));
    let reviewed = ("verify", art("pass", "abc1234"));
    assert!(rebases(&[
        brief.clone(),
        built.clone(),
        reviewed,
        ("review", moved("fail"))
    ]));
    assert!(!rebases(&[
        brief.clone(),
        built.clone(),
        ("verify", art("fail", "abc1234"))
    ]));
    // Build's own fail is a failed attempt, whatever it says.
    assert!(!rebases(&[brief.clone(), ("build", moved("fail"))]));
    // The line names the base moving only in its first line.
    let mut later = art("fail", "abc1234");
    later.body = "Tests fail\nbase moved too\n".into();
    assert!(!rebases(&[brief.clone(), built.clone(), ("verify", later)]));
    let mut mid = art("fail", "abc1234");
    mid.body = "Tests fail; not because the base moved\n".into();
    assert!(!rebases(&[brief, built, ("verify", mid)]));
}

#[test]
fn state_table() {
    assert_eq!(next(&[]), "triage");
    let mut s = state(&[]);
    s.has_issue = false;
    assert_eq!(phase_of(&decide(&s)), "stuck");
    let brief = ("triage", art("pass", ""));
    let build = ("build", art("pass", "abc1234"));
    let ev = ("verify", art("pass", "abc12"));
    let rv = ("review", art("pass", "abc1234"));
    assert_eq!(next(std::slice::from_ref(&brief)), "build");
    assert_eq!(next(&[brief.clone(), ("build", art("fail", ""))]), "build");
    assert_eq!(next(&[brief.clone(), build.clone()]), "verify");
    // A shorter sha prefix still matches HEAD.
    assert_eq!(next(&[brief.clone(), build.clone(), ev.clone()]), "review");
    let stale = ("verify", art("pass", "fff0000"));
    assert_eq!(next(&[brief.clone(), build.clone(), stale]), "verify");
    let d = decide(&state(&[
        brief.clone(),
        build.clone(),
        ("verify", art("fail", "abc1234")),
    ]));
    assert!(
        matches!(&d, Decision::Run { phase: "build", feedback, .. } if feedback == "fail body")
    );
    let d = decide(&state(&[
        brief.clone(),
        build.clone(),
        ev.clone(),
        ("review", art("fail", "abc1234")),
    ]));
    assert!(matches!(&d, Decision::Run { phase: "build", .. }));
    let base = [brief.clone(), build.clone(), ev.clone(), rv.clone()];
    assert_eq!(next(&base), "ship");
    let with = |extra: (&'static str, Art)| {
        let mut v = base.to_vec();
        v.push(extra);
        next(&v)
    };
    let mut sf = art("fail", "abc1234");
    sf.body = "evidence is stale; ns-review must rerun".into();
    assert_eq!(with(("ship", sf)), "review");
    assert_eq!(with(("ship", art("fail", "abc1234"))), "stuck");
    let mut moved = art("fail", "abc1234");
    moved.body = "NO-GO: base moved, the branch conflicts with main\n".into();
    assert_eq!(with(("ship", moved)), "build");
    assert_eq!(with(("ship", art("pass", "abc1234"))), "done");
    let old = [
        brief.clone(),
        build.clone(),
        ("verify", art("pass", "0000000")),
        ("review", art("pass", "0000000")),
        ("ship", art("pass", "abc1234")),
    ];
    assert_eq!(next(&old), "verify");
    // A pass at an older sha ships again once verify and review hold at HEAD.
    assert_eq!(with(("ship", art("pass", "0000000"))), "ship");
    assert_eq!(
        next(&[
            brief.clone(),
            build.clone(),
            ev.clone(),
            ("review", art("blocked", "abc1234"))
        ]),
        "stuck"
    );
    assert_eq!(next(&[("triage", art("fail", ""))]), "stuck");
}

#[test]
fn archiving_keeps_only_current_downstream_passes() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let w = |f: &str, status: &str, sha: &str| {
        fs::write(
            d.join(f),
            format!("---\nstatus: {status}\nsha: {sha}\npr: 12\n---\nbody\n"),
        )
        .unwrap()
    };
    w("build.md", "pass", "abc1234");
    w("evidence.md", "pass", "abc1234");
    w("review.md", "fail", "abc1234");
    w("pr.md", "pass", "0000000");
    let moves = archive_for(d, "build", &state(&[])).unwrap();
    assert!(!d.join("build.md").exists());
    assert!(d.join("evidence.md").exists());
    assert!(!d.join("review.md").exists());
    assert!(d.join("history/build-1.md").exists());
    assert!(d.join("history/review-1.md").exists());
    assert!(d.join("history/pr-1.md").exists());
    assert_eq!(known_pr(d), Some(12));
    restore(&moves);
    assert!(d.join("build.md").exists() && d.join("pr.md").exists());
    // A restored slot is free again; a second archive takes the next number.
    archive_for(d, "build", &state(&[])).unwrap();
    w("build.md", "pass", "abc1234");
    archive_for(d, "build", &state(&[])).unwrap();
    assert!(d.join("history/build-1.md").exists());
    assert!(d.join("history/build-2.md").exists());
}

fn pr_art(dir: &Path, name: &str, pr: u64, updated: &str) {
    fs::create_dir_all(dir.join("history")).unwrap();
    fs::write(
        dir.join(name),
        format!(
            "---\nunit: u\nphase: ship\nstatus: pass\nsha: abc1234\npr: https://github.com/o/r/pull/{pr}\nupdated: {updated}\n---\nbody\n"
        ),
    )
    .unwrap();
}

#[test]
fn known_pr_reads_timed_out_archives_newest_first() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    assert_eq!(known_pr(d), None);
    fs::create_dir_all(d.join("history")).unwrap();
    assert_eq!(known_pr(d), None);
    pr_art(d, "history/pr-timeout-1.md", 21, "2026-10-08T01:00:00Z");
    assert_eq!(known_pr(d), Some(21));
    pr_art(d, "history/pr-2.md", 22, "2026-10-08T02:00:00Z");
    assert_eq!(known_pr(d), Some(22));
    pr_art(d, "history/pr-timeout-3.md", 23, "2026-10-08T03:00:00Z");
    assert_eq!(known_pr(d), Some(23));
    pr_art(d, "pr.md", 24, "2026-10-08T04:00:00Z");
    assert_eq!(known_pr(d), Some(24));
}

#[test]
fn known_pr_picks_the_latest_archive_of_either_kind() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    pr_art(d, "history/pr-1.md", 21, "2026-10-08T01:00:00Z");
    pr_art(d, "history/pr-2.md", 22, "2026-10-08T02:00:00Z");
    pr_art(d, "history/pr-timeout-1.md", 23, "2026-10-08T03:00:00Z");
    assert_eq!(known_pr(d), Some(23));
    // The reverse: a later pr-<n> beats an earlier pr-timeout-<m> with m > n.
    pr_art(d, "history/pr-timeout-5.md", 25, "2026-10-08T04:00:00Z");
    pr_art(d, "history/pr-3.md", 26, "2026-10-08T05:00:00Z");
    assert_eq!(known_pr(d), Some(26));
}

#[test]
fn known_pr_breaks_updated_ties_by_counter_and_ranks_undated_oldest() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    let same = "2026-10-08T01:00:00Z";
    pr_art(d, "history/pr-2.md", 22, same);
    pr_art(d, "history/pr-timeout-1.md", 21, same);
    assert_eq!(known_pr(d), Some(22));
    pr_art(d, "history/pr-timeout-9.md", 29, "");
    assert_eq!(known_pr(d), Some(22));
}

#[test]
fn known_pr_breaks_updated_and_counter_ties_by_file_name() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    let same = "2026-10-08T01:00:00Z";
    pr_art(d, "history/pr-1.md", 21, same);
    pr_art(d, "history/pr-timeout-1.md", 23, same);
    assert_eq!(known_pr(d), Some(23));
}

#[test]
fn known_pr_prefers_pr_md_over_a_newer_archive() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    pr_art(d, "pr.md", 24, "2026-10-08T01:00:00Z");
    pr_art(d, "history/pr-timeout-1.md", 21, "2026-10-08T05:00:00Z");
    assert_eq!(known_pr(d), Some(24));
}

#[test]
fn known_pr_skips_an_archive_whose_pr_does_not_parse() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    pr_art(d, "history/pr-1.md", 21, "2026-10-08T01:00:00Z");
    fs::write(
        d.join("history/pr-timeout-1.md"),
        "---\nstatus: fail\npr: none\nupdated: 2026-10-08T02:00:00Z\n---\nbody\n",
    )
    .unwrap();
    assert_eq!(known_pr(d), Some(21));
}

/// The unit's state at `head`, with the diff ids of `shas` read against `main`.
fn state_at(r: &Rebased, head: &str, shas: &[&str]) -> State {
    let mut s = State {
        head: head.to_string(),
        has_issue: true,
        diffs: Some(Diffs {
            head: git::diff_id(&r.dir, "main", head),
            of: BTreeMap::new(),
        }),
        ..State::default()
    };
    for sha in shas {
        learn_sha(&mut s, &r.dir, "main", sha);
    }
    s
}

fn real_state(r: &Rebased) -> State {
    state_at(r, &r.rebased, &[&r.reviewed])
}

#[test]
fn current_matches_head_a_rebased_diff_and_nothing_else() {
    let r = rebased_unit();
    let first = g(&r.dir, &["rev-parse", "main"]);
    let s = state_at(&r, &r.rebased, &[&r.reviewed, "0000000", &first]);
    assert!(s.current(&r.rebased));
    assert!(s.current(&r.rebased[..7]));
    assert!(s.current(&r.reviewed));
    assert!(!s.current("0000000"));
    assert!(!s.current(""));
    assert!(!s.current(&first));
    let mut no_base = real_state(&r);
    no_base.diffs = None;
    assert!(!no_base.current(&r.reviewed));
}

#[test]
fn decide_is_done_for_artifacts_at_the_pre_rebase_sha() {
    let r = rebased_unit();
    let mut s = real_state(&r);
    for p in ["triage", "build", "verify", "review", "ship"] {
        s.arts.insert(p, art("pass", &r.reviewed));
    }
    assert_eq!(decide(&s), Decision::Done);
    s.diffs = None;
    assert_ne!(decide(&s), Decision::Done);
}

#[test]
fn a_rebase_archives_nothing_downstream() {
    let r = rebased_unit();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for f in ["evidence.md", "review.md", "pr.md"] {
        fs::write(
            d.join(f),
            format!("---\nstatus: pass\nsha: {}\npr: 12\n---\n", r.reviewed),
        )
        .unwrap();
    }
    let moves = archive_for(d, "verify", &real_state(&r)).unwrap();
    assert_eq!(moves.moved.len(), 1);
    assert!(!d.join("evidence.md").exists());
    assert!(d.join("review.md").exists() && d.join("pr.md").exists());
    let moves = archive_for(d, "review", &real_state(&r)).unwrap();
    assert_eq!(moves.moved.len(), 1);
    assert!(d.join("pr.md").exists());
    fs::write(
        d.join("review.md"),
        format!("---\nstatus: pass\nsha: {}\n---\n", r.reviewed),
    )
    .unwrap();
    commit_file(&r.dir, "work.txt", "two\n");
    let head = g(&r.dir, &["rev-parse", "HEAD"]);
    let changed = state_at(&r, &head, &[&r.reviewed]);
    archive_for(d, "verify", &changed).unwrap();
    assert!(!d.join("review.md").exists() && !d.join("pr.md").exists());
}

#[test]
fn archiving_drops_a_current_pr_behind_a_stale_review() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for (f, sha) in [
        ("evidence.md", "0000000"),
        ("review.md", "0000000"),
        ("pr.md", "abc1234"),
    ] {
        fs::write(d.join(f), format!("---\nstatus: pass\nsha: {sha}\n---\n")).unwrap();
    }
    archive_for(d, "verify", &state(&[])).unwrap();
    assert!(d.join("history/review-1.md").exists());
    assert!(d.join("history/pr-1.md").exists());
}

#[test]
fn sha_and_pr_are_read_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("evidence.md");
    for (sha, want) in [
        ("0e05787", "0e05787"),
        ("1234567", "1234567"),
        ("\"00ab12c\"", "00ab12c"),
        ("3f9c2e1   # HEAD", "3f9c2e1"),
    ] {
        fs::write(
            &p,
            format!("---\nstatus: pass\nsha: {sha}\npr: 0012\n---\nbody\n"),
        )
        .unwrap();
        let a = read_art(&p).unwrap();
        assert_eq!(a.sha.as_deref(), Some(want));
        assert_eq!(a.pr.as_deref(), Some("0012"));
        assert_eq!(a.status, "pass");
    }
}

#[test]
fn carried_evidence_skips_verify_and_carried_review_runs_a_delta_review() {
    let carried = |phases: &[&'static str]| {
        let mut s = state(&[
            ("triage", art("pass", "abc1234")),
            ("build", art("pass", "abc1234")),
            ("verify", art("pass", "old0001")),
            ("review", art("pass", "old0001")),
            ("ship", art("pass", "abc1234")),
        ]);
        for p in phases {
            s.carried.insert(
                p,
                Carried {
                    from: "old0001".into(),
                    lines: 3,
                },
            );
        }
        s
    };
    assert_eq!(phase_of(&decide(&carried(&[]))), "verify");
    let d = decide(&carried(&["verify", "review"]));
    assert_eq!(phase_of(&d), "review");
    let Decision::Run { feedback, why, .. } = d else {
        unreachable!()
    };
    assert!(
        feedback.starts_with("Delta review. review.md passed at old0001."),
        "{feedback}"
    );
    assert!(feedback.contains("delta_of: old0001"), "{feedback}");
    assert_eq!(
        why,
        "review.md sha old0001 is behind a 3-line change the base brought in: delta review"
    );
    let mut s = carried(&["verify"]);
    s.arts.insert("review", art("pass", "abc1234"));
    assert_eq!(decide(&s), Decision::Done);
}

#[test]
fn a_failed_artifact_is_never_carried() {
    let mut s = state(&[
        ("triage", art("pass", "abc1234")),
        ("build", art("pass", "abc1234")),
        ("verify", art("fail", "old0001")),
    ]);
    s.carried.insert(
        "verify",
        Carried {
            from: "old0001".into(),
            lines: 1,
        },
    );
    assert_eq!(phase_of(&decide(&s)), "verify");
}

fn v2(dir: &Path, file: &str, phase: &str, status: &str, cause: &str, next: &str, extra: &str) {
    fs::write(
        dir.join(file),
        format!(
            "---\nschema: ns-artifact/2\nunit: 7-x\nphase: {phase}\nstatus: {status}\ncause: {cause}\nnext: {next}\nreason: \"the reason\"\nsha: abc1234\n{extra}updated: 2026-10-14T10:00:00Z\n---\n\n# Body\n\nreview this, then verify\n"
        ),
    )
    .unwrap();
}

#[test]
fn a_v2_artifact_routes_on_its_cause_and_next() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    v2(
        d,
        "evidence.md",
        "verify",
        "fail",
        "base_moved",
        "build",
        "",
    );
    let a = read_art(&d.join("evidence.md")).unwrap();
    assert!(base_moved(&a.body), "{}", a.body);
    v2(d, "pr.md", "ship", "fail", "criteria_failed", "verify", "");
    let a = read_art(&d.join("pr.md")).unwrap();
    assert_eq!(a.next.as_deref(), Some("verify"));
    v2(
        d,
        "review.md",
        "review",
        "blocked",
        "needs_human",
        "human",
        "",
    );
    let a = read_art(&d.join("review.md")).unwrap();
    assert_eq!(first_line(&a.body), "the reason");
    assert_eq!(
        (a.status.as_str(), a.sha.as_deref()),
        ("blocked", Some("abc1234"))
    );
}

#[test]
fn an_invalid_v2_artifact_counts_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    v2(d, "evidence.md", "verify", "fail", "nonsense", "build", "");
    let a = read_art(&d.join("evidence.md")).unwrap();
    assert!(a.invalid.as_ref().is_some_and(|e| !e.is_empty()), "{a:?}");
    let s = read_state(d, "abc1234".into(), true, None, Settings::default());
    assert!(!s.arts.contains_key("verify"));
}

#[test]
fn a_v2_artifacts_next_routes_it() {
    let with_next = |phase: &'static str, status: &str, next: &str| {
        let mut s = state(&[
            ("triage", art("pass", "abc1234")),
            ("build", art("pass", "abc1234")),
            ("verify", art("pass", "abc1234")),
            ("review", art("pass", "abc1234")),
        ]);
        let mut a = art(status, "abc1234");
        a.next = Some(next.into());
        s.arts.insert(phase, a);
        decide(&s)
    };
    for (n, want) in [
        ("verify", "verify"),
        ("review", "review"),
        ("ship", "ship"),
        ("build", "build"),
    ] {
        assert_eq!(
            phase_of(&with_next("ship", "fail", n)),
            want,
            "pr.md next: {n}"
        );
    }
    assert!(matches!(
        with_next("ship", "fail", "human"),
        Decision::Stuck(_)
    ));
    assert!(matches!(
        with_next("verify", "fail", "human"),
        Decision::Stuck(_)
    ));
    assert_eq!(phase_of(&with_next("review", "fail", "build")), "build");
    let mut s = state(&[("triage", art("pass", "abc1234"))]);
    s.arts.get_mut("triage").unwrap().next = Some("define".into());
    let Decision::Stuck(r) = decide(&s) else {
        panic!("a brief routed to define was built")
    };
    assert!(r.starts_with("triage routed the issue to define"), "{r}");
}

/// A unit mid review loop at `abc1234`: review.md left I1 open with one fix attempt, fix cycle 1
/// wrote fix.md, and `ns`'s record holds the first pass and the fix.
fn mid_loop(d: &Path) {
    v2(
        d,
        "review.md",
        "review",
        "fail",
        "findings_open",
        "review",
        "findings: [I1]\n",
    );
    let body = "\n## Important\n### I1. t\n- Location: `a.rs:1`\n- Scope: changed\n- Cycle: 0\n- Attempts: 1\n- Status: open\n### I2. u\n- Location: `a.rs:2`\n- Scope: changed\n- Cycle: 0\n- Status: open\n";
    let text = fs::read_to_string(d.join("review.md")).unwrap();
    fs::write(d.join("review.md"), text + body).unwrap();
    v2(
        d,
        "fix.md",
        "review-fix",
        "pass",
        "none",
        "review",
        "findings: [I1]\n",
    );
    for (phase, cycle, fresh) in [("review", 0, true), ("review-fix", 1, false)] {
        let run = crate::review_runs::ReviewRun {
            phase: phase.into(),
            cycle,
            kind: if cycle == 0 { "first" } else { "full" }.into(),
            started: "2026-10-14T10:00:00Z".into(),
            ended: "2026-10-14T10:05:00Z".into(),
            sha: "abc1234".into(),
        };
        crate::review_runs::append(d, run, fresh).unwrap();
    }
}

#[test]
fn the_state_reads_fix_md_and_the_review_loop() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    let s = read_state(d, "abc1234".into(), true, None, Settings::default());
    let fix = &s.arts["review-fix"];
    assert_eq!(fix.findings, ["I1"]);
    assert_eq!(s.arts["review"].next.as_deref(), Some("review"));
    assert_eq!(s.review.fixes, 1);
    assert_eq!(s.review.attempts.get("I1"), Some(&1));
    assert_eq!(
        s.review.attempts.get("I2"),
        Some(&0),
        "no Attempts: is none yet"
    );
}

#[test]
fn inside_a_review_loop_verify_and_the_next_pass_keep_review_md() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    let s = read_state(d, "def5678".into(), true, None, Settings::default());
    archive_for(d, "verify", &s).unwrap();
    assert!(d.join("review.md").exists() && d.join("fix.md").exists());
    let s = read_state(d, "def5678".into(), true, None, Settings::default());
    let moves = archive_for(d, "review", &s).unwrap();
    assert!(d.join("review.md").exists() && d.join(crate::review_runs::FILE).exists());
    assert!(!d.join("fix.md").exists() && d.join("history/fix-1.md").exists());
    restore(&moves);
    assert!(d.join("fix.md").exists());
}

#[test]
fn a_first_pass_archives_review_md_with_its_record_and_a_restore_puts_both_back() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    fs::remove_file(d.join("fix.md")).unwrap();
    v2(d, "review.md", "review", "pass", "none", "ship", "");
    let s = read_state(d, "def5678".into(), true, None, Settings::default());
    let moves = archive_for(d, "review", &s).unwrap();
    assert!(!d.join("review.md").exists() && !d.join(crate::review_runs::FILE).exists());
    assert!(d.join("history/review-1.md").exists());
    assert_eq!(
        crate::review_runs::read_in(&d.join("history/review-1"))
            .unwrap()
            .len(),
        2
    );
    restore(&moves);
    assert!(d.join("review.md").exists() && d.join(crate::review_runs::FILE).exists());
    assert!(!d.join("history/review-1").exists());
}

#[test]
fn a_build_ends_the_review_loop_and_archives_both() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    v2(d, "evidence.md", "verify", "pass", "none", "review", "");
    let s = read_state(d, "def5678".into(), true, None, Settings::default());
    archive_for(d, "build", &s).unwrap();
    assert!(!d.join("review.md").exists() && !d.join("fix.md").exists());
    assert!(d.join("history/review-1/cycles.json").is_file());
    assert!(
        !d.join("history/evidence-1").exists(),
        "the record goes only with review.md"
    );
}

#[test]
fn review_run_numbers_each_pass_and_fix_cycle_of_the_loop() {
    use crate::run::core::route::review_run;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    let mut s = read_state(d, "abc1234".into(), true, None, Settings::default());
    assert_eq!(review_run("review", &s), Some((1, "full", false)));
    assert_eq!(review_run("build", &s), None);
    s.arts.remove("review-fix");
    assert_eq!(review_run("review-fix", &s), Some((2, "full", false)));
    s.review.fixes = 3;
    assert_eq!(review_run("review-fix", &s), Some((4, "fix-only", false)));
    s.arts.remove("review");
    assert_eq!(review_run("review", &s), Some((0, "first", true)));
}

#[test]
fn a_loop_whose_record_is_missing_or_unreadable_stops_rather_than_count_from_0() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    let record = d.join(crate::review_runs::FILE);
    let at = |d: &Path| {
        let mut s = read_state(d, "abc1234".into(), true, None, Settings::default());
        for p in ["triage", "build", "verify"] {
            s.arts.insert(p, art("pass", "abc1234"));
        }
        s
    };
    let stuck = |d: &Path| {
        let s = at(d);
        assert_eq!(s.review.fixes, 0);
        match decide(&s) {
            Decision::Stuck(r) => r,
            d => panic!("{d:?}"),
        }
    };
    fs::write(&record, "not json").unwrap();
    let r = stuck(d);
    assert!(
        r.starts_with("review.md has a review loop under way, but ns's record of it can't be used: cannot parse"),
        "{r}"
    );
    fs::remove_file(&record).unwrap();
    assert_eq!(
        stuck(d),
        "review.md has a review loop under way, but ns's record of it can't be used: review/cycles.json is missing"
    );
    let s = read_state(d, "abc1234".into(), true, None, Settings::default());
    assert!(s.review.record_problem.is_some());
    v2(d, "review.md", "review", "pass", "none", "ship", "");
    assert!(
        matches!(decide(&at(d)), Decision::Stuck(_)),
        "a fix cycle for a red gate after a passed review is a loop too"
    );
    fs::remove_file(d.join("fix.md")).unwrap();
    assert!(
        !matches!(decide(&at(d)), Decision::Stuck(_)),
        "outside a loop the record is not needed"
    );
}

#[test]
fn a_pass_set_aside_inside_a_loop_puts_back_review_md_fix_md_and_the_record() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    let (review, record) = (d.join("review.md"), d.join(crate::review_runs::FILE));
    let (review_was, record_was) = (fs::read(&review).unwrap(), fs::read(&record).unwrap());
    let s = read_state(d, "abc1234".into(), true, None, Settings::default());
    let moves = archive_for(d, "review", &s).unwrap();
    fs::write(&review, "the pass rewrote it").unwrap();
    let run = crate::review_runs::read(d).unwrap()[0].clone();
    crate::review_runs::append(d, run, false).unwrap();
    archive_as(d, "review.md", "review-interrupted").unwrap();
    restore(&moves);
    assert_eq!(fs::read(&review).unwrap(), review_was);
    assert_eq!(fs::read(&record).unwrap(), record_was);
    assert!(d.join("fix.md").exists());
}

#[test]
fn a_fix_cycle_set_aside_takes_its_entry_out_of_the_record() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    fs::remove_file(d.join("fix.md")).unwrap();
    let record = d.join(crate::review_runs::FILE);
    let was = fs::read(&record).unwrap();
    let s = read_state(d, "abc1234".into(), true, None, Settings::default());
    let moves = archive_for(d, "review-fix", &s).unwrap();
    assert_eq!(moves.kept.len(), 1, "fix.md isn't kept: review.md is");
    let run = crate::review_runs::read(d).unwrap()[1].clone();
    crate::review_runs::append(d, run, false).unwrap();
    restore(&moves);
    assert_eq!(fs::read(&record).unwrap(), was);
    assert!(d.join("review.md").exists());
}

#[test]
fn a_first_pass_set_aside_puts_back_the_old_record_over_its_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    fs::remove_file(d.join("fix.md")).unwrap();
    v2(d, "review.md", "review", "pass", "none", "ship", "");
    let record = d.join(crate::review_runs::FILE);
    let was = fs::read(&record).unwrap();
    let s = read_state(d, "def5678".into(), true, None, Settings::default());
    let moves = archive_for(d, "review", &s).unwrap();
    let run = crate::review_runs::ReviewRun {
        cycle: 0,
        ..crate::review_runs::read_in(&d.join("history/review-1")).unwrap()[0].clone()
    };
    crate::review_runs::append(d, run, true).unwrap();
    v2(d, "review.md", "review", "pass", "none", "ship", "");
    archive_as(d, "review.md", "review-interrupted").unwrap();
    restore(&moves);
    assert_eq!(fs::read(&record).unwrap(), was);
    assert!(d.join("review.md").exists() && !d.join("history/review-1").exists());
}

#[test]
fn mark_blocked_sets_status_next_cause_reason_and_updated_and_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    mid_loop(d);
    let path = d.join("review.md");
    let before = fs::read_to_string(&path)
        .unwrap()
        .replace("cause: findings_open", "cause: none");
    fs::write(&path, &before).unwrap();
    mark_blocked(&path, "limit: I1 \"open\"", "2026-10-14T12:00:00Z").unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let t = crate::artifact::check(&text, Some("review")).unwrap();
    assert_eq!(t.status.as_str(), "blocked");
    assert_eq!(t.next.as_str(), "human");
    assert_eq!(t.cause, crate::artifact::Cause::FindingsOpen);
    assert_eq!(t.reason, "limit: I1 \"open\"");
    assert_eq!(t.findings, ["I1"]);
    assert_eq!(
        frontmatter::field(&text, "updated").as_deref(),
        Some("2026-10-14T12:00:00Z")
    );
    assert_eq!(
        frontmatter::split(&text).unwrap().1,
        frontmatter::split(&before).unwrap().1
    );
    fs::write(&path, "no frontmatter").unwrap();
    assert!(mark_blocked(&path, "r", "t").is_err());
}

#[test]
fn cycle_limits_near_u32_max_do_not_overflow() {
    use crate::run::core::route::fix_cycle;
    let mut s = state(&[]);
    s.settings.cycles = crate::factory::Cycles {
        full: 4_000_000_000,
        fix_only: 2_000_000_000,
    };
    s.review.fixes = 4_000_000_000;
    let mut review = art("fail", "abc1234");
    review.findings = vec!["I1".into()];
    assert_eq!(
        fix_cycle(&review, &s),
        Ok((4_000_000_001, "fix-only", vec!["I1".to_string()]))
    );
}
