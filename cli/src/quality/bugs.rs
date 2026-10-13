//! `ns quality --bugs`: the `type:fix` issues opened in each ISO week, by source and severity
//! (docs/factory/quality.md).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde::Serialize;

use super::blame::{self, Blame};
use super::Window;
use crate::clock::{self, Clock};
use crate::error::SfError;
use crate::factory;
use crate::git::{self, Repo};
use crate::watch::forge::{self, Filed, IssueState, PrState};
use crate::watch::FIX_MAIN_TITLE;
use crate::worktree::BRANCH_PREFIX;

/// The label every issue counted carries.
pub const LABEL: &str = "type:fix";

/// The body line naming an issue's source, as `Source: review_escape`.
const MARKER: &str = "Source:";

/// The sentence `ns-review` ends every escape it files with.
const ESCAPED: &str = "This escaped an earlier review.";

const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    ReviewEscape,
    FactoryIntroduced,
    CiMain,
    Human,
}

impl Source {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "review_escape" => Self::ReviewEscape,
            "factory_introduced" => Self::FactoryIntroduced,
            "ci_main" => Self::CiMain,
            "human" => Self::Human,
            _ => return None,
        })
    }
}

/// What decided an issue's source.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// A `Source:` line in the body.
    Marker,
    /// The sentence `ns-review` files escapes with.
    ReviewText,
    /// The title `ns watch` files a red `main` with.
    CiTitle,
    /// Blame of the first `path:line` in the body.
    Blame,
    /// Nothing else did.
    Default,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    /// `priority:critical` or `priority:high`.
    CriticalHigh,
    /// Any other priority, or none.
    Rest,
}

#[derive(Serialize, Debug)]
pub struct Bug {
    pub number: u64,
    pub title: String,
    pub opened: String,
    pub week: String,
    pub source: Source,
    pub by: By,
    pub severity: Band,
    /// The bugs it counts as: one per checklist item for a `review_escape`, at least one.
    pub bugs: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub introduced_by: Option<Blame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blame_error: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct Excluded {
    pub number: u64,
    pub title: String,
    pub week: String,
}

#[derive(Serialize, Default, Debug, PartialEq, Eq)]
pub struct Counts {
    pub opened: usize,
    pub review_escape: usize,
    pub factory_introduced: usize,
    pub ci_main: usize,
    pub human: usize,
    pub critical_high: usize,
    pub rest: usize,
}

impl Counts {
    fn add(&mut self, b: &Bug) {
        self.opened += b.bugs;
        *match b.source {
            Source::ReviewEscape => &mut self.review_escape,
            Source::FactoryIntroduced => &mut self.factory_introduced,
            Source::CiMain => &mut self.ci_main,
            Source::Human => &mut self.human,
        } += b.bugs;
        *match b.severity {
            Band::CriticalHigh => &mut self.critical_high,
            Band::Rest => &mut self.rest,
        } += b.bugs;
    }
}

#[derive(Serialize, Debug)]
pub struct Week {
    pub week: String,
    pub monday: String,
    #[serde(flatten)]
    pub counts: Counts,
}

#[derive(Serialize, Debug)]
pub struct Report {
    pub ok: bool,
    pub label: &'static str,
    pub since: Option<String>,
    pub until: Option<String>,
    pub weeks: Vec<Week>,
    pub total: Counts,
    pub issues: Vec<Bug>,
    pub excluded: Vec<Excluded>,
}

/// `2026-W41`: the ISO week holding unix time `t`, in UTC.
pub fn iso_week(t: i64) -> String {
    let day = t.div_euclid(DAY);
    let thursday = day - weekday(day) + 3;
    let year = &clock::iso(thursday * DAY)[..4];
    let jan1 = clock::parse_iso(&format!("{year}-01-01")).unwrap_or_default() / DAY;
    format!("{year}-W{:02}", (thursday - jan1) / 7 + 1)
}

/// Days since Monday of day number `day` (0 is 1970-01-01, a Thursday).
fn weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

/// 00:00 UTC on the Monday of the week holding `t`.
pub fn monday(t: i64) -> i64 {
    let day = t.div_euclid(DAY);
    (day - weekday(day)) * DAY
}

/// The source an issue names on a `Source:` line, the first such line that names one.
fn marked(body: &str) -> Option<Source> {
    body.lines()
        .filter_map(|l| l.trim().strip_prefix(MARKER))
        .find_map(|v| Source::parse(v.trim().trim_matches('`')))
}

/// The `- [ ]` and `- [x]` lines of `body` outside code fences: one per finding `ns-review` groups
/// into an escape issue.
fn checklist_items(body: &str) -> usize {
    let mut fenced = false;
    body.lines()
        .map(str::trim_start)
        .filter(|l| {
            if l.starts_with("```") {
                fenced = !fenced;
            }
            !fenced && ["- [ ]", "- [x]", "- [X]"].iter().any(|b| l.starts_with(b))
        })
        .count()
}

fn band(labels: &[String]) -> Band {
    if labels
        .iter()
        .any(|l| l == "priority:critical" || l == "priority:high")
    {
        Band::CriticalHigh
    } else {
        Band::Rest
    }
}

/// `f`'s source: its marker, else the text a phase files it with, else the PR `trace` blames its
/// first `path:line` on when that is a factory PR, else `human`. `trace` runs only when needed.
pub fn classify(
    f: &Filed,
    trace: &dyn Fn(&Filed) -> Result<Blame, String>,
    factory_prs: &BTreeSet<u64>,
) -> Bug {
    let i = &f.issue;
    let mut bug = Bug {
        number: i.number,
        title: i.title.clone(),
        opened: clock::iso(f.opened),
        week: iso_week(f.opened),
        source: Source::Human,
        by: By::Default,
        severity: band(&i.labels),
        bugs: 1,
        introduced_by: None,
        blame_error: None,
    };
    let guess = if let Some(s) = marked(&i.body) {
        Some((s, By::Marker))
    } else if i.body.contains(ESCAPED) {
        Some((Source::ReviewEscape, By::ReviewText))
    } else if i.title.starts_with(FIX_MAIN_TITLE) {
        Some((Source::CiMain, By::CiTitle))
    } else {
        None
    };
    if let Some((s, by)) = guess {
        (bug.source, bug.by) = (s, by);
        if s == Source::ReviewEscape {
            bug.bugs = checklist_items(&i.body).max(1);
        }
        return bug;
    }
    match trace(f) {
        Ok(b) => {
            if b.pr.is_some_and(|pr| factory_prs.contains(&pr)) {
                (bug.source, bug.by) = (Source::FactoryIntroduced, By::Blame);
            }
            bug.introduced_by = Some(b);
        }
        Err(e) => bug.blame_error = Some(e),
    }
    bug
}

/// The report on `filed`, kept to `window`. Weeks run from the window's start, else the oldest
/// issue's week, through the week of its end, else of `now`; a week with no issue counts zero.
pub fn report(
    filed: &[Filed],
    window: Window,
    now: i64,
    trace: &dyn Fn(&Filed) -> Result<Blame, String>,
    factory_prs: &BTreeSet<u64>,
) -> Report {
    let kept: Vec<&Filed> = filed
        .iter()
        .filter(|f| window.contains(Some(f.opened)))
        .collect();
    let (gone, counted): (Vec<&Filed>, Vec<&Filed>) = kept
        .into_iter()
        .partition(|f| f.state == IssueState::NotPlanned);
    let mut issues: Vec<Bug> = counted
        .iter()
        .map(|f| classify(f, trace, factory_prs))
        .collect();
    issues.sort_by_key(|b| b.number);
    let mut excluded: Vec<Excluded> = gone
        .iter()
        .map(|f| Excluded {
            number: f.issue.number,
            title: f.issue.title.clone(),
            week: iso_week(f.opened),
        })
        .collect();
    excluded.sort_by_key(|e| e.number);
    let first = window
        .since
        .or_else(|| counted.iter().map(|f| f.opened).min());
    let last = window.until.unwrap_or(now);
    let mut by_week: BTreeMap<&str, Vec<&Bug>> = BTreeMap::new();
    for b in &issues {
        by_week.entry(b.week.as_str()).or_default().push(b);
    }
    let mut weeks = Vec::new();
    let mut total = Counts::default();
    if let Some(first) = first {
        let mut m = monday(first);
        while m <= last {
            let week = iso_week(m);
            let mut counts = Counts::default();
            for b in by_week.remove(week.as_str()).unwrap_or_default() {
                counts.add(b);
                total.add(b);
            }
            weeks.push(Week {
                week,
                monday: clock::iso(m)[..10].to_string(),
                counts,
            });
            m += WEEK;
        }
    }
    Report {
        ok: true,
        label: LABEL,
        since: window.since.map(clock::iso),
        until: window.until.map(clock::iso),
        weeks,
        total,
        issues,
        excluded,
    }
}

impl Report {
    pub fn text(&self) -> String {
        let mut s = format!(
            "{} issues opened per ISO week (UTC), by source and severity\n\n",
            self.label
        );
        let _ = writeln!(
            s,
            "{:<9} {:>6} {:>6} {:>7} {:>7} {:>5} {:>9} {:>4}",
            "week", "opened", "escape", "factory", "ci-main", "human", "crit/high", "rest"
        );
        let row = |s: &mut String, name: &str, c: &Counts| {
            let _ = writeln!(
                s,
                "{name:<9} {:>6} {:>6} {:>7} {:>7} {:>5} {:>9} {:>4}",
                c.opened,
                c.review_escape,
                c.factory_introduced,
                c.ci_main,
                c.human,
                c.critical_high,
                c.rest
            );
        };
        for w in &self.weeks {
            row(&mut s, &w.week, &w.counts);
        }
        row(&mut s, "total", &self.total);
        if !self.excluded.is_empty() {
            let n: Vec<String> = self
                .excluded
                .iter()
                .map(|e| format!("#{}", e.number))
                .collect();
            let _ = writeln!(
                s,
                "\nLeft out, closed as duplicate or not planned: {}",
                n.join(", ")
            );
        }
        s
    }
}

/// The window `--since`, `--until` and `--weeks` give: `--weeks N` starts at Monday of the
/// week N-1 weeks before `now`'s, or at `--since` when that is later.
pub fn window(
    since: Option<String>,
    until: Option<String>,
    weeks: Option<u32>,
    now: i64,
) -> Result<Window> {
    let mut w = super::window(since, until)?;
    if let Some(n) = weeks {
        if n == 0 {
            return Err(
                SfError::usage("--weeks must be 1 or more", "ns quality --bugs --weeks 4").into(),
            );
        }
        let start = monday(now) - i64::from(n - 1) * WEEK;
        if let Some(u) = w.until.filter(|u| *u < start) {
            return Err(SfError::usage(
                format!(
                    "--until {} is before --weeks {n} starts at {}",
                    clock::iso(u),
                    clock::iso(start)
                ),
                "ns quality --bugs --weeks 4",
            )
            .into());
        }
        w.since = Some(w.since.map_or(start, |s| s.max(start)));
    }
    Ok(w)
}

/// The commit on `base` at time `opened`, and the blame of the first `path:line` in `body` there.
fn trace(repo: &Repo, base: Option<&str>, f: &Filed) -> Result<Blame, String> {
    let base = base.ok_or("the repo has no default branch")?;
    let before = format!("--before={}", clock::iso(f.opened));
    let at = git::run(&repo.root, &["rev-list", "-1", &before, base, "--"])
        .map_err(|e| e.to_string())?;
    if at.is_empty() {
        return Err(format!("no commit on {base} before the issue opened"));
    }
    blame::blame(&repo.root, Some(&at), Some(&f.issue.body))
}

pub fn cli(
    since: Option<String>,
    until: Option<String>,
    weeks: Option<u32>,
    as_json: bool,
) -> Result<ExitCode> {
    let now = Clock::from_env().now();
    let window = window(since, until, weeks, now)?;
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let fac = factory::load(&factory::root(None, &repo.root))?;
    let forge = forge::open(&fac.queue, &repo.root)?;
    let filed = forge.labelled_issues(LABEL)?;
    let factory_prs: BTreeSet<u64> = forge
        .closing_prs(PrState::Merged)?
        .into_values()
        .filter(|(_, head)| head.starts_with(BRANCH_PREFIX))
        .map(|(pr, _)| pr)
        .collect();
    let base = repo.default_base().ok();
    let r = report(
        &filed,
        window,
        now,
        &|f| trace(&repo, base.as_deref(), f),
        &factory_prs,
    );
    if as_json {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print!("{}", r.text());
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
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
    }

    fn t(s: &str) -> i64 {
        clock::parse_iso(s).unwrap()
    }

    fn filed(n: u64, opened: &str, title: &str, body: &str, labels: &[&str]) -> Filed {
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

    fn no_trace(_: &Filed) -> Result<Blame, String> {
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
        );
        assert_eq!(r.weeks[0].counts, counts([4, 4, 0, 0, 0, 3, 1]));
        assert_eq!(r.weeks[1].counts, counts([1, 0, 0, 0, 1, 0, 1]));
        assert_eq!(r.total, counts([5, 4, 0, 0, 1, 3, 2]));
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

    fn counts(c: [usize; 7]) -> Counts {
        let [opened, review_escape, factory_introduced, ci_main, human, critical_high, rest] = c;
        Counts {
            opened,
            review_escape,
            factory_introduced,
            ci_main,
            human,
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
        assert_eq!(r.weeks[0].counts, counts([1, 0, 0, 0, 1, 0, 1]));
        assert_eq!(r.weeks[1].counts, Counts::default());
        assert_eq!(r.weeks[2].counts, counts([2, 1, 0, 1, 0, 2, 0]));
        assert_eq!(r.weeks[3].counts, counts([2, 0, 1, 0, 1, 0, 2]));
        assert_eq!(r.total, counts([5, 1, 1, 1, 2, 2, 3]));
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
        let r = report(&[], w, t("2026-10-12"), &no_trace, &BTreeSet::new());
        let weeks: Vec<&str> = r.weeks.iter().map(|w| w.week.as_str()).collect();
        assert_eq!(weeks, ["2026-W41", "2026-W42"]);
    }

    #[test]
    fn no_issues_and_no_since_is_no_weeks() {
        let r = report(&[], Window::default(), 0, &no_trace, &BTreeSet::new());
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
        );
        let text = r.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "type:fix issues opened per ISO week (UTC), by source and severity"
        );
        assert_eq!(
            lines[2],
            "week      opened escape factory ci-main human crit/high rest"
        );
        assert_eq!(
            lines[5],
            "2026-W41       2      1       0       1     0         2    0"
        );
        assert_eq!(
            lines[7],
            "total          5      1       0       1     3         2    3"
        );
        assert_eq!(
            lines[9],
            "Left out, closed as duplicate or not planned: #5, #6"
        );
    }
}
