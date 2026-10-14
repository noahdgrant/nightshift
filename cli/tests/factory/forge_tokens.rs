/// Unit 2 ready and set to merge on its own PR 12, as the auto-merge tests do.
fn a_unit_that_merges(e: &Env) {
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file(
        "prs.json",
        r#"[{"number":12,"body":"Closes #2","headRefName":"ns/2-fix-a","isCrossRepository":false}]"#,
    );
}

#[test]
fn a_split_read_that_fails_does_not_hold_up_the_night() {
    let e = Env::new();
    a_unit_that_merges(&e);
    split_parents(&e);
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *labels=status:split*) echo 'HTTP 502' >&2; exit 1 ;; esac\n",
    );
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("split parents not checked") && err.contains("HTTP 502"),
        "{err}"
    );
    let calls = e.gh_calls();
    assert!(calls.contains("pr merge 12"), "{calls}");
    assert!(
        calls.contains("issue close 2 --reason completed"),
        "{calls}"
    );
    assert!(!calls.contains("issue close 5"), "{calls}");
    assert_eq!(e.labels(5), ["type:feat", "status:split"]);
    assert!(!e.root.join(".git/ns/split-swept").exists());
}

#[test]
fn a_dry_run_whose_split_read_fails_says_so_and_exits_ok() {
    let e = Env::new();
    split_parents(&e);
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *labels=status:split*) echo 'HTTP 502' >&2; exit 1 ;; esac\n",
    );
    let out = e.ns().args(["watch", "--dry-run"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("split parents not checked") && err.contains("HTTP 502"),
        "{err}"
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["split_parents"], serde_json::json!([]), "{v}");
}

#[test]
fn a_sweep_marker_that_cannot_be_written_leaves_the_sweep_to_run_again() {
    let e = Env::new();
    split_parents(&e);
    // The marker is a link to a directory that is not there, so the write fails and the
    // clone still looks unswept.
    let marker = e.root.join(".git/ns/split-swept");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(e.base.join("missing/marker"), &marker).unwrap();
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("the split sweep may run again"), "{err}");
    assert_eq!(e.labels(6), ["type:feat", "status:split"]);
    // The next night sweeps again: an old-label parent that turned up since moves.
    e.open_by(11, "Design", &["status:needs-define"], "", Some("MEMBER"));
    e.gh_file("subs-11", "10\n");
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(11), ["status:split"]);
}

#[test]
fn watch_dry_run_lists_split_parents_and_changes_nothing() {
    let e = Env::new();
    split_parents(&e);
    let v = e.run(&["watch", "--dry-run"], 0);
    let parents = &v["split_parents"];
    assert_eq!(parents[0]["number"], 5, "{v}");
    assert_eq!(parents[0]["action"], "close");
    assert_eq!(parents[0]["children"], serde_json::json!([8, 9]));
    assert_eq!(parents[1]["number"], 6);
    assert_eq!(parents[1]["action"], "relabel");
    assert_eq!(parents[1]["children"], serde_json::json!([10]));
    assert_eq!(parents[1]["open"], 1);
    let calls = e.gh_calls();
    assert!(
        !calls.contains("issue close") && !calls.contains("issue edit"),
        "{calls}"
    );
    // A dry run leaves the sweep to the first real night.
    let again = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(again["split_parents"][1]["action"], "relabel");
}

#[test]
fn watch_dry_run_reports_waiting_and_childless_split_parents() {
    let e = Env::new();
    e.open_by(5, "Waiting", &["status:split"], "", Some("MEMBER"));
    e.gh_file("subs-5", "8\n");
    e.issue(8, "c", "OPEN");
    e.open_by(6, "Bare", &["status:split"], "", Some("MEMBER"));
    let v = e.run(&["watch", "--dry-run"], 0);
    let parents = &v["split_parents"];
    assert_eq!(parents[0]["number"], 5, "{v}");
    assert_eq!(parents[0]["action"], "wait");
    assert_eq!(parents[0]["open"], 1);
    assert_eq!(parents[1]["number"], 6);
    assert_eq!(parents[1]["action"], "no_children");
    assert_eq!(parents[1]["children"], serde_json::json!([]));
}

#[test]
fn watch_leaves_the_sweep_pending_when_a_swept_parent_cannot_be_read() {
    let e = Env::new();
    e.open_by(
        6,
        "Old",
        &["type:feat", "status:needs-define"],
        "",
        Some("MEMBER"),
    );
    e.gh_file("subs-6", "99\n");
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(6), ["type:feat", "status:needs-define"]);

    e.gh_file("subs-6", "10\n");
    e.issue(10, "c", "OPEN");
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(6), ["type:feat", "status:split"]);
}

#[test]
fn watch_reports_a_split_parent_with_no_children_once_a_night() {
    let e = Env::new();
    e.open_by(5, "Parent", &["status:split"], "", Some("MEMBER"));
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["split:into #8 and #9."]);
    let out = e
        .ns()
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    let said = |n: u64| {
        err.matches(&format!(
            "#{n} is labelled status:split but has no children"
        ))
        .count()
    };
    // Filled at start and again once unit 2 ended: two passes, one report each.
    assert_eq!((said(5), said(2)), (1, 1), "{err}");
    assert_eq!(e.labels(5), ["status:split"]);
}

#[test]
fn watch_closes_a_split_parent_whose_last_child_closed_during_the_run() {
    let e = Env::new();
    e.open_by(5, "Parent", &["status:split"], "", Some("MEMBER"));
    e.gh_file("subs-5", "8\n");
    e.issue(8, "Child", "OPEN");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["split:script"]);
    e.ctl(
        "triage.sh",
        "sed -i 's/\"OPEN\"/\"CLOSED\"/' \"$FAKE_GH_DIR/issue-8.json\"\n",
    );
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "split", "{v}");
    assert!(e.gh_calls().contains("issue close 5 --reason completed"));
    assert_eq!(v["split_parents"][0]["number"], 5, "{v}");
    assert_eq!(v["split_parents"][0]["action"], "close", "{v}");
}

#[test]
fn watch_finds_split_children_by_their_part_of_line() {
    let e = Env::new();
    e.open_by(5, "Parent", &["status:split"], "", Some("MEMBER"));
    let child = |n: u64, body: &str, assoc: &str| serde_json::json!({"number": n, "title": "c", "labels": [], "body": body, "authorAssociation": assoc});
    let found = [
        child(8, "Part of #5\n\nbody", "MEMBER"),
        child(9, "Part of #5", "NONE"),
        child(10, "see #5", "MEMBER"),
    ];
    let lines: String = found.iter().map(|c| format!("{c}\n")).collect();
    e.gh_file("search.jsonl", &lines);
    e.issue(8, "c", "CLOSED");
    for n in [9, 10] {
        e.issue(n, "c", "OPEN");
    }
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(
        v["split_parents"][0]["children"],
        serde_json::json!([8]),
        "{v}"
    );
    let calls = e.gh_calls();
    assert!(
        calls.contains("api search/issues --method GET -F q=repo:{owner}/{repo} is:issue in:body \"Part of #5\""),
        "{calls}"
    );
    assert!(calls.contains("issue close 5"), "{calls}");
}

#[test]
fn watch_sends_a_split_parent_with_a_dropped_child_to_a_human() {
    let e = Env::new();
    e.open_by(5, "Parent", &["status:split"], "", Some("MEMBER"));
    e.gh_file("subs-5", "8\n");
    e.gh_file(
        "issue-8.json",
        r#"{"number":8,"title":"c","url":"u","state":"CLOSED","stateReason":"NOT_PLANNED"}"#,
    );
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(5), ["status:ready-for-human"]);
    assert_eq!(v["split_parents"][0]["action"], "ready_for_human", "{v}");
    assert!(!e.gh_calls().contains("issue close 5"));
}

#[test]
fn watch_split_sets_a_custom_split_label() {
    let e = Env::new();
    e.factory("[queue]\nsplit_label = \"tracking\"\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["split:into #8 and #9."]);
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(2), ["type:fix", "tracking"]);
}

#[test]
fn watch_merged_leaves_no_status_label() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once"]);
    assert_eq!(labels, ["type:fix"]);
}

#[test]
fn watch_given_back_leaves_only_the_ready_label() {
    let e = Env::new();
    e.ctl("reset", &(NOW + 8 * 3600).to_string());
    e.queue("build", &["limit"]);
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once", "--until", "06:30"]);
    assert_eq!(labels, ["type:fix", "status:ready-for-agent"]);
}

#[test]
fn watch_budget_leaves_only_the_ready_label() {
    let e = Env::new();
    e.factory("[limits]\nbudget_usd = 1.0\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl(
        "triage.sh",
        &format!("gh issue edit 2 --add-label {READY}\n"),
    );
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "budget", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
}

#[test]
fn watch_done_removes_an_unconfigured_status_label() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    let labels = labels_after_triage_adds(&e, "status:needs-info", &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:in-review"]);
}

#[test]
fn watch_stuck_removes_an_unconfigured_status_label() {
    let e = Env::new();
    e.queue("verify", &["blocked:needs a board"]);
    let labels = labels_after_triage_adds(&e, "status:needs-info", &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:ready-for-human"]);
}

#[test]
fn watch_merged_removes_an_unconfigured_status_label() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let labels = labels_after_triage_adds(&e, "status:needs-info", &["watch", "--once"]);
    assert_eq!(labels, ["type:fix"]);
}

#[test]
fn watch_restores_the_ready_label_when_a_run_errors() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl(
        "triage.sh",
        "gh issue edit 2 --add-label status:ready-for-agent\n",
    );
    e.ctl("verify.sh", "touch \".ns/$NS_UNIT/history\"\n");
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["fail:script"]);
    let out = e.ns().args(["watch", "--once"]).output();
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
}

#[test]
fn watch_needing_a_human_leaves_only_the_stuck_label() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", ".github/workflows/ci.yml\n");
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:ready-for-human"]);
    let calls = e.gh_calls();
    let comment = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(comment.contains("needs a human on unit"), "{comment}");
    assert!(!comment.contains("got stuck"), "{comment}");
}

#[test]
fn watch_removes_a_stray_custom_ready_label() {
    let e = Env::new();
    e.factory(
        "[queue]\nready_label = \"ready\"\nin_progress_label = \"wip\"\ndone_label = \"shipped\"\nstuck_label = \"blocked\"\n",
    );
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("labels-2", "type:fix\nready\n");
    e.ctl("triage.sh", "gh issue edit 2 --add-label ready\n");
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(2), ["type:fix", "shipped"]);
}

#[test]
fn watch_retries_a_label_edit_that_did_not_land() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("edit.drop-once", "");
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(2), ["type:fix", "status:in-review"]);
    let dropped =
        "issue edit 2 --remove-label status:ready-for-agent --add-label status:in-progress";
    assert!(
        e.gh_calls().matches(dropped).count() >= 2,
        "{}",
        e.gh_calls()
    );
}

#[test]
fn watch_fails_loudly_when_a_label_edit_does_not_land() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("edit.noop", "");
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("#2") && err.contains("status:ready-for-agent"),
        "{err}"
    );
    assert!(e.calls().is_empty());
}

#[test]
fn watch_fails_loudly_when_the_end_label_edit_does_not_land() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("triage.sh", "touch \"$FAKE_GH_DIR/edit.noop\"\n");
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("#2") && err.contains("missing status:in-review"),
        "{err}"
    );
}

#[test]
fn watch_posts_the_stuck_comment_even_when_the_label_edit_fails() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("triage.sh", "touch \"$FAKE_GH_DIR/edit.noop\"\n");
    e.queue("triage", &["pass:script"]);
    e.queue("verify", &["blocked:needs a board"]);
    e.ns().args(["watch", "--once"]).assert().code(1);
    assert!(e.gh_calls().contains("issue comment 2"), "{}", e.gh_calls());
}

#[test]
fn watch_closes_the_merged_issue_even_when_the_label_edit_fails() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.ctl("triage.sh", "touch \"$FAKE_GH_DIR/edit.noop\"\n");
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.ns().args(["watch", "--once"]).assert().code(1);
    assert!(e.gh_calls().contains("issue close 2"), "{}", e.gh_calls());
}

#[test]
fn watch_respects_max_units() {
    let e = Env::new();
    for n in [2, 3, 4] {
        e.ready(n, &format!("Fix {n}"), &["type:fix"], "");
    }
    e.queue("build", &["pass:commit", "pass:commit", "pass:commit"]);
    let v = e.run(&["watch", "--max-units", "2"], 0);
    let issues: Vec<u64> = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["issue"].as_u64().unwrap())
        .collect();
    assert_eq!(issues, [2, 3]);
    assert_eq!(v["stopped"], "max_units");
}

#[test]
fn watch_keeps_its_start_config_when_the_files_break_between_units() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    e.factory("[defaults]\nmax_attempts = 3\n");
    let factory = e.root.join(".nightshift/nightshift.toml");
    let config = e.base.join("no-config.toml");
    let sha = |p: &Path| {
        Sha256::digest(fs::read(p).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let config_sha = sha(&config);
    let factory_sha = sha(&factory);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.ctl(
        "triage.sh",
        &format!(
            "echo '[forge]\nbogus = 1' > {0}\necho 'not = [toml' > {1}\n",
            config.display(),
            factory.display()
        ),
    );
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit", "pass:commit"]);
    let out = e
        .ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .args(["watch", "--max-units", "2"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let outcomes: Vec<&str> = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["done", "done"], "{v}");
    assert!(fs::read_to_string(&config).unwrap().contains("bogus"));
    let started = &v["started_with"];
    assert_eq!(started["config"]["path"], config.to_str().unwrap());
    assert_eq!(started["factory"]["path"], factory.to_str().unwrap());
    assert_eq!(started["config"]["sha256"], config_sha.as_str());
    assert_eq!(started["factory"]["sha256"], factory_sha.as_str());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(config.to_str().unwrap()), "{err}");
    assert!(
        err.contains(started["config"]["sha256"].as_str().unwrap()),
        "{err}"
    );
    assert!(err.contains(factory.to_str().unwrap()), "{err}");
    assert!(err.contains(&factory_sha), "{err}");
    assert!(!err.contains(FAKE_TOKEN), "{err}");
}

#[test]
fn watch_stops_when_queue_empties() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "queue empty");
}

#[test]
fn watch_sleeps_through_a_usage_limit_before_until() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("build", &["limit", "pass:commit"]);
    let v = e.run(&["watch", "--once", "--until", "06:30"], 0);
    let units = v["units"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{v}");
    assert_eq!(units[0]["outcome"], "paused");
    assert_eq!(units[0]["reset_at"], "2026-10-09T01:00:00+00:00");
    assert_eq!(units[1]["outcome"], "done");
    assert!(!e.gh_calls().contains("--add-label status:ready-for-agent"));
}

#[test]
fn watch_gives_back_the_issue_when_the_limit_resets_after_until() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.ctl("reset", &(NOW + 8 * 3600).to_string());
    e.queue("build", &["limit"]);
    let v = e.run(&["watch", "--until", "06:30"], 0);
    assert_eq!(v["stopped"], "usage limit resets after --until");
    assert!(e.gh_calls().contains(
        "issue edit 2 --remove-label status:in-progress --add-label status:ready-for-agent"
    ));
    assert!(!e.gh_calls().contains("issue edit 3"));
}

#[test]
fn watch_merged_removes_in_progress_and_bases_on_origin() {
    let e = Env::new();
    let remote = e.base.join("remote.git");
    // Someone else lands a commit on origin/main; the local main lags behind.
    let other = e.base.join("other");
    git(
        &e.base,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    fs::write(other.join("NEW"), "x\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "upstream"]);
    git(&other, &["push", "-q", "origin", "main"]);
    let upstream = git(&other, &["rev-parse", "HEAD"]);

    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.issue(2, "Fix a", "OPEN");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(
        calls.contains("issue edit 2 --remove-label status:in-progress\n"),
        "{calls}"
    );
    let merged = merged_sha(&e);
    assert!(git(
        &e.root,
        &["merge-base", "--is-ancestor", &upstream, &merged]
    )
    .is_empty());
}

#[test]
fn watch_closes_a_merged_issue_and_never_restarts_it() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("ready-sticks", "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    // A cap above one, so a restart fails the test instead of looping.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(v["units"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(v["stopped"], "queue empty");
    let calls = e.gh_calls();
    assert_eq!(
        calls.matches("labels=status:ready-for-agent").count(),
        2,
        "{calls}"
    );
    // The merged issue carries no status label now, but it is not triaged again.
    assert!(v["triaged"].as_array().unwrap().is_empty(), "{v}");
    assert!(
        calls.contains("issue close 2 --reason completed"),
        "{calls}"
    );
}

#[test]
fn watch_keeps_going_when_the_merged_issue_is_already_closed() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("close.fail", "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert!(e.gh_calls().contains("issue close 2"));
}

#[test]
fn watch_never_restarts_an_issue_it_finished() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("ready-sticks", "");
    e.queue("build", &["pass:commit", "pass:commit"]);
    // A cap above one, so a restart fails the test instead of looping.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(v["units"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["units"][0]["outcome"], "done");
    assert_eq!(v["stopped"], "queue empty");
    assert!(!e.gh_calls().contains("issue close"));
}

#[test]
fn watch_skips_an_issue_a_merged_pr_closes() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.gh_file("prs-merged.json", r#"[{"number":41,"body":"Fixes #3"}]"#);
    let v = e.run(&["watch", "--dry-run"], 0);
    let order: Vec<u64> = queue_order(&v).iter().map(|q| q.0).collect();
    assert_eq!(order, [2]);
    assert_eq!(v["skipped"][0]["number"], 3);
    assert_eq!(v["skipped"][0]["reason"], "merged PR #41 closes it");
}

#[test]
fn watch_has_no_unit_cap_by_default() {
    let e = Env::new();
    for n in [2, 3, 4, 5, 6] {
        e.ready(n, &format!("Fix {n}"), &["type:fix"], "");
    }
    e.queue("build", &["pass:commit"; 5]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["units"].as_array().unwrap().len(), 5, "{v}");
    assert_eq!(v["stopped"], "queue empty");
}

const FAKE_TOKEN: &str = "fake-token-4f2a9c";

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn forge_token_reaches_every_child_and_never_the_logs() {
    let e = Env::new();
    let cmd = e.script("token.sh", &format!("echo '  {FAKE_TOKEN}  '"));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let out = e
        .ns()
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!printed.contains(FAKE_TOKEN), "{printed}");
    let ns_dir = e.root.join(".git/ns");
    let logged = files_under(&ns_dir);
    assert!(logged.iter().any(|p| p.ends_with("runs.jsonl")));
    assert!(logged
        .iter()
        .any(|p| p.to_string_lossy().contains("transcripts")));
    for p in logged {
        let text = fs::read_to_string(&p).unwrap_or_default();
        assert!(!text.contains(FAKE_TOKEN), "token in {}", p.display());
    }
}

#[test]
fn forge_token_env_is_read_and_exported() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    let out = e
        .ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
}

#[test]
fn a_set_gh_token_wins_and_no_command_runs() {
    let e = Env::new();
    let marker = e.base.join("ran");
    let cmd = e.script("token.sh", &format!("touch {}; exit 1", marker.display()));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ns()
        .env("GH_TOKEN", "from-env")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(
        !marker.exists(),
        "token_command ran though GH_TOKEN was set"
    );
    assert_eq!(e.tokens_seen(), ["from-env"]);
}

const OVERRIDES: &str = "[forge.github]
token_env = \"DEFAULT_TOKEN\"
[forge.github.owners.\"example-org\"]
token_env = \"OWNER_TOKEN\"
[forge.github.repos.\"example-org/backend\"]
token_env = \"REPO_TOKEN\"
";

const OVERRIDE_WARNING: &str = "GH_TOKEN from the environment overrides the config's choice";

#[test]
fn the_token_comes_from_the_table_origin_matches() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.config(OVERRIDES);
    for (origin, want) in [
        ("git@github-work:example-org/backend.git", "repo-tok"),
        ("https://github.com/Example-Org/frontend.git", "owner-tok"),
        ("ssh://git@github.com/someone/else", "default-tok"),
        ("https://github.com/example-org", "default-tok"),
    ] {
        git(&e.root, &["remote", "set-url", "origin", origin]);
        let _ = fs::remove_file(e.ghd.join("tokens"));
        let _ = fs::remove_file(e.ghd.join("calls"));
        let out = e
            .ns()
            .env("DEFAULT_TOKEN", "default-tok")
            .env("OWNER_TOKEN", "owner-tok")
            .env("REPO_TOKEN", "repo-tok")
            .args(["watch", "--dry-run"])
            .assert()
            .code(0)
            .get_output()
            .clone();
        assert_eq!(e.tokens_seen(), [want], "{origin}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!err.contains(OVERRIDE_WARNING), "{err}");
        let checked = e
            .gh_calls()
            .lines()
            .filter(|l| l.starts_with("api repos/") && l.ends_with(".full_name"))
            .count();
        assert_eq!(checked, 0, "{origin}: a dry run checked: {}", e.gh_calls());
    }
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:example-org/backend.git",
        ],
    );
    let _ = fs::remove_file(e.ghd.join("tokens"));
    let gitlab_default = e.script(
        "gl-default.sh",
        &format!("touch {}/gl-default; echo gl", e.base.display()),
    );
    let gitlab_owner = e.script(
        "gl-owner.sh",
        &format!("touch {}/gl-owner; echo gl", e.base.display()),
    );
    e.config(&format!(
        "{OVERRIDES}[forge.gitlab]\ntoken_command = {:?}\n[forge.gitlab.owners.\"example-org\"]\ntoken_command = {:?}\n",
        gitlab_default.to_str().unwrap(),
        gitlab_owner.to_str().unwrap()
    ));
    e.ns()
        .env_remove("GITLAB_TOKEN")
        .env("DEFAULT_TOKEN", "default-tok")
        .env("OWNER_TOKEN", "owner-tok")
        .env("REPO_TOKEN", "repo-tok")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), ["repo-tok"]);
    assert!(
        e.base.join("gl-default").exists(),
        "the other forge's default table ran no command"
    );
    assert!(
        !e.base.join("gl-owner").exists(),
        "a github origin matched a gitlab owner table"
    );
}

#[test]
fn a_set_gh_token_wins_over_a_matching_table_and_warns_once_a_night() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:example-org/backend.git",
        ],
    );
    e.config(OVERRIDES);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let out = e
        .ns()
        .env("GH_TOKEN", "from-env")
        .env("REPO_TOKEN", "repo-tok")
        .args(["watch", "--once"])
        .assert()
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.matches(OVERRIDE_WARNING).count(), 1, "{err}");
    assert!(
        err.contains("[forge.github.repos.\"example-org/backend\"]"),
        "{err}"
    );
    assert_eq!(e.tokens_seen(), ["from-env"]);
    assert!(!e.calls().is_empty(), "the unit never ran: {err}");
    let checks = e
        .gh_calls()
        .lines()
        .filter(|l| *l == "api repos/example-org/backend --jq .full_name")
        .count();
    assert_eq!(
        checks,
        1,
        "the unit checked the repo again: {}",
        e.gh_calls()
    );

    for args in [&["run", "--issue", "7"][..], &["watch", "--dry-run"][..]] {
        let out = e
            .ns()
            .env("GH_TOKEN", "from-env")
            .args(args)
            .assert()
            .code(0)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(err.matches(OVERRIDE_WARNING).count(), 1, "{args:?}: {err}");
    }
    e.config("");
    let out = e
        .ns()
        .env("GH_TOKEN", "from-env")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains(OVERRIDE_WARNING),
        "no config, nothing to override: {err}"
    );
}
