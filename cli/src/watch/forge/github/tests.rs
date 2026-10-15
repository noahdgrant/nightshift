use super::*;

#[test]
fn a_closed_issue_is_not_planned_only_when_its_reason_says_other_than_completed() {
    let state = |v: Value| issue_state(&v);
    use IssueState::*;
    assert_eq!(state(serde_json::json!({"state": "OPEN"})), Open);
    assert_eq!(
        state(serde_json::json!({"state": "OPEN", "stateReason": "REOPENED"})),
        Open
    );
    for reason in [Value::Null, "".into(), "COMPLETED".into()] {
        let v = serde_json::json!({"state": "CLOSED", "stateReason": reason});
        assert_eq!(state(v), Completed);
    }
    assert_eq!(state(serde_json::json!({"state": "CLOSED"})), Completed);
    for reason in ["NOT_PLANNED", "DUPLICATE", "not_planned", "duplicate"] {
        let v = serde_json::json!({"state": "CLOSED", "stateReason": reason});
        assert_eq!(state(v), NotPlanned, "{reason}");
    }
    let v = serde_json::json!({"state": "closed", "stateReason": "completed"});
    assert_eq!(state(v), Completed);
    assert_eq!(state(serde_json::json!({"state": "open"})), Open);
}

#[test]
fn parse_closing_keeps_every_pr_that_closes_an_issue() {
    let prs = serde_json::json!([
        {"number": 12, "body": "Closes #5", "headRefName": "other", "isCrossRepository": false},
        {"number": 10, "body": "Closes #5. Fixes #6", "headRefName": "ns/5-x", "isCrossRepository": false},
        {"number": 10, "body": "Closes #5", "headRefName": "ns/5-x", "isCrossRepository": false},
        {"number": 9, "body": "Fixes #6", "headRefName": "fork", "isCrossRepository": true},
        {"number": 8, "body": "no reference", "headRefName": "x", "isCrossRepository": false},
    ]);
    let by_issue = parse_closing(&prs);
    assert_eq!(
        by_issue,
        ClosingPrs::from([
            (5, vec![(12, "other".into()), (10, "ns/5-x".into())]),
            (6, vec![(10, "ns/5-x".into()), (9, String::new())]),
        ])
    );
    assert!(parse_closing(&serde_json::json!({})).is_empty());
}

#[test]
fn parse_merged_reads_each_prs_head_merge_time_and_changed_lines() {
    let prs = serde_json::json!([
        {"number": 41, "headRefName": "ns/9-b", "isCrossRepository": false,
         "mergedAt": "2026-10-05T10:00:00Z", "additions": 30, "deletions": 12},
        {"number": 42, "headRefName": "ns/9-b", "isCrossRepository": true,
         "mergedAt": "2026-10-06T00:00:00Z", "additions": 1, "deletions": 0},
        {"number": 43, "headRefName": "x", "isCrossRepository": false,
         "mergedAt": "2026-10-06T00:00:00Z"},
        {"number": 44, "headRefName": "x", "isCrossRepository": false, "mergedAt": null},
        {"number": 45, "headRefName": "ns/9-b", "mergedAt": "2026-10-07T00:00:00Z"},
    ]);
    let m = parse_merged(&prs);
    let at = |s| crate::clock::parse_iso(s).unwrap();
    assert_eq!(
        m.prs,
        [
            MergedPr {
                number: 41,
                head: "ns/9-b".into(),
                merged: at("2026-10-05T10:00:00Z"),
                changed: Some(42)
            },
            MergedPr {
                number: 42,
                head: String::new(),
                merged: at("2026-10-06T00:00:00Z"),
                changed: Some(1)
            },
            MergedPr {
                number: 43,
                head: "x".into(),
                merged: at("2026-10-06T00:00:00Z"),
                changed: None
            },
            MergedPr {
                number: 45,
                head: String::new(),
                merged: at("2026-10-07T00:00:00Z"),
                changed: None
            },
        ]
    );
    assert!(!m.truncated);
    assert_eq!(PR_LIMIT, 200);
    let full: Vec<Value> = (1..=PR_LIMIT as u64)
        .map(|n| serde_json::json!({"number": n, "mergedAt": null}))
        .collect();
    assert!(parse_merged(&Value::from(full.clone())).truncated);
    assert!(!parse_merged(&Value::from(full[1..].to_vec())).truncated);
    assert!(parse_merged(&serde_json::json!({})).prs.is_empty());
}

#[test]
fn parse_pr_maps_each_state_and_reads_its_head_and_body() {
    let v = serde_json::json!({"state": "MERGED", "headRefOid": "abc",
                               "headRefName": "ns/7-x", "body": "Closes #7"});
    assert_eq!(
        parse_pr(&v),
        Pr {
            status: PrStatus::Merged,
            head_sha: "abc".into(),
            head_branch: "ns/7-x".into(),
            body: "Closes #7".into()
        }
    );
    let status = |s: &str| parse_pr(&serde_json::json!({"state": s})).status;
    assert_eq!(status("CLOSED"), PrStatus::Closed);
    assert_eq!(status("OPEN"), PrStatus::Open);
    let none = parse_pr(&serde_json::json!({"headRefOid": null}));
    assert_eq!((none.status, none.head_sha.as_str()), (PrStatus::Open, ""));
    assert_eq!(PrStatus::Closed.name(), "CLOSED");
}

#[test]
fn parse_commit_prs_reads_each_prs_head_and_whether_it_merged() {
    let pulls = serde_json::json!([
        {"number": 3, "merged_at": "2026-10-10T01:00:00Z", "head": {"ref": "ns/3-b"}},
        {"number": 4, "merged_at": null, "head": {"ref": "x"}},
        {"number": 5, "merged_at": ""},
        {"merged_at": "2026-10-10T01:00:00Z", "head": {"ref": "no-number"}},
    ]);
    let pr = |number, head: &str, merged| CommitPr {
        number,
        head: head.into(),
        merged,
    };
    assert_eq!(
        parse_commit_prs(&pulls),
        [pr(3, "ns/3-b", true), pr(4, "x", false), pr(5, "", false)]
    );
    assert!(parse_commit_prs(&serde_json::json!({})).is_empty());
}

#[test]
fn parse_filed_reads_when_each_issue_opened_and_how_it_closed() {
    let text = [
        r#"{"number": 7, "title": "fix: a", "labels": [{"name": "type:fix"}], "body": "b", "authorAssociation": "OWNER", "createdAt": "2026-10-05T10:00:00Z", "state": "closed", "stateReason": "duplicate"}"#,
        r#"{"number": 8, "title": "fix: b", "labels": [], "body": "", "createdAt": "2026-10-06T00:00:00Z", "state": "open", "stateReason": null}"#,
        r#"{"number": 9, "title": "no time", "createdAt": null, "state": "open"}"#,
        r#"{"number": 0, "title": "no number", "createdAt": "2026-10-06T00:00:00Z", "state": "open"}"#,
        "not json",
    ]
    .join("\n");
    let f = parse_filed(&text);
    assert_eq!(f.len(), 2, "{f:?}");
    assert_eq!(f[0].issue.number, 7);
    assert_eq!(f[0].issue.labels, ["type:fix"]);
    assert!(f[0].issue.team);
    assert_eq!(
        f[0].opened,
        crate::clock::parse_iso("2026-10-05T10:00:00Z").unwrap()
    );
    assert_eq!(f[0].state, IssueState::NotPlanned);
    assert_eq!((f[1].issue.number, f[1].state), (8, IssueState::Open));
}

#[test]
fn created_number_reads_the_url_gh_prints() {
    assert_eq!(
        created_number("https://github.com/o/r/issues/123\n").unwrap(),
        123
    );
    assert!(created_number("").is_err());
    assert!(created_number("https://github.com/o/r/issues/new").is_err());
}

#[test]
fn parse_view_reads_the_fields_a_read_asked_for_and_none_it_did_not() {
    let v = serde_json::json!({"state": "OPEN", "headRefOid": "abc", "mergeStateStatus": "BEHIND"});
    let want = PrView {
        state: Some("OPEN".into()),
        head: Some("abc".into()),
        merge_state: Some("BEHIND".into()),
    };
    assert_eq!(parse_view(&v), want);
    let head_only = serde_json::json!({"headRefOid": null});
    assert_eq!(parse_view(&head_only), PrView::default());
}

#[test]
fn parse_outcome_reads_the_state_merge_commit_and_review_count() {
    let v = serde_json::json!({"state": "MERGED", "mergeCommit": {"oid": "abc1234"}, "reviews": [{}, {}]});
    let o = parse_outcome(&v);
    assert_eq!(o.status, PrStatus::Merged);
    assert_eq!(o.merge_commit.as_deref(), Some("abc1234"));
    assert_eq!(o.reviews, 2);
    let open = serde_json::json!({"state": "OPEN", "mergeCommit": null, "reviews": []});
    let o = parse_outcome(&open);
    assert_eq!(
        (o.status, o.merge_commit, o.reviews),
        (PrStatus::Open, None, 0)
    );
    let closed = parse_outcome(&serde_json::json!({"state": "CLOSED"}));
    assert_eq!(closed.status, PrStatus::Closed);
}

#[test]
fn parse_pr_checks_reads_each_check_and_none_from_anything_but_a_list() {
    let v = serde_json::json!([
        {"name": "ci", "state": "FAILURE", "bucket": "fail", "link": "https://x/runs/1/job/2"},
        {"name": "lint"}
    ]);
    let checks = parse_pr_checks(&v);
    assert_eq!(
        checks[0],
        PrCheck {
            name: Some("ci".into()),
            state: Some("FAILURE".into()),
            bucket: Some("fail".into()),
            link: Some("https://x/runs/1/job/2".into()),
        }
    );
    assert_eq!(
        checks[1],
        PrCheck {
            name: Some("lint".into()),
            ..PrCheck::default()
        }
    );
    assert!(parse_pr_checks(&serde_json::json!({})).is_empty());
}

#[test]
fn parse_count_reads_a_number_and_names_the_query_otherwise() {
    assert_eq!(parse_count("3\n", "status").unwrap(), 3);
    let e = parse_count("null\n", "check-runs").unwrap_err();
    assert_eq!(
        e.to_string(),
        "unexpected output from the check-runs query: \"null\""
    );
}

#[test]
fn a_pr_head_is_fetched_from_its_pull_ref() {
    let github = GitHub {
        root: std::env::temp_dir(),
    };
    assert_eq!(github.pr_ref(5), "refs/pull/5/head");
}

const MERGE_CALLS_MARK: &str = "NS_TEST_GH_MERGE_CALLS_PROBE";

/// The merge step's calls run the gh commands the step ran before they moved onto `Forge`.
#[test]
fn the_merge_step_runs_the_same_gh_commands_through_the_trait() {
    crate::testutil::probe(
        "watch::forge::github::tests::merge_calls_probe",
        MERGE_CALLS_MARK,
        "1",
    );
}

#[test]
fn merge_calls_probe() {
    if std::env::var_os(MERGE_CALLS_MARK).is_none() {
        return;
    }
    let fake = crate::testutil::fake_on_path(
        "gh",
        "echo \"$*\" >> \"$(dirname \"$0\")/calls\"\necho '{}'",
    );
    let github = GitHub {
        root: fake.path().to_path_buf(),
    };
    for read in [
        PrRead::State,
        PrRead::Head,
        PrRead::StateAndHead,
        PrRead::MergeState,
        PrRead::HeadAndMergeState,
    ] {
        github.view_pr(12, read).unwrap();
    }
    github.update_branch(12).unwrap();
    let sha = "a".repeat(40);
    for count in github.check_counts(&sha) {
        assert!(count.is_err(), "{{}} is no count");
    }
    let limit = crate::proc::Kind::Forge.timeout();
    assert!(!github.watch_checks(12, limit).unwrap());
    assert!(github.pr_checks(12).unwrap().is_empty());
    github.run_log("99").unwrap();
    assert_eq!(github.changed_files(12).unwrap(), ["{}"]);
    github.squash_merge(12, "abc").unwrap();
    github.delete_branch("ns/12-x").unwrap();
    let calls = std::fs::read_to_string(fake.path().join("calls")).unwrap();
    let commits = format!("repos/{{owner}}/{{repo}}/commits/{sha}");
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        [
            "pr view 12 --json state",
            "pr view 12 --json headRefOid",
            "pr view 12 --json state,headRefOid",
            "pr view 12 --json mergeStateStatus",
            "pr view 12 --json headRefOid,mergeStateStatus",
            "pr update-branch 12",
            &format!("api {commits}/check-runs --jq .total_count"),
            &format!("api {commits}/status --jq .total_count"),
            "pr checks 12 --watch",
            "pr checks 12 --json name,state,bucket,link",
            "run view 99 --log-failed",
            "pr diff 12 --name-only",
            "pr merge 12 --squash --match-head-commit abc",
            "api repos/{owner}/{repo}/git/refs/heads/ns/12-x -X DELETE",
        ]
    );
}
