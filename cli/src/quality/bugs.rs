//! `ns quality --bugs`: the `type:fix` issues opened in each ISO week, by source and severity,
//! their rate per merged unit and per 1,000 changed lines, and each rate's trend
//! (docs/factory/quality.md).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde::Serialize;

use self::rates::{Listed, Rates};
use super::blame::{self, Blame};
use super::Window;
use crate::clock::{self, Clock};
use crate::error::SfError;
use crate::factory;
use crate::git::{self, Repo};
use crate::watch::forge::{self, Filed, IssueState, MergedPr, MergedPrs, PrState};
use crate::watch::FIX_MAIN_TITLE;
use crate::worktree::BRANCH_PREFIX;

/// The label every issue counted carries.
pub const LABEL: &str = "type:fix";

/// The body line naming an issue's source, as `Source: review_escape`.
const MARKER: &str = "Source:";

/// The sentence `ns-review` ends every escape it files with.
const ESCAPED: &str = "This escaped an earlier review.";

mod rates;

const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    ReviewEscape,
    FactoryIntroduced,
    CiMain,
    Human,
    /// Pre-existing and unrelated to the unit that found it: `ns-ship` files a build `file:` line.
    Incidental,
}

impl Source {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "review_escape" => Self::ReviewEscape,
            "factory_introduced" => Self::FactoryIntroduced,
            "ci_main" => Self::CiMain,
            "human" => Self::Human,
            "incidental" => Self::Incidental,
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
    pub incidental: usize,
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
            Source::Incidental => &mut self.incidental,
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
    #[serde(flatten)]
    pub rates: Rates,
}

#[derive(Serialize, Debug)]
pub struct Report {
    pub ok: bool,
    pub label: &'static str,
    pub since: Option<String>,
    pub until: Option<String>,
    pub weeks: Vec<Week>,
    pub total: Counts,
    pub trend: rates::Trends,
    pub merged_prs: Listed,
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
/// The PRs in `merged` from `ns/` branches merged inside `window` are the factory's merged units.
pub fn report(
    filed: &[Filed],
    window: Window,
    now: i64,
    trace: &dyn Fn(&Filed) -> Result<Blame, String>,
    factory_prs: &BTreeSet<u64>,
    merged: &MergedPrs,
) -> Report {
    let units: Vec<&MergedPr> = merged
        .prs
        .iter()
        .filter(|p| p.head.starts_with(BRANCH_PREFIX) && window.contains(Some(p.merged)))
        .collect();
    let counted = counted_issues(filed, window);
    let gone: Vec<&Filed> = in_window(filed, window)
        .filter(|f| f.state == IssueState::NotPlanned)
        .collect();
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
    let last = window.until.unwrap_or(now);
    let mut by_week: BTreeMap<&str, Vec<&Bug>> = BTreeMap::new();
    for b in &issues {
        by_week.entry(b.week.as_str()).or_default().push(b);
    }
    let mut units_by_week: BTreeMap<String, Vec<&MergedPr>> = BTreeMap::new();
    for p in units {
        units_by_week.entry(iso_week(p.merged)).or_default().push(p);
    }
    let oldest = merged.prs.iter().map(|p| p.merged).min();
    let mut weeks = Vec::new();
    let mut total = Counts::default();
    if let Some(mut m) = first_week(filed, window) {
        while m <= last {
            let week = iso_week(m);
            let mut counts = Counts::default();
            for b in by_week.remove(week.as_str()).unwrap_or_default() {
                counts.add(b);
                total.add(b);
            }
            let undercounted = merged.truncated && oldest.is_none_or(|o| m < o);
            let units = units_by_week.remove(&week).unwrap_or_default();
            let rates = Rates::new(counts.opened, &units, undercounted);
            weeks.push(Week {
                week,
                monday: clock::iso(m)[..10].to_string(),
                counts,
                rates,
            });
            m += WEEK;
        }
    }
    let trend = rates::trends(&weeks);
    Report {
        ok: true,
        label: LABEL,
        since: window.since.map(clock::iso),
        until: window.until.map(clock::iso),
        weeks,
        total,
        trend,
        merged_prs: Listed {
            listed: merged.prs.len(),
            truncated: merged.truncated,
            oldest: oldest.map(clock::iso),
        },
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
            "{:<9} {:>6} {:>6} {:>7} {:>7} {:>5} {:>10} {:>9} {:>4}",
            "week",
            "opened",
            "escape",
            "factory",
            "ci-main",
            "human",
            "incidental",
            "crit/high",
            "rest"
        );
        let row = |s: &mut String, name: &str, c: &Counts| {
            let _ = writeln!(
                s,
                "{name:<9} {:>6} {:>6} {:>7} {:>7} {:>5} {:>10} {:>9} {:>4}",
                c.opened,
                c.review_escape,
                c.factory_introduced,
                c.ci_main,
                c.human,
                c.incidental,
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
        self.rates_text(&mut s);
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

fn in_window(filed: &[Filed], window: Window) -> impl Iterator<Item = &Filed> {
    filed
        .iter()
        .filter(move |f| window.contains(Some(f.opened)))
}

/// The issues inside `window` the report counts: those not closed as not planned.
fn counted_issues(filed: &[Filed], window: Window) -> Vec<&Filed> {
    in_window(filed, window)
        .filter(|f| f.state != IssueState::NotPlanned)
        .collect()
}

/// When the first week of the report starts: its window's start, else the oldest issue's week.
fn first_week(filed: &[Filed], window: Window) -> Option<i64> {
    window
        .since
        .or_else(|| {
            counted_issues(filed, window)
                .into_iter()
                .map(|f| f.opened)
                .min()
        })
        .map(monday)
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
        .flatten()
        .filter(|(_, head)| head.starts_with(BRANCH_PREFIX))
        .map(|(pr, _)| pr)
        .collect();
    let merged = forge.merged_prs(first_week(&filed, window))?;
    let base = repo.default_base().ok();
    let r = report(
        &filed,
        window,
        now,
        &|f| trace(&repo, base.as_deref(), f),
        &factory_prs,
        &merged,
    );
    if as_json {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print!("{}", r.text());
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests;
