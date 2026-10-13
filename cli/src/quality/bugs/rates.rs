//! The fix issues per merged unit and per 1,000 changed lines in each week of `ns quality --bugs`,
//! and each rate's trend (docs/FACTORY.md, Quality, Bug counts).

use std::fmt::Write as _;

use serde::Serialize;

use super::{Report, Week};
use crate::watch::forge::MergedPr;

/// The fewest weeks with a rate a trend is fitted on.
const TREND_MIN: usize = 4;

/// A slope within this share of the mean rate, per week, is flat.
const FLAT: f64 = 0.05;

/// A week's merged units and the fix issues per unit and per 1,000 of their changed lines.
#[derive(Serialize, Debug, PartialEq)]
pub struct Rates {
    pub merged_units: usize,
    /// Null when the forge gives no line counts.
    pub changed_lines: Option<u64>,
    /// Null when no unit merged.
    pub per_unit: Option<f64>,
    /// Null when no line changed or the count is unknown.
    pub per_kloc: Option<f64>,
    /// The week starts before the oldest merged PR in a list the forge's cap cut short.
    pub undercounted: bool,
}

impl Rates {
    pub(super) fn new(fixes: usize, units: &[&MergedPr], undercounted: bool) -> Self {
        let changed_lines = units.iter().map(|p| p.changed).sum::<Option<u64>>();
        let rate = |per: f64| (per > 0.0).then(|| round(fixes as f64 / per));
        Rates {
            merged_units: units.len(),
            changed_lines,
            per_unit: rate(units.len() as f64),
            per_kloc: changed_lines.and_then(|l| rate(l as f64 / 1000.0)),
            undercounted,
        }
    }
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Rising,
    Flat,
    Falling,
    /// Fewer than [`TREND_MIN`] weeks have a rate.
    Insufficient,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Trend {
    pub direction: Direction,
    /// The least-squares change in the rate per week, null when insufficient.
    pub slope: Option<f64>,
    /// The weeks fitted: those with a rate that is not undercounted.
    pub weeks: usize,
}

/// The trend of the rates `(week index, rate)`.
pub fn trend(points: &[(usize, f64)]) -> Trend {
    let n = points.len();
    if n < TREND_MIN {
        return Trend {
            direction: Direction::Insufficient,
            slope: None,
            weeks: n,
        };
    }
    let mean_x = points.iter().map(|p| p.0 as f64).sum::<f64>() / n as f64;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n as f64;
    let (num, den) = points.iter().fold((0.0, 0.0), |(num, den), &(x, y)| {
        let dx = x as f64 - mean_x;
        (num + dx * (y - mean_y), den + dx * dx)
    });
    let slope = num / den;
    let direction = if slope.abs() <= FLAT * mean_y {
        Direction::Flat
    } else if slope > 0.0 {
        Direction::Rising
    } else {
        Direction::Falling
    };
    Trend {
        direction,
        slope: Some(round(slope)),
        weeks: n,
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Trends {
    pub per_unit: Trend,
    pub per_kloc: Trend,
}

/// The merged-PR list the rates come from.
#[derive(Serialize, Debug)]
pub struct Listed {
    pub listed: usize,
    pub truncated: bool,
    /// When the oldest listed PR merged.
    pub oldest: Option<String>,
}

/// Each rate's trend over `weeks`, fitted on the weeks with that rate that are not undercounted.
pub fn trends(weeks: &[Week]) -> Trends {
    let fit = |rate: fn(&Rates) -> Option<f64>| {
        let points: Vec<(usize, f64)> = weeks
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.rates.undercounted)
            .filter_map(|(i, w)| Some((i, rate(&w.rates)?)))
            .collect();
        trend(&points)
    };
    Trends {
        per_unit: fit(|r| r.per_unit),
        per_kloc: fit(|r| r.per_kloc),
    }
}

impl Report {
    pub(super) fn rates_text(&self, s: &mut String) {
        let _ = writeln!(
            s,
            "\nfix issues per merged unit (a PR from an ns/ branch) and per 1,000 changed lines\n"
        );
        let _ = writeln!(
            s,
            "{:<9} {:>5} {:>6} {:>8} {:>8}",
            "week", "units", "lines", "fix/unit", "fix/kloc"
        );
        let num = |n: Option<f64>| n.map_or("-".to_string(), |n| format!("{n:.2}"));
        for w in &self.weeks {
            let r = &w.rates;
            let _ = writeln!(
                s,
                "{:<9} {:>5} {:>6} {:>8} {:>8}{}",
                w.week,
                r.merged_units,
                r.changed_lines.map_or("-".to_string(), |l| l.to_string()),
                num(r.per_unit),
                num(r.per_kloc),
                if r.undercounted { " *" } else { "" }
            );
        }
        let _ = writeln!(s);
        for (name, t) in [
            ("per unit", &self.trend.per_unit),
            ("per 1,000 lines", &self.trend.per_kloc),
        ] {
            let _ = match t.slope {
                Some(slope) => writeln!(
                    s,
                    "trend {name}: {}, {slope:+.3} a week over {} weeks",
                    format!("{:?}", t.direction).to_lowercase(),
                    t.weeks
                ),
                None => writeln!(
                    s,
                    "trend {name}: insufficient, needs {TREND_MIN} weeks with a rate, has {}",
                    t.weeks
                ),
            };
        }
        if self.weeks.iter().any(|w| w.rates.undercounted) {
            let _ = writeln!(
                s,
                "\n* The forge listed only its {} most recent merged PRs, the oldest merged {}. \
                 Units and lines in a week before that may be undercounted, so its rates may be \
                 high; the trend leaves it out.",
                self.merged_prs.listed,
                self.merged_prs.oldest.as_deref().unwrap_or("never")
            );
        }
        if self.weeks.iter().any(|w| w.rates.changed_lines.is_none()) {
            let _ = writeln!(
                s,
                "\nThe forge's merged-PR list gives no changed lines, so there is no rate per 1,000 lines."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::bugs::tests::{filed, merged, no_trace, pr, t};
    use crate::quality::bugs::{report, WEEK};
    use crate::quality::Window;
    use crate::watch::forge::Filed;

    fn direction(rates: &[f64]) -> Direction {
        let points: Vec<(usize, f64)> = rates.iter().copied().enumerate().collect();
        trend(&points).direction
    }

    #[test]
    fn a_trend_is_the_least_squares_slope_and_needs_four_weeks() {
        let falling = [(0, 2.0), (1, 1.5), (2, 1.0), (3, 0.5)];
        assert_eq!(
            trend(&falling),
            Trend {
                direction: Direction::Falling,
                slope: Some(-0.5),
                weeks: 4
            }
        );
        assert_eq!(direction(&[0.1, 0.4, 0.3, 0.9, 1.0]), Direction::Rising);
        assert_eq!(direction(&[1.0, 0.8, 0.9, 0.2]), Direction::Falling);
        assert_eq!(
            trend(&[(0, 0.5), (1, 1.0), (5, 2.0), (9, 4.0)]).slope,
            Some(0.372)
        );
        assert_eq!(
            trend(&falling[..3]),
            Trend {
                direction: Direction::Insufficient,
                slope: None,
                weeks: 3
            }
        );
        assert_eq!(trend(&[]).direction, Direction::Insufficient);
    }

    #[test]
    fn a_slope_within_five_percent_of_the_mean_a_week_is_flat() {
        assert_eq!(direction(&[0.0, 0.0, 0.0, 0.0]), Direction::Flat);
        assert_eq!(direction(&[1.0, 1.0, 1.0, 1.0]), Direction::Flat);
        // Mean 1.075, slope 0.05: within 0.05375.
        assert_eq!(direction(&[1.0, 1.05, 1.1, 1.15]), Direction::Flat);
        assert_eq!(direction(&[1.15, 1.1, 1.05, 1.0]), Direction::Flat);
        // Mean 1.09, slope 0.06: past 0.0545.
        assert_eq!(direction(&[1.0, 1.06, 1.12, 1.18]), Direction::Rising);
        assert_eq!(direction(&[1.18, 1.12, 1.06, 1.0]), Direction::Falling);
    }

    #[test]
    fn a_rate_with_nothing_merged_or_no_lines_is_null() {
        let a = pr(1, "ns/1", "2026-10-05", Some(300));
        let b = pr(2, "ns/2", "2026-10-06", Some(100));
        assert_eq!(
            Rates::new(3, &[&a, &b], false),
            Rates {
                merged_units: 2,
                changed_lines: Some(400),
                per_unit: Some(1.5),
                per_kloc: Some(7.5),
                undercounted: false
            }
        );
        assert_eq!(Rates::new(2, &[&a], true).per_kloc, Some(6.667));
        let none = Rates::new(2, &[], false);
        assert_eq!(
            (
                none.merged_units,
                none.changed_lines,
                none.per_unit,
                none.per_kloc
            ),
            (0, Some(0), None, None)
        );
        let empty = pr(3, "ns/3", "2026-10-05", Some(0));
        let r = Rates::new(1, &[&empty], false);
        assert_eq!((r.per_unit, r.per_kloc), (Some(1.0), None));
        let unknown = pr(4, "ns/4", "2026-10-05", None);
        let r = Rates::new(1, &[&a, &unknown], false);
        assert_eq!(
            (r.changed_lines, r.per_unit, r.per_kloc),
            (None, Some(0.5), None)
        );
    }

    /// One fix issue a week for six weeks from 2026-09-07, `units` ns/ PRs of 100 lines merged
    /// in each, and a fork PR and a human PR every week that never count.
    fn series(units: &[usize], truncated: bool) -> Report {
        let filed: Vec<Filed> = (0..6)
            .map(|i| {
                let mut f = filed(
                    i + 1,
                    "2026-09-07T12:00:00Z",
                    "fix: x",
                    "Source: human",
                    &[],
                );
                f.opened += i as i64 * WEEK;
                f
            })
            .collect();
        let mut prs = Vec::new();
        for (i, &n) in units.iter().enumerate() {
            let at = |p: MergedPr| MergedPr {
                merged: p.merged + i as i64 * WEEK,
                ..p
            };
            for j in 0..n {
                prs.push(at(pr(
                    100 + (i * 10 + j) as u64,
                    "ns/u",
                    "2026-09-08",
                    Some(100),
                )));
            }
            prs.push(at(pr(900 + i as u64, "", "2026-09-08", Some(1000))));
            prs.push(at(pr(
                950 + i as u64,
                "someone/x",
                "2026-09-08",
                Some(1000),
            )));
        }
        prs.push(pr(999, "ns/old", "2026-08-01", Some(50)));
        report(
            &filed,
            Window {
                since: Some(t("2026-09-07")),
                until: None,
            },
            t("2026-10-14"),
            &|_| Err("x".into()),
            &merged(&prs, truncated),
        )
    }

    #[test]
    fn rates_fall_as_more_units_merge_for_the_same_fixes() {
        let r = series(&[1, 1, 2, 2, 4, 5], false);
        let per_unit: Vec<Option<f64>> = r.weeks.iter().map(|w| w.rates.per_unit).collect();
        assert_eq!(
            per_unit,
            [
                Some(1.0),
                Some(1.0),
                Some(0.5),
                Some(0.5),
                Some(0.25),
                Some(0.2)
            ]
        );
        assert_eq!(r.weeks[2].rates.changed_lines, Some(200));
        assert_eq!(r.weeks[2].rates.per_kloc, Some(5.0));
        assert_eq!(r.trend.per_unit.direction, Direction::Falling);
        assert_eq!(r.trend.per_kloc.direction, Direction::Falling);
        assert_eq!(r.trend.per_unit.weeks, 6);
        assert_eq!(
            (r.merged_prs.listed, r.merged_prs.truncated),
            (15 + 2 * 6 + 1, false)
        );
        assert_eq!(r.merged_prs.oldest.as_deref(), Some("2026-08-01T00:00:00Z"));
    }

    #[test]
    fn rates_rise_as_fewer_units_merge_for_the_same_fixes() {
        let r = series(&[5, 4, 2, 2, 1, 1], false);
        assert_eq!(r.trend.per_unit.direction, Direction::Rising);
        assert_eq!(r.trend.per_kloc.direction, Direction::Rising);
    }

    #[test]
    fn weeks_with_no_unit_merged_are_null_and_not_fitted() {
        let r = series(&[1, 0, 2, 0, 0, 4], false);
        assert_eq!(r.weeks[1].rates.per_unit, None);
        assert_eq!(r.weeks[1].rates.per_kloc, None);
        assert_eq!(
            r.trend.per_unit,
            Trend {
                direction: Direction::Insufficient,
                slope: None,
                weeks: 3
            }
        );
        assert_eq!(r.trend.per_kloc.direction, Direction::Insufficient);
        let r = series(&[1, 0, 2, 0, 4, 4], false);
        assert_eq!(r.trend.per_unit.weeks, 4);
        assert_eq!(r.trend.per_unit.direction, Direction::Falling);
    }

    #[test]
    fn weeks_before_the_oldest_pr_of_a_truncated_list_are_undercounted_and_not_fitted() {
        let r = series(&[1, 1, 2, 2, 4, 5], true);
        let under: Vec<bool> = r.weeks.iter().map(|w| w.rates.undercounted).collect();
        assert_eq!(under, [false; 6]);
        assert_eq!(r.trend.per_unit.weeks, 6);
        // Without the August PR the oldest merged 2026-09-08, after the first week starts.
        let filed: Vec<Filed> = Vec::new();
        let prs: Vec<MergedPr> = vec![
            pr(1, "ns/a", "2026-09-08", Some(10)),
            pr(2, "ns/b", "2026-09-14T00:00:00Z", Some(10)),
        ];
        let w = Window {
            since: Some(t("2026-09-07")),
            until: Some(t("2026-09-20")),
        };
        let mut r = report(&filed, w, 0, &no_trace, &merged(&prs, true));
        let under: Vec<bool> = r.weeks.iter().map(|w| w.rates.undercounted).collect();
        assert_eq!(under, [true, false]);
        r = report(&filed, w, 0, &no_trace, &merged(&prs, false));
        assert!(r.weeks.iter().all(|w| !w.rates.undercounted));
        let monday = [pr(2, "ns/b", "2026-09-14T00:00:00Z", Some(10))];
        r = report(&filed, w, 0, &no_trace, &merged(&monday, true));
        let under: Vec<bool> = r.weeks.iter().map(|w| w.rates.undercounted).collect();
        assert_eq!(under, [true, false]);
        r = report(&filed, w, 0, &no_trace, &merged(&[], true));
        assert!(r.weeks.iter().all(|w| w.rates.undercounted));
        assert_eq!(r.merged_prs.oldest, None);
    }

    #[test]
    fn undercounted_weeks_are_left_out_of_the_trend() {
        let filed: Vec<Filed> = (0..6)
            .map(|i| {
                let mut f = filed(
                    i + 1,
                    "2026-09-07T12:00:00Z",
                    "fix: x",
                    "Source: human",
                    &[],
                );
                f.opened += i as i64 * WEEK;
                f
            })
            .collect();
        // Two fixes a unit in the first week, which starts before the oldest listed PR, then one
        // a unit: flat once the first is left out.
        let mut prs = vec![pr(1, "ns/a", "2026-09-07T12:00:00Z", Some(100))];
        prs.push(pr(2, "ns/b", "2026-09-14T12:00:00Z", Some(100)));
        for i in 0..4 {
            prs.push(MergedPr {
                merged: t("2026-09-21T12:00:00Z") + i * WEEK,
                ..pr(10 + i as u64, "ns/c", "2026-09-21", Some(100))
            });
        }
        let w = Window {
            since: Some(t("2026-09-07")),
            until: Some(t("2026-10-12T23:59:59Z")),
        };
        let mut all = filed.clone();
        all.extend(filed.iter().take(1).map(|f| {
            let mut f = f.clone();
            f.issue.number += 50;
            f
        }));
        let r = report(&all, w, 0, &no_trace, &merged(&prs, true));
        let under: Vec<bool> = r.weeks.iter().map(|w| w.rates.undercounted).collect();
        assert_eq!(under, [true, false, false, false, false, false]);
        assert_eq!(r.weeks[0].rates.per_unit, Some(2.0));
        assert_eq!(
            r.trend.per_unit,
            Trend {
                direction: Direction::Flat,
                slope: Some(0.0),
                weeks: 5
            }
        );
    }

    #[test]
    fn text_shows_each_weeks_rates_the_trends_and_what_is_undercounted() {
        let filed: Vec<Filed> = vec![
            filed(1, "2026-09-08", "fix: a", "Source: human", &[]),
            filed(2, "2026-09-09", "fix: b", "Source: human", &[]),
        ];
        let prs = vec![
            pr(1, "ns/a", "2026-09-08", Some(300)),
            pr(2, "ns/b", "2026-09-16", None),
        ];
        let w = Window {
            since: Some(t("2026-09-07")),
            until: Some(t("2026-09-27")),
        };
        let r = report(&filed, w, 0, &no_trace, &merged(&prs, true));
        let text = r.text();
        let rates: Vec<&str> = text
            .lines()
            .skip_while(|l| !l.starts_with("fix issues per merged unit"))
            .collect();
        assert_eq!(
            rates,
            [
                "fix issues per merged unit (a PR from an ns/ branch) and per 1,000 changed lines",
                "",
                "week      units  lines fix/unit fix/kloc",
                "2026-W37      1    300     2.00     6.67 *",
                "2026-W38      1      -     0.00        -",
                "2026-W39      0      0        -        -",
                "",
                "trend per unit: insufficient, needs 4 weeks with a rate, has 1",
                "trend per 1,000 lines: insufficient, needs 4 weeks with a rate, has 0",
                "",
                "* The forge listed only its 2 most recent merged PRs, the oldest merged \
                 2026-09-08T00:00:00Z. Units and lines in a week before that may be undercounted, \
                 so its rates may be high; the trend leaves it out.",
                "",
                "The forge's merged-PR list gives no changed lines, so there is no rate per 1,000 lines.",
            ]
        );
        let r = series(&[1, 1, 2, 2, 4, 5], false);
        let text = r.text();
        assert!(
            text.contains("\ntrend per unit: falling, -0.179 a week over 6 weeks\n"),
            "{text}"
        );
        assert!(!text.contains("* The forge"), "{text}");
        assert!(!text.contains("no changed lines"), "{text}");
    }
}
