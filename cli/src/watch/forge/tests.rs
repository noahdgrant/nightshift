use super::*;

#[test]
fn a_post_opens_with_the_disclosure_then_a_blank_line_then_the_text() {
    let post = Post::new("Fixed in #3.\n");
    let mut lines = post.as_str().lines();
    assert_eq!(
        lines.next(),
        Some("🤖 *Posted by nightshift, an AI agent.*")
    );
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), Some("Fixed in #3."));
    assert_eq!(lines.next(), None);
}

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
fn parse_issue_link_reads_the_title_and_the_named_url() {
    let v = serde_json::json!({"title": "Fix it", "url": "u1", "web_url": "u2"});
    let link = |title: &str, url: &str| IssueLink {
        title: title.into(),
        url: url.into(),
    };
    assert_eq!(parse_issue_link(&v, "url"), link("Fix it", "u1"));
    assert_eq!(parse_issue_link(&v, "web_url"), link("Fix it", "u2"));
    assert_eq!(
        parse_issue_link(&serde_json::json!({}), "url"),
        link("", "")
    );
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
fn source_is_inferred_from_the_origin_host() {
    let cases = [
        (Some("https://gitlab.com/o/r.git"), "gitlab"),
        (Some("git@gitlab.example:o/r.git"), "gitlab"),
        (Some("https://gitlab.corp.example/o/r"), "gitlab"),
        (Some("https://GitLab.com/o/r"), "gitlab"),
        (Some("https://github.com/o/r"), "github"),
        (Some("/srv/git/gitlab-mirror.git"), "github"),
        (None, "github"),
    ];
    for (origin, want) in cases {
        assert_eq!(inferred(origin, None).name(), want, "{origin:?}");
    }
    assert_eq!(
        check(Some("github"), Some("https://gitlab.com/o/r"), None)
            .unwrap()
            .name(),
        "github"
    );
}

#[test]
fn source_is_gitlab_on_the_configured_gitlab_host() {
    let origin = Some("git@Code.example.com:o/r.git");
    for h in ["code.example.com", "https://CODE.example.com"] {
        assert_eq!(inferred(origin, Some(h)).name(), "gitlab", "{h}");
    }
    assert_eq!(inferred(origin, Some("other.example")).name(), "github");
    assert_eq!(
        check(Some("github"), origin, Some("code.example.com"))
            .unwrap()
            .name(),
        "github"
    );
}

#[test]
fn check_takes_github_and_gitlab_and_refuses_unknown_with_the_hint() {
    assert_eq!(
        check(None, Some("https://github.com/o/r"), None).unwrap(),
        ForgeKind::Github
    );
    assert_eq!(
        check(Some("github"), None, None).unwrap(),
        ForgeKind::Github
    );
    assert_eq!(
        check(Some("gitlab"), None, None).unwrap(),
        ForgeKind::Gitlab
    );
    assert_eq!(
        check(None, Some("https://gitlab.com/o/r"), None).unwrap(),
        ForgeKind::Gitlab
    );
    for (s, o) in [
        (Some("bitbucket"), None),
        (Some("svn"), Some("https://github.com/o/r")),
    ] {
        let e = check(s, o, None).unwrap_err().to_string();
        assert!(e.contains("[queue] source"), "{e}");
    }
}

#[test]
fn last_green_skips_runs_on_the_red_head() {
    let runs = serde_json::json!([
        {"headSha": "aaaaaaa"}, {"headSha": "aaaaaaa"}, {"headSha": "bbbbbbb"},
        {"headSha": "ccccccc"}
    ]);
    assert_eq!(
        first_other(&runs, "headSha", "aaaaaaa").as_deref(),
        Some("bbbbbbb")
    );
    let only = serde_json::json!([{"headSha": "aaaaaaa"}]);
    assert_eq!(first_other(&only, "headSha", "aaaaaaa"), None);
    assert_eq!(first_other(&serde_json::json!([]), "headSha", "red"), None);
}

#[test]
fn commit_prs_refuses_what_is_not_a_commit_id_before_calling_the_forge() {
    let root = std::env::temp_dir();
    let github = GitHub { root: root.clone() };
    let gitlab = gitlab::GitLab { root };
    for sha in ["--output=x", "abc1234", &"a".repeat(41)] {
        for forge in [&github as &dyn Forge, &gitlab] {
            let e = forge.commit_prs(sha).unwrap_err();
            assert!(format!("{e:#}").contains("not a commit id"), "{sha}: {e:#}");
        }
    }
}

#[test]
fn last_green_skips_what_is_not_a_commit_id() {
    let runs = serde_json::json!([
        {"headSha": "--output=x"}, {"headSha": "abc"}, {"headSha": "zzzzzzzz"},
        {"headSha": "é".repeat(8)}, {"headSha": "a".repeat(65)}, {"headSha": "AbC1234"}
    ]);
    assert_eq!(
        first_other(&runs, "headSha", "red").as_deref(),
        Some("AbC1234")
    );
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
fn path_reads_url_and_scp_forms_and_no_path_from_a_local_path() {
    assert_eq!(path("https://gitlab.com/o/r.git"), Some("o/r.git"));
    assert_eq!(path("ssh://git@gitlab.example:2222/o/r"), Some("o/r"));
    assert_eq!(path("git@github.com:o/r.git"), Some("o/r.git"));
    assert_eq!(path("https://gitlab.com"), None);
    assert_eq!(path("/srv/git/gitlab:mirror.git"), None);
}

#[test]
fn host_reads_url_and_scp_forms_and_no_host_from_a_path() {
    assert_eq!(host("https://gitlab.com/o/r.git"), Some("gitlab.com"));
    assert_eq!(
        host("ssh://git@gitlab.example:2222/o/r"),
        Some("gitlab.example")
    );
    assert_eq!(host("https://user:pw@github.com/o/r"), Some("github.com"));
    assert_eq!(host("git@github.com:o/r.git"), Some("github.com"));
    assert_eq!(host("/srv/git/gitlab:mirror.git"), None);
    assert_eq!(host("../gitlab-mirror"), None);
}
