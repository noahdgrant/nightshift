#[test]
fn a_fix_unit_cut_short_by_until_stops_the_night_as_until() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    b.ready(2, "Fix a", &["type:fix"], "");
    main_red(&b);
    b.ctl(
        "triage.sh",
        &format!("gh issue edit 100 --add-label {READY}\n"),
    );
    b.queue("triage", &["pass:script"]);
    b.queue("build", &["pass:commit"]);
    b.queue("verify", &["pass:script"]);
    let out = b
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "verify=86400000")
        .args(["watch", "--until", "00:30"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    held.release();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(by_issue(&units(&v)), each(&[100], "budget"), "{v}");
    assert_eq!(v["stopped"], "until", "{v}");
    assert!(events_named(&b, "main_red_unfixed").is_empty());
}

#[test]
fn a_fix_unit_that_ends_stuck_after_until_passed_still_logs_main_red_unfixed() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    b.ready(2, "Fix a", &["type:fix"], "");
    b.queue(&format!("build.{A}"), &["pass:commit"]);
    b.queue(&format!("verify.{A}"), &["pass:script"]);
    let fix = "100-fix-main-is-red-at-abcdef0-test";
    b.queue(&format!("triage.{fix}"), &["none"]);
    let (in_a, go_a) = hold_phase(&b, A, "build");
    let (in_t, go_t) = hold_phase(&b, fix, "triage");
    let mut w = Watching::start(
        b.ns()
            .env("NS_REFILL_MS", "100")
            .env("NS_PHASE_TIMEOUT_MS", "verify=86400000")
            .args(["watch", "--parallel", "2", "--until", "00:30"]),
    );
    w.entered(&in_a);
    main_red(&b);
    w.entered(&in_t);
    // #2 waits for the held lock until --until and ends budget; the fix unit is still running.
    release(&go_a);
    w.line("#2 budget");
    release(&go_t);
    let (code, v, err) = w.finish();
    held.release();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        by_issue(&units(&v)),
        [(2, "budget".into()), (100, "stuck".into())],
        "{v}"
    );
    assert_eq!(v["stopped"], "until", "{v}");
    let unfixed = events_named(&b, "main_red_unfixed");
    assert_eq!(unfixed.len(), 1, "{unfixed:?}");
    assert_eq!(unfixed[0]["outcome"], "stuck");
}

#[test]
fn a_fix_unit_that_ends_unfixed_after_main_turned_green_stops_nothing() {
    let (e, v) = fix_ends(|e| {
        e.queue("triage", &["none", "none"]);
        e.gh_file(
            "main-ci.green",
            &main_ci_at(MAIN_SHA, &[], &[("test", "COMPLETED", "SUCCESS")]),
        );
        // Main turns green, by another hand, while the fix unit runs.
        e.gh_file(
            "hook.sh",
            "[ \"$1 $2 $3\" = \"issue edit 100\" ] && cp \"$d/main-ci.green\" \"$d/main-ci.json\"\n",
        );
    });
    assert_eq!(by_issue(&units(&v)), each(&[2, 100], "stuck"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(events_named(&e, "main_red_unfixed").is_empty());
}

#[test]
fn a_fix_unit_that_ends_unfixed_when_main_ci_cannot_be_read_stops_nothing() {
    let (e, v) = fix_ends(|e| {
        e.queue("triage", &["none", "none"]);
        e.gh_file(
            "hook.sh",
            "[ \"$1 $2 $3\" = \"issue edit 100\" ] && echo 'HTTP 502' > \"$d/main-ci.fail\"\n",
        );
    });
    assert_eq!(by_issue(&units(&v)), each(&[2, 100], "stuck"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(events_named(&e, "main_red_unfixed").is_empty());
}

#[test]
fn a_fix_unit_the_harness_fails_does_not_stop_the_night() {
    let (e, v) = fix_ends(|e| e.queue("triage", &["crash", "crash"]));
    assert_eq!(by_issue(&units(&v)), each(&[100], "harness_failing"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(events_named(&e, "main_red_unfixed").is_empty());
}

#[test]
fn a_stuck_unit_that_is_not_the_fix_does_not_stop_the_night() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue(&format!("build.{A}"), &["pass:commit", "pass:commit"]);
    e.queue(&format!("verify.{A}"), &["fail", "fail"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = Watching::start(e.ns().env("NS_REFILL_MS", "100").args([
        "watch",
        "--parallel",
        "2",
        "--max-units",
        "3",
    ]));
    w.entered(&in_a);
    main_red(&e);
    // #2 ends stuck after the fix issue is filed and started.
    w.line("ns watch: main is red");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        by_issue(&units(&v)),
        [(2, "stuck".into()), (100, "done".into())],
        "{v}"
    );
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(events_named(&e, "main_red_unfixed").is_empty());
}

#[test]
fn a_unit_running_when_the_fix_unit_ends_unfixed_finishes_and_nothing_new_starts() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage.100-fix-main-is-red-at-abcdef0-test", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = Watching::start(e.ns().env("NS_REFILL_MS", "100").args([
        "watch",
        "--parallel",
        "2",
        "--max-units",
        "4",
    ]));
    w.entered(&in_a);
    main_red(&e);
    e.ready(3, "Fix b", &["type:fix"], "");
    w.line("the night stops");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        by_issue(&units(&v)),
        [(2, "done".into()), (100, "stuck".into())],
        "{v}"
    );
    assert_eq!(v["stopped"], "main_red_unfixed", "{v}");
    assert_eq!(events_named(&e, "main_red_unfixed").len(), 1);
    assert!(!e.gh_calls().contains("issue edit 3 "));
}

#[test]
fn watch_with_fix_main_first_off_runs_the_queue_while_main_is_red() {
    let e = Env::new();
    e.factory("[queue]\nfix_main_first = false\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
    assert!(events_named(&e, "main_red").is_empty());
}

#[test]
fn watch_dry_run_shows_main_ci_and_holds_the_queue_when_it_is_red() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let (v, err) = dry_run(&e);
    assert_eq!(v["main_ci"]["state"], "none", "{v}");
    assert_eq!(v["queue_held"], false);
    assert!(err.contains("main's head has no CI"), "{err}");
    let calls = e.gh_calls();
    assert!(
        calls.contains("api graphql -f query=query($owner: String!, $repo: String!)"),
        "{calls}"
    );
    assert!(calls.contains("-F owner={owner} -F repo={repo}"), "{calls}");

    main_ci(
        &e,
        &["test"],
        &[
            ("test", "COMPLETED", "FAILURE"),
            ("bench", "COMPLETED", "FAILURE"),
        ],
    );
    let (v, err) = dry_run(&e);
    assert_eq!(
        v["main_ci"],
        serde_json::json!({"state": "red", "sha": MAIN_SHA, "failing": ["test"]})
    );
    assert_eq!(v["queue_held"], true);
    assert_eq!(queued(&v), [2], "the queue is only shown held, not emptied");
    assert!(!err.contains("fix_main_first"), "{err}");

    main_ci(
        &e,
        &["test"],
        &[
            ("test", "IN_PROGRESS", ""),
            ("lint", "COMPLETED", "SUCCESS"),
        ],
    );
    let (v, _) = dry_run(&e);
    assert_eq!(
        v["main_ci"],
        serde_json::json!({"state": "pending", "sha": MAIN_SHA})
    );
    assert_eq!(v["queue_held"], false);

    main_ci(&e, &[], &[("test", "COMPLETED", "SUCCESS")]);
    let (v, _) = dry_run(&e);
    assert_eq!(v["main_ci"]["state"], "green");
    assert_eq!(v["queue_held"], false);
}

#[test]
fn watch_dry_run_with_fix_main_first_off_reads_no_ci_and_says_so() {
    let e = Env::new();
    e.factory("[queue]\nfix_main_first = false\n");
    main_ci(&e, &[], &[("test", "COMPLETED", "FAILURE")]);
    let (v, err) = dry_run(&e);
    assert_eq!(v["main_ci"], serde_json::json!({"state": "off"}));
    assert_eq!(v["queue_held"], false);
    assert!(err.contains("fix_main_first is off"), "{err}");
    assert!(!e.gh_calls().contains("graphql"), "{}", e.gh_calls());
}

#[test]
fn a_night_with_fix_main_first_off_says_so_once_and_reads_no_ci() {
    let e = Env::new();
    e.factory("[queue]\nfix_main_first = false\n");
    let out = e.ns().args(["watch", "--once"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert_eq!(err.matches("fix_main_first is off").count(), 1, "{err}");
    assert!(!e.gh_calls().contains("graphql"), "{}", e.gh_calls());
}

#[test]
fn a_night_says_once_at_start_when_main_has_no_ci_and_runs_on_when_it_cannot_read_it() {
    let e = Env::new();
    let night = |e: &Env| {
        let out = e.ns().args(["watch", "--once"]).output();
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{err}");
        err
    };
    let err = night(&e);
    assert_eq!(err.matches("main's head has no CI").count(), 1, "{err}");

    main_ci(&e, &[], &[("test", "COMPLETED", "FAILURE")]);
    let err = night(&e);
    assert!(!err.contains("fix_main_first"), "{err}");

    e.gh_file("main-ci.fail", "HTTP 502");
    let err = night(&e);
    assert!(
        err.contains("warning: cannot read main's CI") && err.contains("HTTP 502"),
        "{err}"
    );
}

#[test]
fn watch_dry_run_fails_when_main_ci_cannot_be_read() {
    let e = Env::new();
    e.gh_file("main-ci.fail", "HTTP 502");
    let out = e.ns().args(["watch", "--dry-run"]).output();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("HTTP 502"), "{err}");
}

fn queue_order(v: &serde_json::Value) -> Vec<(u64, String, String)> {
    v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["number"].as_u64().unwrap(),
                i["priority_label"].as_str().unwrap_or("").to_string(),
                i["order_label"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

#[test]
fn watch_sorts_by_priority_before_type() {
    let e = Env::new();
    e.ready(1, "Low fix", &["type:fix", "priority:low"], "");
    e.ready(2, "High docs", &["type:docs", "priority:high"], "");
    e.ready(3, "Unprioritised fix", &["type:fix"], "");
    e.ready(4, "Medium feat", &["type:feat", "priority:medium"], "");
    e.ready(5, "Medium fix", &["priority:medium", "type:fix"], "");
    e.ready(6, "High fix", &["type:fix", "priority:high"], "");
    let v = e.run(&["watch", "--dry-run"], 0);
    let s = |x: &str| x.to_string();
    assert_eq!(
        queue_order(&v),
        [
            (6, s("priority:high"), s("type:fix")),
            (2, s("priority:high"), s("type:docs")),
            (5, s("priority:medium"), s("type:fix")),
            (4, s("priority:medium"), s("type:feat")),
            (1, s("priority:low"), s("type:fix")),
            (3, s(""), s("type:fix")),
        ]
    );
    assert!(v["queue"][5]["priority_label"].is_null());
}

#[test]
fn watch_uses_a_custom_priority_list() {
    let e = Env::new();
    e.factory("[queue]\npriority = [\"p0\", \"p1\"]\n");
    e.ready(1, "p1 fix", &["type:fix", "p1"], "");
    e.ready(2, "p0 docs", &["type:docs", "p0"], "");
    e.ready(3, "default label", &["type:fix", "priority:high"], "");
    let v = e.run(&["watch", "--dry-run"], 0);
    let order: Vec<u64> = queue_order(&v).iter().map(|q| q.0).collect();
    assert_eq!(order, [2, 1, 3]);
    assert_eq!(v["queue"][0]["priority_label"], "p0");
}

#[test]
fn watch_queues_only_team_authored_issues() {
    let e = Env::new();
    e.ready_by(1, "Owner", &[], "", Some("OWNER"));
    e.ready_by(2, "Member", &[], "", Some("MEMBER"));
    e.ready_by(3, "Collaborator", &[], "", Some("COLLABORATOR"));
    e.ready_by(4, "Contributor", &[], "", Some("CONTRIBUTOR"));
    e.ready_by(5, "Stranger", &[], "", Some("NONE"));
    e.ready_by(6, "Unknown", &[], "", None);
    let v = e.run(&["watch", "--dry-run"], 0);
    let order: Vec<u64> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    assert_eq!(order, [1, 2, 3]);
    let skipped: Vec<(u64, &str)> = v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["number"].as_u64().unwrap(), i["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        skipped,
        [
            (4, "author outside the team"),
            (5, "author outside the team"),
            (6, "author outside the team"),
        ]
    );
    assert!(e.gh_calls().contains(
        "api --paginate repos/{owner}/{repo}/issues?state=open&labels=status:ready-for-agent&per_page=100 --jq"
    ));
}

#[test]
fn watch_never_runs_a_non_team_issue() {
    let e = Env::new();
    e.ready_by(2, "Stranger", &["type:fix"], "", Some("NONE"));
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "queue empty");
    assert!(v["units"].as_array().unwrap().is_empty());
    assert!(e.calls().is_empty());
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn watch_once_swaps_labels_on_done() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"].as_array().unwrap().len(), 1);
    assert_eq!(v["units"][0]["outcome"], "done");
    assert_eq!(v["stopped"], "max_units");
    let calls = e.gh_calls();
    assert!(calls.contains(
        "issue edit 2 --remove-label status:ready-for-agent --add-label status:in-progress"
    ));
    assert!(calls
        .contains("issue edit 2 --remove-label status:in-progress --add-label status:in-review"));
    assert!(!calls.contains("issue edit 3"));
}

#[test]
fn watch_stuck_swaps_label_and_comments() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["none"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck");
    let calls = e.gh_calls();
    assert!(
        calls.contains("--add-label status:ready-for-human"),
        "{calls}"
    );
    let all = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(all.starts_with(&format!(" --body {DISCLOSED}")), "{all}");
    assert!(all.contains("triage wrote no brief"), "{all}");
    assert!(all.contains("Last artifact"));
}

const HUMAN_MERGE: &str = "needs-human-merge";

/// How every body nightshift posts to the forge opens (skills/ns-contract, Disclosure).
const DISCLOSED: &str = "🤖 *Posted by nightshift, an AI agent.*\n\n";

/// The `pr edit` and `pr comment` calls on PR 12, and whether a label was created.
fn pr_12_calls(e: &Env) -> (Vec<String>, bool) {
    let calls = e.gh_calls();
    let on_pr = calls
        .lines()
        .filter(|l| l.starts_with("pr edit 12") || l.starts_with("pr comment 12"))
        .map(String::from)
        .collect();
    (on_pr, calls.contains("label create"))
}

#[test]
fn watch_done_with_a_pr_labels_it_for_a_human_merge_and_says_why() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:in-review"]);
    let calls = e.gh_calls();
    assert!(
        calls.contains(&format!(
            "label create {HUMAN_MERGE} --color d73a4a --description nightshift left this PR for a human to merge"
        )),
        "{calls}"
    );
    assert_eq!(
        fs::read_to_string(e.ghd.join("pr-labels-12")).unwrap(),
        format!("{HUMAN_MERGE}\n")
    );
    let comment = calls
        .split("pr comment 12 --body ")
        .nth(1)
        .expect("comment");
    assert!(comment.contains("needs a human merge"), "{comment}");
    assert!(comment.contains("merge.policy"), "{comment}");
    assert!(comment.starts_with(DISCLOSED), "{comment}");
}

#[test]
fn watch_done_needing_a_human_merge_labels_the_pr_with_the_merge_reason() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", ".nightshift/rules-baseline.json\n");
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-human"]);
    let calls = e.gh_calls();
    assert!(
        calls.contains(&format!("pr edit 12 --add-label {HUMAN_MERGE}")),
        "{calls}"
    );
    let comment = calls
        .split("pr comment 12 --body ")
        .nth(1)
        .expect("comment");
    assert!(
        comment.contains(
            "changes files that need human review; needs a human merge (.nightshift/rules-baseline.json)"
        ),
        "{comment}"
    );
    assert!(comment.starts_with(DISCLOSED), "{comment}");
}

#[test]
fn watch_done_without_a_pr_leaves_prs_alone() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(v["units"][0]["pr"], Value::Null, "{v}");
    assert_eq!(pr_12_calls(&e), (vec![], false));
    assert!(!e.gh_calls().contains("pr edit"));
}

#[test]
fn watch_takes_the_human_merge_label_from_the_queue_config() {
    let e = Env::new();
    e.factory("[queue]\nhuman_merge_label = \"merge-me\"\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    let calls = e.gh_calls();
    assert!(calls.contains("label create merge-me "), "{calls}");
    assert!(calls.contains("pr edit 12 --add-label merge-me"), "{calls}");
}

#[test]
fn watch_adds_an_existing_human_merge_label_without_recreating_it() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file("label.exists", "");
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    let calls = e.gh_calls();
    assert!(
        calls.contains(&format!("pr edit 12 --add-label {HUMAN_MERGE}")),
        "{calls}"
    );
    assert!(calls.contains("pr comment 12"), "{calls}");
}

#[test]
fn a_pr_that_cannot_be_labelled_still_gets_its_issue_relabelled_and_comment() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file(
        "label.fail",
        "HTTP 403: Resource not accessible by integration",
    );
    e.queue("build", &["pass:commit"]);
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:in-review"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("PR #12 not labelled {HUMAN_MERGE}"))
            && stderr.contains("HTTP 403"),
        "{stderr}"
    );
    let calls = e.gh_calls();
    assert!(!calls.contains("pr edit 12"), "{calls}");
    assert!(calls.contains("pr comment 12"), "{calls}");
}

#[test]
fn a_pr_comment_that_fails_is_reported_and_the_night_goes_on() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file(
        "hook.sh",
        "case \"$*\" in \"pr comment\"*) echo 'HTTP 502' >&2; exit 1 ;; esac\n",
    );
    e.queue("build", &["pass:commit"]);
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("PR #12 not commented") && stderr.contains("HTTP 502"),
        "{stderr}"
    );
    assert!(e
        .gh_calls()
        .contains(&format!("pr edit 12 --add-label {HUMAN_MERGE}")));
}

#[test]
fn watch_stuck_after_ship_leaves_the_pr_alone() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["blocked:CI never started"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck", "{v}");
    assert_eq!(v["units"][0]["pr"], 12, "{v}");
    assert_eq!(pr_12_calls(&e), (vec![], false));
}

#[test]
fn watch_merged_removes_the_human_merge_label_a_past_night_set() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("pr-labels-12", &format!("type:fix\n{HUMAN_MERGE}\n"));
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(
        pr_12_calls(&e),
        (
            vec![format!("pr edit 12 --remove-label {HUMAN_MERGE}")],
            false
        )
    );
    assert_eq!(
        fs::read_to_string(e.ghd.join("pr-labels-12")).unwrap(),
        "type:fix\n"
    );
}

#[test]
fn watch_merged_without_the_label_edits_no_pr() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(pr_12_calls(&e), (vec![], false));
    assert!(e.gh_calls().contains("pr view 12 --json labels"));
}

#[test]
fn a_merged_pr_whose_label_cannot_be_read_is_reported_and_the_issue_still_closes() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file(
        "hook.sh",
        "case \"$*\" in \"pr view 12 --json labels\") echo 'HTTP 502' >&2; exit 1 ;; esac\n",
    );
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("PR #12 still labelled {HUMAN_MERGE}?")),
        "{stderr}"
    );
    assert!(e.gh_calls().contains("issue close 2"));
}

#[test]
fn watch_paused_at_ship_leaves_the_pr_alone() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.ctl("reset", &(NOW + 36_000).to_string());
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["session"]);
    let v = e.run(&["watch", "--once", "--until", "06:30"], 0);
    assert_eq!(v["units"][0]["outcome"], "paused", "{v}");
    assert_eq!(pr_12_calls(&e), (vec![], false));
}

#[test]
fn watch_dry_run_names_the_human_merge_label() {
    let e = Env::new();
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["human_merge_label"], HUMAN_MERGE, "{v}");
    e.factory("[queue]\nhuman_merge_label = \"merge-me\"\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["human_merge_label"], "merge-me", "{v}");
}

#[test]
fn watch_stuck_comment_quotes_the_blocker_and_a_relative_path() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["blocked:needs a board"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck");
    let unit = v["units"][0]["unit"].as_str().unwrap();
    let calls = e.gh_calls();
    let comment = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(
        comment.contains("evidence.md is blocked: verify said blocked needs a board"),
        "{comment}"
    );
    assert!(
        comment.contains(&format!("`.ns/{unit}/evidence.md` on branch `ns/{unit}`")),
        "{comment}"
    );
    assert!(!comment.contains(e.base.to_str().unwrap()), "{comment}");
}

#[test]
fn a_blocked_review_stops_at_once_and_the_comment_quotes_it() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["blocked:C1 open after 3 cycles"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review"]);
    let calls = e.gh_calls();
    let comment = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(
        comment.contains("review.md is blocked: review said blocked C1 open after 3 cycles"),
        "{comment}"
    );
}

#[test]
fn a_blocked_review_comment_lists_its_open_findings() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.queue(
        "review",
        &["blocked:Open after at least 2 fix attempts: C1, I2."],
    );
    e.ctl(
        "review.body",
        "\n## Critical\n### C1. cancel_all drops every SKU\n- Location: `src/w.py:119`\n- Status: open\n\
         \n## Important\n### I1. Fixed thing\n- Location: `src/w.py:3`\n- Status: fixed (cycle 1, abc)\n\
         ### I2. No test for UnknownItem\n- Location: `src/w.py:118`\n- Status: open. The fix did not land.\n\
         ### I3. Old code smell\n- Location: `src/old.py:9`\n- Status: deferred: #77\n\
         \n## Suggestion\n### S1. Rename x\n- Location: `src/w.py:5`\n- Status: open\n",
    );
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck");
    let calls = e.gh_calls();
    let comment = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(
        comment.contains(
            "Open findings in `review.md`:\n```text\nC1. cancel_all drops every SKU (src/w.py:119)\nI2. No test for UnknownItem (src/w.py:118)\n```"
        ),
        "{comment}"
    );
    for gone in ["I1.", "I3.", "S1."] {
        assert!(!comment.contains(gone), "{gone} in {comment}");
    }
}

#[test]
fn a_run_stopped_by_an_earlier_blocked_review_names_it() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue(
        "review",
        &["blocked:Open after at least 2 fix attempts: I2."],
    );
    let first = e.run(&["run", "--issue", "7"], 1);
    let again = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(e.calls(), ["triage", "build", "verify", "review"]);
    assert_eq!(again["outcome"], "stuck");
    assert_eq!(again["artifact"], first["artifact"]);
    assert!(
        again["artifact"].as_str().unwrap().ends_with("/review.md"),
        "{again}"
    );
}

#[test]
fn a_failed_review_goes_back_to_build() {
    let e = Env::new();
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("review", &["fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "build", "verify", "review", "ship"]
    );
    let p = e.prompt(5, "build");
    assert!(p.contains("review said fail"), "{p}");
}

const READY: &str = "status:ready-for-agent";

fn labels_after_triage_adds(e: &Env, stray: &str, args: &[&str]) -> Vec<String> {
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl(
        "triage.sh",
        &format!("gh issue edit 2 --add-label {stray}\n"),
    );
    e.queue("triage", &["pass:script"]);
    e.run(args, 0);
    let labels = e.labels(2);
    assert!(labels.contains(&"type:fix".to_string()), "{labels:?}");
    labels
}

#[test]
fn watch_done_leaves_only_the_done_label() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:in-review"]);
}

#[test]
fn watch_stuck_leaves_only_the_stuck_label() {
    let e = Env::new();
    e.queue("verify", &["blocked:needs a board"]);
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:ready-for-human"]);
}

#[test]
fn a_split_brief_ends_the_run_as_split_not_stuck() {
    let e = Env::new();
    e.queue(
        "triage",
        &["split:into #8 and #9.", "split:into #8 and #9."],
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(e.calls(), ["triage"]);
    assert_eq!(v["outcome"], "split", "{v}");
    assert_eq!(v["phase"], "triage", "{v}");
    assert_eq!(
        v["reason"], "brief.md is split: triage said split into #8 and #9.",
        "{v}"
    );
    let artifacts = Path::new(v["worktree"].as_str().unwrap()).join(".ns/7-fix-the-thing");
    assert_eq!(
        v["artifact"].as_str().unwrap(),
        artifacts.join("history/brief-1.md").to_str().unwrap(),
        "{v}"
    );
    assert!(!artifacts.join("brief.md").exists());
    // The split brief is history, so a parent sent back to the queue is triaged afresh.
    let again = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(again["outcome"], "split", "{again}");
    assert_eq!(e.calls(), ["triage", "triage"]);
}

#[test]
fn dry_run_reports_a_split_brief() {
    let e = Env::new();
    e.queue("triage", &["blocked"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    let brief = Path::new(v["worktree"].as_str().unwrap()).join(".ns/7-fix-the-thing/brief.md");
    fs::write(&brief, "---\nstatus: split\n---\nSplit into #8 and #9.\n").unwrap();
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(d["decision"]["action"], "split", "{d}");
    assert_eq!(
        d["decision"]["reason"], "brief.md is split: Split into #8 and #9.",
        "{d}"
    );
}

#[test]
fn watch_split_moves_the_parent_to_split_label_without_a_comment() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["split:into #8 and #9."]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "split", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:split"]);
    let calls = e.gh_calls();
    assert!(!calls.contains("issue comment 2"), "{calls}");
    assert!(!calls.contains("status:ready-for-human"), "{calls}");
    assert!(!calls.contains("pr edit"), "{calls}");
    assert!(
        calls.contains("label create status:split --color d73a4a --description Split into child issues; closes when they're done"),
        "{calls}"
    );
}

/// Split parent 5 under `status:split` whose sub-issues are 8 and 9, both closed as completed,
/// and parent 6 under the old split label whose triage notes list open child 10.
fn split_parents(e: &Env) {
    e.open_by(
        5,
        "Parent",
        &["type:feat", "status:split"],
        "",
        Some("MEMBER"),
    );
    e.gh_file("subs-5", "8\n9\n");
    e.issue(8, "Child a", "CLOSED");
    e.issue(9, "Child b", "CLOSED");
    e.open_by(
        6,
        "Old",
        &["type:feat", "status:needs-define"],
        "",
        Some("MEMBER"),
    );
    let notes = serde_json::json!({"number": 1, "body": "Children:\n- #10 c\n", "authorAssociation": "MEMBER"});
    let outside =
        serde_json::json!({"number": 1, "body": "Children:\n- #8\n", "authorAssociation": "NONE"});
    e.gh_file("comments-6.jsonl", &format!("{notes}\n{outside}\n"));
    e.issue(10, "Child c", "OPEN");
}

#[test]
fn watch_closes_split_parents_whose_children_are_done_and_sweeps_once() {
    let e = Env::new();
    split_parents(&e);
    // The fake still lists closed #5, now with no state: its triage writes no brief.
    e.queue("triage", &["none"]);
    let v = e.run(&["watch", "--once"], 0);
    let calls = e.gh_calls();
    assert!(
        calls.contains("issue close 5 --reason completed"),
        "{calls}"
    );
    assert!(
        calls.contains(&format!("issue comment 5 --body {DISCLOSED}Every child")),
        "{calls}"
    );
    assert_eq!(e.labels(5), ["type:feat"]);
    assert_eq!(e.labels(6), ["type:feat", "status:split"]);
    let actions: Vec<_> = v["split_parents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["number"].as_u64().unwrap(), p["action"].as_str().unwrap()))
        .collect();
    assert_eq!(actions, [(5, "close"), (6, "relabel")], "{v}");

    // The sweep ran: the next night leaves the old label alone.
    e.open_by(11, "Design", &["status:needs-define"], "", Some("MEMBER"));
    e.gh_file("subs-11", "10\n");
    fs::write(e.ghd.join("calls"), "").unwrap();
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(11), ["status:needs-define"]);
    assert!(!e.gh_calls().contains("labels=status:needs-define"));
}
