//! Which merged units are worth keeping as review cases, from the run log and the grades.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::grades::{week_of, weeks_back, Entry};

/// A merged unit with at least one signal, and the reason for each.
#[derive(Debug, PartialEq, Eq)]
pub struct Candidate {
    pub unit: String,
    pub reasons: Vec<String>,
}

/// The units merged in the last `weeks` ISO weeks up to `now` that review sent back to `build`
/// at least `rounds` times, that were `stuck` at some point, or that have a grade that did not
/// pass. A unit with more reasons comes first; ties go by unit number. A unit with no `merged_at`
/// is not merged and is left out.
pub fn suggest(
    events: &[Value],
    entries: &[Entry],
    now: i64,
    weeks: u32,
    rounds: u32,
) -> Vec<Candidate> {
    let keys = weeks_back(now, weeks);
    let mut merged: BTreeSet<&str> = BTreeSet::new();
    let mut rounds_by: BTreeMap<&str, u32> = BTreeMap::new();
    let mut stuck_in: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for e in events {
        let Some(unit) = e["unit"].as_str() else {
            continue;
        };
        if e["merged_at"]
            .as_i64()
            .and_then(week_of)
            .is_some_and(|w| keys.contains(&w))
        {
            merged.insert(unit);
        }
        match (e["from"].as_str(), e["to"].as_str()) {
            (Some("review"), Some("build")) => *rounds_by.entry(unit).or_default() += 1,
            (from, Some("stuck")) => {
                let at = from.map_or("was stuck".to_string(), |s| format!("was stuck in {s}"));
                let list = stuck_in.entry(unit).or_default();
                if !list.contains(&at) {
                    list.push(at);
                }
            }
            _ => {}
        }
    }
    let mut out: Vec<Candidate> = merged
        .into_iter()
        .filter_map(|unit| {
            let mut reasons = Vec::new();
            let n = rounds_by.get(unit).copied().unwrap_or(0);
            if n >= rounds {
                reasons.push(format!("went round review {n} times"));
            }
            reasons.extend(stuck_in.remove(unit).unwrap_or_default());
            let failed: BTreeSet<&str> = entries
                .iter()
                .filter(|g| g.unit == unit && !g.pass)
                .map(|g| g.rubric.as_str())
                .collect();
            reasons.extend(failed.iter().map(|r| format!("grade `{r}` did not pass")));
            (!reasons.is_empty()).then(|| Candidate {
                unit: unit.to_string(),
                reasons,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.reasons.len().cmp(&a.reasons.len()).then_with(|| {
            let n = |c: &Candidate| c.unit.parse::<u64>().unwrap_or(u64::MAX);
            n(a).cmp(&n(b))
        })
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Friday 2026-11-06, in ISO week 45.
    const NOW: i64 = 1_793_966_400;
    const DAY: i64 = 86_400;

    fn merged(unit: &str, ago_days: i64) -> Value {
        json!({"unit": unit, "from": "ship", "to": "done", "merged_at": NOW - ago_days * DAY, "at": NOW})
    }
    fn mv(unit: &str, from: &str, to: &str) -> Value {
        json!({"unit": unit, "from": from, "to": to, "at": NOW})
    }
    fn grade(unit: &str, rubric: &str, pass: bool) -> Entry {
        Entry {
            unit: unit.into(),
            rubric: rubric.into(),
            pass,
            at: NOW,
            version: "1.0.0".into(),
            sessions: Vec::new(),
        }
    }
    fn reasons(c: &[Candidate]) -> Vec<(&str, Vec<&str>)> {
        c.iter()
            .map(|c| {
                (
                    c.unit.as_str(),
                    c.reasons.iter().map(String::as_str).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn review_sending_a_unit_back_to_build_enough_times_is_a_signal() {
        let mut ev = vec![merged("1", 0)];
        ev.extend((0..3).map(|_| mv("1", "review", "build")));
        ev.push(merged("2", 0));
        ev.extend((0..1).map(|_| mv("2", "review", "build")));
        ev.extend((0..2).map(|_| mv("2", "verify", "build")));
        let got = suggest(&ev, &[], NOW, 8, 2);
        assert_eq!(reasons(&got), [("1", vec!["went round review 3 times"])]);
    }

    #[test]
    fn the_rounds_threshold_is_a_flag() {
        let mut ev = vec![merged("1", 0)];
        ev.extend((0..3).map(|_| mv("1", "review", "build")));
        assert_eq!(suggest(&ev, &[], NOW, 8, 4), []);
        assert_eq!(suggest(&ev, &[], NOW, 8, 3).len(), 1);
    }

    #[test]
    fn a_unit_that_was_stuck_is_a_signal_naming_the_state() {
        let ev = vec![
            merged("1", 0),
            mv("1", "verify", "stuck"),
            mv("1", "verify", "stuck"),
        ];
        let got = suggest(&ev, &[], NOW, 8, 2);
        assert_eq!(reasons(&got), [("1", vec!["was stuck in verify"])]);
    }

    #[test]
    fn a_grade_that_did_not_pass_is_a_signal_naming_the_rubric() {
        let entries = [
            grade("1", "scope", false),
            grade("1", "tone", true),
            grade("2", "scope", true),
        ];
        let ev = vec![merged("1", 0), merged("2", 0)];
        let got = suggest(&ev, &entries, NOW, 8, 2);
        assert_eq!(reasons(&got), [("1", vec!["grade `scope` did not pass"])]);
    }

    #[test]
    fn a_unit_with_two_signals_comes_before_one_with_one() {
        let ev = vec![
            merged("1", 0),
            mv("1", "build", "stuck"),
            merged("2", 0),
            mv("2", "verify", "stuck"),
        ];
        let entries = [grade("2", "scope", false)];
        let got = suggest(&ev, &entries, NOW, 8, 2);
        assert_eq!(
            reasons(&got),
            [
                (
                    "2",
                    vec!["was stuck in verify", "grade `scope` did not pass"]
                ),
                ("1", vec!["was stuck in build"]),
            ]
        );
    }

    #[test]
    fn a_unit_merged_outside_the_weeks_or_not_merged_is_left_out() {
        let ev = vec![
            merged("1", 70),
            mv("1", "verify", "stuck"),
            mv("2", "verify", "stuck"),
        ];
        assert_eq!(suggest(&ev, &[], NOW, 2, 2), []);
        assert_eq!(suggest(&ev, &[], NOW, 8, 2), []);
        assert_eq!(suggest(&ev, &[], NOW, 12, 2).len(), 1);
    }

    #[test]
    fn a_merged_unit_with_no_signal_is_left_out() {
        let ev = vec![merged("1", 0), mv("1", "review", "build")];
        assert_eq!(suggest(&ev, &[grade("1", "scope", true)], NOW, 8, 2), []);
    }
}
