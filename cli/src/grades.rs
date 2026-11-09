//! The pass rate of each rubric over time, from the grades in `grades.jsonl`.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, Duration, NaiveDate};
use serde_json::{json, Value};

use crate::rubric::Grade;

/// One grade, as much of it as the tables need.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub rubric: String,
    pub pass: bool,
    /// Seconds since the epoch.
    pub at: i64,
    pub version: String,
}

/// The line the retrospective appends for one grade; `parse` reads the same field names back.
/// The run log adds `at` when it appends the line.
pub fn line(unit: &str, grade: &Grade, pass: bool, base: &str) -> Value {
    json!({
        "unit": unit,
        "rubric": grade.rubric,
        "score": grade.score,
        "pass": pass,
        "reason": grade.reason,
        "version": env!("CARGO_PKG_VERSION"),
        "base": base,
    })
}

/// The grades among `events`; an event that is not a whole grade is left out.
pub fn parse(events: &[Value]) -> Vec<Entry> {
    events
        .iter()
        .filter_map(|v| {
            Some(Entry {
                rubric: v["rubric"].as_str()?.to_string(),
                pass: v["pass"].as_bool()?,
                at: v["at"].as_i64()?,
                version: v["version"].as_str()?.to_string(),
            })
        })
        .collect()
}

/// The ISO week of `at`, as `2026-W45`.
fn week_of(at: i64) -> Option<String> {
    let week = DateTime::from_timestamp(at, 0)?.iso_week();
    Some(format!("{}-W{:02}", week.year(), week.week()))
}

/// The `weeks` ISO weeks ending with the one that holds `now`, oldest first.
fn weeks_back(now: i64, weeks: u32) -> Vec<String> {
    let Some(today) = DateTime::from_timestamp(now, 0).map(|t| t.date_naive()) else {
        return Vec::new();
    };
    let monday: NaiveDate = today - Duration::days(today.weekday().num_days_from_monday().into());
    (0..weeks)
        .rev()
        .map(|back| {
            let week = (monday - Duration::weeks(back.into())).iso_week();
            format!("{}-W{:02}", week.year(), week.week())
        })
        .collect()
}

/// A version's numbers, so that 0.10.0 sorts after 0.9.0.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect()
}

/// One period's grades for one rubric.
#[derive(Debug, PartialEq)]
pub struct Cell {
    pub period: String,
    pub rubric: String,
    pub grades: usize,
    pub passed: usize,
}

/// A grade inside the window, with the week it was made in.
struct Kept<'a> {
    entry: &'a Entry,
    week: String,
}

#[derive(Default)]
struct Tally {
    grades: usize,
    passed: usize,
}

/// One ISO week's units that waited for a person, by the week they were merged in.
#[derive(Debug, PartialEq)]
pub struct WaitRow {
    pub period: String,
    pub units: usize,
    pub median_hours: f64,
    pub p90_hours: f64,
}

pub struct Report {
    pub weeks: Vec<Cell>,
    pub waits: Vec<WaitRow>,
    pub versions: Vec<Cell>,
    week_keys: Vec<String>,
    version_keys: Vec<String>,
    rubrics: Vec<String>,
}

/// The pass rates of the grades made in the last `weeks` weeks up to `now`.
pub fn report(entries: &[Entry], now: i64, weeks: u32) -> Report {
    let week_keys = weeks_back(now, weeks);
    let kept: Vec<Kept> = entries
        .iter()
        .filter_map(|entry| {
            Some(Kept {
                entry,
                week: week_of(entry.at)?,
            })
        })
        .filter(|k| week_keys.contains(&k.week))
        .collect();
    let rubrics: Vec<String> = kept
        .iter()
        .map(|k| k.entry.rubric.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut version_keys: Vec<String> = kept
        .iter()
        .map(|k| k.entry.version.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    version_keys.sort_by_key(|v| version_key(v));
    let count = |period_of: &dyn Fn(&Kept) -> String| {
        let mut tally: BTreeMap<(String, String), Tally> = BTreeMap::new();
        for k in &kept {
            let t = tally
                .entry((period_of(k), k.entry.rubric.clone()))
                .or_default();
            t.grades += 1;
            t.passed += usize::from(k.entry.pass);
        }
        tally
    };
    let cells = |keys: &[String], tally: BTreeMap<(String, String), Tally>| {
        keys.iter()
            .flat_map(|k| rubrics.iter().map(move |r| (k, r)))
            .filter_map(|(k, r)| {
                let t = tally.get(&(k.clone(), r.clone()))?;
                Some(Cell {
                    period: k.clone(),
                    rubric: r.clone(),
                    grades: t.grades,
                    passed: t.passed,
                })
            })
            .collect()
    };
    Report {
        weeks: cells(&week_keys, count(&|k| k.week.clone())),
        waits: Vec::new(),
        versions: cells(&version_keys, count(&|k| k.entry.version.clone())),
        week_keys,
        version_keys,
        rubrics,
    }
}

/// The value at fraction `q` of the sorted `values`, between its neighbours when it falls
/// between two.
fn quantile(values: &[f64], q: f64) -> f64 {
    let at = q * (values.len() - 1) as f64;
    let (low, high) = (at.floor() as usize, at.ceil() as usize);
    values[low] + (values[high] - values[low]) * (at - low as f64)
}

/// How long units waited for a person, from the run log's `events`: from a unit's first
/// `not_merged` line to the `merged_at` of its take-over, counted in the ISO week of `merged_at`
/// and only when that is one of the last `weeks` weeks up to `now`. A unit with no `merged_at`
/// is not counted; a week with no unit has no row.
fn waits(events: &[Value], now: i64, weeks: u32) -> Vec<WaitRow> {
    let mut first: BTreeMap<&str, i64> = BTreeMap::new();
    let mut merged: BTreeMap<&str, i64> = BTreeMap::new();
    for e in events {
        let (Some(unit), Some(at)) = (e["unit"].as_str(), e["at"].as_i64()) else {
            continue;
        };
        if e.get("not_merged").is_some() {
            first.entry(unit).or_insert(at);
        }
        if let Some(m) = e["merged_at"].as_i64() {
            merged.insert(unit, m);
        }
    }
    let mut by_week: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (unit, merged_at) in merged {
        let (Some(from), Some(week)) = (first.get(unit), week_of(merged_at)) else {
            continue;
        };
        by_week
            .entry(week)
            .or_default()
            .push((merged_at - from).max(0) as f64 / 3600.0);
    }
    weeks_back(now, weeks)
        .into_iter()
        .filter_map(|period| {
            let mut hours = by_week.remove(&period)?;
            hours.sort_by(f64::total_cmp);
            Some(WaitRow {
                period,
                units: hours.len(),
                median_hours: quantile(&hours, 0.5),
                p90_hours: quantile(&hours, 0.9),
            })
        })
        .collect()
}

impl Report {
    /// Adds the `Human wait` table, from the run log's `events`, for the same weeks.
    pub fn with_waits(mut self, events: &[Value], now: i64, weeks: u32) -> Report {
        self.waits = waits(events, now, weeks);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.weeks.is_empty()
    }

    /// Two tables, the weeks and then the versions, each period a row and each rubric a column
    /// holding the pass rate and, in brackets, the number of grades; `-` where there are none.
    pub fn render(&self) -> String {
        let mut out = format!(
            "{}\n{}",
            table("week", &self.week_keys, &self.rubrics, &self.weeks),
            table("version", &self.version_keys, &self.rubrics, &self.versions)
        );
        if !self.waits.is_empty() {
            out.push_str("\nHuman wait\n");
            out.push_str(&grid(
                std::iter::once(
                    ["week", "units", "median hours", "p90 hours"]
                        .map(String::from)
                        .to_vec(),
                )
                .chain(self.waits.iter().map(|w| {
                    vec![
                        w.period.clone(),
                        w.units.to_string(),
                        format!("{:.1}", w.median_hours),
                        format!("{:.1}", w.p90_hours),
                    ]
                }))
                .collect(),
            ));
        }
        out
    }

    /// One JSON line for each rubric and period that has grades.
    pub fn json_lines(&self) -> Vec<String> {
        let line = |kind: &str, c: &Cell| {
            json!({
                "kind": kind,
                "period": c.period,
                "rubric": c.rubric,
                "grades": c.grades,
                "passed": c.passed,
                "pass_rate": c.passed as f64 / c.grades as f64,
            })
            .to_string()
        };
        let weeks = self.weeks.iter().map(|c| line("week", c));
        let versions = self.versions.iter().map(|c| line("version", c));
        let waits = self.waits.iter().map(|w| {
            json!({
                "kind": "human_wait",
                "period": w.period,
                "units": w.units,
                "median_hours": w.median_hours,
                "p90_hours": w.p90_hours,
            })
            .to_string()
        });
        weeks.chain(versions).chain(waits).collect()
    }
}

fn table(title: &str, keys: &[String], rubrics: &[String], cells: &[Cell]) -> String {
    let mut rows = vec![std::iter::once(title.to_string())
        .chain(rubrics.iter().cloned())
        .collect::<Vec<_>>()];
    for key in keys {
        let mut row = vec![key.clone()];
        for rubric in rubrics {
            row.push(
                cells
                    .iter()
                    .find(|c| &c.period == key && &c.rubric == rubric)
                    .map_or("-".to_string(), |c| {
                        format!(
                            "{:.0}% ({})",
                            100.0 * c.passed as f64 / c.grades as f64,
                            c.grades
                        )
                    }),
            );
        }
        rows.push(row);
    }
    grid(rows)
}

/// `rows` as plain text with each column as wide as its widest cell.
fn grid(rows: Vec<Vec<String>>) -> String {
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Friday 2026-11-06, in ISO week 45.
    const NOW: i64 = 1_794_000_000;

    fn entry(rubric: &str, pass: bool, days_ago: i64, version: &str) -> Entry {
        Entry {
            rubric: rubric.into(),
            pass,
            at: NOW - days_ago * 86_400,
            version: version.into(),
        }
    }

    fn log(unit: &str, at: i64, extra: Value) -> Value {
        let mut e = json!({"unit": unit, "at": at});
        for (k, v) in extra.as_object().unwrap() {
            e[k] = v.clone();
        }
        e
    }

    fn not_merged(unit: &str, at: i64) -> Value {
        log(
            unit,
            at,
            json!({"state": "ship", "not_merged": "a person merges it"}),
        )
    }

    fn took_over(unit: &str, at: i64, merged_at: i64) -> Value {
        log(unit, at, json!({"took_over": 9, "merged_at": merged_at}))
    }

    const H: i64 = 3600;

    #[test]
    fn a_unit_waits_from_its_not_merged_line_to_its_merged_at() {
        let events = [
            not_merged("1", NOW - 10 * H),
            took_over("1", NOW, NOW - 4 * H),
        ];
        assert_eq!(
            waits(&events, NOW, 2),
            [WaitRow {
                period: "2026-W45".into(),
                units: 1,
                median_hours: 6.0,
                p90_hours: 6.0
            }]
        );
    }

    #[test]
    fn the_first_not_merged_line_starts_the_wait() {
        let events = [
            not_merged("1", NOW - 20 * H),
            not_merged("1", NOW - 10 * H),
            took_over("1", NOW, NOW - 5 * H),
        ];
        assert_eq!(waits(&events, NOW, 1)[0].median_hours, 15.0);
    }

    #[test]
    fn a_unit_with_no_merged_at_is_not_counted() {
        let events = [
            not_merged("1", NOW - 10 * H),
            log("1", NOW, json!({"took_over": 9})),
            not_merged("2", NOW - 10 * H),
        ];
        assert_eq!(waits(&events, NOW, 1), []);
    }

    #[test]
    fn the_median_and_90th_percentile_run_over_the_units_of_a_week() {
        let events: Vec<Value> = [1, 2, 3, 4, 10]
            .iter()
            .enumerate()
            .flat_map(|(i, h)| {
                let unit = i.to_string();
                [
                    not_merged(&unit, NOW - 100 * H),
                    took_over(&unit, NOW, NOW - 100 * H + h * H),
                ]
            })
            .collect();
        let w = &waits(&events, NOW, 1)[0];
        assert_eq!((w.units, w.median_hours), (5, 3.0));
        assert!((w.p90_hours - 7.6).abs() < 1e-9, "{}", w.p90_hours);
    }

    #[test]
    fn a_unit_is_counted_in_the_week_it_merged() {
        let events = [
            not_merged("1", NOW - 20 * 86_400),
            took_over("1", NOW, NOW - 7 * 86_400),
        ];
        let rows = waits(&events, NOW, 3);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].period, "2026-W44");
        assert_eq!(rows[0].median_hours, 13.0 * 24.0);
    }

    #[test]
    fn the_human_wait_table_is_printed_and_carried_in_json() {
        let entries = [entry("a", true, 0, "1.0.0")];
        let events = [
            not_merged("1", NOW - 10 * H),
            took_over("1", NOW, NOW - 4 * H),
        ];
        let r = report(&entries, NOW, 1).with_waits(&events, NOW, 1);
        assert!(
            r.render()
                .ends_with("\nHuman wait\nweek      units  median hours  p90 hours\n2026-W45  1      6.0           6.0\n"),
            "{}",
            r.render()
        );
        assert_eq!(
            r.json_lines().last().unwrap(),
            r#"{"kind":"human_wait","median_hours":6.0,"p90_hours":6.0,"period":"2026-W45","units":1}"#
        );
    }

    #[test]
    fn with_no_unit_counted_the_human_wait_table_is_left_out() {
        let entries = [entry("a", true, 0, "1.0.0")];
        let r = report(&entries, NOW, 1).with_waits(&[], NOW, 1);
        assert!(!r.render().contains("Human wait"));
        assert!(r.json_lines().iter().all(|l| !l.contains("human_wait")));
    }

    #[test]
    fn the_weeks_end_with_the_week_of_now() {
        assert_eq!(week_of(NOW).unwrap(), "2026-W45");
        assert_eq!(weeks_back(NOW, 3), ["2026-W43", "2026-W44", "2026-W45"]);
    }

    #[test]
    fn the_weeks_cross_the_new_year() {
        // Friday 2027-01-01 is in week 53 of 2026.
        assert_eq!(
            weeks_back(1_798_800_000, 3),
            ["2026-W51", "2026-W52", "2026-W53"]
        );
    }

    #[test]
    fn versions_sort_by_number() {
        let entries = [entry("a", true, 0, "0.10.0"), entry("a", true, 0, "0.9.0")];
        assert_eq!(report(&entries, NOW, 1).version_keys, ["0.9.0", "0.10.0"]);
    }

    #[test]
    fn a_line_the_retrospective_writes_is_read_back() {
        let grade = Grade {
            rubric: "tests".into(),
            score: 4.0,
            reason: "ok".into(),
        };
        let mut written = line("u1", &grade, true, "abc");
        written["at"] = 100.into();
        assert_eq!(
            parse(&[written]),
            [Entry {
                rubric: "tests".into(),
                pass: true,
                at: 100,
                version: env!("CARGO_PKG_VERSION").into(),
            }]
        );
    }

    #[test]
    fn a_line_that_is_not_a_grade_is_left_out() {
        let events = [
            json!("nonsense"),
            json!({"rubric": "a"}),
            json!({"rubric": "a", "pass": true, "at": 5, "version": "1"}),
        ];
        assert_eq!(parse(&events).len(), 1);
    }

    #[test]
    fn a_grade_older_than_the_window_is_left_out() {
        let entries = [entry("a", true, 30, "1.0.0"), entry("a", false, 0, "1.0.0")];
        let r = report(&entries, NOW, 2);
        assert_eq!(r.weeks.len(), 1);
        assert_eq!(r.weeks[0].grades, 1);
    }
}
