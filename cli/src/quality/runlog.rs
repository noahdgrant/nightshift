//! Per-unit review runs, review cost and outcome from `<git-common-dir>/ns/runs.jsonl`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::metrics::RunStats;
use crate::clock;
use crate::stats::round;

#[derive(Debug, Serialize)]
pub struct RunLog {
    pub path: String,
    pub present: bool,
    /// Events that name a unit, on or after `--since`.
    pub events: usize,
    pub bad_lines: usize,
    pub review_runs: u32,
    pub review_cost_usd: f64,
    /// Units with review runs in the log and no review artifact in any worktree.
    pub units_without_artifacts: Vec<String>,
}

/// Whether `ts` is at or after the instant `since`. An undated one never is.
pub fn on_or_after(ts: Option<i64>, since: Option<i64>) -> bool {
    since.is_none_or(|s| ts.is_some_and(|t| t >= s))
}

/// Review runs, review cost, and the last outcome and PR, per unit.
pub fn read(path: &Path, since: Option<i64>) -> (RunLog, BTreeMap<String, RunStats>) {
    let mut log = RunLog {
        path: path.display().to_string(),
        present: path.is_file(),
        events: 0,
        bad_lines: 0,
        review_runs: 0,
        review_cost_usd: 0.0,
        units_without_artifacts: Vec::new(),
    };
    let mut per_unit: BTreeMap<String, RunStats> = BTreeMap::new();
    let text = fs::read_to_string(path).unwrap_or_default();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            log.bad_lines += 1;
            continue;
        };
        let Some(unit) = ev["unit"].as_str() else {
            continue;
        };
        if !on_or_after(ev["ts"].as_str().and_then(clock::parse_iso), since) {
            continue;
        }
        log.events += 1;
        let s = per_unit.entry(unit.to_string()).or_default();
        match (ev["event"].as_str(), ev["phase"].as_str()) {
            (Some("phase"), Some("review")) => {
                let cost = ev["cost_usd"].as_f64().unwrap_or(0.0);
                s.review_runs += 1;
                s.review_cost_usd += cost;
                log.review_runs += 1;
                log.review_cost_usd += cost;
            }
            (Some("end"), _) => {
                s.outcome = ev["outcome"].as_str().map(String::from);
                s.pr = ev["pr"].as_u64().or(s.pr);
            }
            (Some("merged"), _) => s.pr = ev["pr"].as_u64().or(s.pr),
            _ => {}
        }
    }
    log.review_cost_usd = round(log.review_cost_usd, 2);
    for s in per_unit.values_mut() {
        s.review_cost_usd = round(s.review_cost_usd, 2);
    }
    (log, per_unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_are_counted_per_unit() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("runs.jsonl");
        assert!(!read(&p, None).0.present);
        fs::write(
            &p,
            r#"{"event":"phase","phase":"review","cost_usd":1.25,"unit":"a","ts":"2026-10-08T10:00:00Z"}
{"event":"phase","phase":"review","cost_usd":2.5,"unit":"a","ts":"2026-10-09T10:00:00Z"}
{"event":"phase","phase":"build","cost_usd":9,"unit":"a","ts":"2026-10-09T10:00:00Z"}
{"event":"merged","pr":12,"unit":"a","ts":"2026-10-09T11:00:00Z"}
{"event":"end","outcome":"merged","unit":"a","ts":"2026-10-09T11:00:00Z"}
{"event":"end","outcome":"stuck","pr":7,"unit":"b","ts":"2026-10-09T11:00:00Z"}
{"event":"phase","phase":"review","cost_usd":0.1,"unit":"c","ts":"2026-10-09T10:00:00Z"}
{"event":"phase","phase":"review","cost_usd":0.2,"unit":"c","ts":"2026-10-09T10:00:00Z"}
{"event":"start","ts":"2026-10-09T11:00:00Z"}
not json

"#,
        )
        .unwrap();
        let (log, runs) = read(&p, None);
        assert!(log.present);
        assert_eq!(
            (
                log.events,
                log.bad_lines,
                log.review_runs,
                log.review_cost_usd
            ),
            (8, 1, 4, 4.05)
        );
        assert_eq!(runs["a"].review_runs, 2);
        assert_eq!(runs["a"].outcome.as_deref(), Some("merged"));
        assert_eq!(runs["a"].pr, Some(12));
        assert_eq!(runs["b"].pr, Some(7));
        assert_eq!(runs["b"].outcome.as_deref(), Some("stuck"));
        assert_eq!(runs["c"].review_cost_usd, 0.3);
    }

    #[test]
    fn undated_events_are_dropped_only_with_since() {
        assert!(on_or_after(None, None));
        assert!(on_or_after(Some(0), None));
        assert!(!on_or_after(None, Some(0)));
    }

    #[test]
    fn since_cuts_off_at_the_instant() {
        let since = clock::parse_iso("2026-10-10T00:00:00Z");
        assert!(on_or_after(clock::parse_iso("2026-10-10T02:43:00Z"), since));
        assert!(on_or_after(since, since));
        assert!(!on_or_after(
            clock::parse_iso("2026-10-09T23:59:59Z"),
            since
        ));
    }
}
