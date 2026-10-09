//! `ns quality`: how well units pass review, read from the review artifacts in every unit
//! worktree and the run log (docs/FACTORY.md, Quality).

mod artifacts;
mod metrics;

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::clock;
use crate::error::SfError;
use crate::git::Repo;
use crate::review_md::{Finding, Scope, Severity, Status};
use artifacts::Unparsed;
use metrics::Unit;

fn since_date(s: &str) -> Result<String> {
    let ok = s.len() == 10 && clock::parse_iso(s).is_some();
    if ok {
        return Ok(s.to_string());
    }
    Err(SfError::usage(
        format!("--since {s:?} is not a date: use YYYY-MM-DD"),
        "ns quality --since 2026-10-01",
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
        "findings_without_scope": count(&|f| f.scope == Scope::Unknown),
        "findings_without_cycle": count(&|f| f.cycle.is_none()),
        "findings_without_location": count(&|f| f.location.is_none()),
        "findings_without_status": count(&|f| f.status == Status::Unknown),
        "headings_with_status_not_read_as_findings": units.iter().map(|u| u.last().stray_statuses).sum::<usize>(),
        "units_first_pass_unknown": units_where(&|u| metrics::first_pass(u.first()).verdict == metrics::Verdict::Unknown),
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
    })
}

fn report(units: &[&Unit], since: Option<&str>, root: &Path, unparsed: Vec<Unparsed>) -> Value {
    let leftovers: Vec<Value> = units
        .iter()
        .flat_map(|u| metrics::leftovers(u.last()).into_iter().map(|f| item(u, f)))
        .collect();
    let escapes: Vec<Value> = units
        .iter()
        .flat_map(|u| metrics::escapes(u.last()).into_iter().map(|f| item(u, f)))
        .collect();
    let trend: Vec<Value> = metrics::trend(units)
        .into_iter()
        .map(|(day, s)| {
            let mut v = json!(s);
            v["day"] = json!(day);
            v
        })
        .collect();
    let mut out = json!({ "ok": true, "repo": root.display().to_string(), "since": since });
    let Value::Object(summary) = json!(metrics::summarize(units)) else {
        unreachable!("a struct serialises to an object")
    };
    out.as_object_mut().unwrap().extend(summary);
    out["leftover_findings"] = json!(leftovers);
    out["escape_findings"] = json!(escapes);
    out["trend"] = json!(trend);
    out["gaps"] = gaps(units, unparsed);
    out["per_unit"] = json!(units.iter().map(|u| per_unit(u)).collect::<Vec<_>>());
    out
}

pub fn cli(since: Option<String>) -> Result<ExitCode> {
    let since = since.as_deref().map(since_date).transpose()?;
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let mut unparsed = Vec::new();
    let units = artifacts::discover(&repo, &mut unparsed)?;
    let kept: Vec<&Unit> = units
        .iter()
        .filter(|u| {
            since
                .as_deref()
                .is_none_or(|s| u.day.as_deref().is_some_and(|d| d >= s))
        })
        .collect();
    let r = report(&kept, since.as_deref(), &repo.root, unparsed);
    println!("{}", serde_json::to_string_pretty(&r)?);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_md;
    use metrics::Attempt;

    #[test]
    fn since_must_be_a_calendar_date() {
        assert_eq!(since_date("2026-10-01").unwrap(), "2026-10-01");
        for bad in ["2026-10-1", "2026-13-01", "yesterday", "2026-10-01T00:00Z"] {
            let e = since_date(bad).unwrap_err();
            assert_eq!(e.downcast_ref::<SfError>().unwrap().code, 2, "{bad}");
        }
    }

    #[test]
    fn gaps_count_missing_fields_on_the_newest_attempt() {
        let a = Attempt {
            findings: review_md::parse(
                "### I1. a\n- Raised by: spec\n### I2. b\n- Axis: tests\n- Scope: changed\n- Cycle: 1\n- Location: x:1\n- Status: open\n### I3. c\n",
            ),
            stray_statuses: 2,
            ..Attempt::default()
        };
        let u = Unit {
            id: "u".into(),
            worktree: "/w".into(),
            attempts: vec![a],
            day: None,
        };
        let g = gaps(&[&u], Vec::new());
        assert_eq!(g["findings_without_axis"], 1);
        assert_eq!(g["findings_axis_from_raised_by"], 1);
        assert_eq!(g["findings_without_scope"], 2);
        assert_eq!(g["findings_without_cycle"], 2);
        assert_eq!(g["findings_without_location"], 2);
        assert_eq!(g["findings_without_status"], 2);
        assert_eq!(g["headings_with_status_not_read_as_findings"], 2);
        assert_eq!(g["units_first_pass_unknown"], 1);
        assert_eq!(g["units_without_change_size"], 1);
        assert_eq!(g["units_without_updated"], 1);
    }
}
