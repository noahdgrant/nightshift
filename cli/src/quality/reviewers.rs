//! `ns quality --reviewers`: per reviewer, what it raised, what only it raised that got fixed,
//! how often the lead dismissed it, the escapes it missed, its overlap with the others and its
//! cost (docs/factory/quality.md, Reviewers). No IO here.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use serde::Serialize;

use super::metrics::Unit;
use crate::review_md::{Scope, Severity, Status, Worker, AXES};
use crate::stats::{median, round};

/// One reviewer's numbers for one severity. Raised findings are Critical or Important ones in
/// changed code, from every review attempt.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Counts {
    pub raised: usize,
    /// Raised by this reviewer alone.
    pub unique: usize,
    pub unique_fixed: usize,
    /// Raises the lead dismissed: the whole finding, or this reviewer's version of it.
    pub dismissed: usize,
    pub dismiss_rate: Option<f64>,
    /// Escapes whose `missed_by` names this reviewer.
    pub escapes: usize,
    /// `raised` and `unique_fixed` from attempts with a pass that has a panel, where `reviews` is known.
    pub raised_measured: usize,
    pub unique_fixed_measured: usize,
    pub per_review: Option<f64>,
    pub unique_fixed_per_review: Option<f64>,
    /// Over `cost.cost_usd.total`, when every worker reported a cost.
    pub unique_fixed_per_usd: Option<f64>,
}

/// One cost column over a reviewer's workers. Workers that reported nothing are left out.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Spend {
    pub reported: usize,
    pub median: Option<f64>,
    pub total: Option<f64>,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Cost {
    pub workers: usize,
    pub tokens: Spend,
    pub wall_s: Spend,
    pub cost_usd: Spend,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Reviewer {
    /// Passes whose panel ran this reviewer.
    pub reviews: usize,
    pub critical: Counts,
    pub important: Counts,
    /// Critical and Important together.
    pub total: Counts,
    pub cost: Cost,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Overlap {
    pub reviewers: [String; 2],
    /// Raised findings both raised.
    pub shared: usize,
    /// Shared over raised by either.
    pub jaccard: Option<f64>,
}

/// What the numbers leave out.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Excluded {
    /// Critical or Important in changed code with no reviewer named.
    pub findings_without_axis: usize,
    /// Critical or Important with no `Scope`, so neither raised nor an escape.
    pub findings_without_scope: usize,
    /// Attempts with no Passes table: their findings count as raised, but not per review.
    pub attempts_without_passes: usize,
    /// Passes with no panel; they are left out of `reviews` and cost.
    pub passes_without_panel: usize,
    /// Critical or Important escapes with no `missed_by`.
    pub escapes_unattributed: usize,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Report {
    pub attempts: usize,
    /// Attempts with at least one pass that has a panel.
    pub measured_attempts: usize,
    pub reviewers: BTreeMap<String, Reviewer>,
    pub overlap: Vec<Overlap>,
    pub excluded: Excluded,
}

fn counts<'a>(r: &'a mut BTreeMap<String, Reviewer>, name: &str, s: Severity) -> &'a mut Counts {
    let rv = r.entry(name.to_string()).or_default();
    match s {
        Severity::Critical => &mut rv.critical,
        _ => &mut rv.important,
    }
}

fn ratio(n: usize, d: f64) -> Option<f64> {
    (d > 0.0).then(|| round(n as f64 / d, 3))
}

fn spend(values: Vec<f64>) -> Spend {
    Spend {
        reported: values.len(),
        total: (!values.is_empty()).then(|| round(values.iter().sum(), 3)),
        median: median(values).map(|m| round(m, 3)),
    }
}

fn cost(workers: &[&Worker]) -> Cost {
    let col =
        |f: &dyn Fn(&Worker) -> Option<f64>| spend(workers.iter().filter_map(|w| f(w)).collect());
    Cost {
        workers: workers.len(),
        tokens: col(&|w| w.tokens.map(|t| t as f64)),
        wall_s: col(&|w| w.wall_s),
        cost_usd: col(&|w| w.cost_usd),
    }
}

fn finish(r: &mut BTreeMap<String, Reviewer>, workers: &BTreeMap<String, Vec<&Worker>>) {
    for (name, rv) in r {
        rv.cost = cost(workers.get(name).map_or(&[][..], Vec::as_slice));
        let usd = rv
            .cost
            .cost_usd
            .total
            .filter(|_| rv.cost.cost_usd.reported == rv.cost.workers);
        let reviews = rv.reviews as f64;
        let (c, i) = (&rv.critical, &rv.important);
        rv.total = Counts {
            raised: c.raised + i.raised,
            unique: c.unique + i.unique,
            unique_fixed: c.unique_fixed + i.unique_fixed,
            dismissed: c.dismissed + i.dismissed,
            escapes: c.escapes + i.escapes,
            raised_measured: c.raised_measured + i.raised_measured,
            unique_fixed_measured: c.unique_fixed_measured + i.unique_fixed_measured,
            ..Counts::default()
        };
        for c in [&mut rv.critical, &mut rv.important, &mut rv.total] {
            c.dismiss_rate = ratio(c.dismissed, c.raised as f64);
            c.per_review = ratio(c.raised_measured, reviews);
            c.unique_fixed_per_review = ratio(c.unique_fixed_measured, reviews);
            c.unique_fixed_per_usd = usd.and_then(|d| ratio(c.unique_fixed_measured, d));
        }
    }
}

pub fn reviewers(units: &[&Unit]) -> Report {
    let mut r: BTreeMap<String, Reviewer> = AXES
        .iter()
        .map(|a| (a.to_string(), Reviewer::default()))
        .collect();
    let mut x = Excluded::default();
    let mut workers: BTreeMap<String, Vec<&Worker>> = BTreeMap::new();
    let mut raised: Vec<Vec<&str>> = Vec::new();
    let (mut attempts, mut measured_attempts) = (0, 0);
    for a in units.iter().flat_map(|u| &u.attempts) {
        attempts += 1;
        let paneled = a.passes.iter().filter(|p| p.panel.is_some()).count();
        x.passes_without_panel += a.passes.len() - paneled;
        x.attempts_without_passes += usize::from(a.passes.is_empty());
        let measured = paneled > 0;
        measured_attempts += usize::from(measured);
        for p in a.passes.iter().filter(|p| p.panel.is_some()) {
            let ran: BTreeSet<&str> = p.workers.iter().map(|w| w.reviewer.as_str()).collect();
            for name in ran {
                r.entry(name.to_string()).or_default().reviews += 1;
            }
            for w in &p.workers {
                workers.entry(w.reviewer.clone()).or_default().push(w);
            }
        }
        for f in &a.findings {
            if !matches!(f.severity, Severity::Critical | Severity::Important) {
                continue;
            }
            let by: Vec<&str> = AXES
                .into_iter()
                .filter(|a| f.axes.iter().any(|x| x == a))
                .collect();
            match f.scope {
                Scope::PreExisting if f.status == Status::Dismissed => {}
                Scope::PreExisting => match &f.missed_by {
                    Some(m) => counts(&mut r, m, f.severity).escapes += 1,
                    None => x.escapes_unattributed += 1,
                },
                Scope::Unknown => x.findings_without_scope += 1,
                Scope::Changed if by.is_empty() => x.findings_without_axis += 1,
                Scope::Changed => {
                    let unique = by.len() == 1;
                    let unique_fixed = usize::from(unique && f.status == Status::Fixed);
                    for axis in &by {
                        let c = counts(&mut r, axis, f.severity);
                        c.raised += 1;
                        c.unique += usize::from(unique);
                        c.unique_fixed += unique_fixed;
                        c.dismissed += usize::from(
                            f.status == Status::Dismissed
                                || f.dismissed_raises.iter().any(|d| d == axis),
                        );
                        if measured {
                            c.raised_measured += 1;
                            c.unique_fixed_measured += unique_fixed;
                        }
                    }
                    raised.push(by);
                }
            }
        }
    }
    finish(&mut r, &workers);
    let names: Vec<&String> = r.keys().collect();
    let by = |n: &str| r[n].critical.raised + r[n].important.raised;
    let mut overlap = Vec::new();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            let shared = raised
                .iter()
                .filter(|by| by.contains(&a.as_str()) && by.contains(&b.as_str()))
                .count();
            overlap.push(Overlap {
                reviewers: [(*a).clone(), (*b).clone()],
                shared,
                jaccard: ratio(shared, (by(a) + by(b) - shared) as f64),
            });
        }
    }
    Report {
        attempts,
        measured_attempts,
        reviewers: r,
        overlap,
        excluded: x,
    }
}

fn num(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_string(), |n| format!("{n}"))
}

/// The report as plain-text tables for a person.
pub fn render(r: &Report, s: &mut impl Write) -> io::Result<()> {
    writeln!(s,
        "Review attempts: {} ({} with a panel). Critical and Important findings in changed code, from every review attempt.\n",
        r.attempts, r.measured_attempts
    )?;
    writeln!(
        s,
        "{:<13} {:<9} {:>7} {:>6} {:>7} {:>6} {:>8} {:>8} {:>9} {:>8} {:>7}",
        "reviewer",
        "severity",
        "reviews",
        "raised",
        "/review",
        "unique",
        "uniq-fix",
        "fix/rev",
        "dismissed",
        "dismiss%",
        "escapes"
    )?;
    for (name, rv) in &r.reviewers {
        for (sev, c) in [
            ("critical", &rv.critical),
            ("important", &rv.important),
            ("total", &rv.total),
        ] {
            writeln!(
                s,
                "{:<13} {:<9} {:>7} {:>6} {:>7} {:>6} {:>8} {:>8} {:>9} {:>8} {:>7}",
                name,
                sev,
                rv.reviews,
                c.raised,
                num(c.per_review),
                c.unique,
                c.unique_fixed,
                num(c.unique_fixed_per_review),
                c.dismissed,
                num(c.dismiss_rate),
                c.escapes
            )?;
        }
    }
    writeln!(
        s,
        "\nCost per reviewer worker (median / total; reported of workers)"
    )?;
    writeln!(
        s,
        "{:<13} {:>7} {:>21} {:>17} {:>15} {:>11}",
        "reviewer", "workers", "tokens", "seconds", "usd", "uniq-fix/$"
    )?;
    for (name, rv) in &r.reviewers {
        let c = &rv.cost;
        let col = |sp: &Spend| format!("{} / {} ({})", num(sp.median), num(sp.total), sp.reported);
        writeln!(
            s,
            "{:<13} {:>7} {:>21} {:>17} {:>15} {:>11}",
            name,
            c.workers,
            col(&c.tokens),
            col(&c.wall_s),
            col(&c.cost_usd),
            num(rv.total.unique_fixed_per_usd)
        )?;
    }
    let names: Vec<&String> = r.reviewers.keys().collect();
    writeln!(s, "\nOverlap: Jaccard (shared findings)")?;
    let short = |n: &str| n.chars().take(11).collect::<String>();
    write!(s, "{:<13}", "")?;
    for n in &names[1..] {
        write!(s, " {:>11}", short(n))?;
    }
    writeln!(s)?;
    for a in &names[..names.len() - 1] {
        write!(s, "{a:<13}")?;
        for b in &names[1..] {
            let cell = r
                .overlap
                .iter()
                .find(|o| &o.reviewers[0] == *a && &o.reviewers[1] == *b)
                .map_or(String::new(), |o| {
                    format!("{} ({})", num(o.jaccard), o.shared)
                });
            write!(s, " {cell:>11}")?;
        }
        writeln!(s)?;
    }
    let e = &r.excluded;
    writeln!(s,
        "\nLeft out: {} findings without an axis, {} without a scope, {} unattributed escapes, \
         {} passes without a panel; {} attempts without a Passes table count as raised, not per review.",
        e.findings_without_axis,
        e.findings_without_scope,
        e.escapes_unattributed,
        e.passes_without_panel,
        e.attempts_without_passes
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::metrics::{Attempt, RunStats};
    use crate::quality::record;
    use crate::review_md::{parse, passes};

    fn attempt(text: &str) -> Attempt {
        Attempt {
            findings: parse(text),
            passes: passes(text),
            ..Attempt::default()
        }
    }

    fn unit(id: &str, attempts: Vec<Attempt>) -> Unit {
        Unit {
            id: id.into(),
            worktree: None,
            attempts,
            day: None,
            run: RunStats::default(),
        }
    }

    /// A reduced first pass and a correctness-and-tests cycle pass. `spec`'s cost is unknown.
    const REDUCED: &str = "\
## Passes
| Pass | Panel | Reviewer | Role | Provider | Tokens | Seconds | Cost (USD) |
|---|---|---|---|---|---|---|---|
| 0 | reduced | correctness | review.correctness | claude | 40000 | 100 | 0.40 |
| 0 | reduced | spec | review.spec | codex | 30000 | 90 | |
| 0 | reduced | tests | review.tests | claude | 20000 | 60 | 0.20 |
| 1 | cycle | correctness | review.correctness | claude | 10000 | 50 | 0.10 |
| 1 | cycle | tests | review.tests | claude | 12000 | 40 | 0.30 |

## Critical
### C1. only tests, fixed
- Axis: tests
- Scope: changed
- Status: fixed (cycle 1, abc)
## Important
### I1. two raisers, fixed
- Axis: correctness, tests
- Scope: changed
- Status: fixed (cycle 1, abc)
### I2. spec alone, dismissed
- Axis: spec
- Scope: changed
- Status: dismissed: the brief says so
### I3. spec's version rejected
- Axis: correctness, spec
- Dismissed raises: spec
- Scope: changed
- Status: open
### I4. escape tests missed
- Axis: correctness
- Scope: pre-existing
- Missed by: tests
- Status: deferred: #50
### I5. unattributed escape
- Axis: security
- Scope: pre-existing
- Status: deferred: #51
### I6. no axis
- Scope: changed
- Status: open
### I7. no scope
- Axis: tests
- Status: fixed
### I8. dismissed escape
- Axis: tests
- Scope: pre-existing
- Missed by: tests
- Status: dismissed: not a defect
## Suggestion
### S1. ignored
- Axis: readability
- Scope: changed
- Status: open
";

    /// Every reviewer on a full first pass; one worker reran inline.
    fn full() -> String {
        let mut t = String::from(
            "## Passes\n| Pass | Panel | Reviewer | Tokens | Seconds | Cost (USD) |\n|---|---|---|---|---|---|\n",
        );
        for a in AXES {
            t.push_str(&format!("| 0 | full | {a} | 1000 | 10 | 0.05 |\n"));
        }
        t.push_str("| 0 | full | tests | 1000 | 10 | 0.05 |\n");
        t + "## Important\n### I1. performance alone\n- Axis: performance\n- Scope: changed\n- Status: fixed\n"
    }

    /// A record from before #424: no passes, no dismissed raises, no missed_by.
    const OLD: &str = r#"{"v":1,"unit":"9-old","issue":9,"pr":10,"outcome":"merged","recorded":"2026-10-10T15:00:00Z","attempt":1,"attempts":1,"artifact":".ns/9-old/review.md","status":"pass","updated":"2026-10-09T16:00:00Z","day":"2026-10-09","cycles":1,"blame_at":null,"changed_lines":10,"changed_lines_source":"summary","stray_statuses":0,"findings":[{"id":"I1","severity":"important","axes":["tests"],"axes_from_raised_by":false,"scope":"changed","cycle":0,"status":"fixed","fixed_cycle":1,"location":null},{"id":"I2","severity":"important","axes":["security"],"axes_from_raised_by":false,"scope":"pre-existing","cycle":0,"status":"deferred","fixed_cycle":null,"location":null}],"first_pass_file":[]}"#;

    fn table(r: &Report) -> String {
        let mut out = Vec::new();
        render(r, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn report() -> Report {
        let (records, bad) = record::parse(OLD);
        assert_eq!(bad, 0);
        let old = record::units(&records);
        let u = unit("1-a", vec![attempt(REDUCED)]);
        let w = unit("3-c", vec![attempt(&full())]);
        let units: Vec<&Unit> = [&u, &w].into_iter().chain(&old).collect();
        reviewers(&units)
    }

    #[test]
    fn reviews_come_from_the_panels_that_ran_each_reviewer() {
        let r = report();
        assert_eq!((r.attempts, r.measured_attempts), (3, 2));
        let reviews = |n: &str| r.reviewers[n].reviews;
        assert_eq!(reviews("correctness"), 3);
        assert_eq!(reviews("tests"), 3, "a rerun worker is still one review");
        assert_eq!(reviews("spec"), 2);
        assert_eq!(reviews("performance"), 1);
        assert_eq!(r.reviewers.len(), AXES.len());
    }

    #[test]
    fn raised_unique_and_fixed_split_by_severity() {
        let r = report();
        let tests = &r.reviewers["tests"];
        assert_eq!(
            (
                tests.critical.raised,
                tests.critical.unique,
                tests.critical.unique_fixed
            ),
            (1, 1, 1)
        );
        // I1 shared with correctness, plus the old record's unique fixed I1.
        assert_eq!(
            (
                tests.important.raised,
                tests.important.unique,
                tests.important.unique_fixed
            ),
            (2, 1, 1)
        );
        assert_eq!(
            (
                tests.important.raised_measured,
                tests.important.unique_fixed_measured
            ),
            (1, 0),
            "the old record has no passes, so it is not per review"
        );
        assert_eq!(tests.critical.per_review, Some(0.333));
        assert_eq!(tests.critical.unique_fixed_per_review, Some(0.333));
        assert_eq!(tests.important.unique_fixed_per_review, Some(0.0));
        let c = &r.reviewers["correctness"].important;
        assert_eq!(
            (c.raised, c.unique, c.unique_fixed, c.dismissed),
            (2, 0, 0, 0)
        );
        assert_eq!(c.per_review, Some(0.667));
        let perf = &r.reviewers["performance"].important;
        assert_eq!(
            (perf.raised, perf.unique_fixed, perf.per_review),
            (1, 1, Some(1.0))
        );
        assert_eq!(
            r.reviewers["readability"].important,
            Counts {
                per_review: Some(0.0),
                unique_fixed_per_review: Some(0.0),
                unique_fixed_per_usd: Some(0.0),
                ..Counts::default()
            }
        );
    }

    #[test]
    fn a_dismissed_finding_or_raise_counts_against_its_reviewer() {
        let spec = &report().reviewers["spec"].important;
        assert_eq!((spec.raised, spec.unique, spec.unique_fixed), (2, 1, 0));
        assert_eq!((spec.dismissed, spec.dismiss_rate), (2, Some(1.0)));
        assert_eq!(
            report().reviewers["tests"].important.dismiss_rate,
            Some(0.0)
        );
        assert_eq!(report().reviewers["security"].important.dismiss_rate, None);
    }

    #[test]
    fn escapes_go_to_the_reviewer_that_missed_them() {
        let r = report();
        assert_eq!(r.reviewers["tests"].important.escapes, 1);
        assert_eq!(r.reviewers["correctness"].important.escapes, 0);
        assert_eq!(r.reviewers["security"].important.escapes, 0);
        assert_eq!(
            r.excluded.escapes_unattributed, 2,
            "I5 and the old record's I2"
        );
    }

    #[test]
    fn findings_and_passes_it_cannot_place_are_left_out_and_counted() {
        let r = report();
        assert_eq!(
            r.excluded,
            Excluded {
                findings_without_axis: 1,
                findings_without_scope: 1,
                attempts_without_passes: 1,
                passes_without_panel: 0,
                escapes_unattributed: 2,
            }
        );
        let blank = "## Passes\n| Pass | Panel | Reviewer | Tokens |\n|---|---|---|---|\n| 0 | | tests | 500 |\n| 1 | cycle | tests | 100 |\n### I1. x\n- Axis: tests\n- Scope: changed\n- Status: fixed\n";
        let u = unit("2-b", vec![attempt(blank)]);
        let r = reviewers(&[&u]);
        assert_eq!(r.excluded.passes_without_panel, 1);
        assert_eq!(r.measured_attempts, 1, "one pass has a panel");
        let t = &r.reviewers["tests"];
        assert_eq!((t.reviews, t.cost.workers), (1, 1), "only the paneled pass");
        assert_eq!(t.cost.tokens.total, Some(100.0));
        assert_eq!((t.important.raised, t.important.raised_measured), (1, 1));
        assert_eq!(t.important.per_review, Some(1.0));
        let none = "## Passes\n| Pass | Panel | Reviewer |\n|---|---|---|\n| 0 | | tests |\n### I1. x\n- Axis: tests\n- Scope: changed\n- Status: fixed\n";
        let u = unit("3-d", vec![attempt(none)]);
        let r = reviewers(&[&u]);
        assert_eq!(r.measured_attempts, 0);
        let t = &r.reviewers["tests"];
        assert_eq!((t.reviews, t.cost.workers), (0, 0));
        assert_eq!((t.important.raised, t.important.raised_measured), (1, 0));
        assert_eq!(t.important.per_review, None);
    }

    #[test]
    fn a_finding_with_no_scope_is_left_out_and_counted() {
        let u = unit(
            "4-s",
            vec![attempt(
                "### I1. old record\n- Axis: tests\n- Status: fixed\n### I2. no axis\n- Scope: changed\n- Status: open\n",
            )],
        );
        let r = reviewers(&[&u]);
        let i = &r.reviewers["tests"].important;
        assert_eq!((i.raised, i.unique, i.unique_fixed), (0, 0, 0));
        assert_eq!(r.excluded.findings_without_scope, 1);
        assert_eq!(r.excluded.findings_without_axis, 1);
    }

    #[test]
    fn every_attempt_of_a_unit_counts_its_findings() {
        const PASSES: &str = "## Passes\n| Pass | Panel | Reviewer | Tokens |\n|---|---|---|---|\n| 0 | reduced | tests | 100 |\n";
        const EARLY: &str = "### I1. x\n- Axis: tests\n- Scope: changed\n- Status: fixed\n";
        const FINAL: &str = "### I1. y\n- Axis: correctness\n- Scope: changed\n- Status: open\n### I2. z\n- Axis: tests\n- Scope: pre-existing\n- Missed by: tests\n- Status: deferred: #1\n";
        let u = unit(
            "4-f",
            vec![attempt(EARLY), attempt(&format!("{PASSES}{FINAL}"))],
        );
        let r = reviewers(&[&u]);
        assert_eq!(r.excluded.attempts_without_passes, 1);
        assert_eq!((r.attempts, r.measured_attempts), (2, 1));
        let t = &r.reviewers["tests"];
        assert_eq!((t.reviews, t.cost.workers), (1, 1));
        let i = &t.important;
        assert_eq!(
            (
                i.raised,
                i.unique,
                i.unique_fixed,
                i.escapes,
                i.raised_measured
            ),
            (1, 1, 1, 1, 0),
            "the early attempt's fixed finding counts, but not per review"
        );
        let i = &r.reviewers["correctness"].important;
        assert_eq!((i.raised, i.raised_measured), (1, 1));
    }

    #[test]
    fn a_finding_reviewers_are_only_the_eight_reviewer_axes() {
        let mut a = attempt(
            "### I1. x\n- Axis: tests\n- Scope: changed\n- Status: fixed\n### I2. y\n- Axis: tests\n- Scope: changed\n- Status: fixed\n",
        );
        a.findings[0].axes = vec!["lead".into(), "tests".into()];
        a.findings[1].axes = vec!["lead".into()];
        let u = unit("5-x", vec![a]);
        let r = reviewers(&[&u]);
        let i = &r.reviewers["tests"].important;
        assert_eq!((i.raised, i.unique, i.unique_fixed), (1, 1, 1));
        assert_eq!(r.excluded.findings_without_axis, 1);
        assert!(!r.reviewers.contains_key("lead"));
    }

    #[test]
    fn total_adds_critical_and_important() {
        let r = report();
        let t = &r.reviewers["tests"].total;
        assert_eq!(
            (t.raised, t.unique, t.unique_fixed, t.raised_measured),
            (3, 2, 2, 2)
        );
        assert_eq!(t.per_review, Some(0.667));
        let spec = &r.reviewers["spec"].total;
        assert_eq!((spec.dismissed, spec.dismiss_rate), (2, Some(1.0)));
        assert_eq!(r.reviewers["tests"].total.escapes, 1);
        let perf = &r.reviewers["performance"].total;
        assert_eq!(
            (perf.unique_fixed_measured, perf.unique_fixed_per_usd),
            (1, Some(20.0))
        );
    }

    #[test]
    fn cost_skips_unreported_values_and_per_dollar_needs_every_worker_priced() {
        let r = report();
        let t = &r.reviewers["tests"].cost;
        assert_eq!(t.workers, 4);
        assert_eq!(
            t.tokens,
            Spend {
                reported: 4,
                median: Some(6500.0),
                total: Some(34000.0)
            }
        );
        assert_eq!(t.cost_usd.total, Some(0.6));
        assert_eq!(
            t.wall_s,
            Spend {
                reported: 4,
                median: Some(25.0),
                total: Some(120.0)
            }
        );
        assert_eq!(
            r.reviewers["tests"].critical.unique_fixed_per_usd,
            Some(1.667)
        );
        let spec = &r.reviewers["spec"];
        assert_eq!(
            spec.cost.cost_usd,
            Spend {
                reported: 1,
                median: Some(0.05),
                total: Some(0.05)
            }
        );
        assert_eq!(
            spec.important.unique_fixed_per_usd, None,
            "one spec worker has no cost"
        );
        let none = reviewers(&[]);
        assert_eq!(none.reviewers["tests"].cost, Cost::default());
        assert_eq!(none.reviewers["tests"].critical.unique_fixed_per_usd, None);
    }

    #[test]
    fn overlap_is_shared_findings_and_jaccard_per_pair() {
        let r = report();
        assert_eq!(r.overlap.len(), AXES.len() * (AXES.len() - 1) / 2);
        let pair = |a: &str, b: &str| {
            r.overlap
                .iter()
                .find(|o| o.reviewers == [a.to_string(), b.to_string()])
                .unwrap()
        };
        // correctness raised I1, I3; tests C1, I1, old I1; spec I2, I3.
        assert_eq!(
            (
                pair("correctness", "tests").shared,
                pair("correctness", "tests").jaccard
            ),
            (1, Some(0.25))
        );
        assert_eq!(
            (
                pair("correctness", "spec").shared,
                pair("correctness", "spec").jaccard
            ),
            (1, Some(0.333))
        );
        assert_eq!(pair("spec", "tests").jaccard, Some(0.0));
        assert_eq!(pair("readability", "security").jaccard, None);
    }

    #[test]
    fn the_table_has_a_row_per_reviewer_and_severity_and_the_exclusions() {
        let t = table(&report());
        assert!(t.contains("Review attempts: 3 (2 with a panel)"), "{t}");
        let row = t
            .lines()
            .find(|l| l.starts_with("tests ") && l.contains("critical"))
            .unwrap();
        assert_eq!(
            row.split_whitespace().collect::<Vec<_>>(),
            ["tests", "critical", "3", "1", "0.333", "1", "1", "0.333", "0", "0", "0"]
        );
        assert!(t.contains("0.25 (1)"), "{t}");
        assert!(t.contains("1 findings without an axis, 1 without a scope, 2 unattributed escapes, 0 passes without a panel; 1 attempts"), "{t}");
        let total = t
            .lines()
            .find(|l| l.starts_with("tests ") && l.contains("total"))
            .unwrap();
        assert_eq!(
            total.split_whitespace().collect::<Vec<_>>(),
            ["tests", "total", "3", "3", "0.667", "2", "2", "0.333", "0", "0", "1"]
        );
        let cost = t
            .lines()
            .filter(|l| l.starts_with("tests "))
            .nth(3)
            .unwrap();
        assert!(cost.contains("6500 / 34000 (4)"), "{cost}");
        assert!(cost.trim_end().ends_with("1.667"), "{cost}");
        let perf = t
            .lines()
            .filter(|l| l.starts_with("performance "))
            .nth(3)
            .unwrap();
        assert!(
            perf.trim_end().ends_with(" 20"),
            "the total's per dollar: {perf}"
        );
        let spec = t.lines().filter(|l| l.starts_with("spec ")).nth(3).unwrap();
        assert!(spec.trim_end().ends_with('-'), "{spec}");
        let mut x = report();
        x.excluded.findings_without_scope = 7;
        assert!(table(&x).contains("1 findings without an axis, 7 without a scope,"));
    }
}
