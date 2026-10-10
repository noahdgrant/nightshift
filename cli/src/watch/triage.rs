//! `ns watch`'s triage pass: the triage phase alone, on issues nobody has triaged yet, so the
//! follow-ups and escapes filed tonight can join tonight's queue (docs/FACTORY.md, ns watch).

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use super::{
    is_status, labels, order, skip, Issue, Listing, Night, HARNESS_FAIL_LIMIT, PAUSED_PAST_UNTIL,
};
use crate::clock;
use crate::factory::{Factory, Queue as QueueConfig};
use crate::git::Repo;
use crate::run::{self, Loaded, Outcome, RunArgs};

/// The night's triage-only runs.
#[derive(Default)]
pub(super) struct Tally {
    /// Issues given a triage-only run tonight, whatever came of it, so none is run twice.
    tried: BTreeSet<u64>,
    /// Runs started, against `triage_per_night`. A run resumed after a usage limit counts once.
    runs: u32,
    /// The error the latest run that failed to start gave.
    last_error: Option<String>,
    /// Two runs failed to start with the same error: something global, such as a missing
    /// login, not the issue. The pass is off for the rest of the night.
    off: bool,
    pub records: Vec<Value>,
}

#[derive(Default)]
pub(super) struct Candidates {
    /// Each issue to triage, in queue order, with why it needs triage.
    pub issues: Vec<(Issue, String)>,
    pub skipped: Vec<Value>,
}

/// The night's cap on triage-only runs. Under `gates = "stop"` triage applies nothing, so a run
/// would only spend the night: none run.
pub(super) fn cap(fac: &Factory) -> u32 {
    if fac.gates == "stop" {
        0
    } else {
        fac.queue.triage_per_night
    }
}

/// Why an issue needs triage: it carries the triage label, or no state label at all.
fn needs_triage(q: &QueueConfig, labels: &[String]) -> Option<String> {
    if labels.contains(&q.triage_label) {
        return Some(q.triage_label.clone());
    }
    (!labels.iter().any(|l| is_status(q, l))).then(|| "no status label".to_string())
}

/// Open team-authored issues that need triage and that no PR closes, minus those in `done`.
pub(super) fn candidates(root: &Path, fac: &Factory, done: &BTreeSet<u64>) -> Result<Candidates> {
    let q = &fac.queue;
    let listing = Listing::read(root, None)?;
    let mut picked = Vec::new();
    let mut skipped = Vec::new();
    for i in &listing.issues {
        if done.contains(&i.number) {
            continue;
        }
        let Some(why) = needs_triage(q, &i.labels) else {
            continue;
        };
        match listing.passed_over(i) {
            Some(reason) => skipped.push(skip(i, reason)),
            None => picked.push((i.clone(), why)),
        }
    }
    picked.sort_by_key(|(i, _)| order(q, i));
    Ok(Candidates {
        issues: picked,
        skipped,
    })
}

/// Triage-only runs, best candidate first, until none is left or `triage_per_night` is spent.
/// Returns why the night ends, when it does.
pub(super) fn pass(
    repo: &Repo,
    loaded: &Loaded,
    deadline: Option<i64>,
    night: &mut Night,
) -> Result<Option<String>> {
    let q = &loaded.fac.queue;
    loop {
        if let Some(stop) = night.over(deadline, &loaded.fac) {
            return Ok(Some(stop.into()));
        }
        if night.triage.off || night.triage.runs >= cap(&loaded.fac) {
            return Ok(None);
        }
        let done = night.triage.tried.union(&night.finished).copied().collect();
        let Some((issue, _)) = candidates(&repo.root, &loaded.fac, &done)?
            .issues
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        night.triage.runs += 1;
        night.triage.tried.insert(issue.number);
        eprintln!("ns watch: triage #{} {}", issue.number, issue.title);
        let rargs = RunArgs {
            issue: Some(issue.number),
            base: night.base(repo, issue.number),
            triage_only: true,
            ..RunArgs::default()
        };
        let r = match run::execute(&rargs, &mut night.shared, loaded) {
            Ok(r) => r,
            Err(e) => {
                // One broken candidate must not end every night: record it and go on.
                let reason = format!("{e:#}");
                eprintln!("ns watch: triage #{} failed: {reason}", issue.number);
                night.triage.records.push(json!({
                    "issue": issue.number,
                    "outcome": "error",
                    "reason": reason,
                }));
                if night.triage.last_error.as_ref() == Some(&reason) {
                    eprintln!("ns watch: triage pass off for the night: the same error twice");
                    night.triage.off = true;
                }
                night.triage.last_error = Some(reason);
                continue;
            }
        };
        let failing = night.harness(&r);
        let mut rec = json!({
            "issue": issue.number,
            "unit": r.unit,
            "outcome": if failing { "harness_failing" } else { r.outcome.label() },
            "reason": r.reason,
            "cost_usd": r.cost_usd,
        });
        if r.outcome == Outcome::Paused {
            let reset = night.resume_at(&r);
            rec["reset_at"] = json!(clock::local_iso(reset));
            night.triage.records.push(rec);
            if deadline.is_some_and(|d| reset >= d) {
                return Ok(Some(PAUSED_PAST_UNTIL.into()));
            }
            // The limit stopped the run before it triaged: run it again, uncounted.
            night.triage.runs -= 1;
            night.triage.tried.remove(&issue.number);
            night.sleep_until(reset)?;
            continue;
        }
        rec["state"] = json!(labels(&repo.root, &issue.number.to_string())
            .ok()
            .map(|ls| ls
                .into_iter()
                .filter(|l| is_status(q, l))
                .collect::<Vec<_>>()));
        night.triage.records.push(rec);
        if failing && night.harness_fails >= HARNESS_FAIL_LIMIT {
            return Ok(Some("harness failing".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_triage_is_the_triage_label_or_no_state_at_all() {
        let q = QueueConfig {
            triage_label: "triage-me".into(),
            ready_label: "go".into(),
            ..QueueConfig::default()
        };
        let l = |ls: &[&str]| ls.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            needs_triage(&q, &l(&["type:fix", "triage-me"])).as_deref(),
            Some("triage-me")
        );
        assert_eq!(
            needs_triage(&q, &l(&["type:fix"])).as_deref(),
            Some("no status label")
        );
        assert_eq!(needs_triage(&q, &[]).as_deref(), Some("no status label"));
        assert_eq!(needs_triage(&q, &l(&["status:needs-info"])), None);
        assert_eq!(needs_triage(&q, &l(&["go"])), None);
        // A custom triage label is a state, so starting a unit removes it like any other.
        assert!(is_status(&q, "triage-me"));
        assert!(!is_status(&q, "type:fix"));
    }
}
