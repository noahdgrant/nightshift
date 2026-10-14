#[test]
fn factory_validate_warns_on_auto_merge_with_no_human_review() {
    let e = Env::new();
    e.factory("[merge]\npolicy = \"auto\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert!(
        v["warnings"]
            .to_string()
            .contains("merge.human_review is empty"),
        "{v}"
    );
    e.factory("[merge]\npolicy = \"auto\"\nhuman_review = [\".nightshift/**\"]\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    e.factory("[merge]\npolicy = \"human\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
}

#[test]
fn factory_validate_prints_the_supervise_settings() {
    let e = Env::new();
    let validate = || -> Value {
        let out = e
            .ns()
            .args(["factory", "validate"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&out).unwrap()
    };
    e.factory("");
    let v = validate();
    assert_eq!(v["supervise"]["notify"], Value::Null);
    assert_eq!(v["supervise"]["silence_minutes"], 45);
    e.factory("[supervise]\nnotify = \"notify-send ns\"\nsilence_minutes = 20\n");
    let v = validate();
    assert_eq!(v["supervise"]["notify"], "notify-send ns");
    assert_eq!(v["supervise"]["silence_minutes"], 20);
    e.factory("[supervise]\nsilence = 20\n");
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["supervise"], Value::Null);

    e.factory("[supervise]\nsilence_minutes = 0\n");
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["supervise"]["silence_minutes"], 0);
    assert!(
        v["errors"]
            .to_string()
            .contains("[supervise] silence_minutes must be at least 1"),
        "{v}"
    );

    fs::remove_file(e.root.join(".nightshift/nightshift.toml")).unwrap();
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["supervise"], Value::Null);
}

#[test]
fn factory_validate_checks_scorers() {
    let e = Env::new();
    e.factory("");
    e.config("[roles.review]\nharness = \"claude\"\n");
    fs::create_dir_all(e.root.join(".nightshift/scorers")).unwrap();
    let scorer = "---\nrole: review\npassing_score: 1\nsampling_rate: 0.5\nlabels:\n  - value: pass\n    score: 1\n---\nGrade it.\n";
    fs::write(e.root.join(".nightshift/scorers/quality.md"), scorer).unwrap();
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["scorers"][0]["name"], "quality");
    assert_eq!(v["scorers"][0]["sampling_rate"], 0.5);

    fs::write(
        e.root.join(".nightshift/scorers/quality.md"),
        scorer.replace("role: review", "role: judge"),
    )
    .unwrap();
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert!(
        v["errors"][0]
            .as_str()
            .unwrap()
            .starts_with("scorers/quality.md: role \"judge\" is not configured"),
        "{v}"
    );
}

#[test]
fn custom_agent_prompt_is_rendered() {
    let e = Env::new();
    fs::create_dir_all(e.root.join(".nightshift/agents/triage")).unwrap();
    fs::write(
        e.root.join(".nightshift/agents/triage/agent.md"),
        "---\nrole: triage\n---\nPhase: triage\nCustom {skill} {unit} #{issue} gates={gates}\n",
    )
    .unwrap();
    let v = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(
        v["prompt"],
        format!("Phase: triage\nCustom ns-triage {UNIT} #7 gates=auto\n")
    );
}

// ---------------------------------------------------------------- ns watch

/// `ns watch` stdout and stderr with `TZ` and `NS_NOW` overridden.
fn watch_in(e: &Env, tz: &str, now: i64, args: &[&str]) -> (Value, String) {
    let out = e
        .ns()
        .env("TZ", tz)
        .env("NS_NOW", now.to_string())
        .arg("watch")
        .args(args)
        .assert()
        .code(0)
        .get_output()
        .clone();
    (
        serde_json::from_slice(&out.stdout).unwrap(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn watch_until_is_local_time_with_its_offset() {
    let e = Env::new();
    // 2026-10-08T20:00 EDT.
    let (v, _) = watch_in(
        &e,
        "America/Toronto",
        NOW,
        &["--dry-run", "--until", "06:30"],
    );
    assert_eq!(v["until"], "2026-10-09T06:30:00-04:00");
    // 2026-01-14T19:00 EST.
    let (v, _) = watch_in(
        &e,
        "America/Toronto",
        1_768_435_200,
        &["--dry-run", "--until", "06:30"],
    );
    assert_eq!(v["until"], "2026-01-15T06:30:00-05:00");
    let (v, _) = watch_in(&e, "UTC", NOW, &["--dry-run", "--until", "06:30"]);
    assert_eq!(v["until"], "2026-10-09T06:30:00+00:00");
}

#[test]
fn watch_until_keeps_half_hour_and_positive_offsets() {
    let e = Env::new();
    let (v, _) = watch_in(&e, "Asia/Kolkata", NOW, &["--dry-run", "--until", "06:30"]);
    assert_eq!(v["until"], "2026-10-09T06:30:00+05:30");
    let (v, _) = watch_in(
        &e,
        "America/St_Johns",
        NOW,
        &["--dry-run", "--until", "06:30"],
    );
    assert_eq!(v["until"], "2026-10-09T06:30:00-02:30");
}

#[test]
fn watch_reports_until_and_the_usage_reset_in_local_time() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("build", &["limit", "pass:commit"]);
    let (v, err) = watch_in(&e, "America/Toronto", NOW, &["--once", "--until", "06:30"]);
    assert_eq!(v["until"], "2026-10-09T06:30:00-04:00", "{v}");
    assert_eq!(
        v["units"][0]["reset_at"], "2026-10-08T21:00:00-04:00",
        "{v}"
    );
    assert!(
        err.contains("sleeping until 2026-10-08T21:00:00-04:00"),
        "{err}"
    );
    let log = fs::read_to_string(e.root.join(".git/ns/runs.jsonl")).unwrap();
    assert!(log.lines().count() > 0);
    for line in log.lines() {
        let ts = serde_json::from_str::<Value>(line).unwrap()["ts"].clone();
        assert!(ts.as_str().unwrap().ends_with('Z'), "{line}");
    }
}

#[test]
fn watch_dry_run_orders_and_skips() {
    let e = Env::new();
    e.ready(1, "Docs", &["type:docs"], "");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix blocked", &["type:fix"], "Blocked by: #9\n\nmore");
    e.ready(4, "Feat", &["type:feat"], "Blocked by: #8");
    e.ready(5, "Fix with PR", &["type:fix"], "");
    e.ready(6, "Unlabelled", &[], "");
    e.issue(9, "open blocker", "OPEN");
    e.issue(8, "closed blocker", "CLOSED");
    e.gh_file("prs.json", r#"[{"number":40,"body":"Closes #5"}]"#);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert!(v["until"].is_null());
    let order: Vec<u64> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    assert_eq!(order, [2, 4, 1, 6]);
    let skipped = v["skipped"].to_string();
    assert!(skipped.contains("blocked by open #9"), "{skipped}");
    assert!(skipped.contains("open PR #40 closes it"), "{skipped}");
    assert!(e.calls().is_empty());
    assert!(!e.gh_calls().contains("issue edit"));
}

/// `gh issue view` reports a pull request's state too: a merged PR as `MERGED`.
#[test]
fn watch_dry_run_clears_a_merged_pr_blocker() {
    let e = Env::new();
    e.ready(1, "After merged PR", &["type:fix"], "Blocked by: #40");
    e.ready(2, "After open PR", &["type:fix"], "Blocked by: #41");
    e.issue(40, "merged PR", "MERGED");
    e.issue(41, "open PR", "OPEN");
    let v = e.run(&["watch", "--dry-run"], 0);
    let order: Vec<u64> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    assert_eq!(order, [1]);
    let skipped = v["skipped"].to_string();
    assert!(skipped.contains("blocked by open #41"), "{skipped}");
    assert!(!skipped.contains("#40"), "{skipped}");
}

/// `ns watch --dry-run`'s output and stderr.
fn dry_run(e: &Env) -> (Value, String) {
    let out = e.ns().args(["watch", "--dry-run"]).output();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{err}");
    (serde_json::from_slice(&out.stdout).unwrap(), err)
}

const MAIN_SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

/// GitHub's answer for main's CI: head `MAIN_SHA`, `required` named by a ruleset, and check runs
/// `(name, status, conclusion)`.
fn main_ci(e: &Env, required: &[&str], runs: &[(&str, &str, &str)]) {
    e.gh_file("main-ci.json", &main_ci_at(MAIN_SHA, required, runs));
}

/// GitHub's answer for main's CI at head `sha`; each check run's job id is 40 plus its index.
fn main_ci_at(sha: &str, required: &[&str], runs: &[(&str, &str, &str)]) -> String {
    let rules = serde_json::json!([{"parameters": {"requiredStatusChecks":
        required.iter().map(|c| serde_json::json!({"context": c})).collect::<Vec<_>>()}}]);
    let nodes: Vec<Value> = runs
        .iter()
        .enumerate()
        .map(|(i, (n, s, c))| {
            serde_json::json!({"name": n, "status": s, "conclusion": c, "databaseId": 40 + i})
        })
        .collect();
    serde_json::json!({"data": {"repository": {"defaultBranchRef": {
        "rules": {"nodes": rules},
        "target": {"oid": sha, "statusCheckRollup": {"contexts": {"nodes": nodes}}},
    }}}})
    .to_string()
}

/// Main red at `MAIN_SHA`: check `test` failed.
fn main_red(e: &Env) {
    main_ci(e, &[], &[("test", "COMPLETED", "FAILURE")]);
}

const FIX_TITLE: &str = "fix: main is red at abcdef0: test";

#[test]
fn watch_with_main_green_runs_the_queue_and_files_nothing() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    main_ci(&e, &[], &[("test", "COMPLETED", "SUCCESS")]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
    assert!(events_named(&e, "main_red").is_empty());
    assert!(events_named(&e, "main_green").is_empty());
}

#[test]
fn watch_with_main_red_files_one_fix_issue_and_starts_only_it() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    main_red(&e);
    // A cap above one, so a second unit or a second filing fails the test instead of looping.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(v["triaged"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(e.triage_only_runs(), 0);
    let calls = e.gh_calls();
    assert_eq!(calls.matches("issue create").count(), 1, "{calls}");
    assert!(
        calls.contains(&format!("issue create --title {FIX_TITLE} --body ")),
        "{calls}"
    );
    assert!(
        calls.contains("--label type:fix --label priority:critical --label status:ready-for-agent"),
        "{calls}"
    );
    assert!(!calls.contains("issue edit 2 "), "{calls}");
    for l in ["type:fix", "priority:critical", "status:ready-for-agent"] {
        assert!(
            calls.contains(&format!("label create {l} --color d73a4a --description ")),
            "{calls}"
        );
    }
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.contains("### test"), "{body}");
    assert!(body.contains("untrusted output from CI"), "{body}");
    assert!(body.contains("FAILED tests/test_x.py::test_y"), "{body}");
    assert!(
        body.contains("No green run on the default branch found"),
        "{body}"
    );
    assert!(body.starts_with(DISCLOSED), "{body}");
    assert!(calls.contains("run view --job 40 --log-failed"), "{calls}");
}

#[test]
fn watch_with_main_red_runs_no_triage_only_run() {
    let e = Env::new();
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    main_red(&e);
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(e.triage_only_runs(), 0);
    assert!(v["triaged"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
}

#[test]
fn watch_stops_triaging_when_main_turns_red_after_a_triage_only_run() {
    let e = Env::new();
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    e.untriaged(6, "Fix f", &["status:needs-triage", "type:fix"]);
    main_ci(&e, &[], &[("test", "COMPLETED", "SUCCESS")]);
    e.gh_file(
        "main-ci.red",
        &main_ci_at(MAIN_SHA, &[], &[("test", "COMPLETED", "FAILURE")]),
    );
    // #5's triage-only run edits its issue, and main is red from then on.
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *\"issue edit 5 \"*) cp \"$d/main-ci.red\" \"$d/main-ci.json\" ;; esac\n",
    );
    e.triage_sets(&["status:needs-info"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    e.queue("triage.6-fix-f", &["none"]);
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(e.triage_only_runs(), 1, "{:?}", e.calls());
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    assert_eq!(e.gh_calls().matches("issue create").count(), 1);
}

#[test]
fn a_resumed_night_holds_at_its_first_fill_when_main_is_red() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    main_red(&e);
    let path = empty_night(&e);
    let out = e
        .ns()
        .args(["watch", "--max-units", "3", "--resume-night"])
        .arg(&path)
        .output();
    let err = String::from_utf8_lossy(&out.stderr);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    assert_eq!(e.triage_only_runs(), 0);
    assert_eq!(e.gh_calls().matches("issue create").count(), 1);
}

#[test]
fn a_unit_running_when_main_turns_red_finishes_and_the_backlog_waits() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w =
        Watching::start(
            e.ns()
                .env("NS_REFILL_MS", "100")
                .args(["watch", "--parallel", "2"]),
        );
    w.entered(&in_a);
    // Main is red before the backlog's next issue appears, so no fill sees one without the other.
    main_red(&e);
    e.ready(3, "Fix b", &["type:fix"], "");
    w.line("ns watch: main is red");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        by_issue(&units(&v)),
        [(2, "done".into()), (100, "done".into())],
        "{v}"
    );
    assert_eq!(e.gh_calls().matches("issue create").count(), 1);
    assert!(!e.gh_calls().contains("issue edit 3 "));
}

#[test]
fn watch_starts_a_fix_issue_it_just_filed_before_github_lists_it() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("create.unlisted", "");
    main_red(&e);
    // A cap above one: a fill after the fix ends must reuse it, not file a second.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    let calls = e.gh_calls();
    assert_eq!(calls.matches("issue create").count(), 1, "{calls}");
}

#[test]
fn a_fix_issue_that_cannot_be_filed_ends_the_night_with_the_error() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("create.fail", "");
    main_red(&e);
    let out = e.ns().args(["watch", "--once"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!out.status.success());
    assert_eq!(v["stopped"], "error", "{v}");
    assert!(v["error"].as_str().unwrap().contains("HTTP 502"), "{v}");
    assert!(units(&v).is_empty(), "{v}");

    let e = gitlab_env();
    e.gl_issue(2, &["type:fix", READY], "", 1);
    e.gh_file(
        "gl-statuses.json",
        &serde_json::json!([{"name": "test", "status": "failed"}]).to_string(),
    );
    e.gh_file("gl-create.raw", "{}");
    let out = e.ns().args(["watch", "--once"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("GitLab named no number"),
        "{v}"
    );
}

#[test]
fn a_fix_issue_is_filed_when_its_labels_already_exist() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("label.exists", "");
    main_red(&e);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    let calls = e.gh_calls();
    assert_eq!(calls.matches("label create").count(), 3, "{calls}");
    assert!(
        calls.contains("--label type:fix --label priority:critical --label status:ready-for-agent"),
        "{calls}"
    );
}

#[test]
fn a_label_that_cannot_be_created_ends_the_night_and_files_nothing() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("label.fail", "HTTP 403: forbidden\n");
    main_red(&e);
    let out = e.ns().args(["watch", "--once"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!out.status.success());
    assert_eq!(v["stopped"], "error", "{v}");
    assert!(v["error"].as_str().unwrap().contains("HTTP 403"), "{v}");
    assert!(units(&v).is_empty(), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
}

#[test]
fn a_closed_fix_issue_at_the_same_red_sha_is_not_filed_again() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    // The fix issue is closed, as by a person, once its unit has begun; main stays red.
    e.gh_file(
        "hook.sh",
        "[ \"$1 $2 $3\" = \"issue edit 100\" ] && { sed -i '/\"number\":100/d' \"$d/issues.jsonl\"; \
         sed -i 's/OPEN/CLOSED/' \"$d/issue-100.json\"; }\n",
    );
    let out = e.ns().args(["watch", "--max-units", "3"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    let calls = e.gh_calls();
    assert_eq!(calls.matches("issue create").count(), 1, "{calls}");
    assert_eq!(err.matches("already filed for it").count(), 1, "{err}");
    assert!(!calls.contains("issue edit 2 "), "{calls}");
}

#[test]
fn a_unit_paused_before_main_turned_red_resumes_and_ends_as_it_would() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue(&format!("build.{A}"), &["limit", "pass:commit"]);
    // Main is green for the night's start and the first fill, red from the fourth read: the fill
    // after the pause. The unit paused before then resumes and finishes beside the fix.
    main_ci(&e, &[], &[("test", "COMPLETED", "SUCCESS")]);
    e.gh_file(
        "main-ci.red",
        &main_ci_at(MAIN_SHA, &[], &[("test", "COMPLETED", "FAILURE")]),
    );
    e.gh_file(
        "hook.sh",
        "if [ \"$1 $2\" = \"api graphql\" ]; then echo x >> \"$d/graphql\"; \
         [ \"$(wc -l < \"$d/graphql\")\" -ge 4 ] && cp \"$d/main-ci.red\" \"$d/main-ci.json\"; fi\n",
    );
    let v = e.run(&["watch", "--max-units", "3"], 0);
    let outcomes: Vec<(u64, String)> = by_issue(&units(&v));
    assert_eq!(
        outcomes,
        [
            (2, "done".into()),
            (2, "paused".into()),
            (100, "done".into())
        ],
        "{v}"
    );
}

#[test]
fn watch_lists_the_commits_since_main_was_last_green_in_the_fix_issue() {
    let e = Env::new();
    let green = git(&e.root, &["rev-parse", "HEAD"]);
    fs::write(e.root.join("README"), "broken\n").unwrap();
    git(&e.root, &["commit", "-qam", "Break the build"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let red = git(&e.root, &["rev-parse", "HEAD"]);
    e.gh_file(
        "main-ci.json",
        &main_ci_at(&red, &[], &[("test", "COMPLETED", "FAILURE")]),
    );
    // Another workflow passed on the red head itself: it is not the last green.
    e.gh_file(
        "runs-green.json",
        &format!(r#"[{{"headSha":"{red}"}},{{"headSha":"{green}"}}]"#),
    );
    e.run(&["watch", "--once"], 0);
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.contains(&format!("Since {}:", &green[..7])), "{body}");
    assert!(
        body.contains(&format!("- {} Break the build", &red[..7])),
        "{body}"
    );
    assert!(e
        .gh_calls()
        .contains("run list --branch main --status success"));
}

#[test]
fn the_commit_list_takes_the_default_branch_from_the_forge_not_from_origins_head() {
    let e = Env::new();
    let green = git(&e.root, &["rev-parse", "HEAD"]);
    fs::write(e.root.join("README"), "broken\n").unwrap();
    git(&e.root, &["commit", "-qam", "Break the build"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let red = git(&e.root, &["rev-parse", "HEAD"]);
    // The remote's HEAD names a branch it lacks, so `git ls-remote --symref` can't name main.
    git(
        &e.base.join("remote.git"),
        &["symbolic-ref", "HEAD", "refs/heads/trunk"],
    );
    e.gh_file(
        "main-ci.json",
        &main_ci_at(&red, &[], &[("test", "COMPLETED", "FAILURE")]),
    );
    e.gh_file("runs-green.json", &format!(r#"[{{"headSha":"{green}"}}]"#));
    e.run(&["watch", "--once"], 0);
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.contains(&format!("Since {}:", &green[..7])), "{body}");
    assert!(e.gh_calls().contains("run list --branch main "));
}

#[test]
fn watch_reuses_the_open_fix_issue_instead_of_filing_another() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    // Outside the team: never reused, since anyone may file a title.
    e.ready_by(
        8,
        "fix: main is red at 1234567: lint",
        &[],
        "",
        Some("NONE"),
    );
    e.ready(
        9,
        "fix: main is red at 1234567: lint",
        &["priority:critical"],
        "",
    );
    main_red(&e);
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[9], "done"), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
    let red = events_named(&e, "main_red");
    assert_eq!(red.len(), 1, "{red:?}");
    assert_eq!(red[0]["issue"], 9);
}

#[test]
fn watch_reuses_a_parked_fix_issue_and_files_no_duplicate() {
    let e = Env::new();
    e.open_by(
        9,
        "fix: main is red at 1234567: lint",
        &["status:needs-info"],
        "",
        Some("MEMBER"),
    );
    main_red(&e);
    let out = e.ns().args(["watch", "--max-units", "3"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{err}");
    assert!(units(&v).is_empty(), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
    assert!(
        err.contains("fix issue #9 is open but the queue will not start it"),
        "{err}"
    );
    let red = events_named(&e, "main_red");
    assert_eq!(red.len(), 1, "{red:?}");
    assert_eq!(red[0]["issue"], 9);
}

#[test]
fn watch_resumes_the_queue_once_the_fix_merges_and_main_is_green() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    let green_sha = "1111111111111111111111111111111111111111";
    e.gh_file(
        "main-ci.green",
        &main_ci_at(green_sha, &[], &[("test", "COMPLETED", "SUCCESS")]),
    );
    // Merging the fix's PR turns main green.
    e.gh_file(
        "hook.sh",
        "[ \"$1 $2 $3\" = \"pr merge 12\" ] && cp \"$d/main-ci.green\" \"$d/main-ci.json\"\n",
    );
    e.ctl("pr-100-fix-main-is-red-at-abcdef0-test", "12");
    e.ctl("pr-2-fix-a", "13");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(
        units(&v)
            .iter()
            .map(|u| (
                u["issue"].as_u64().unwrap(),
                u["outcome"].as_str().unwrap().to_string()
            ))
            .collect::<Vec<_>>(),
        [(100, "merged".to_string()), (2, "merged".to_string())],
        "{v}"
    );
    let red = events_named(&e, "main_red");
    assert_eq!(red.len(), 1, "{red:?}");
    assert_eq!(red[0]["sha"], MAIN_SHA);
    assert_eq!(red[0]["failing"], serde_json::json!(["test"]));
    assert_eq!(red[0]["issue"], 100);
    let green = events_named(&e, "main_green");
    assert_eq!(green.len(), 1, "{green:?}");
    assert_eq!(green[0]["sha"], green_sha);
    assert_eq!(green[0]["state"], "green");
}

#[test]
fn watch_on_gitlab_lists_the_commits_since_the_last_green_pipeline() {
    let e = gitlab_env();
    e.gl_issue(2, &["type:fix", READY], "", 1);
    let green = git(&e.root, &["rev-parse", "HEAD"]);
    fs::write(e.root.join("README"), "broken\n").unwrap();
    git(&e.root, &["commit", "-qam", "Break the build"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let red = git(&e.root, &["rev-parse", "HEAD"]);
    e.gh_file(
        "gl-statuses.json",
        &serde_json::json!([{"name": "test", "status": "failed", "allow_failure": false}])
            .to_string(),
    );
    e.gh_file(
        "gl-pipelines-green.json",
        &serde_json::json!([{"sha": red}, {"sha": green}]).to_string(),
    );
    e.run(&["watch", "--once"], 0);
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.contains(&format!("Since {}:", &green[..7])), "{body}");
    assert!(
        body.contains(&format!("- {} Break the build", &red[..7])),
        "{body}"
    );
}

/// The hook line that turns main to `file`'s answer when PR `pr` merges.
fn main_becomes(pr: u64, file: &str) -> String {
    format!(
        "[ \"$1 $2 $3\" = \"pr merge {pr}\" ] && cp \"$d/main-ci.{file}\" \"$d/main-ci.json\"\n"
    )
}

#[test]
fn watch_holds_nothing_when_main_ci_cannot_be_read_mid_night() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    main_red(&e);
    e.gh_file(
        "hook.sh",
        "[ \"$1 $2 $3\" = \"pr merge 12\" ] && echo 'HTTP 502' > \"$d/main-ci.fail\"\n",
    );
    e.factory(AUTO);
    e.ctl("pr-100-fix-main-is-red-at-abcdef0-test", "12");
    e.ctl("pr-2-fix-a", "13");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    let out = e.ns().args(["watch", "--max-units", "2"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        units(&v)
            .iter()
            .map(|u| u["issue"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [100, 2],
        "{v}\n{err}"
    );
    assert!(err.contains("warning: cannot read main's CI"), "{err}");
    assert!(events_named(&e, "main_green").is_empty());
    assert_eq!(e.gh_calls().matches("issue create").count(), 1);
}

#[test]
fn watch_logs_a_second_main_red_for_a_new_sha_and_again_after_green() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    let sha = |c: char| c.to_string().repeat(40);
    let at = |c: char, run: (&str, &str, &str)| main_ci_at(&sha(c), &[], &[run]);
    let (fail, pass) = (
        ("test", "COMPLETED", "FAILURE"),
        ("test", "COMPLETED", "SUCCESS"),
    );
    main_red(&e);
    e.gh_file("main-ci.second", &at('2', fail));
    e.gh_file("main-ci.green", &at('3', pass));
    e.gh_file("main-ci.third", &at('4', fail));
    // Each merge moves main on: red at a new head, green, then red again.
    let moves = [(12, "second", 100), (13, "green", 101), (14, "third", 2)];
    let mut hook = String::new();
    for (pr, file, n) in moves {
        hook.push_str(&main_becomes(pr, file));
        // Merging closes the fix issue, as the forge does with a closing PR.
        hook.push_str(&format!(
            "[ \"$1 $2 $3\" = \"pr merge {pr}\" ] && sed -i 's/OPEN/CLOSED/' \"$d/issue-{n}.json\" && sed -i '/\"number\":{n},/d' \"$d/issues.jsonl\"\n"
        ));
    }
    e.gh_file("hook.sh", &hook);
    e.ctl("pr-100-fix-main-is-red-at-abcdef0-test", "12");
    e.ctl("pr-101-fix-main-is-red-at-2222222-test", "13");
    e.ctl("pr-2-fix-a", "14");
    e.ctl("pr-102-fix-main-is-red-at-4444444-test", "15");
    e.queue(
        "build",
        &["pass:commit", "pass:commit", "pass:commit", "pass:commit"],
    );
    for pr in 12..=15 {
        e.gh_file(&format!("checks-{pr}.json"), GREEN);
    }
    let v = e.run(&["watch", "--max-units", "4"], 0);
    assert_eq!(
        units(&v)
            .iter()
            .map(|u| u["issue"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [100, 101, 2, 102],
        "{v}"
    );
    let red: Vec<(String, u64)> = events_named(&e, "main_red")
        .iter()
        .map(|r| {
            (
                r["sha"].as_str().unwrap().to_string(),
                r["issue"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        red,
        [
            (MAIN_SHA.to_string(), 100),
            (sha('2'), 101),
            (sha('4'), 102)
        ]
    );
    let green = events_named(&e, "main_green");
    assert_eq!(green.len(), 1, "{green:?}");
    assert_eq!(green[0]["sha"], sha('3'));
}

#[test]
fn watch_resumes_the_queue_when_main_is_pending_with_nothing_failing() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    e.gh_file(
        "main-ci.pending",
        &main_ci_at(MAIN_SHA, &[], &[("test", "IN_PROGRESS", "")]),
    );
    e.gh_file("hook.sh", &main_becomes(12, "pending"));
    e.ctl("pr-100-fix-main-is-red-at-abcdef0-test", "12");
    e.ctl("pr-2-fix-a", "13");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    let v = e.run(&["watch", "--max-units", "2"], 0);
    assert_eq!(
        units(&v)
            .iter()
            .map(|u| u["issue"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [100, 2],
        "{v}"
    );
    let green = events_named(&e, "main_green");
    assert_eq!(green.len(), 1, "{green:?}");
    assert_eq!(green[0]["state"], "pending");
}

/// A night with main red, #2 ready and the fix issue #100, set up further by `setup`.
fn fix_ends(setup: impl Fn(&Env)) -> (Env, Value) {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    setup(&e);
    // A cap above one: a night that goes on would file or start more.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    (e, v)
}

/// The fix issue's unit, set up by `setup`, ends `outcome` with main still red: the night stops.
fn stops_unfixed(outcome: &str, setup: impl Fn(&Env)) {
    let (e, v) = fix_ends(setup);
    assert_eq!(by_issue(&units(&v)), each(&[100], outcome), "{v}");
    assert_eq!(v["stopped"], "main_red_unfixed", "{v}");
    let unfixed = events_named(&e, "main_red_unfixed");
    assert_eq!(unfixed.len(), 1, "{unfixed:?}");
    assert_eq!(unfixed[0]["sha"], MAIN_SHA);
    assert_eq!(unfixed[0]["issue"], 100);
    assert_eq!(unfixed[0]["outcome"], outcome);
    let calls = e.gh_calls();
    assert_eq!(calls.matches("issue create").count(), 1, "{calls}");
    assert!(!calls.contains("issue edit 2 "), "{calls}");
}

#[test]
fn a_fix_unit_that_ends_unfixed_stops_the_night() {
    stops_unfixed("stuck", |e| e.queue("triage", &["none"]));
    stops_unfixed("split", |e| e.queue("triage", &["split:into #8 and #9."]));
    stops_unfixed("budget", |e| {
        e.factory("[limits]\nbudget_usd = 1.0\n");
        e.ctl(
            "triage.sh",
            &format!("gh issue edit 100 --add-label {READY}\n"),
        );
        e.queue("triage", &["pass:script"]);
        e.queue("build", &["pass:commit"]);
    });
}
