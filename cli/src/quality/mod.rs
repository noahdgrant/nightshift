//! `ns quality`: how well units pass review, read from the quality records on the
//! `nightshift/quality` branch, the review artifacts in every unit worktree that has no record,
//! and the run log (docs/FACTORY.md, Quality).

mod artifacts;
mod blame;
mod metrics;
mod record;
mod runlog;
mod store;
mod sync;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::clock;
use crate::error::SfError;
use crate::git::Repo;
use crate::review_md::{Finding, Scope, Severity, Status};
use artifacts::Unparsed;
use metrics::{RunStats, Unit};
pub use record::{issue_of, Meta};
use runlog::RunLog;
pub use sync::{import, record_unit};

/// `--since` as unix seconds: `YYYY-MM-DDTHH:MM:SSZ`, or `YYYY-MM-DD` for 00:00:00 UTC.
fn since_instant(s: &str) -> Result<i64> {
    let form = s.len() == 10 || (s.len() == 20 && s.as_bytes()[10] == b'T');
    if let Some(t) = clock::parse_iso(s).filter(|_| form) {
        return Ok(t);
    }
    Err(SfError::usage(
        format!(
            "--since {s:?} is not a UTC time: use YYYY-MM-DDTHH:MM:SSZ or YYYY-MM-DD (00:00:00 UTC)"
        ),
        "ns quality --since 2026-10-01T00:00:00Z",
    )
    .into())
}

fn item(u: &Unit, f: &Finding) -> Value {
    json!({
        "unit": u.id,
        "id": f.id,
        "severity": f.severity,
        "title": f.title,
        "location": f.location,
        "scope": f.scope,
    })
}

fn gaps(units: &[&Unit], unparsed: Vec<Unparsed>) -> Value {
    let findings: Vec<&Finding> = units
        .iter()
        .flat_map(|u| u.last().findings.iter())
        .collect();
    let count = |p: &dyn Fn(&Finding) -> bool| findings.iter().filter(|f| p(f)).count();
    let units_where = |p: &dyn Fn(&Unit) -> bool| units.iter().filter(|u| p(u)).count();
    json!({
        "findings_without_axis": count(&|f| f.axes.is_empty()),
        "findings_axis_from_raised_by": count(&|f| f.axes_from_raised_by && !f.axes.is_empty()),
        "findings_without_severity": count(&|f| f.severity == Severity::Unknown),
        "findings_without_scope": count(&|f| f.scope == Scope::Unknown),
        "findings_without_cycle": count(&|f| f.cycle.is_none()),
        "findings_without_location": count(&|f| f.location.is_none()),
        "findings_without_status": count(&|f| f.status == Status::Unknown),
        "headings_with_status_not_read_as_findings": units.iter().map(|u| u.last().stray_statuses).sum::<usize>(),
        "units_first_pass_unknown": units_where(&|u| metrics::first_pass(u.first()).verdict == metrics::Verdict::Unknown),
        "units_without_first_pass_count": units_where(&|u| metrics::first_pass(u.first()).blocking.is_none()),
        "units_without_change_size": units_where(&|u| u.first().changed_lines.is_none()),
        "units_without_updated": units_where(&|u| u.day.is_none()),
        "unparsed": unparsed,
    })
}

fn per_unit(u: &Unit) -> Value {
    let (first, last) = (u.first(), u.last());
    let fp = metrics::first_pass(first);
    let sev = |s: Severity| last.findings.iter().filter(|f| f.severity == s).count();
    let cycles = metrics::cycles_to_clean(last);
    json!({
        "unit": u.id,
        "source": if u.worktree.is_some() { "worktree" } else { "record" },
        "worktree": u.worktree,
        "artifact": last.path,
        "attempts": u.attempts.len(),
        "status": last.status,
        "day": u.day,
        "cycles": last.cycles,
        "changed_lines": first.changed_lines,
        "changed_lines_source": first.changed_lines_source,
        "first_pass": fp.verdict,
        "first_pass_blocking": fp.blocking,
        "reached_clean": cycles.label(),
        "cycles_to_clean": match cycles {
            metrics::Cycles::Clean(c) => Some(c),
            _ => None,
        },
        "critical": sev(Severity::Critical),
        "important": sev(Severity::Important),
        "suggestion": sev(Severity::Suggestion),
        "leftovers": metrics::leftovers(last).len(),
        "escapes": metrics::escapes(last).len(),
        "run": u.run,
    })
}

fn report(
    units: &[&Unit],
    since: Option<i64>,
    root: &Path,
    log: RunLog,
    unparsed: Vec<Unparsed>,
) -> Value {
    let leftovers: Vec<Value> = units
        .iter()
        .flat_map(|u| metrics::leftovers(u.last()).into_iter().map(|f| item(u, f)))
        .collect();
    let escapes: Vec<Value> = units
        .iter()
        .flat_map(|u| {
            let last = u.last();
            metrics::escapes(last).into_iter().map(|f| {
                let recorded = last
                    .findings
                    .iter()
                    .position(|x| std::ptr::eq(x, f))
                    .and_then(|i| last.introduced_by.get(&i));
                let b = match recorded {
                    Some(b) => Ok(b.clone()),
                    None => blame::blame(
                        u.worktree.as_deref().map_or(root, Path::new),
                        last.blame_at.as_deref(),
                        f.location.as_deref(),
                    ),
                };
                let mut v = item(u, f);
                v["introduced_by"] = b.as_ref().map_or(Value::Null, |b| json!(b));
                v["blame_error"] = b.err().map_or(Value::Null, Value::String);
                v
            })
        })
        .collect();
    let trend: Vec<Value> = metrics::trend(units)
        .into_iter()
        .map(|(day, s)| {
            let mut v = json!(s);
            v["day"] = json!(day);
            v
        })
        .collect();
    let mut out =
        json!({ "ok": true, "repo": root.display().to_string(), "since": since.map(clock::iso) });
    let Value::Object(summary) = json!(metrics::summarize(units)) else {
        unreachable!("a struct serialises to an object")
    };
    out.as_object_mut().unwrap().extend(summary);
    out["leftover_findings"] = json!(leftovers);
    out["escape_findings"] = json!(escapes);
    out["trend"] = json!(trend);
    out["run_log"] = json!(log);
    out["gaps"] = gaps(units, unparsed);
    out["per_unit"] = json!(units.iter().map(|u| per_unit(u)).collect::<Vec<_>>());
    out
}

pub fn cli(since: Option<String>) -> Result<ExitCode> {
    let since = since.as_deref().map(since_instant).transpose()?;
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let mut unparsed = Vec::new();
    let (mut units, records) = sync::stored_units(&repo);
    let recorded: BTreeSet<String> = units.iter().map(|u| u.id.clone()).collect();
    units.extend(
        artifacts::discover(&repo, &mut unparsed)?
            .into_iter()
            .filter(|u| !recorded.contains(&u.id)),
    );
    units.sort_by(|a, b| a.id.cmp(&b.id));
    let (mut log, mut runs) = runlog::read(&repo.common_dir.join("ns").join("runs.jsonl"), since);
    let known: BTreeSet<&str> = units.iter().map(|u| u.id.as_str()).collect();
    log.units_without_artifacts = runs
        .iter()
        .filter(|(id, s)| s.review_runs > 0 && !known.contains(id.as_str()))
        .map(|(id, _)| id.clone())
        .collect();
    for u in &mut units {
        let logged = runs.remove(&u.id).unwrap_or_default();
        u.run = RunStats {
            outcome: logged.outcome.or(u.run.outcome.take()),
            pr: logged.pr.or(u.run.pr),
            ..logged
        };
    }
    let kept: Vec<&Unit> = units
        .iter()
        .filter(|u| runlog::on_or_after(u.last().updated, since))
        .collect();
    let mut r = report(&kept, since, &repo.root, log, unparsed);
    r["records"] = records;
    println!("{}", serde_json::to_string_pretty(&r)?);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_md;
    use metrics::{Attempt, RunStats};

    #[test]
    fn since_is_a_utc_instant_or_a_utc_date() {
        let t = clock::parse_iso("2026-10-01T00:00:00Z").unwrap();
        assert_eq!(since_instant("2026-10-01").unwrap(), t);
        assert_eq!(since_instant("2026-10-01T00:00:00Z").unwrap(), t);
        assert_eq!(since_instant("2026-10-01T02:43:09Z").unwrap(), t + 9789);
        for bad in [
            "2026-10-1",
            "2026-13-01",
            "yesterday",
            "2026-10-01T00:00Z",
            "2026-10-01T00:00:00",
            "2026-10-01T00:00:00+02:00",
            "2026-10-01 00:00:00Z",
            "2026-10-01T24:00:00Z",
        ] {
            let e = since_instant(bad).unwrap_err();
            let e = e.downcast_ref::<SfError>().unwrap();
            assert_eq!(e.code, 2, "{bad}");
            assert!(e.to_string().contains("YYYY-MM-DDTHH:MM:SSZ"), "{e}");
            assert!(e.to_string().contains("YYYY-MM-DD "), "{e}");
        }
    }

    #[test]
    fn gaps_count_missing_fields_on_the_newest_attempt() {
        let a = Attempt {
            findings: review_md::parse(
                "### I1. a\n- Raised by: spec\n### I2. b\n- Axis: tests\n- Scope: changed\n- Cycle: 1\n- Location: x:1\n- Status: open\n### I3. c\n### E1. d\n- Axis: tests\n- Scope: changed\n- Cycle: 1\n- Location: x:2\n- Status: open\n",
            ),
            stray_statuses: 2,
            ..Attempt::default()
        };
        let u = Unit {
            id: "u".into(),
            worktree: Some("/w".into()),
            attempts: vec![a],
            day: None,
            run: RunStats::default(),
        };
        let g = gaps(&[&u], Vec::new());
        assert_eq!(g["findings_without_severity"], 1);
        assert_eq!(g["findings_without_axis"], 1);
        assert_eq!(g["findings_axis_from_raised_by"], 1);
        assert_eq!(g["findings_without_scope"], 2);
        assert_eq!(g["findings_without_cycle"], 2);
        assert_eq!(g["findings_without_location"], 2);
        assert_eq!(g["findings_without_status"], 2);
        assert_eq!(g["headings_with_status_not_read_as_findings"], 2);
        assert_eq!(g["units_first_pass_unknown"], 1);
        assert_eq!(g["units_without_first_pass_count"], 1);
        assert_eq!(g["units_without_change_size"], 1);
        assert_eq!(g["units_without_updated"], 1);
    }
}
