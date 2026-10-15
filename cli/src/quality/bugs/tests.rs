use super::*;
use crate::watch::Issue;

#[test]
fn ns_review_files_escapes_with_the_text_marker_and_checklist_read_here() {
    let skill = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills/ns-review/SKILL.md"),
    )
    .unwrap();
    assert!(skill.contains(ESCAPED), "{ESCAPED}");
    let marker = format!("`{MARKER} review_escape`");
    assert!(skill.contains(&marker), "{marker}");
    let item = "   - [ ] `<file:line>`";
    assert!(skill.contains(item), "{item}");
    let flat: String = skill
        .lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    let raise = "Raise that issue's priority label to the highest among its existing and new items";
    assert!(flat.contains(raise), "{raise}");
    assert!(flat.contains("never lower it"), "never lower it");
    let reuse = "that already carries the `Source: review_escape` line";
    assert!(flat.contains(reuse), "{reuse}");
    let dup = "skipping any finding whose location and defect an existing item already names";
    assert!(flat.contains(dup), "{dup}");
    let legacy = "When the issue's body has no checklist items, first turn its original finding into an item (location, severity and evidence from its body), then append the new ones";
    assert!(flat.contains(legacy), "{legacy}");
}

#[test]
fn ns_ship_tags_an_unrelated_risk_with_the_incidental_source_read_here() {
    let skill = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills/ns-ship/SKILL.md"),
    )
    .unwrap();
    let flat = skill.split_whitespace().collect::<Vec<_>>().join(" ");
    let tag = format!("`{MARKER} incidental`");
    let rule = format!("A `file:` line tagged `unrelated:` adds a last line {tag}");
    assert!(flat.contains(&rule), "{rule}");
    let body = format!("🤖 *Posted by nightshift, an AI agent.*\n\nrisk\n\n{MARKER} incidental");
    assert_eq!(marked(&body), Some(Source::Incidental));
}

pub(super) fn t(s: &str) -> i64 {
    clock::parse_iso(s).unwrap()
}

pub(super) fn filed(n: u64, opened: &str, title: &str, body: &str, labels: &[&str]) -> Filed {
    Filed {
        issue: Issue {
            number: n,
            title: title.into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            body: body.into(),
            team: true,
        },
        opened: t(opened),
        state: IssueState::Open,
    }
}

fn blamed(pr: Option<u64>) -> Blame {
    Blame {
        commit: "abc".into(),
        summary: "feat: x".into(),
        pr,
    }
}

pub(super) fn pr(number: u64, head: &str, merged: &str, changed: Option<u64>) -> MergedPr {
    MergedPr {
        number,
        head: head.into(),
        merged: t(merged),
        changed,
    }
}

pub(super) fn merged(prs: &[MergedPr], truncated: bool) -> MergedPrs {
    MergedPrs {
        prs: prs.to_vec(),
        truncated,
    }
}

pub(super) fn no_trace(_: &Filed) -> Result<Blame, String> {
    panic!("traced an issue whose source was already known")
}

#[test]
fn iso_weeks_cross_years_as_iso_8601_says() {
    for (day, week) in [
        ("2026-10-05T00:00:00Z", "2026-W41"),
        ("2026-10-11T23:59:59Z", "2026-W41"),
        ("2026-10-12T00:00:00Z", "2026-W42"),
        ("2021-01-03T12:00:00Z", "2020-W53"),
        ("2024-12-30T00:00:00Z", "2025-W01"),
        ("2026-01-01T00:00:00Z", "2026-W01"),
        ("1970-01-01T00:00:00Z", "1970-W01"),
    ] {
        assert_eq!(iso_week(t(day)), week, "{day}");
    }
    assert_eq!(monday(t("2026-10-11T23:59:59Z")), t("2026-10-05"));
    assert_eq!(monday(t("2026-10-05T00:00:00Z")), t("2026-10-05"));
}

#[test]
fn a_marker_line_names_the_source_ahead_of_any_guess() {
    let f = filed(
        1,
        "2026-10-05",
        "fix: main is red at abc: test",
        "This escaped an earlier review.\n  Source: `factory_introduced`\nSource: human",
        &[],
    );
    let b = classify(&f, &no_trace, &BTreeSet::new());
    assert_eq!((b.source, b.by), (Source::FactoryIntroduced, By::Marker));
    for (line, s) in [
        ("Source: review_escape", Source::ReviewEscape),
        ("Source: ci_main", Source::CiMain),
        ("Source:human", Source::Human),
        ("Source: incidental", Source::Incidental),
    ] {
        let f = filed(1, "2026-10-05", "fix: x", line, &[]);
        assert_eq!(
            classify(&f, &no_trace, &BTreeSet::new()).source,
            s,
            "{line}"
        );
    }
}

#[test]
fn an_unknown_marker_is_no_marker() {
    let f = filed(1, "2026-10-05", "fix: x", "Source: robots\nx", &[]);
    let b = classify(&f, &|_| Err("no path".into()), &BTreeSet::new());
    assert_eq!((b.source, b.by), (Source::Human, By::Default));
}

#[test]
fn a_marker_line_after_an_unknown_one_still_names_the_source() {
    let f = filed(
        1,
        "2026-10-05",
        "fix: x",
        "Source: robots\nSource: ci_main",
        &[],
    );
    let b = classify(&f, &no_trace, &BTreeSet::new());
    assert_eq!((b.source, b.by), (Source::CiMain, By::Marker));
}

#[test]
fn a_review_escape_is_read_from_the_sentence_review_files_it_with() {
    let f = filed(
        2,
        "2026-10-05",
        "fix(cli): off by one",
        "`a.rs:3` reads past the end.\n\nThis escaped an earlier review.",
        &[],
    );
    let b = classify(&f, &no_trace, &BTreeSet::new());
    assert_eq!((b.source, b.by), (Source::ReviewEscape, By::ReviewText));
}

#[test]
fn a_red_main_is_read_from_the_title_watch_files_it_with() {
    let f = filed(
        3,
        "2026-10-05",
        "fix: main is red at abcdef0: test",
        "",
        &[],
    );
    let b = classify(&f, &no_trace, &BTreeSet::new());
    assert_eq!((b.source, b.by), (Source::CiMain, By::CiTitle));
}

#[test]
fn blame_on_a_factory_pr_is_factory_introduced_and_on_any_other_is_human() {
    let factory = BTreeSet::from([41]);
    let f = filed(4, "2026-10-05", "fix: crash", "`a.rs:2` panics", &[]);
    let b = classify(&f, &|_| Ok(blamed(Some(41))), &factory);
    assert_eq!((b.source, b.by), (Source::FactoryIntroduced, By::Blame));
    assert_eq!(b.introduced_by, Some(blamed(Some(41))));
    let b = classify(&f, &|_| Ok(blamed(Some(40))), &factory);
    assert_eq!((b.source, b.by), (Source::Human, By::Default));
    assert_eq!(b.introduced_by, Some(blamed(Some(40))));
    let b = classify(&f, &|_| Ok(blamed(None)), &factory);
    assert_eq!(b.source, Source::Human);
    let b = classify(&f, &|_| Err("location has no path:line".into()), &factory);
    assert_eq!((b.source, b.by), (Source::Human, By::Default));
    assert_eq!(b.blame_error.as_deref(), Some("location has no path:line"));
    assert_eq!(b.introduced_by, None);
}

#[test]
fn severity_is_critical_high_only_for_those_priorities() {
    let sev = |labels: &[&str]| {
        classify(
            &filed(5, "2026-10-05", "fix: x", "Source: human", labels),
            &no_trace,
            &BTreeSet::new(),
        )
        .severity
    };
    assert_eq!(sev(&["type:fix", "priority:critical"]), Band::CriticalHigh);
    assert_eq!(sev(&["priority:high"]), Band::CriticalHigh);
    assert_eq!(sev(&["priority:medium"]), Band::Rest);
    assert_eq!(sev(&["priority:low"]), Band::Rest);
    assert_eq!(sev(&["priority:highest"]), Band::Rest);
    assert_eq!(sev(&[]), Band::Rest);
}

#[test]
fn a_review_escape_counts_one_bug_per_checklist_item_and_others_count_one() {
    let grouped = format!(
        "{ESCAPED}\n\n- [ ] `a.rs:1` x\n  - [x] `a.rs:2` y\n- [X] `a.rs:3` z\n\n```\n- [ ] fenced\n```\n> - [ ] quoted\n-  [ ] loose\n- [] bare"
    );
    let item_less = "Source: review_escape\n`a.rs:1` x";
    let human = "Source: human\n- [ ] a\n- [ ] b";
    let bugs = |body: &str| {
        classify(
            &filed(1, "2026-10-05", "fix: x", body, &[]),
            &no_trace,
            &BTreeSet::new(),
        )
        .bugs
    };
    assert_eq!(bugs(&grouped), 3);
    assert_eq!(bugs(item_less), 1);
    assert_eq!(bugs(human), 1);
    for other in ["ci_main", "factory_introduced"] {
        assert_eq!(
            bugs(&format!("Source: {other}\n- [ ] a\n- [ ] b")),
            1,
            "{other}"
        );
    }
    let r = report(
        &[
            filed(1, "2026-10-05", "fix: a", &grouped, &["priority:high"]),
            filed(2, "2026-10-06", "fix: b", item_less, &[]),
            filed(3, "2026-10-12", "fix: c", human, &[]),
        ],
        Window::default(),
        t("2026-10-14"),
        &no_trace,
        &BTreeSet::new(),
        &MergedPrs::default(),
    );
    assert_eq!(r.weeks[0].counts, counts([4, 4, 0, 0, 0, 0, 3, 1]));
    assert_eq!(r.weeks[1].counts, counts([1, 0, 0, 0, 1, 0, 0, 1]));
    assert_eq!(r.total, counts([5, 4, 0, 0, 1, 0, 3, 2]));
}

#[test]
fn an_incidental_issue_counts_apart_from_escapes_and_human_bugs() {
    let r = report(
        &[filed(1, "2026-10-05", "fix: a", "Source: incidental", &[])],
        Window::default(),
        t("2026-10-07"),
        &no_trace,
        &BTreeSet::new(),
        &MergedPrs::default(),
    );
    assert_eq!(r.total, counts([1, 0, 0, 0, 0, 1, 0, 1]));
    assert!(
        r.text().contains(
            "\ntotal          1      0       0       0     0          1         0    1\n"
        ),
        "{}",
        r.text()
    );
}

fn fixtures() -> Vec<Filed> {
    let mut dup = filed(6, "2026-10-06", "fix: same as #2", "Source: human", &[]);
    dup.state = IssueState::NotPlanned;
    let mut wontfix = filed(5, "2026-10-13", "fix: never", "Source: human", &[]);
    wontfix.state = IssueState::NotPlanned;
    let mut done = filed(
        7,
        "2026-10-13T08:00:00Z",
        "fix: z",
        "Source: human",
        &["priority:low"],
    );
    done.state = IssueState::Completed;
    vec![
        filed(
            2,
            "2026-10-05T09:00:00Z",
            "fix: a",
            ESCAPED,
            &["priority:high"],
        ),
        filed(
            3,
            "2026-10-11T23:59:59Z",
            "fix: main is red at abcdef0: test",
            "",
            &["priority:critical"],
        ),
        filed(4, "2026-10-12T00:00:00Z", "fix: b", "`a.rs:2` panics", &[]),
        dup,
        done,
        filed(1, "2026-09-21T00:00:00Z", "fix: c", "Source: human", &[]),
        wontfix,
    ]
}

fn counts(c: [usize; 8]) -> Counts {
    let [opened, review_escape, factory_introduced, ci_main, human, incidental, critical_high, rest] =
        c;
    Counts {
        opened,
        review_escape,
        factory_introduced,
        ci_main,
        human,
        incidental,
        critical_high,
        rest,
    }
}

#[test]
fn issues_count_in_the_week_they_opened_and_not_planned_ones_are_left_out() {
    let now = t("2026-10-14T12:00:00Z");
    let r = report(
        &fixtures(),
        Window::default(),
        now,
        &|_| Ok(blamed(Some(41))),
        &BTreeSet::from([41]),
        &merged(&[pr(41, "ns/9-b", "2026-09-01", Some(10))], false),
    );
    let weeks: Vec<(&str, &str)> = r
        .weeks
        .iter()
        .map(|w| (w.week.as_str(), w.monday.as_str()))
        .collect();
    assert_eq!(
        weeks,
        [
            ("2026-W39", "2026-09-21"),
            ("2026-W40", "2026-09-28"),
            ("2026-W41", "2026-10-05"),
            ("2026-W42", "2026-10-12"),
        ]
    );
    assert_eq!(r.weeks[0].counts, counts([1, 0, 0, 0, 1, 0, 0, 1]));
    assert_eq!(r.weeks[1].counts, Counts::default());
    assert_eq!(r.weeks[2].counts, counts([2, 1, 0, 1, 0, 0, 2, 0]));
    assert_eq!(r.weeks[3].counts, counts([2, 0, 1, 0, 1, 0, 0, 2]));
    assert_eq!(r.total, counts([5, 1, 1, 1, 2, 0, 2, 3]));
    let numbers: Vec<u64> = r.issues.iter().map(|b| b.number).collect();
    assert_eq!(numbers, [1, 2, 3, 4, 7]);
    let left: Vec<(u64, &str)> = r
        .excluded
        .iter()
        .map(|e| (e.number, e.week.as_str()))
        .collect();
    assert_eq!(left, [(5, "2026-W42"), (6, "2026-W41")]);
    assert_eq!((r.since, r.until), (None, None));
}

#[test]
fn the_window_keeps_issues_and_weeks_inside_it() {
    let w = Window {
        since: Some(t("2026-10-06")),
        until: Some(t("2026-10-12T23:59:59Z")),
    };
    let r = report(
        &fixtures(),
        w,
        t("2026-12-01"),
        &|_| Err("x".into()),
        &BTreeSet::new(),
        &MergedPrs::default(),
    );
    let weeks: Vec<&str> = r.weeks.iter().map(|w| w.week.as_str()).collect();
    assert_eq!(weeks, ["2026-W41", "2026-W42"]);
    let numbers: Vec<u64> = r.issues.iter().map(|b| b.number).collect();
    assert_eq!(numbers, [3, 4]);
    let left: Vec<u64> = r.excluded.iter().map(|e| e.number).collect();
    assert_eq!(left, [6]);
    assert_eq!(r.since.as_deref(), Some("2026-10-06T00:00:00Z"));
    assert_eq!(r.until.as_deref(), Some("2026-10-12T23:59:59Z"));
}

#[test]
fn the_week_that_starts_at_the_end_of_the_range_is_kept() {
    let w = Window {
        since: Some(t("2026-10-05")),
        until: None,
    };
    let r = report(
        &[],
        w,
        t("2026-10-12"),
        &no_trace,
        &BTreeSet::new(),
        &MergedPrs::default(),
    );
    let weeks: Vec<&str> = r.weeks.iter().map(|w| w.week.as_str()).collect();
    assert_eq!(weeks, ["2026-W41", "2026-W42"]);
}

fn undercounted_weeks(prs: &[MergedPr], truncated: bool) -> Vec<bool> {
    let w = Window {
        since: Some(t("2026-10-05")),
        until: None,
    };
    report(
        &[],
        w,
        t("2026-10-20"),
        &no_trace,
        &BTreeSet::new(),
        &merged(prs, truncated),
    )
    .weeks
    .iter()
    .map(|w| w.rates.undercounted)
    .collect()
}

#[test]
fn a_week_starting_exactly_at_the_oldest_listed_merge_is_not_undercounted() {
    let prs = [pr(1, "ns/1", "2026-10-12T00:00:00Z", Some(1))];
    assert_eq!(undercounted_weeks(&prs, true), [true, false, false]);
    let prs = [pr(1, "ns/1", "2026-10-12T00:00:01Z", Some(1))];
    assert_eq!(undercounted_weeks(&prs, true), [true, true, false]);
}

#[test]
fn a_full_list_with_no_usable_pr_leaves_every_week_undercounted() {
    assert_eq!(undercounted_weeks(&[], true), [true, true, true]);
    assert_eq!(undercounted_weeks(&[], false), [false, false, false]);
}

#[test]
fn no_issues_and_no_since_is_no_weeks() {
    let r = report(
        &[],
        Window::default(),
        0,
        &no_trace,
        &BTreeSet::new(),
        &MergedPrs::default(),
    );
    assert!(r.weeks.is_empty());
    assert_eq!(r.total, Counts::default());
    assert!(r.text().contains("total"));
}

#[test]
fn weeks_starts_at_monday_n_minus_1_weeks_back_unless_since_is_later() {
    let now = t("2026-10-14T12:00:00Z");
    let w = window(None, None, Some(1), now).unwrap();
    assert_eq!(w.since, Some(t("2026-10-12")));
    let w = window(None, None, Some(3), now).unwrap();
    assert_eq!(w.since, Some(t("2026-09-28")));
    let w = window(Some("2026-10-01".into()), None, Some(3), now).unwrap();
    assert_eq!(w.since, Some(t("2026-10-01")));
    let w = window(Some("2026-09-01".into()), None, Some(3), now).unwrap();
    assert_eq!(w.since, Some(t("2026-09-28")));
    let w = window(Some("2026-09-01".into()), None, None, now).unwrap();
    assert_eq!(w.since, Some(t("2026-09-01")));
    let e = window(None, None, Some(0), now).unwrap_err();
    assert!(e.to_string().contains("--weeks must be 1 or more"), "{e}");
    let e = window(None, Some("2026-10-11".into()), Some(1), now).unwrap_err();
    assert_eq!(
        e.to_string(),
        "--until 2026-10-11T23:59:59Z is before --weeks 1 starts at 2026-10-12T00:00:00Z"
    );
    let w = window(None, Some("2026-10-12".into()), Some(1), now).unwrap();
    assert_eq!(w.until, Some(t("2026-10-12T23:59:59Z")));
    let w = window(None, Some("2026-10-12T00:00:00Z".into()), Some(1), now).unwrap();
    assert_eq!(w.since, w.until);
}

#[test]
fn text_has_a_row_per_week_a_total_and_the_left_out_issues() {
    let r = report(
        &fixtures(),
        Window::default(),
        t("2026-10-14"),
        &|_| Err("x".into()),
        &BTreeSet::new(),
        &MergedPrs::default(),
    );
    let text = r.text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "type:fix issues opened per ISO week (UTC), by source and severity"
    );
    assert_eq!(
        lines[2],
        "week      opened escape factory ci-main human incidental crit/high rest"
    );
    assert_eq!(
        lines[5],
        "2026-W41       2      1       0       1     0          0         2    0"
    );
    assert_eq!(
        lines[7],
        "total          5      1       0       1     3          0         2    3"
    );
    assert_eq!(
        lines[9],
        "Left out, closed as duplicate or not planned: #5, #6"
    );
}
