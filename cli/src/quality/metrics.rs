//! The `ns quality` numbers, computed from parsed review artifacts. No IO here.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::review_md::{CycleEntry, Finding, Scope, Severity, Status, AXES};
use crate::stats::{median, round};

/// Fix cycles `ns-review` runs before it stops (skills/ns-review/SKILL.md).
pub const CYCLE_LIMIT: u32 = 3;

/// One review artifact: `review.md`, or an archived `history/review*.md`.
#[derive(Debug, Clone, Default)]
pub struct Attempt {
    /// Relative to the unit's worktree, e.g. `.ns/<unit>/review.md`.
    pub path: String,
    pub status: Option<String>,
    pub updated: Option<i64>,
    pub cycles: Option<u32>,
    /// The commit escapes are blamed at: the reviewed `sha:`, else `head:`, else the sha in
    /// `base: <branch>@<sha>`. A location's line number matches the reviewed code.
    pub blame_at: Option<String>,
    pub changed_lines: Option<u64>,
    pub changed_lines_source: Option<&'static str>,
    pub findings: Vec<Finding>,
    /// Headings with a `Status:` field that didn't parse as findings.
    pub stray_statuses: usize,
    /// The Critical and Important findings the first pass's `cycle-0.md` or `cycle-1.md` lists.
    pub first_pass_file: Vec<CycleEntry>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RunStats {
    pub review_runs: u32,
    pub review_cost_usd: f64,
    pub outcome: Option<String>,
    pub pr: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Unit {
    pub id: String,
    pub worktree: String,
    /// Oldest first; never empty.
    pub attempts: Vec<Attempt>,
    /// Local date of the newest attempt's `updated:`.
    pub day: Option<String>,
    pub run: RunStats,
}

impl Unit {
    pub fn first(&self) -> &Attempt {
        &self.attempts[0]
    }

    pub fn last(&self) -> &Attempt {
        &self.attempts[self.attempts.len() - 1]
    }
}

/// Whether `f` was raised in the attempt's first review pass. `None` when nothing says.
pub fn in_first_pass(a: &Attempt, f: &Finding) -> Option<bool> {
    if let Some(c) = f.cycle {
        return Some(c == 0);
    }
    (a.cycles == Some(0) || f.fixed_cycle == Some(1)).then_some(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Clean,
    Dirty,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FirstPass {
    pub verdict: Verdict,
    /// Findings against the unit raised in the first pass; `None` when that is unknown.
    pub blocking: Option<usize>,
    pub by_axis: BTreeMap<String, usize>,
}

/// The first pass is dirty when it raised a Critical or Important in changed code that was not
/// dismissed. When the findings don't say which pass raised them, the first pass's cycle file
/// is its record: old artifacts reuse ids across passes, so ids are not matched against it.
/// Failing that, a fix cycle runs only for such a finding, so `cycles >= 1` is dirty.
pub fn first_pass(a: &Attempt) -> FirstPass {
    let blocking: Vec<&Finding> = a.findings.iter().filter(|f| f.against_unit()).collect();
    let unknown = blocking.iter().any(|f| in_first_pass(a, f).is_none());
    if a.findings.is_empty() && a.status.as_deref() != Some("pass") {
        return FirstPass {
            verdict: Verdict::Unknown,
            blocking: None,
            by_axis: BTreeMap::new(),
        };
    }
    if unknown && !a.first_pass_file.is_empty() {
        return from_cycle_file(&a.first_pass_file);
    }
    let first: Vec<&Finding> = blocking
        .iter()
        .copied()
        .filter(|f| in_first_pass(a, f) == Some(true))
        .collect();
    let verdict = if !first.is_empty() || (unknown && a.cycles.is_some_and(|c| c >= 1)) {
        Verdict::Dirty
    } else if unknown {
        Verdict::Unknown
    } else {
        Verdict::Clean
    };
    let mut by_axis = BTreeMap::new();
    if !unknown {
        for f in &first {
            for axis in axes_or_unknown(f) {
                *by_axis.entry(axis).or_insert(0) += 1;
            }
        }
    }
    FirstPass {
        verdict,
        blocking: (!unknown).then_some(first.len()),
        by_axis,
    }
}

fn from_cycle_file(entries: &[CycleEntry]) -> FirstPass {
    let mut by_axis = BTreeMap::new();
    for e in entries {
        for axis in or_unknown(&e.axes) {
            *by_axis.entry(axis).or_insert(0) += 1;
        }
    }
    FirstPass {
        verdict: Verdict::Dirty,
        blocking: Some(entries.len()),
        by_axis,
    }
}

fn axes_or_unknown(f: &Finding) -> Vec<String> {
    or_unknown(&f.axes)
}

fn or_unknown(axes: &[String]) -> Vec<String> {
    if axes.is_empty() {
        vec!["unknown".to_string()]
    } else {
        axes.to_vec()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cycles {
    /// Ended `pass` with no open blocking finding, after this many fix cycles.
    Clean(u32),
    NotClean,
    /// Ended clean, but `cycles:` is missing, or claims fix cycles that fixed nothing (older
    /// artifacts counted review passes there).
    Unknown,
}

impl Cycles {
    pub fn label(self) -> &'static str {
        match self {
            Cycles::Clean(_) => "clean",
            Cycles::NotClean => "not_clean",
            Cycles::Unknown => "unknown",
        }
    }
}

pub fn cycles_to_clean(a: &Attempt) -> Cycles {
    let open = a
        .findings
        .iter()
        .any(|f| f.against_unit() && f.status == Status::Open);
    if a.status.as_deref() != Some("pass") || open {
        return Cycles::NotClean;
    }
    let fixed = a
        .findings
        .iter()
        .any(|f| f.against_unit() && f.status == Status::Fixed);
    match a.cycles {
        Some(c) if c == 0 || fixed => Cycles::Clean(c),
        _ => Cycles::Unknown,
    }
}

/// Critical or Important findings still open when the review stopped at the cycle limit.
pub fn leftovers(a: &Attempt) -> Vec<&Finding> {
    if a.cycles.is_none_or(|c| c < CYCLE_LIMIT) {
        return Vec::new();
    }
    a.findings
        .iter()
        .filter(|f| f.against_unit() && f.status == Status::Open)
        .collect()
}

/// Findings about code the unit didn't change, unless dismissed.
pub fn escapes(a: &Attempt) -> Vec<&Finding> {
    a.findings
        .iter()
        .filter(|f| f.scope == Scope::PreExisting && f.status != Status::Dismissed)
        .collect()
}

fn per_100(findings: usize, lines: u64) -> Option<f64> {
    (lines > 0).then(|| round(findings as f64 * 100.0 / lines as f64, 2))
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct FindingCounts {
    pub total: usize,
    pub critical: usize,
    pub important: usize,
    pub suggestion: usize,
    pub unknown_severity: usize,
    pub by_status: BTreeMap<&'static str, usize>,
    pub by_axis: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct AxisRate {
    pub findings: usize,
    pub per_100_lines: Option<f64>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct FirstPassSummary {
    pub clean: usize,
    pub dirty: usize,
    pub unknown: usize,
    /// clean / (clean + dirty).
    #[serde(rename = "yield")]
    pub yield_: Option<f64>,
    /// Units with a known first-pass count and a known change size.
    pub measured_units: usize,
    pub blocking_findings: usize,
    pub changed_lines: u64,
    pub per_100_lines: Option<f64>,
    pub by_axis: BTreeMap<String, AxisRate>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct CyclesSummary {
    pub units: usize,
    pub median: Option<f64>,
    pub max: Option<u32>,
    pub not_clean: usize,
    pub unknown: usize,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Summary {
    pub units: usize,
    pub findings: FindingCounts,
    pub first_pass: FirstPassSummary,
    pub cycles_to_clean: CyclesSummary,
    pub leftovers: usize,
    pub escapes: usize,
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Open => "open",
        Status::Fixed => "fixed",
        Status::Dismissed => "dismissed",
        Status::Deferred => "deferred",
        Status::Unknown => "unknown",
    }
}

fn count_findings<'a>(fs: impl Iterator<Item = &'a Finding>) -> FindingCounts {
    let mut c = FindingCounts::default();
    for f in fs {
        c.total += 1;
        match f.severity {
            Severity::Critical => c.critical += 1,
            Severity::Important => c.important += 1,
            Severity::Suggestion => c.suggestion += 1,
            Severity::Unknown => c.unknown_severity += 1,
        }
        *c.by_status.entry(status_name(f.status)).or_insert(0) += 1;
        for axis in axes_or_unknown(f) {
            *c.by_axis.entry(axis).or_insert(0) += 1;
        }
    }
    c
}

/// First-pass numbers come from each unit's oldest attempt, everything else from its newest.
pub fn summarize(units: &[&Unit]) -> Summary {
    let mut fp = FirstPassSummary {
        clean: 0,
        dirty: 0,
        unknown: 0,
        yield_: None,
        measured_units: 0,
        blocking_findings: 0,
        changed_lines: 0,
        per_100_lines: None,
        by_axis: BTreeMap::new(),
    };
    let mut axis_counts: BTreeMap<String, usize> =
        AXES.iter().map(|a| (a.to_string(), 0)).collect();
    let mut cycles = Vec::new();
    let (mut not_clean, mut cycles_unknown) = (0, 0);
    for u in units {
        let first = first_pass(u.first());
        match first.verdict {
            Verdict::Clean => fp.clean += 1,
            Verdict::Dirty => fp.dirty += 1,
            Verdict::Unknown => fp.unknown += 1,
        }
        if let (Some(n), Some(lines)) = (first.blocking, u.first().changed_lines) {
            fp.measured_units += 1;
            fp.blocking_findings += n;
            fp.changed_lines += lines;
            for (axis, k) in first.by_axis {
                *axis_counts.entry(axis).or_insert(0) += k;
            }
        }
        match cycles_to_clean(u.last()) {
            Cycles::Clean(c) => cycles.push(c),
            Cycles::NotClean => not_clean += 1,
            Cycles::Unknown => cycles_unknown += 1,
        }
    }
    let judged = fp.clean + fp.dirty;
    fp.yield_ = (judged > 0).then(|| round(fp.clean as f64 / judged as f64, 3));
    fp.per_100_lines = per_100(fp.blocking_findings, fp.changed_lines);
    fp.by_axis = axis_counts
        .into_iter()
        .map(|(axis, n)| {
            let rate = AxisRate {
                findings: n,
                per_100_lines: per_100(n, fp.changed_lines),
            };
            (axis, rate)
        })
        .collect();
    Summary {
        units: units.len(),
        findings: count_findings(units.iter().flat_map(|u| u.last().findings.iter())),
        first_pass: fp,
        cycles_to_clean: CyclesSummary {
            units: cycles.len(),
            max: cycles.iter().copied().max(),
            median: median(cycles.iter().map(|c| f64::from(*c)).collect()),
            not_clean,
            unknown: cycles_unknown,
        },
        leftovers: units.iter().map(|u| leftovers(u.last()).len()).sum(),
        escapes: units.iter().map(|u| escapes(u.last()).len()).sum(),
    }
}

/// One summary per local day, oldest first; undated units last, under `null`.
pub fn trend<'a>(units: &[&'a Unit]) -> Vec<(Option<String>, Summary)> {
    let mut days: BTreeMap<(bool, Option<String>), Vec<&'a Unit>> = BTreeMap::new();
    for u in units {
        days.entry((u.day.is_none(), u.day.clone()))
            .or_default()
            .push(u);
    }
    days.into_iter()
        .map(|((_, day), us)| (day, summarize(&us)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_md::parse;

    fn attempt(cycles: Option<u32>, status: &str, review: &str) -> Attempt {
        Attempt {
            path: ".ns/u/review.md".into(),
            status: Some(status.into()),
            cycles,
            findings: parse(review),
            ..Attempt::default()
        }
    }

    fn unit(id: &str, day: Option<&str>, attempts: Vec<Attempt>) -> Unit {
        Unit {
            id: id.into(),
            worktree: format!("/w/{id}"),
            attempts,
            day: day.map(String::from),
            run: RunStats::default(),
        }
    }

    const NEW: &str = "\
## Critical
### C1. a
- Axis: security
- Scope: changed
- Cycle: 0
- Status: fixed (cycle 1, abc)
## Important
### I1. b
- Axis: correctness, tests
- Scope: pre-existing
- Location: `a.rs:3`
- Cycle: 0
- Status: deferred: #9
### I2. c
- Axis: tests
- Cycle: 1
- Status: open
### I3. d
- Axis: spec
- Cycle: 0
- Status: dismissed: wrong
## Suggestion
### S1. e
- Axis: readability
- Scope: pre-existing
- Cycle: 0
- Status: open
";

    #[test]
    fn first_pass_counts_undismissed_changed_blocking_findings_from_pass_zero() {
        let a = attempt(Some(3), "blocked", NEW);
        let fp = first_pass(&a);
        assert_eq!(fp.verdict, Verdict::Dirty);
        assert_eq!(fp.blocking, Some(1));
        assert_eq!(fp.by_axis, BTreeMap::from([("security".to_string(), 1)]));
        let escape_only = attempt(
            Some(0),
            "pass",
            "### I1. a\n- Scope: pre-existing\n- Status: deferred: #4\n",
        );
        assert_eq!(first_pass(&escape_only).verdict, Verdict::Clean);
    }

    #[test]
    fn a_first_pass_with_only_later_or_dismissed_findings_is_clean() {
        let a = attempt(Some(1), "pass", "### I1. x\n- Cycle: 1\n- Status: fixed\n### I2. y\n- Cycle: 0\n- Status: dismissed: no\n### S1. z\n- Cycle: 0\n");
        let fp = first_pass(&a);
        assert_eq!((fp.verdict, fp.blocking), (Verdict::Clean, Some(0)));
        let none = attempt(None, "pass", "");
        assert_eq!(first_pass(&none).verdict, Verdict::Clean);
        let ended_early = attempt(Some(0), "blocked", "");
        let fp = first_pass(&ended_early);
        assert_eq!((fp.verdict, fp.blocking), (Verdict::Unknown, None));
    }

    #[test]
    fn old_artifacts_infer_the_first_pass() {
        let old = "### I1. a\n- Raised by: tests\n- Status: fixed (cycle 1, x)\n### I2. b\n- Raised by: spec\n- Status: fixed (cycle 2, y)\n";
        let mut a = attempt(Some(2), "pass", old);
        assert_eq!(in_first_pass(&a, &a.findings[0]), Some(true));
        assert_eq!(in_first_pass(&a, &a.findings[1]), None);
        let fp = first_pass(&a);
        assert_eq!((fp.verdict, fp.blocking), (Verdict::Dirty, None));
        assert!(fp.by_axis.is_empty());

        // The first pass's cycle file is its record; ids aren't matched, since old artifacts
        // reuse them across passes.
        a.first_pass_file =
            crate::review_md::cycle_entries("- I1 perf: x\n- I7 arch/spec: y\n- I8 z\n");
        let fp = first_pass(&a);
        assert_eq!((fp.verdict, fp.blocking), (Verdict::Dirty, Some(3)));
        assert_eq!(
            fp.by_axis,
            BTreeMap::from([
                ("architecture".to_string(), 1),
                ("performance".to_string(), 1),
                ("spec".to_string(), 1),
                ("unknown".to_string(), 1)
            ])
        );
        let placed = attempt(Some(2), "pass", "### I1. a\n- Cycle: 0\n- Status: fixed\n");
        let with_file = Attempt {
            first_pass_file: a.first_pass_file.clone(),
            ..placed
        };
        assert_eq!(first_pass(&with_file).blocking, Some(1));

        let b = attempt(Some(1), "pass", "### I1. a\n- Status: open\n");
        assert_eq!(first_pass(&b).verdict, Verdict::Dirty);
        let c = attempt(None, "pass", "### I1. a\n- Status: open\n");
        assert_eq!(first_pass(&c).verdict, Verdict::Unknown);
        let d = attempt(Some(0), "blocked", "### I1. a\n- Status: open\n");
        assert_eq!(first_pass(&d).blocking, Some(1));
    }

    #[test]
    fn cycles_to_clean_needs_a_pass_with_nothing_blocking_open() {
        assert_eq!(
            cycles_to_clean(&attempt(Some(2), "pass", "### I1. a\n- Status: fixed\n")),
            Cycles::Clean(2)
        );
        assert_eq!(
            cycles_to_clean(&attempt(Some(0), "pass", "### S1. a\n- Status: open\n")),
            Cycles::Clean(0)
        );
        assert_eq!(
            cycles_to_clean(&attempt(Some(1), "pass", "### S1. a\n- Status: fixed\n")),
            Cycles::Unknown
        );
        assert_eq!(
            cycles_to_clean(&attempt(
                Some(0),
                "pass",
                "### I1. a\n- Scope: pre-existing\n- Status: open\n"
            )),
            Cycles::Clean(0)
        );
        assert_eq!(
            cycles_to_clean(&attempt(
                Some(1),
                "pass",
                "### I1. a\n- Scope: pre-existing\n- Status: fixed\n"
            )),
            Cycles::Unknown
        );
        assert_eq!(Cycles::Clean(1).label(), "clean");
        assert_eq!(Cycles::NotClean.label(), "not_clean");
        assert_eq!(Cycles::Unknown.label(), "unknown");
        assert_eq!(cycles_to_clean(&attempt(None, "pass", "")), Cycles::Unknown);
        assert_eq!(
            cycles_to_clean(&attempt(Some(3), "blocked", "")),
            Cycles::NotClean
        );
        assert_eq!(
            cycles_to_clean(&attempt(Some(1), "pass", "### I1. a\n- Status: open\n")),
            Cycles::NotClean
        );
        assert_eq!(
            cycles_to_clean(&attempt(
                Some(1),
                "pass",
                "### I1. a\n- Status: deferred: #3\n"
            )),
            Cycles::Unknown
        );
        let mut no_status = attempt(Some(0), "pass", "");
        no_status.status = None;
        assert_eq!(cycles_to_clean(&no_status), Cycles::NotClean);
    }

    #[test]
    fn leftovers_are_open_blocking_findings_at_the_cycle_limit() {
        let review = "### C1. a\n- Status: open\n### I1. b\n- Status: fixed\n### I2. c\n- Status: open\n### S1. d\n- Status: open\n### I3. e\n- Status: dismissed: x\n### C4. f\n- Scope: pre-existing\n- Status: open\n";
        let ids = |a: &Attempt| {
            leftovers(a)
                .iter()
                .map(|f| f.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&attempt(Some(3), "blocked", review)), ["C1", "I2"]);
        assert_eq!(ids(&attempt(Some(4), "blocked", review)), ["C1", "I2"]);
        assert!(ids(&attempt(Some(2), "fail", review)).is_empty());
        assert!(ids(&attempt(None, "fail", review)).is_empty());
    }

    #[test]
    fn escapes_are_undismissed_pre_existing_findings() {
        let review = "### I1. a\n- Scope: pre-existing\n### S1. b\n- Scope: pre-existing\n- Status: open\n### I2. c\n- Scope: pre-existing\n- Status: dismissed: x\n### I3. d\n- Scope: changed\n### I4. e\n";
        let ids: Vec<_> = escapes(&attempt(Some(0), "pass", review))
            .iter()
            .map(|f| f.id.clone())
            .collect();
        assert_eq!(ids, ["I1", "S1"]);
    }

    /// Unit 114's review wrote its escape under its own section with an `E` id.
    const ESCAPE_SECTION: &str = "\
## Critical
### C1. Unresolved merge-conflict markers in docs/FACTORY.md
- Location: `docs/FACTORY.md:274`
- Axis: correctness, spec
- Scope: changed
- Cycle: 0
- Status: fixed (cycle 1, abc1234)

## Escapes
### E1. Non-Unix `clock::local_date` returns the UTC date
- Location: `cli/src/clock.rs:242`
- Axis: architecture
- Scope: pre-existing
- Cycle: 1
- Raised by: architecture (claude), raised as Important
- Finding: `iso(t)[..10]` contradicts its doc comment (\"in the zone `TZ` names\"). Used only by `ns quality`, unchanged by the diff.
- Evidence: the function is not in the diff; `git diff main...HEAD -- cli/src/clock.rs` never touches it.
- Status: deferred: #172

## Dismissed
- D1. Non-Unix `next_local` has no runtime test (tests, Important). dismissed: the brief verifies it with `cargo check`.
";

    #[test]
    fn an_escape_under_its_own_section_with_an_e_id_counts() {
        let a = attempt(Some(1), "pass", ESCAPE_SECTION);
        let got: Vec<_> = escapes(&a)
            .iter()
            .map(|f| (f.id.clone(), f.severity))
            .collect();
        assert_eq!(got, [("E1".to_string(), Severity::Important)]);
        let s = summarize(&[&unit("u", None, vec![a])]);
        assert_eq!(s.escapes, 1);
        assert_eq!((s.findings.total, s.findings.important), (2, 1));
        let unsure = attempt(Some(0), "pass", "### E2. no note\n- Scope: pre-existing\n");
        let s = summarize(&[&unit("u", None, vec![unsure])]);
        assert_eq!((s.escapes, s.findings.unknown_severity), (1, 1));
    }

    #[test]
    fn summarize_uses_the_oldest_attempt_for_the_first_pass_and_the_newest_for_the_rest() {
        let mut first = attempt(Some(0), "fail", "### I1. a\n- Axis: tests\n- Status: open\n### I2. b\n- Raised by: correctness, tests\n- Status: open\n");
        first.changed_lines = Some(200);
        let a = unit(
            "a",
            Some("2026-10-08"),
            vec![first, attempt(Some(3), "blocked", NEW)],
        );
        let mut clean = attempt(Some(0), "pass", "### S1. s\n- Status: open\n");
        clean.changed_lines = Some(100);
        let b = unit("b", Some("2026-10-09"), vec![clean]);
        let c = unit(
            "c",
            None,
            vec![attempt(None, "pass", "### I1. x\n- Status: fixed\n")],
        );
        let s = summarize(&[&a, &b, &c]);
        assert_eq!(s.units, 3);
        assert_eq!(s.findings.total, 7);
        assert_eq!(
            (
                s.findings.critical,
                s.findings.important,
                s.findings.suggestion
            ),
            (1, 4, 2)
        );
        assert_eq!(
            s.findings.by_status,
            BTreeMap::from([("deferred", 1), ("dismissed", 1), ("fixed", 2), ("open", 3)])
        );
        assert_eq!(s.findings.by_axis["tests"], 2);
        assert_eq!(s.findings.by_axis["unknown"], 2);
        let fp = &s.first_pass;
        assert_eq!((fp.clean, fp.dirty, fp.unknown), (1, 1, 1));
        assert_eq!(fp.yield_, Some(0.5));
        assert_eq!(
            (fp.measured_units, fp.blocking_findings, fp.changed_lines),
            (2, 2, 300)
        );
        assert_eq!(fp.per_100_lines, Some(0.67));
        assert_eq!(
            fp.by_axis["tests"],
            AxisRate {
                findings: 2,
                per_100_lines: Some(0.67)
            }
        );
        assert_eq!(
            fp.by_axis["correctness"],
            AxisRate {
                findings: 1,
                per_100_lines: Some(0.33)
            }
        );
        assert_eq!(
            fp.by_axis["security"],
            AxisRate {
                findings: 0,
                per_100_lines: Some(0.0)
            }
        );
        assert_eq!(fp.by_axis.len(), 8);
        assert_eq!(
            s.cycles_to_clean,
            CyclesSummary {
                units: 1,
                median: Some(0.0),
                max: Some(0),
                not_clean: 1,
                unknown: 1
            }
        );
        assert_eq!((s.leftovers, s.escapes), (1, 2));
    }

    #[test]
    fn summarize_with_nothing_measured() {
        let s = summarize(&[]);
        assert_eq!(s.units, 0);
        assert_eq!(s.first_pass.yield_, None);
        assert_eq!(s.first_pass.per_100_lines, None);
        assert_eq!(
            s.first_pass.by_axis["spec"],
            AxisRate {
                findings: 0,
                per_100_lines: None
            }
        );
        assert_eq!(s.cycles_to_clean.median, None);
        assert_eq!(s.cycles_to_clean.max, None);
    }

    #[test]
    fn a_unit_without_a_change_size_is_not_measured() {
        let u = unit(
            "a",
            None,
            vec![attempt(Some(0), "pass", "### I1. a\n- Status: open\n")],
        );
        let s = summarize(&[&u]);
        assert_eq!((s.first_pass.dirty, s.first_pass.measured_units), (1, 0));
        assert_eq!(s.first_pass.blocking_findings, 0);
    }

    #[test]
    fn trend_groups_by_day_with_undated_last() {
        let u = |id, day| unit(id, day, vec![attempt(Some(0), "pass", "")]);
        let (a, b, c, d) = (
            u("a", Some("2026-10-09")),
            u("b", None),
            u("c", Some("2026-10-08")),
            u("d", Some("2026-10-09")),
        );
        let t = trend(&[&a, &b, &c, &d]);
        let days: Vec<_> = t.iter().map(|(d, s)| (d.as_deref(), s.units)).collect();
        assert_eq!(
            days,
            [(Some("2026-10-08"), 1), (Some("2026-10-09"), 2), (None, 1)]
        );
    }
}
