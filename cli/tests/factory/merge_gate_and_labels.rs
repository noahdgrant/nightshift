#[test]
fn a_red_gate_on_an_unmoved_main_logs_merge_gate_failed() {
    // Ship rebased the branch, so the PR head is the older commit, which the gate fails.
    let gate = "gate = \"test -f UPSTREAM || test ! -f $FAKE_GH_DIR/pr-12.shipped\"\n";
    let (e, v, _) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.factory(&format!("{AUTO}[phases.build]\n{gate}"));
        e.queue("build", &["pass:commit", "fail"]);
    });
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert!(!e.gh_calls().contains("pr merge"));
    let failed = merge_gate_failures(&e);
    assert!(!failed.is_empty(), "{failed:?}");
    assert!(failed.iter().all(|f| f["reused"] == false), "{failed:?}");
    assert_eq!(failed[0]["pr"], 12);
    assert!(failed[0]["sha"].is_string(), "{failed:?}");
    assert!(failed[0]["base"].is_string(), "{failed:?}");
    let p = e.prompt(6, "build");
    assert!(!p.contains("combining it with main"), "{p}");
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_small_change_a_merge_of_main_brings_in_keeps_evidence_and_runs_a_delta_review() {
    let resolve = "git merge -q --no-ff --no-commit upstream\n\
                   echo resolved >> work.txt && git add work.txt\n\
                   git -c user.name=f -c user.email=f@f commit -q --no-edit\n\
                   git update-ref refs/remotes/origin/main upstream\n\
                   git rev-parse HEAD > \"$FAKE_GH_DIR/pr-12.head\"\n";
    let (e, v, code) = main_moves_after_review(
        &["pass:script", "pass"],
        PRINT_HEAD_SHA_CMD,
        resolve,
        |_| {},
    );
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "review"],
        "verify's evidence carried over the one-line merge resolution"
    );
    let review = run_events(&e)
        .into_iter()
        .rfind(|ev| ev["event"] == "phase" && ev["phase"] == "review")
        .expect("a review phase event");
    assert!(
        review["decision"]
            .as_str()
            .is_some_and(|d| d.ends_with("change the base brought in: delta review")),
        "{review}"
    );
}

#[test]
fn ci_runs_without_the_merge_lock_and_only_the_merge_waits_for_it() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let held = hold(
        &e.root.join(".git/ns-merge.lock"),
        r#"{"pid":42,"unit":"other"}"#,
    );
    let mut g = e.ns().args(["run", "--issue", "7"]).start_piped();
    let stdout = read_all(g.take_stdout());
    let (tx, rx) = mpsc::channel();
    send_lines(g.take_stderr(), tx);
    let mut seen = Vec::new();
    let waited = line_until(
        &g,
        &rx,
        "ns run: 7-fix-the-thing merge waits for the merge lock (held by pid 42, unit other)",
        &mut seen,
    );
    assert!(waited, "{}", seen.join("\n"));
    // The CI wait ended while another unit held the lock; the merge has not run.
    let calls = e.gh_calls();
    assert!(calls.contains("pr checks 12 --watch"), "{calls}");
    assert!(!calls.contains("pr merge"), "{calls}");
    drop(held);
    assert!(g.wait().success());
    let events: Vec<String> = run_events(&e)
        .iter()
        .filter_map(|ev| ev["event"].as_str().map(String::from))
        .filter(|ev| ["ci_passed", "lock_wait", "merged"].contains(&ev.as_str()))
        .collect();
    assert_eq!(events, ["ci_passed", "lock_wait", "merged"]);
    let v: Value = serde_json::from_slice(&g.recv(&stdout)).unwrap();
    assert_eq!(v["outcome"], "merged", "{v}");
}

/// Hook text that moves `origin`'s main in `work.txt`, a file PR 12 changes, and puts PR 12
/// behind it on PR 12's CI waits: every wait, or only the first when `once`.
fn main_moves_during_ci(e: &Env, once: bool) {
    let guard = if once {
        format!("mkdir {:?} 2>/dev/null && ", e.ctrl.join("moved"))
    } else {
        String::new()
    };
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$*\" in \"pr checks 12 --watch\") {guard}{{ {}; echo BEHIND > \"$d/pr-12.merge\"; }} ;; esac\n",
            move_main_in_work_sh(e)
        ),
    );
}

fn base_moves(e: &Env) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|ev| ev["event"] == "merge_base_moved")
        .collect()
}

#[test]
fn a_main_that_keeps_moving_holds_the_lock_past_the_cap_and_ends_stuck_at_the_hard_cap() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    main_moves_during_ci(&e, false);
    // update-branch hands out a new head each time, as GitHub's merge commits are.
    let n = e.ctrl.join("updates");
    let mut hook = fs::read_to_string(e.ghd.join("hook.sh")).unwrap();
    hook.push_str(&format!(
        "case \"$*\" in \"pr update-branch 12\") echo x >> {n:?}; printf '%040d\\n' \"$(wc -l < {n:?})\" > \"$d/updated-12.head\" ;; esac\n"
    ));
    e.gh_file("hook.sh", &hook);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(
        v["reason"],
        "the default branch moved 10 times during this PR's CI, the last 5 with the merge lock held (cap 10)",
        "{v}"
    );
    let attempts: Vec<Value> = base_moves(&e)
        .iter()
        .map(|m| m["attempt"].clone())
        .collect();
    assert_eq!(attempts, (1..=11).map(|a| json!(a)).collect::<Vec<_>>());
    let held: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "merge_lock_held_through_ci")
        .collect();
    assert_eq!(held.len(), 1, "the lock is taken once, at the cap, and kept: {held:?}");
    assert_eq!(held[0]["moves"], 5);
    let calls = e.gh_calls();
    assert_eq!(calls.matches("pr checks 12 --watch").count(), 11, "{calls}");
    assert!(!calls.contains("pr merge"), "{calls}");
    assert!(lock_is_free(&e.root.join(".git/ns-merge.lock")));
}

#[test]
fn a_main_unreadable_under_the_lock_is_no_move_and_the_region_check_needs_a_human() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "work.txt\n");
    e.gh_file(
        "hook.sh",
        "case \"$*\" in \"pr checks 12 --watch\") git remote set-url origin /nonexistent ;; esac\n",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("cannot check human-review regions: git fetch -q origin main failed"),
        "{v}"
    );
    assert_eq!(base_moves(&e), Vec::<Value>::new());
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_ready_issue_that_overlaps_a_running_unit_waits_for_it_to_end() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    brief_comment(&e, 2, "`cli/**`");
    e.ready(3, "Fix b", &["type:fix"], "");
    brief_comment(&e, 3, "`cli/src/run.rs`");
    // #2 holds at its triage, so the pick reads its prediction from its issue.
    let (in_a, go_a) = hold_phase(&e, A, "triage");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.line("ns watch: #3 waits: its touches overlap running 2-fix-a");
    release(&go_a);
    w.entered(&in_b);
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    // A free slot stayed free: #3 started only once #2 had ended.
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    let started_3 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 3);
    assert!(ended_2 < started_3, "{:?}", run_events(&e));
    let held = events_named(&e, "overlap_held");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(
        (&held[0]["issue"], &held[0]["overlaps"]),
        (&json!(3), &json!(["2-fix-a"]))
    );
    assert!(events_named(&e, "overlap_forced").is_empty());
}

#[test]
fn a_triage_only_split_moves_the_parent_to_split_and_closes_it_once_its_children_are_done() {
    let e = Env::new();
    e.untriaged(5, "Big", &["status:needs-triage", "type:feat"]);
    e.gh_file("subs-5", "8\n9\n");
    e.issue(8, "Child a", "OPEN");
    e.issue(9, "Child b", "OPEN");
    e.queue("triage", &["split:into #8 and #9."]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    // ns, not the skill, moved the parent off the triage label to its own split label.
    assert_eq!(e.labels(5), ["type:feat", "status:split"]);
    assert_eq!(v["triaged"][0]["state"], json!(["status:split"]), "{v}");
    assert!(units(&v).is_empty(), "{v}");
    // Once both children close as completed, the next night closes the parent (#409).
    e.issue(8, "Child a", "CLOSED");
    e.issue(9, "Child b", "CLOSED");
    // The fake still lists the closed parent, with no state: its triage writes no brief.
    e.queue("triage", &["none"]);
    let v = e.run(&["watch", "--once"], 0);
    assert!(
        e.gh_calls().contains("issue close 5 --reason completed"),
        "{v}"
    );
    assert_eq!(e.labels(5), ["type:feat"]);
}

#[test]
fn a_triage_brief_routes_by_its_next_and_its_status_and_ns_writes_the_label() {
    for (front, status, want) in [
        ("next: define\n", "pass", "status:needs-define"),
        ("next: troubleshoot\n", "blocked", "status:needs-repro"),
        ("next: human\n", "pass", "status:ready-for-human"),
        ("next: info\n", "blocked", "status:needs-info"),
        ("", "blocked", "status:needs-define"),
    ] {
        let e = Env::new();
        e.untriaged(5, "Unclear", &["status:needs-triage", "type:fix"]);
        e.ctl("triage.front", front);
        e.queue("triage", &[status]);
        let v = e.run(&["watch"], 0);
        assert_eq!(e.labels(5), ["type:fix", want], "{front:?} {status}: {v}");
        assert!(units(&v).is_empty(), "{v}");
    }
    // `pass` with no `next:` is ready: the queue starts it the same night.
    let e = Env::new();
    e.untriaged(5, "Clear", &["status:needs-triage", "type:fix"]);
    e.queue("triage", &["pass", "pass"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[5], "done"), "{v}");
}

#[test]
fn a_triage_brief_that_routes_nowhere_leaves_the_labels_and_says_why() {
    let e = Env::new();
    e.untriaged(5, "Unclear", &["status:needs-triage"]);
    e.ctl("triage.front", "next: ship\n");
    let out = e.ns().args(["watch"]).assert().code(0).get_output().clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("ns watch: triage #5 left its labels: brief.md says next: ship"),
        "{err}"
    );
    assert_eq!(
        v["triaged"][0]["label_error"], "brief.md says next: ship",
        "{v}"
    );
    assert_eq!(e.labels(5), ["status:needs-triage"]);
}
