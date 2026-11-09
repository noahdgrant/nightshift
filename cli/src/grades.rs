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

pub struct Report {
    pub weeks: Vec<Cell>,
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
        versions: cells(&version_keys, count(&|k| k.entry.version.clone())),
        week_keys,
        version_keys,
        rubrics,
    }
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.weeks.is_empty()
    }

    /// Two tables, the weeks and then the versions, each period a row and each rubric a column
    /// holding the pass rate and, in brackets, the number of grades; `-` where there are none.
    pub fn render(&self) -> String {
        format!(
            "{}\n{}",
            table("week", &self.week_keys, &self.rubrics, &self.weeks),
            table("version", &self.version_keys, &self.rubrics, &self.versions)
        )
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
        weeks.chain(versions).collect()
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
