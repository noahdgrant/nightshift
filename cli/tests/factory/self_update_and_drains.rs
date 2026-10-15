#[test]
fn the_budget_stop_during_a_build_ends_the_night_without_a_hand_off() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    // Unit 3 spends 2.5 and unit 2 reaches the budget at its review.
    e.factory("[limits]\nbudget_usd = 4.0\n");
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(watch_from(&e, &from).args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    land(&e, "cli/x", "new\n");
    release(&go_b);
    w.line("self-update: building");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(v["stopped"], "budget", "{v}");
    assert!(events_named(&e, "self_update").is_empty(), "{err}");
    assert!(!e.ctrl.join("handed-off").exists(), "{err}");
}

#[test]
fn a_signal_during_a_build_ends_the_night_without_a_hand_off() {
    for sig in [libc::SIGTERM, libc::SIGINT] {
        let e = Env::new();
        let from = nightshift_repo(&e);
        two_ready(&e);
        let (in_a, go_a) = hold_phase(&e, A, "build");
        let (in_b, go_b) = hold_phase(&e, B, "build");
        let mut w = Watching::start(watch_from(&e, &from).args(["watch", "--parallel", "2"]));
        w.entered(&in_a);
        w.entered(&in_b);
        land(&e, "cli/x", "new\n");
        release(&go_b);
        w.line("self-update: building");
        w.signal(sig, false);
        release(&go_a);
        let (code, v, err) = w.finish();
        let name = if sig == libc::SIGTERM {
            "SIGTERM"
        } else {
            "SIGINT"
        };
        assert_eq!(code, Some(128 + sig), "{err}");
        assert_eq!(v["stopped"], name, "{v}");
        assert!(events_named(&e, "self_update").is_empty(), "{err}");
        assert!(!e.ctrl.join("handed-off").exists(), "{err}");
    }
}

#[test]
fn the_harness_breaker_and_finished_issues_survive_the_hand_off() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    e.ready(2, "Fix 2", &["type:fix"], "");
    // Each unit's triage phase fails instantly on both attempts.
    e.queue("triage", &["crash"; 4]);
    // The new binary's check holds once the old night's last fill is done.
    let (in_dry, go_dry) = (e.ctrl.join("dry-entered"), e.ctrl.join("dry-go"));
    mkfifo(&in_dry);
    mkfifo(&go_dry);
    let (in_2, go_2) = hold_phase(&e, "2-fix-2", "triage");
    let w = Watching::start(watch_from(&e, &from).args(["watch"]));
    w.entered(&in_2);
    land(&e, "cli/x", "new\n");
    release(&go_2);
    w.entered(&in_dry);
    for n in [3, 4] {
        e.ready(n, &format!("Fix {n}"), &["type:fix"], "");
    }
    release(&go_dry);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(events_named(&e, "self_update").len(), 1, "{err}");
    // Unit 2's failure counts toward the breaker after the hand-off, and unit 2, back to ready,
    // is not taken again.
    assert_eq!(v["stopped"], "harness failing", "{v}");
    assert_eq!(
        by_issue(&units(&v)),
        each(&[2, 3], "harness_failing"),
        "{v}"
    );
}

#[test]
fn the_triage_pass_record_survives_the_hand_off() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage", &["pass:script"]);
    let (in_5, go_5) = hold_phase(&e, "5-a", "build");
    let w = Watching::start(watch_from(&e, &from).args(["watch"]));
    w.entered(&in_5);
    land(&e, "cli/x", "new\n");
    release(&go_5);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(events_named(&e, "self_update").len(), 1, "{err}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[5], "done"), "{v}");
    assert_eq!(e.triage_only_runs(), 1);
}

fn watch_refuses_source(e: &Env, says: &str) {
    e.ns()
        .args(["watch", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("[queue] source"))
        .stderr(predicates::str::contains(says))
        .stderr(predicates::str::contains("source = \"github\""));
    assert!(e.gh_calls().is_empty(), "{}", e.gh_calls());
}

#[test]
fn watch_reads_github_when_source_says_so_or_origin_is_not_gitlab() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.factory("[queue]\nsource = \"github\"\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["queue"][0]["number"], 2, "{v}");
    e.factory("");
    git(
        &e.root,
        &["remote", "set-url", "origin", "git@github.com:o/r.git"],
    );
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["queue"][0]["number"], 2, "{v}");
}

#[test]
fn watch_refuses_an_unknown_source() {
    let e = Env::new();
    e.factory("[queue]\nsource = \"bitbucket\"\n");
    watch_refuses_source(&e, "\"bitbucket\"");
}

/// A GitLab project: member ids 1-6 at Developer, Maintainer, Owner, Reporter, Planner and Guest,
/// and 7 a Developer whose invitation is still awaiting.
fn gitlab_env() -> Env {
    let e = Env::new();
    e.factory("[queue]\nsource = \"gitlab\"\n");
    let awaiting =
        serde_json::json!({"id": 7, "username": "u7", "access_level": 30, "state": "awaiting"});
    let extra = [
        awaiting,
        serde_json::json!({"id": 8, "username": "u8", "access_level": 30}),
        serde_json::json!({"id": 9, "username": "u9", "access_level": 30, "state": "blocked"}),
        serde_json::json!({"id": 11, "username": "u11", "access_level": null}),
    ];
    e.gl_members(
        &[(1, 30), (2, 40), (3, 50), (4, 20), (5, 15), (6, 10)],
        &extra,
    );
    e
}

fn queued(v: &Value) -> Vec<u64> {
    v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect()
}

fn skipped(v: &Value) -> Vec<(u64, String)> {
    v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["number"].as_u64().unwrap(),
                i["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn watch_on_gitlab_queues_developers_and_above_and_skips_the_rest() {
    let e = gitlab_env();
    for (n, author) in [
        (2, 1),
        (3, 2),
        (4, 3),
        (5, 4),
        (6, 5),
        (7, 6),
        (8, 99),
        (10, 7),
        (11, 8),
        (12, 9),
        (13, 11),
    ] {
        e.gl_issue(n, &[READY, "type:fix"], "", author);
    }
    e.gl_issue(9, &["type:fix"], "", 1);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [2, 3, 4, 11], "{v}");
    let outside = "author outside the team".to_string();
    assert_eq!(
        skipped(&v),
        [5, 6, 7, 8, 10, 12, 13].map(|n| (n, outside.clone())),
        "{v}"
    );
    assert!(e.gh_calls().is_empty(), "{}", e.gh_calls());
    assert!(
        e.glab_calls()
            .contains("api --paginate projects/:id/members/all"),
        "{}",
        e.glab_calls()
    );
}

#[test]
fn watch_on_gitlab_closes_a_split_parent_from_its_team_notes() {
    let e = gitlab_env();
    e.gl_issue(5, &["status:split"], "", 1);
    let notes = serde_json::json!([
        {"body": "Children:\n- #8\n", "author": {"id": 1}, "system": false},
        {"body": "Children:\n- #9\n", "author": {"id": 4}, "system": false},
        {"body": "Children:\n- #9\n", "author": {"id": 1}, "system": true},
    ]);
    e.gh_file("gl-notes-5.json", &notes.to_string());
    e.gh_file("gl-state-8", "closed");
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(
        v["split_parents"][0]["children"],
        serde_json::json!([8]),
        "{v}"
    );
    let calls = e.glab_calls();
    assert!(calls.contains("issue close 5"), "{calls}");
    assert!(
        calls.contains(&format!("issue note 5 --message {DISCLOSED}Every child")),
        "{calls}"
    );
    assert!(
        calls.contains("api --paginate projects/:id/issues/5/notes?sort=asc&per_page=100"),
        "{calls}"
    );
    assert!(e.labels(5).is_empty());
}

/// GitLab split parent 5 with no notes, and issues 8 (a developer, closed), 9 (a reporter) and 10
/// (a developer, open or closed per `ten`) saying `Part of #5`, as a search lists them.
fn gl_part_of(e: &Env, ten: &str) {
    e.gl_issue(5, &["status:split"], "", 1);
    let child = |n: u64, author: u64| serde_json::json!({"iid": n, "title": "c", "labels": [], "description": "Part of #5", "author": {"id": author, "username": "u"}});
    let found = serde_json::json!([child(8, 1), child(9, 4), child(10, 2)]);
    e.gh_file("gl-search.json", &found.to_string());
    e.gh_file("gl-state-8", "closed");
    e.gh_file("gl-state-9", "opened");
    e.gh_file("gl-state-10", ten);
}

#[test]
fn watch_on_gitlab_finds_split_children_by_their_part_of_line() {
    let e = gitlab_env();
    gl_part_of(&e, "closed");
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(
        v["split_parents"][0]["children"],
        serde_json::json!([8, 10]),
        "{v}"
    );
    assert_eq!(v["split_parents"][0]["action"], "close", "{v}");
    let calls = e.glab_calls();
    assert!(
        calls.contains("in=description&search=Part%20of%20%235"),
        "{calls}"
    );
    assert!(calls.contains("issue close 5"), "{calls}");
}

#[test]
fn watch_on_gitlab_waits_for_an_open_part_of_child() {
    let e = gitlab_env();
    gl_part_of(&e, "opened");
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["split_parents"], serde_json::json!([]), "{v}");
    assert!(!e.glab_calls().contains("issue close"));
    let d = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(d["split_parents"][0]["action"], "wait", "{d}");
    assert_eq!(d["split_parents"][0]["open"], 1, "{d}");
}

#[test]
fn watch_on_gitlab_percent_encodes_the_ready_label_in_the_issue_query() {
    let e = Env::new();
    e.factory("[queue]\nsource = \"gitlab\"\nready_label = \"needs work,a&b\"\n");
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &["needs work,a&b"], "", 1);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [2], "{v}");
    assert!(
        e.glab_calls().contains("&labels=needs%20work%2Ca%26b&"),
        "{}",
        e.glab_calls()
    );
}

#[test]
fn watch_on_gitlab_passes_over_issues_an_open_or_merged_mr_closes() {
    let e = gitlab_env();
    for n in [2, 3, 4, 5, 6, 7] {
        e.gl_issue(n, &[READY], "", 1);
    }
    // 5's open MR is from its own unit branch, so that unit resumes. 6's is a fork's, and 7's
    // names no project, so it may be one too.
    let mr = |iid: u64, closes: u64, branch: &str| {
        serde_json::json!({
            "iid": iid,
            "description": format!("Closes #{closes}"),
            "source_branch": branch,
        })
    };
    let from = |mut v: Value, project: u64| {
        v["source_project_id"] = project.into();
        v["target_project_id"] = 1.into();
        v
    };
    let open = [
        from(mr(12, 3, "x"), 1),
        from(mr(14, 5, "ns/5-issue-5"), 1),
        from(mr(15, 6, "ns/6-issue-6"), 2),
        mr(16, 7, "ns/7-issue-7"),
    ];
    e.gh_file("gl-mrs.json", &Value::from(open.to_vec()).to_string());
    let merged = vec![from(mr(13, 4, "y"), 1)];
    e.gh_file("gl-mrs-merged.json", &Value::from(merged).to_string());
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [2, 5], "{v}");
    assert_eq!(
        skipped(&v),
        [
            (3, "open PR #12 closes it".to_string()),
            (4, "merged PR #13 closes it".to_string()),
            (6, "open PR #15 closes it".to_string()),
            (7, "open PR #16 closes it".to_string()),
        ],
        "{v}"
    );
}

#[test]
fn watch_on_gitlab_reads_blockers_state() {
    let e = gitlab_env();
    e.gl_issue(2, &[READY], "Blocked by: #20", 1);
    e.gl_issue(3, &[READY], "Blocked by: #21", 1);
    // An unreadable blocker counts as open.
    e.gl_issue(4, &[READY], "Blocked by: #22", 1);
    e.gh_file("gl-state-20", "closed\n");
    e.gh_file("gl-issue-22.raw", "not json\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [2], "{v}");
    assert_eq!(
        skipped(&v),
        [
            (3, "blocked by open #21".to_string()),
            (4, "blocked by open #22".to_string()),
        ],
        "{v}"
    );
}

#[test]
fn watch_on_gitlab_swaps_labels_and_comments_through_glab() {
    let e = gitlab_env();
    e.gl_issue(2, &["type:fix", READY], "", 1);
    e.queue("triage", &["none"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-human"]);
    let gh = e.gh_calls();
    for verb in ["issue list", "issue edit", "issue comment", "issue close"] {
        assert!(!gh.contains(verb), "{verb}: {gh}");
    }
    let calls = e.glab_calls();
    assert!(
        calls
            .contains("issue update 2 --unlabel status:ready-for-agent --label status:in-progress"),
        "{calls}"
    );
    let note = calls.split("issue note 2 --message").nth(1).expect("note");
    assert!(note.contains("triage wrote no brief"), "{note}");
    assert!(note.trim_start().starts_with(DISCLOSED), "{note}");
}

#[test]
fn watch_on_gitlab_closes_a_merged_issue_and_drops_its_status_label() {
    let e = gitlab_env();
    e.factory(&format!("[queue]\nsource = \"gitlab\"\n{AUTO}"));
    e.gl_issue(2, &["type:fix", READY], "", 1);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(e.labels(2), ["type:fix"]);
    let gh = e.gh_calls();
    for verb in ["issue list", "issue edit", "issue comment", "issue close"] {
        assert!(!gh.contains(verb), "{verb}: {gh}");
    }
    let calls = e.glab_calls();
    assert!(!calls.contains("issue note"), "{calls}");
    assert!(calls.contains("issue close 2"), "{calls}");
    assert!(
        calls.contains("issue update 2 --unlabel status:in-progress"),
        "{calls}"
    );
}

#[test]
fn watch_on_gitlab_reads_main_ci_from_the_head_commit_statuses() {
    let e = gitlab_env();
    e.gl_issue(2, &[READY], "", 1);
    let (v, _) = dry_run(&e);
    assert_eq!(v["main_ci"]["state"], "none", "{v}");
    let head = git(&e.root, &["rev-parse", "HEAD"]);
    e.gh_file(
        "gl-statuses.json",
        &serde_json::json!([
            {"name": "test", "status": "failed", "allow_failure": false},
            {"name": "flaky", "status": "failed", "allow_failure": true},
            {"name": "lint", "status": "success", "allow_failure": false},
        ])
        .to_string(),
    );
    let (v, _) = dry_run(&e);
    assert_eq!(
        v["main_ci"],
        serde_json::json!({"state": "red", "sha": head, "failing": ["test"]})
    );
    assert_eq!(v["queue_held"], true);
    let calls = e.glab_calls();
    assert!(
        calls.contains(&format!(
            "projects/:id/repository/commits/{head}/statuses?ref=main"
        )),
        "{calls}"
    );
    assert!(e.gh_calls().is_empty(), "{}", e.gh_calls());
}

#[test]
fn watch_on_gitlab_files_the_fix_issue_through_glab_and_starts_only_it() {
    let e = gitlab_env();
    e.gl_issue(2, &["type:fix", READY], "", 1);
    e.gh_file(
        "gl-statuses.json",
        &serde_json::json!([{"id": 51, "name": "test", "status": "failed", "allow_failure": false}])
            .to_string(),
    );
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    let calls = e.glab_calls();
    assert_eq!(
        calls
            .matches("api --method POST projects/:id/issues")
            .count(),
        1,
        "{calls}"
    );
    assert!(
        calls.contains("--raw-field labels=type:fix,priority:critical,status:ready-for-agent"),
        "{calls}"
    );
    assert!(calls.contains("projects/:id/jobs/51/trace"), "{calls}");
    assert!(
        calls.contains("projects/:id/pipelines?ref=main&status=success&per_page=30"),
        "{calls}"
    );
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.starts_with(DISCLOSED), "{body}");
    assert!(body.contains("ERROR: Job failed"), "{body}");
    assert!(!e.gh_calls().contains("issue create"), "{}", e.gh_calls());
}

#[test]
fn watch_on_gitlab_fails_a_dry_run_when_main_has_no_branch_or_head() {
    let e = gitlab_env();
    e.gl_issue(2, &[READY], "", 1);
    for (file, want) in [
        ("gl-project.json", "GitLab names no default branch"),
        ("gl-branch.json", "GitLab names no head for main"),
    ] {
        e.gh_file(file, "{}");
        let out = e.ns().args(["watch", "--dry-run"]).output();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{err}");
        assert!(err.contains(want), "{err}");
        fs::remove_file(e.ghd.join(file)).unwrap();
    }
}

#[test]
fn watch_on_gitlab_keeps_the_token_out_of_its_errors() {
    let e = gitlab_env();
    e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\n");
    e.gl_issue(2, &[READY], "", 1);
    e.gh_file("glab.fail", "");
    let out = e
        .ns()
        .env("MY_GL_TOKEN", FAKE_TOKEN)
        .env_remove("GITLAB_TOKEN")
        .args(["watch", "--dry-run"])
        .output();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("glab api"), "{err}");
    assert!(err.contains("401 Unauthorized"), "{err}");
    assert!(!err.contains(FAKE_TOKEN), "{err}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
}

#[test]
fn watch_detects_a_gitlab_origin_when_source_is_unset() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    e.ready(3, "Fix b", &["type:fix"], "");
    for origin in ["https://gitlab.com/o/r.git", "git@code.example.com:o/r.git"] {
        git(&e.root, &["remote", "set-url", "origin", origin]);
        e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\nhost = \"code.example.com\"\n");
        let out = e
            .ns()
            .env("MY_GL_TOKEN", FAKE_TOKEN)
            .env_remove("GITLAB_TOKEN")
            .env_remove("GITLAB_HOST")
            .args(["watch", "--dry-run"])
            .assert()
            .code(0)
            .get_output()
            .clone();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(queued(&v), [2], "{origin}: {v}");
    }
    assert!(e.gh_calls().is_empty(), "{}", e.gh_calls());
    e.config("");
    git(
        &e.root,
        &["remote", "set-url", "origin", "https://github.com/o/r"],
    );
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [3], "{v}");
    git(
        &e.root,
        &["remote", "set-url", "origin", "https://gitlab.com/o/r.git"],
    );
    e.factory("[queue]\nsource = \"github\"\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [3], "{v}");
}

/// `skills/ns-a` committed in the main checkout and pushed, then `skills/ns-b` pushed to
/// origin from another clone, with `ns-a` linked into each `$HOME`-relative dir in `dirs`.
fn skills_behind_origin(e: &Env, dirs: &[&str]) {
    let skill = |repo: &Path, name: &str| {
        fs::create_dir_all(repo.join("skills").join(name)).unwrap();
        fs::write(repo.join("skills").join(name).join("SKILL.md"), "s\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", name]);
        git(repo, &["push", "-q", "origin", "main"]);
    };
    skill(&e.root, "ns-a");
    let other = e.base.join("other");
    git(
        &e.base,
        &["clone", "-q", "remote.git", other.to_str().unwrap()],
    );
    skill(&other, "ns-b");
    for d in dirs {
        let dir = e.home.join(d);
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(e.root.join("skills/ns-a"), dir.join("ns-a")).unwrap();
    }
}

/// Runs one unit of `ns watch` and checks the main checkout reached origin's `main`.
fn watch_syncs_skills(e: &Env) {
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    let remote = git(&e.base.join("remote.git"), &["rev-parse", "main"]);
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), remote);
    assert_eq!(events_named(e, "skills_synced").len(), 1);
}

#[test]
fn watch_syncs_skills_installed_only_for_cursor() {
    let e = Env::new();
    skills_behind_origin(&e, &[".cursor/skills"]);
    watch_syncs_skills(&e);
    assert_eq!(
        fs::read_link(e.home.join(".cursor/skills/ns-b")).unwrap(),
        e.root.join("skills/ns-b")
    );
    assert!(!e.home.join(".claude/skills").exists());
    assert!(!e.home.join(".agents/skills").exists());
}

#[test]
fn watch_syncs_skills_installed_into_the_default_dirs() {
    let e = Env::new();
    skills_behind_origin(&e, &[".agents/skills", ".claude/skills"]);
    watch_syncs_skills(&e);
    for d in [".agents/skills", ".claude/skills"] {
        assert_eq!(
            fs::read_link(e.home.join(d).join("ns-b")).unwrap(),
            e.root.join("skills/ns-b")
        );
    }
    assert!(!e.home.join(".cursor/skills").exists());
}

/// Every phase on a `codex` harness, the fake harness under that name.
fn codex_night(e: &Env) {
    let codex = e.bin.join("codex");
    copy_exe(&e.bin.join("claude"), &codex);
    e.config(&format!(
        "[harness.codex]\ncommand = [{c:?}]\ncommand_write = [{c:?}]\n",
        c = codex.to_str().unwrap()
    ));
    e.factory("[defaults]\nharness = \"codex\"\n");
}

/// `skills/ns-a` committed in the main checkout, pushed, and linked into `~/.agents/skills`, so
/// the checkout is current.
fn skills_installed(e: &Env) {
    land(e, "skills/ns-a/SKILL.md", "a\n");
    let dir = e.home.join(".agents/skills");
    fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(e.root.join("skills/ns-a"), dir.join("ns-a")).unwrap();
}

/// Push a commit adding `path` to origin from another clone, so the main checkout lacks it; the
/// commit.
fn push_from_elsewhere(e: &Env, path: &str) -> String {
    let other = e.base.join("other");
    if !other.exists() {
        git(
            &e.base,
            &["clone", "-q", "remote.git", other.to_str().unwrap()],
        );
    }
    git(&other, &["pull", "-q", "origin", "main"]);
    let p = other.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, "new\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", path]);
    git(&other, &["push", "-q", "origin", "main"]);
    git(&other, &["rev-parse", "HEAD"])
}

#[test]
fn a_codex_night_drains_before_it_syncs_a_skill_change() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    let from = git(&e.root, &["rev-parse", "HEAD"]);
    two_ready(&e);
    e.ready(4, "Fix c", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    e.queue("triage.5-fix-e", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w =
        Watching::start(
            e.ns()
                .env("NS_REFILL_MS", "100")
                .args(["watch", "--parallel", "2"]),
        );
    w.entered(&in_a);
    w.entered(&in_b);
    let to = push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    release(&go_b);
    w.line("#3 done");
    w.line("draining");
    // Unit 2 is still in its build: the checkout stays put and unit 4 waits for the sync.
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), from);
    assert!(events_named(&e, "skills_synced").is_empty());
    // The drain claims nothing and runs no triage pass for the untriaged #5.
    assert_eq!(e.triage_only_runs(), 0, "{:?}", e.calls());
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3, 4], "done"), "{v}");
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), to);
    assert_eq!(err.matches("draining").count(), 1, "{err}");
    let short = |c: &str| c[..12].to_string();
    assert!(
        err.contains(&format!("({} -> {})", short(&from), short(&to))),
        "{err}"
    );
    let drain = events_named(&e, "skills_drain");
    assert_eq!(drain.len(), 1, "{drain:?}");
    assert_eq!(
        (drain[0]["from"].as_str(), drain[0]["to"].as_str()),
        (Some(&*from), Some(&*to))
    );
    assert_eq!(events_named(&e, "skills_synced").len(), 1);
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    let synced = event_at(&e, |ev| ev["event"] == "skills_synced");
    let started_4 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 4);
    assert!(
        ended_2 < synced && synced < started_4,
        "{:?}",
        run_events(&e)
    );
}

#[test]
fn a_codex_unit_started_by_a_refill_does_not_sync_under_a_running_one() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    let from = git(&e.root, &["rev-parse", "HEAD"]);
    e.ready(2, "Fix a", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_c, go_c) = hold_phase(&e, "4-fix-c", "build");
    let w = Watching::start(
        e.ns()
            .env("NS_REFILL_MS", "100")
            .args(["watch", "--parallel", "2"]),
    );
    w.entered(&in_a);
    let to = push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    e.ready(4, "Fix c", &["type:fix"], "");
    w.entered(&in_c);
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), from);
    release(&go_c);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 4], "done"), "{v}");
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), to);
}

#[test]
fn a_codex_night_starts_on_current_skills() {
    let e = Env::new();
    codex_night(&e);
    skills_behind_origin(&e, &[".agents/skills"]);
    watch_syncs_skills(&e);
    assert_eq!(events_named(&e, "skills_drain").len(), 1);
}

#[test]
fn a_codex_night_does_not_drain_for_a_change_outside_the_skills() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    push_from_elsewhere(&e, "src/x");
    e.ready(2, "Fix a", &["type:fix"], "");
    let out = e.ns().args(["watch", "--once"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("draining"), "{err}");
    assert!(events_named(&e, "skills_drain").is_empty());
}

#[test]
fn a_codex_night_whose_checkout_cannot_move_warns_once_and_runs_its_units() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    fs::write(e.root.join("skills/ns-a/SKILL.md"), "edited\n").unwrap();
    push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    two_ready(&e);
    let out = e.ns().args(["watch"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(err.matches("draining").count(), 1, "{err}");
    assert_eq!(events_named(&e, "skills_stale").len(), 1);
    assert!(events_named(&e, "skills_synced").is_empty());
}

#[test]
fn a_claude_night_syncs_before_every_unit_without_draining() {
    let e = Env::new();
    skills_installed(&e);
    two_ready(&e);
    e.ready(4, "Fix c", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    let to = push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    let (in_c, go_c) = hold_phase(&e, "4-fix-c", "build");
    release(&go_b);
    w.entered(&in_c);
    // Unit 4 started beside unit 2 on the synced checkout.
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), to);
    release(&go_c);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3, 4], "done"), "{v}");
    assert!(!err.contains("draining"), "{err}");
    assert!(events_named(&e, "skills_drain").is_empty());
}

