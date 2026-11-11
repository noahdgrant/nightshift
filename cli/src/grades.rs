//! The pass rate of each rubric over time, from the grades in `grades.jsonl`.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, Duration, NaiveDate};
use serde_json::{json, Value};

use crate::harness::{Harness, Tokens, Usage};
use crate::rubric::Grade;

/// What one kept session of a unit used: the state it ran for and what its transcript says of
/// its model and tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionUse {
    pub state: String,
    pub usage: Usage,
}

impl SessionUse {
    fn to_json(&self) -> Value {
        json!({
            "state": self.state,
            "model": self.usage.model,
            "tokens": self.usage.tokens.map(|t| json!({
                "input": t.input,
                "output": t.output,
                "cache_read": t.cache_read,
                "cache_write": t.cache_write,
            })),
        })
    }

    fn from_json(v: &Value) -> Option<SessionUse> {
        let n = |t: &Value, key: &str| t[key].as_u64();
        let tokens = v["tokens"].is_object().then(|| {
            let t = &v["tokens"];
            Some(Tokens {
                input: n(t, "input")?,
                output: n(t, "output")?,
                cache_read: n(t, "cache_read")?,
                cache_write: n(t, "cache_write")?,
            })
        });
        Some(SessionUse {
            state: v["state"].as_str()?.to_string(),
            usage: Usage {
                model: v["model"].as_str().map(String::from),
                tokens: tokens.flatten(),
            },
        })
    }
}

/// `sessions` with an entry for the retrospective, no data, added when its transcript was not
/// among them: its grade lines are written before anything else could say what it used.
pub fn with_retrospective(mut sessions: Vec<SessionUse>) -> Vec<SessionUse> {
    if !sessions.iter().any(|s| s.state == "retrospective") {
        sessions.push(SessionUse {
            state: "retrospective".into(),
            usage: Usage::default(),
        });
    }
    sessions
}

/// One grade, as much of it as the tables need.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub unit: String,
    pub rubric: String,
    pub pass: bool,
    /// Seconds since the epoch.
    pub at: i64,
    pub version: String,
    /// The sessions the grade was made from; none for a line written before they were recorded.
    pub sessions: Vec<SessionUse>,
}

impl Entry {
    /// The model that did the unit's `build`: the last `build` session's. `None` when it names
    /// none or the unit has no `build` session.
    fn build_model(&self) -> Option<&str> {
        self.sessions
            .iter()
            .rfind(|s| s.state == "build")?
            .usage
            .model
            .as_deref()
    }
}

/// The line the retrospective appends for one grade; `parse` reads the same field names back.
/// The run log adds `at` when it appends the line.
pub fn line(unit: &str, grade: &Grade, pass: bool, base: &str, sessions: &[SessionUse]) -> Value {
    json!({
        "unit": unit,
        "rubric": grade.rubric,
        "score": grade.score,
        "pass": pass,
        "reason": grade.reason,
        "version": env!("CARGO_PKG_VERSION"),
        "base": base,
        "sessions": sessions.iter().map(SessionUse::to_json).collect::<Vec<_>>(),
    })
}

/// The grades among `events`; an event that is not a whole grade is left out.
pub fn parse(events: &[Value]) -> Vec<Entry> {
    events
        .iter()
        .filter_map(|v| {
            Some(Entry {
                unit: v["unit"].as_str()?.to_string(),
                rubric: v["rubric"].as_str()?.to_string(),
                pass: v["pass"].as_bool()?,
                at: v["at"].as_i64()?,
                version: v["version"].as_str()?.to_string(),
                sessions: v["sessions"]
                    .as_array()
                    .map(|a| a.iter().filter_map(SessionUse::from_json).collect())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// The ISO week of `at`, as `2026-W45`.
pub fn week_of(at: i64) -> Option<String> {
    let week = DateTime::from_timestamp(at, 0)?.iso_week();
    Some(format!("{}-W{:02}", week.year(), week.week()))
}

/// The `weeks` ISO weeks ending with the one that holds `now`, oldest first.
pub fn weeks_back(now: i64, weeks: u32) -> Vec<String> {
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

/// One ISO week's merged units, by the week they were merged in: how long they took from their
/// first move to `merged_at` and the input plus output tokens of their sessions.
#[derive(Debug, PartialEq)]
pub struct CycleRow {
    pub period: String,
    pub units: usize,
    pub median_hours: f64,
    pub p90_hours: f64,
    /// `None` when no unit of the week has token data.
    pub median_tokens: Option<f64>,
    pub p90_tokens: Option<f64>,
    /// The units left out of the token figures, for want of token data.
    pub no_tokens: usize,
    /// The percentage of the week's tokens the `retrospective` sessions took, over the units with
    /// token data; `None` when no unit has any.
    pub retro_share: Option<f64>,
    /// Whether `retro_share` is above the level.
    pub above_level: bool,
}

/// The tokens spent in one ISO week on units that never merged, by the week of their last move.
#[derive(Debug, PartialEq)]
pub struct UnmergedRow {
    pub period: String,
    pub units: usize,
    pub tokens: u64,
}

pub struct Report {
    pub weeks: Vec<Cell>,
    pub waits: Vec<WaitRow>,
    pub cycles: Vec<CycleRow>,
    pub unmerged: Vec<UnmergedRow>,
    /// The level, in percent, above which a week's retrospective share is marked.
    pub retro_level: f64,
    pub versions: Vec<Cell>,
    /// The grades by the model that did the unit's `build`, each model as a `period`.
    pub models: Vec<Cell>,
    /// How many units the model split left out, for want of a `build` model.
    pub unmodelled: usize,
    week_keys: Vec<String>,
    version_keys: Vec<String>,
    model_keys: Vec<String>,
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
    let model_keys: Vec<String> = kept
        .iter()
        .filter_map(|k| k.entry.build_model().map(String::from))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let unmodelled = kept
        .iter()
        .filter(|k| k.entry.build_model().is_none())
        .map(|k| k.entry.unit.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let count = |period_of: &dyn Fn(&Kept) -> Option<String>| {
        let mut tally: BTreeMap<(String, String), Tally> = BTreeMap::new();
        for k in &kept {
            let Some(period) = period_of(k) else { continue };
            let t = tally.entry((period, k.entry.rubric.clone())).or_default();
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
        weeks: cells(&week_keys, count(&|k| Some(k.week.clone()))),
        waits: Vec::new(),
        cycles: Vec::new(),
        unmerged: Vec::new(),
        retro_level: 0.0,
        versions: cells(&version_keys, count(&|k| Some(k.entry.version.clone()))),
        models: cells(
            &model_keys,
            count(&|k| k.entry.build_model().map(String::from)),
        ),
        unmodelled,
        week_keys,
        version_keys,
        model_keys,
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

/// The input plus output tokens of the sessions of a grade line; `None` when no session has
/// token data. Cache reads and writes are not counted.
fn entry_tokens(sessions: &[SessionUse]) -> Option<u64> {
    sessions
        .iter()
        .filter_map(|s| s.usage.tokens)
        .map(|t| t.billable())
        .reduce(|a, b| a + b)
}

/// The tokens of `unit`, input plus output, over the sessions of its latest grade line; `None`
/// when it has no grade line or none of its sessions says its tokens.
pub fn unit_tokens(entries: &[Entry], unit: &str) -> Option<u64> {
    let latest = entries
        .iter()
        .filter(|e| e.unit == unit)
        .max_by_key(|e| e.at)?;
    entry_tokens(&latest.sessions)
}

/// The commit the last grade line of `unit` in `events` names as its `base`. `None` when the
/// unit has no grade line or its lines name none.
pub fn unit_base(events: &[Value], unit: &str) -> Option<String> {
    events
        .iter()
        .rfind(|v| v["unit"].as_str() == Some(unit) && v["rubric"].is_string())
        .and_then(|v| v["base"].as_str())
        .filter(|b| !b.is_empty())
        .map(String::from)
}

/// The input plus output tokens of the `retrospective` sessions of a grade line; 0 for none.
fn retro_tokens(sessions: &[SessionUse]) -> u64 {
    sessions
        .iter()
        .filter(|s| s.state == "retrospective")
        .filter_map(|s| s.usage.tokens)
        .map(|t| t.billable())
        .sum()
}

/// The tokens of a unit's kept session `transcripts`, counted as a grade line's are; `None` when
/// none of them says its tokens.
pub fn kept_tokens(harness: &dyn Harness, transcripts: &[String]) -> Option<u64> {
    let sessions: Vec<SessionUse> = transcripts
        .iter()
        .map(|t| SessionUse {
            state: String::new(),
            usage: harness.transcript_usage(t),
        })
        .collect();
    entry_tokens(&sessions)
}

/// Cycle time and tokens of the units merged in the last `weeks` weeks up to `now`, and the
/// tokens of units that never merged. A unit's cycle runs from its first move in `events` to its
/// `merged_at`; a unit with no `merged_at` is not merged. A merged unit's tokens are those of its
/// latest grade line. A unit never merged is one with no `merged_at` whose last move is into
/// `stuck` (a closed change request ends there too); `kept` gives its tokens from the transcripts
/// still kept.
fn cycles(
    events: &[Value],
    entries: &[Entry],
    kept: &dyn Fn(&str) -> Option<u64>,
    now: i64,
    weeks: u32,
    level: f64,
) -> (Vec<CycleRow>, Vec<UnmergedRow>) {
    let mut first: BTreeMap<&str, i64> = BTreeMap::new();
    let mut last: BTreeMap<&str, (i64, bool)> = BTreeMap::new();
    let mut merged: BTreeMap<&str, i64> = BTreeMap::new();
    for e in events {
        let (Some(unit), Some(at)) = (e["unit"].as_str(), e["at"].as_i64()) else {
            continue;
        };
        if let Some(to) = e["to"].as_str() {
            first.entry(unit).or_insert(at);
            last.insert(unit, (at, to == "stuck"));
        }
        if let Some(m) = e["merged_at"].as_i64() {
            merged.insert(unit, m);
        }
    }
    /// A unit's latest grade line: when it was written, the tokens of all its
    /// sessions (`None` without token data) and those of the retrospective.
    struct GradedUnit {
        at: i64,
        tokens: Option<u64>,
        retro_tokens: u64,
    }
    /// A unit merged in a week: hours from first state to merge, and its tokens.
    struct MergedUnit {
        hours: f64,
        tokens: Option<u64>,
        retro_tokens: u64,
    }
    let mut graded: BTreeMap<&str, GradedUnit> = BTreeMap::new();
    for entry in entries {
        let latest = graded.entry(entry.unit.as_str()).or_insert(GradedUnit {
            at: i64::MIN,
            tokens: None,
            retro_tokens: 0,
        });
        if entry.at >= latest.at {
            *latest = GradedUnit {
                at: entry.at,
                tokens: entry_tokens(&entry.sessions),
                retro_tokens: retro_tokens(&entry.sessions),
            };
        }
    }
    let keys = weeks_back(now, weeks);
    // Per week: each merged unit's hours, and its tokens where it has them.
    let mut by_week: BTreeMap<String, Vec<MergedUnit>> = BTreeMap::new();
    for (unit, merged_at) in &merged {
        let (Some(from), Some(week)) = (first.get(unit), week_of(*merged_at)) else {
            continue;
        };
        let (tokens, retro_tokens) = graded
            .get(unit)
            .map_or((None, 0), |g| (g.tokens, g.retro_tokens));
        by_week.entry(week).or_default().push(MergedUnit {
            hours: (merged_at - from).max(0) as f64 / 3600.0,
            tokens,
            retro_tokens,
        });
    }
    let rows = keys
        .iter()
        .filter_map(|period| {
            let units = by_week.remove(period)?;
            let mut hours: Vec<f64> = units.iter().map(|u| u.hours).collect();
            let mut tokens: Vec<f64> = units
                .iter()
                .filter_map(|u| u.tokens)
                .map(|t| t as f64)
                .collect();
            hours.sort_by(f64::total_cmp);
            tokens.sort_by(f64::total_cmp);
            // One share for the week, over the units that have token data.
            let all: u64 = units.iter().filter_map(|u| u.tokens).sum();
            let retro: u64 = units
                .iter()
                .filter(|u| u.tokens.is_some())
                .map(|u| u.retro_tokens)
                .sum();
            let retro_share = (all > 0).then(|| 100.0 * retro as f64 / all as f64);
            Some(CycleRow {
                period: period.clone(),
                units: units.len(),
                median_hours: quantile(&hours, 0.5),
                p90_hours: quantile(&hours, 0.9),
                median_tokens: (!tokens.is_empty()).then(|| quantile(&tokens, 0.5)),
                p90_tokens: (!tokens.is_empty()).then(|| quantile(&tokens, 0.9)),
                no_tokens: units.len() - tokens.len(),
                retro_share,
                above_level: retro_share.is_some_and(|s| s > level),
            })
        })
        .collect();
    let mut unmerged_by_week: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for (unit, (at, stuck)) in &last {
        if !stuck || merged.contains_key(unit) {
            continue;
        }
        let (Some(week), Some(tokens)) = (week_of(*at), kept(unit)) else {
            continue;
        };
        let t = unmerged_by_week.entry(week).or_default();
        t.0 += 1;
        t.1 += tokens;
    }
    let unmerged = keys
        .into_iter()
        .filter_map(|period| {
            let (units, tokens) = unmerged_by_week.remove(&period)?;
            Some(UnmergedRow {
                period,
                units,
                tokens,
            })
        })
        .collect();
    (rows, unmerged)
}

impl Report {
    /// Adds the `Human wait` table, from the run log's `events`, for the same weeks.
    pub fn with_waits(mut self, events: &[Value], now: i64, weeks: u32) -> Report {
        self.waits = waits(events, now, weeks);
        self
    }

    /// Adds the `Cycle time and tokens` table, from the run log's `events` and the grade
    /// `entries`, for the same weeks. `kept` returns the tokens of the transcripts still kept for
    /// a unit, `None` when there are none. A week whose retrospective share is above `retro_level`
    /// percent is marked.
    pub fn with_cycles(
        mut self,
        events: &[Value],
        entries: &[Entry],
        kept: &dyn Fn(&str) -> Option<u64>,
        now: i64,
        weeks: u32,
        retro_level: f64,
    ) -> Report {
        self.retro_level = retro_level;
        (self.cycles, self.unmerged) = cycles(events, entries, kept, now, weeks, retro_level);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.weeks.is_empty()
    }

    /// Two tables, the weeks and then the versions, and a third by `build` model when any unit
    /// names one. Each period or model is a row and each rubric a column holding the pass rate
    /// and, in brackets, the number of grades; `-` where there are none.
    pub fn render(&self) -> String {
        let mut out = format!(
            "{}\n{}",
            table("week", &self.week_keys, &self.rubrics, &self.weeks),
            table("version", &self.version_keys, &self.rubrics, &self.versions)
        );
        if !self.models.is_empty() {
            out.push('\n');
            out.push_str(&table(
                "model",
                &self.model_keys,
                &self.rubrics,
                &self.models,
            ));
            if self.unmodelled > 0 {
                out.push_str(&format!(
                    "{} units left out: their build session names no model\n",
                    self.unmodelled
                ));
            }
        }
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
        if !self.cycles.is_empty() {
            let count = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.0}"));
            out.push_str("\nCycle time and tokens\n");
            out.push_str(&grid(
                std::iter::once(
                    [
                        "week",
                        "units",
                        "median hours",
                        "p90 hours",
                        "median tokens",
                        "p90 tokens",
                        "retro share",
                    ]
                    .map(String::from)
                    .to_vec(),
                )
                .chain(self.cycles.iter().map(|c| {
                    vec![
                        c.period.clone(),
                        c.units.to_string(),
                        format!("{:.1}", c.median_hours),
                        format!("{:.1}", c.p90_hours),
                        count(c.median_tokens),
                        count(c.p90_tokens),
                        c.retro_share.map_or("-".to_string(), |s| {
                            format!("{s:.0}%{}", if c.above_level { " *" } else { "" })
                        }),
                    ]
                }))
                .collect(),
            ));
            out.push_str(&format!(
                "* the retrospective's share of tokens is above the level of {}%\n",
                self.retro_level
            ));
            let no_tokens: usize = self.cycles.iter().map(|c| c.no_tokens).sum();
            if no_tokens > 0 {
                out.push_str(&format!(
                    "{no_tokens} units left out of the token figures: no token data\n"
                ));
            }
        }
        for u in &self.unmerged {
            out.push_str(&format!(
                "{}  never merged: {} units, {} tokens (kept transcripts only)\n",
                u.period, u.units, u.tokens
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
        let models = self.models.iter().map(|c| line("model", c));
        let left_out = (!self.models.is_empty())
            .then(|| json!({"kind": "model_left_out", "units": self.unmodelled}).to_string());
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
        let cycles = self.cycles.iter().map(|c| {
            json!({
                "kind": "cycle",
                "period": c.period,
                "units": c.units,
                "median_hours": c.median_hours,
                "p90_hours": c.p90_hours,
                "median_tokens": c.median_tokens,
                "p90_tokens": c.p90_tokens,
                "no_tokens_units": c.no_tokens,
                "retro_share": c.retro_share,
                "retro_level": self.retro_level,
                "above_level": c.above_level,
            })
            .to_string()
        });
        let unmerged = self.unmerged.iter().map(|u| {
            json!({
                "kind": "unmerged_tokens",
                "period": u.period,
                "units": u.units,
                "tokens": u.tokens,
            })
            .to_string()
        });
        weeks
            .chain(versions)
            .chain(models)
            .chain(left_out)
            .chain(waits)
            .chain(cycles)
            .chain(unmerged)
            .collect()
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
            unit: "1".into(),
            rubric: rubric.into(),
            pass,
            at: NOW - days_ago * 86_400,
            version: version.into(),
            sessions: Vec::new(),
        }
    }

    fn used(state: &str, model: Option<&str>, tokens: Option<Tokens>) -> SessionUse {
        SessionUse {
            state: state.into(),
            usage: Usage {
                model: model.map(String::from),
                tokens,
            },
        }
    }

    #[test]
    fn a_tokens_object_missing_a_total_reads_as_no_tokens() {
        let v: Value =
            serde_json::from_str(r#"{"state":"build","model":"opus","tokens":{"input":1}}"#)
                .unwrap();
        assert_eq!(
            SessionUse::from_json(&v),
            Some(used("build", Some("opus"), None))
        );
    }

    /// A grade of unit `unit` whose sessions are `sessions`.
    fn built_by(unit: &str, pass: bool, sessions: Vec<SessionUse>) -> Entry {
        Entry {
            unit: unit.into(),
            sessions,
            ..entry("a", pass, 0, "1.0.0")
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

    fn moved(unit: &str, at: i64, to: &str) -> Value {
        log(unit, at, json!({"from": "build", "to": to}))
    }

    fn with_tokens(unit: &str, input: u64, output: u64) -> Entry {
        built_by(
            unit,
            true,
            vec![SessionUse {
                state: "build".into(),
                usage: Usage {
                    model: None,
                    tokens: Some(Tokens {
                        input,
                        output,
                        cache_read: 9_999,
                        cache_write: 9_999,
                    }),
                },
            }],
        )
    }

    fn no_kept(_: &str) -> Option<u64> {
        None
    }

    fn cycle_report(
        events: &[Value],
        entries: &[Entry],
        kept: &dyn Fn(&str) -> Option<u64>,
    ) -> Report {
        report(&[entry("a", true, 0, "1.0.0")], NOW, 1)
            .with_cycles(events, entries, kept, NOW, 1, 25.0)
    }

    #[test]
    fn cycle_time_and_tokens_run_from_the_first_move_to_merged_at_over_the_units_of_a_week() {
        let mut events = Vec::new();
        let mut entries = Vec::new();
        for (unit, hours, tokens) in [
            ("1", 2, 100),
            ("2", 4, 200),
            ("3", 6, 300),
            ("4", 8, 400),
            ("5", 10, 500),
        ] {
            events.push(moved(unit, NOW - 20 * H, "build"));
            events.push(moved(unit, NOW - 19 * H, "verify"));
            events.push(took_over(unit, NOW, NOW - 20 * H + hours * H));
            entries.push(with_tokens(unit, tokens - 10, 10));
        }
        let r = cycle_report(&events, &entries, &no_kept);
        assert_eq!(
            r.cycles,
            vec![CycleRow {
                period: "2026-W45".into(),
                units: 5,
                median_hours: 6.0,
                p90_hours: 9.2,
                median_tokens: Some(300.0),
                p90_tokens: Some(460.0),
                no_tokens: 0,
                retro_share: Some(0.0),
                above_level: false,
            }]
        );
        assert!(
            r.render().ends_with("\nCycle time and tokens\nweek      units  median hours  p90 hours  median tokens  p90 tokens  retro share\n2026-W45  5      6.0           9.2        300            460         0%\n* the retrospective's share of tokens is above the level of 25%\n"),
            "{}",
            r.render()
        );
        assert_eq!(
            r.json_lines().last().unwrap(),
            r#"{"above_level":false,"kind":"cycle","median_hours":6.0,"median_tokens":300.0,"no_tokens_units":0,"p90_hours":9.2,"p90_tokens":460.0,"period":"2026-W45","retro_level":25.0,"retro_share":0.0,"units":5}"#
        );
    }

    fn tok(input: u64, output: u64) -> Option<Tokens> {
        Some(Tokens {
            input,
            output,
            cache_read: 9_999,
            cache_write: 9_999,
        })
    }

    /// A unit merged at `NOW` whose build took `build` tokens and retrospective `retro`.
    fn graded_unit(unit: &str, build: u64, retro: u64) -> (Vec<Value>, Entry) {
        (
            vec![
                moved(unit, NOW - 5 * H, "build"),
                took_over(unit, NOW, NOW - H),
            ],
            built_by(
                unit,
                true,
                vec![
                    used("build", None, tok(build, 0)),
                    used("retrospective", None, tok(retro, 0)),
                ],
            ),
        )
    }

    #[test]
    fn the_retrospective_share_is_one_share_for_the_week_over_its_units() {
        // 10 of 100, 40 of 200 and 50 of 100: 100 of 400 is 25%, not the mean of 10, 20 and 50.
        let (mut events, mut entries) = (Vec::new(), Vec::new());
        for (unit, build, retro) in [("1", 90, 10), ("2", 160, 40), ("3", 50, 50)] {
            let (e, g) = graded_unit(unit, build, retro);
            events.extend(e);
            entries.push(g);
        }
        let r = cycle_report(&events, &entries, &no_kept);
        assert_eq!(r.cycles[0].retro_share, Some(25.0));
        assert!(!r.cycles[0].above_level, "25% is not above 25%");
    }

    #[test]
    fn a_week_above_the_level_is_marked_and_one_below_is_not() {
        let w = 7 * 24 * H;
        let (mut events, mut entries) = (Vec::new(), Vec::new());
        // This week 30 of 100; the week before 20 of 100.
        for (unit, ago, build, retro) in [("1", 0, 70, 30), ("2", w, 80, 20)] {
            events.push(moved(unit, NOW - ago - 5 * H, "build"));
            events.push(took_over(unit, NOW - ago, NOW - ago - H));
            entries.push(built_by(
                unit,
                true,
                vec![
                    used("build", None, tok(build, 0)),
                    used("retrospective", None, tok(retro, 0)),
                ],
            ));
        }
        let r = report(&[entry("a", true, 0, "1.0.0")], NOW, 2)
            .with_cycles(&events, &entries, &no_kept, NOW, 2, 25.0);
        assert_eq!(r.cycles[0].retro_share, Some(20.0));
        assert!(!r.cycles[0].above_level);
        assert_eq!(r.cycles[1].retro_share, Some(30.0));
        assert!(r.cycles[1].above_level);
        let out = r.render();
        assert!(out.contains("100            100         20%\n"), "{out}");
        assert!(out.contains("100            100         30% *\n"), "{out}");
        assert!(
            out.contains("* the retrospective's share of tokens is above the level of 25%\n"),
            "{out}"
        );
        let json = r.json_lines();
        assert!(
            json[json.len() - 1].contains(r#""above_level":true"#)
                && json[json.len() - 1].contains(r#""retro_level":25.0"#)
                && json[json.len() - 1].contains(r#""retro_share":30.0"#),
            "{json:?}"
        );
    }

    #[test]
    fn a_unit_with_no_token_data_is_left_out_of_the_retrospective_share() {
        let (mut events, mut entries) = (Vec::new(), Vec::new());
        let (e, g) = graded_unit("1", 90, 10);
        events.extend(e);
        entries.push(g);
        // Unit 2 has no token data at all; its retrospective would swing the share.
        events.extend([
            moved("2", NOW - 5 * H, "build"),
            took_over("2", NOW, NOW - H),
        ]);
        entries.push(built_by(
            "2",
            true,
            vec![used("build", None, None), used("retrospective", None, None)],
        ));
        let r = cycle_report(&events, &entries, &no_kept);
        assert_eq!(r.cycles[0].retro_share, Some(10.0));
    }

    #[test]
    fn a_unit_whose_retrospective_has_no_tokens_counts_none_of_them_as_retrospective() {
        let (events, g) = graded_unit("1", 100, 0);
        let g = Entry {
            sessions: vec![
                used("build", None, tok(100, 0)),
                used("retrospective", None, None),
            ],
            ..g
        };
        let r = cycle_report(&events, &[g], &no_kept);
        assert_eq!(r.cycles[0].retro_share, Some(0.0));
    }

    #[test]
    fn a_unit_with_no_merged_at_is_not_in_the_cycle_table() {
        let events = [
            moved("1", NOW - 5 * H, "build"),
            moved("1", NOW - H, "ship"),
        ];
        let r = cycle_report(&events, &[with_tokens("1", 1, 1)], &no_kept);
        assert_eq!(r.cycles, vec![]);
        assert!(!r.render().contains("Cycle time"));
    }

    #[test]
    fn a_unit_with_no_token_data_is_left_out_of_the_token_figures_only_and_counted() {
        let events = [
            moved("1", NOW - 5 * H, "build"),
            took_over("1", NOW, NOW - H),
            moved("2", NOW - 9 * H, "build"),
            took_over("2", NOW, NOW - H),
            moved("3", NOW - 9 * H, "build"),
            took_over("3", NOW, NOW - H),
        ];
        // Unit 2's session has no usage; unit 3 was never graded.
        let silent = built_by("2", true, vec![used("build", None, None)]);
        let entries = [with_tokens("1", 70, 30), silent];
        let r = cycle_report(&events, &entries, &no_kept);
        assert_eq!(r.cycles[0].units, 3);
        assert_eq!(r.cycles[0].median_hours, 8.0);
        assert_eq!(r.cycles[0].median_tokens, Some(100.0));
        assert_eq!(r.cycles[0].no_tokens, 2);
        assert!(
            r.render()
                .contains("2 units left out of the token figures: no token data\n"),
            "{}",
            r.render()
        );
    }

    #[test]
    fn units_that_never_merged_are_a_separate_line_of_kept_tokens() {
        let events = [
            moved("1", NOW - 5 * H, "build"),
            moved("1", NOW - H, "stuck"),
            moved("2", NOW - 5 * H, "build"),
            moved("2", NOW - 2 * H, "stuck"),
            moved("3", NOW - 5 * H, "build"),
            moved("3", NOW - 2 * H, "stuck"),
            took_over("3", NOW - H, NOW - H),
            moved("4", NOW - 5 * H, "build"),
            moved("5", NOW - 5 * H, "build"),
            moved("5", NOW - H, "stuck"),
        ];
        // Unit 3 merged after all; unit 4 is in progress; unit 5's transcripts are pruned.
        let kept = |unit: &str| match unit {
            "1" => Some(40),
            "2" => Some(2),
            "3" => Some(1_000),
            "4" => Some(1_000),
            _ => None,
        };
        let r = cycle_report(&events, &[], &kept);
        assert_eq!(
            r.unmerged,
            vec![UnmergedRow {
                period: "2026-W45".into(),
                units: 2,
                tokens: 42
            }]
        );
        assert_eq!(r.cycles.iter().map(|c| c.units).sum::<usize>(), 1);
        assert!(
            r.render()
                .ends_with("2026-W45  never merged: 2 units, 42 tokens (kept transcripts only)\n"),
            "{}",
            r.render()
        );
        assert_eq!(
            r.json_lines().last().unwrap(),
            r#"{"kind":"unmerged_tokens","period":"2026-W45","tokens":42,"units":2}"#
        );
    }

    /// Reads a transcript that is "<input> <output> <cache_read> <cache_write>", or "none".
    struct Fake;

    impl Harness for Fake {
        fn version_get(&self) -> anyhow::Result<String> {
            unreachable!()
        }
        fn skills_dir(&self) -> &'static str {
            unreachable!()
        }
        fn session_start_hook_install(&self, _: &std::path::Path, _: &str) -> anyhow::Result<bool> {
            unreachable!()
        }
        fn task_run(&self, _: &crate::harness::Task) -> anyhow::Result<crate::harness::Reply> {
            unreachable!()
        }
        fn transcript_render(&self, _: &str) -> String {
            unreachable!()
        }
        fn transcript_usage(&self, t: &str) -> Usage {
            let n: Vec<u64> = t.split(' ').filter_map(|x| x.parse().ok()).collect();
            Usage {
                model: None,
                tokens: (n.len() == 4).then(|| Tokens {
                    input: n[0],
                    output: n[1],
                    cache_read: n[2],
                    cache_write: n[3],
                }),
            }
        }
        fn session_interact(&self, _: &std::path::Path, _: &str) -> anyhow::Result<bool> {
            unreachable!()
        }
    }

    #[test]
    fn kept_tokens_sum_input_and_output_over_the_kept_sessions() {
        let two = ["10 5 900 900".to_string(), "20 7 800 800".to_string()];
        assert_eq!(kept_tokens(&Fake, &two), Some(42));
        let one_silent = ["10 5 0 0".to_string(), "none".to_string()];
        assert_eq!(kept_tokens(&Fake, &one_silent), Some(15));
        assert_eq!(kept_tokens(&Fake, &["none".to_string()]), None);
        assert_eq!(kept_tokens(&Fake, &[]), None);
    }

    #[test]
    fn cycle_rows_and_never_merged_lines_are_one_for_each_week_in_the_window() {
        let w = 7 * 24 * H;
        let mut events = Vec::new();
        // Merged this week, three weeks back (a week with none between), and eight weeks back.
        for (unit, ago) in [("1", 0), ("2", 3 * w), ("3", 8 * w)] {
            events.push(moved(unit, NOW - ago - 5 * H, "build"));
            events.push(took_over(unit, NOW - ago, NOW - ago - H));
        }
        // Stuck this week and two weeks back.
        for (unit, ago) in [("4", 0), ("5", 2 * w)] {
            events.push(moved(unit, NOW - ago - 5 * H, "build"));
            events.push(moved(unit, NOW - ago - H, "stuck"));
        }
        let kept = |_: &str| Some(7);
        let r = report(&[entry("a", true, 0, "1.0.0")], NOW, 4).with_cycles(
            &events,
            &[],
            &kept,
            NOW,
            4,
            25.0,
        );
        let periods = |v: Vec<&str>| v.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert_eq!(
            r.cycles
                .iter()
                .map(|c| c.period.clone())
                .collect::<Vec<_>>(),
            periods(vec!["2026-W42", "2026-W45"])
        );
        assert_eq!(
            r.unmerged
                .iter()
                .map(|c| c.period.clone())
                .collect::<Vec<_>>(),
            periods(vec!["2026-W43", "2026-W45"])
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
        let tokens = Tokens {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
        };
        let sessions = vec![
            used("build", Some("claude-opus-5-5"), Some(tokens)),
            used("retrospective", None, None),
        ];
        let mut written = line("u1", &grade, true, "abc", &sessions);
        assert_eq!(
            written["sessions"],
            json!([
                {"state": "build", "model": "claude-opus-5-5",
                 "tokens": {"input": 1, "output": 2, "cache_read": 3, "cache_write": 4}},
                {"state": "retrospective", "model": null, "tokens": null},
            ])
        );
        written["at"] = 100.into();
        assert_eq!(
            parse(&[written]),
            [Entry {
                unit: "u1".into(),
                rubric: "tests".into(),
                pass: true,
                at: 100,
                version: env!("CARGO_PKG_VERSION").into(),
                sessions,
            }]
        );
    }

    #[test]
    fn a_line_written_before_sessions_were_recorded_reads_with_none() {
        let old = json!({"unit": "u", "rubric": "a", "pass": true, "at": 5, "version": "1"});
        assert_eq!(parse(&[old])[0].sessions, []);
    }

    #[test]
    fn the_retrospective_gets_an_entry_with_no_data_when_its_transcript_is_not_kept() {
        let build = used("build", Some("m"), None);
        assert_eq!(
            with_retrospective(vec![build.clone()]),
            [build.clone(), used("retrospective", None, None)]
        );
        let own = used("retrospective", Some("m"), None);
        assert_eq!(
            with_retrospective(vec![build.clone(), own.clone()]),
            [build, own]
        );
    }

    #[test]
    fn grades_split_by_the_last_build_session_model_and_count_the_units_left_out() {
        let entries = [
            built_by("1", true, vec![used("build", Some("opus"), None)]),
            built_by("1", false, vec![used("build", Some("opus"), None)]),
            built_by(
                "2",
                true,
                vec![
                    used("build", Some("opus"), None),
                    used("build", Some("sonnet"), None),
                    used("review", Some("haiku"), None),
                ],
            ),
            built_by("3", true, vec![used("build", None, None)]),
            built_by("4", true, vec![used("review", Some("opus"), None)]),
            built_by("5", true, Vec::new()),
        ];
        let r = report(&entries, NOW, 1);
        let got: Vec<(&str, usize, usize)> = r
            .models
            .iter()
            .map(|c| (c.period.as_str(), c.grades, c.passed))
            .collect();
        assert_eq!(got, [("opus", 2, 1), ("sonnet", 1, 1)]);
        assert_eq!(r.unmodelled, 3);
        assert!(r
            .render()
            .ends_with("model   a\nopus    50% (2)\nsonnet  100% (1)\n3 units left out: their build session names no model\n"));
    }

    #[test]
    fn no_model_data_shows_no_model_table() {
        let r = report(&[entry("a", true, 0, "1.0.0")], NOW, 1);
        assert!(r.models.is_empty());
        assert!(!r.render().contains("model"));
        assert!(r.json_lines().iter().all(|l| !l.contains("model")));
    }

    #[test]
    fn a_line_that_is_not_a_grade_is_left_out() {
        let events = [
            json!("nonsense"),
            json!({"rubric": "a"}),
            json!({"unit": "u", "rubric": "a", "pass": true, "at": 5, "version": "1"}),
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
