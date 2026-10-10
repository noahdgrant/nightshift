//! `ns watch`'s triage pass: the triage phase alone, on issues nobody has triaged yet, so the
//! follow-ups and escapes filed tonight can join tonight's queue (docs/FACTORY.md, ns watch).

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    is_status, labels, queue, rank, skip, Issue, Listing, Night, HARNESS_FAIL_LIMIT,
    PAUSED_PAST_UNTIL,
};
use crate::clock;
use crate::factory::{Factory, Queue as QueueConfig};
use crate::git::Repo;
use crate::run::{self, Loaded, Outcome, RunArgs};

/// The night's triage-only runs.
#[derive(Default, Clone, Serialize, Deserialize)]
pub(super) struct Tally {
    /// Issues given a triage-only run tonight, whatever came of it, so none is run twice.
    tried: BTreeSet<u64>,
    /// Every candidate an earlier pass tonight listed; `None` until the first pass. A candidate
    /// outside it was filed, or became a candidate, since the last pass.
    seen: Option<BTreeSet<u64>>,
    /// The error the latest run that failed to start gave.
    last_error: Option<String>,
    /// Two runs failed to start with the same error: something global, such as a missing
    /// login, not the issue. The pass is off for the rest of the night.
    off: bool,
    /// A candidate a usage limit stopped before it triaged: the next pass runs it first.
    retry: Option<u64>,
    pub records: Vec<Value>,
}

/// Why the pass ended before it ran out of candidates.
pub(super) enum Halt {
    /// The night ends, for this reason.
    Stop(String),
    /// A usage limit stopped a run; it resets at this time.
    Paused(i64),
}

#[derive(Default)]
pub(super) struct Candidates {
    /// Each issue to triage, in triage order, with why it needs triage.
    pub issues: Vec<(Issue, String)>,
    pub skipped: Vec<Value>,
}

/// Whether the pass runs. Under `gates = "stop"` triage applies nothing, so a run would only
/// spend the night.
pub(super) fn on(fac: &Factory) -> bool {
    fac.gates != "stop"
}

/// Triage order: a priority label first, highest first, then the oldest issue. The queue's
/// category order is left out: triage may well change the category.
fn triage_order(q: &QueueConfig, i: &Issue) -> (usize, u64) {
    (rank(&q.priority, &i.labels), i.number)
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
        match listing.passed_over(i, false) {
            Some(reason) => skipped.push(skip(i, reason)),
            None => picked.push((i.clone(), why)),
        }
    }
    picked.sort_by_key(|(i, _)| triage_order(q, i));
    Ok(Candidates {
        issues: picked,
        skipped,
    })
}

/// The candidate to triage next: the first one `before`, the candidates of earlier passes,
/// lacks; else, while `ready()` says the queue has no ready issue, the first of the backlog.
/// At the night's first pass, with no `before`, every candidate is backlog.
fn next(
    found: Vec<(Issue, String)>,
    before: Option<&BTreeSet<u64>>,
    ready: impl FnOnce() -> Result<bool>,
) -> Result<Option<Issue>> {
    let is_new = |i: &Issue| before.is_some_and(|b| !b.contains(&i.number));
    if let Some((i, _)) = found.iter().find(|(i, _)| is_new(i)) {
        return Ok(Some(i.clone()));
    }
    match found.into_iter().next() {
        Some((i, _)) if !ready()? => Ok(Some(i)),
        _ => Ok(None),
    }
}

/// Triage-only runs: every candidate filed since the last pass, then the backlog until the queue
/// has a ready issue, so a big backlog never holds up the next unit. Returns why the pass halted,
/// when it did.
pub(super) fn pass(
    repo: &Repo,
    loaded: &Loaded,
    deadline: Option<i64>,
    night: &mut Night,
) -> Result<Option<Halt>> {
    let q = &loaded.fac.queue;
    if !on(&loaded.fac) {
        return Ok(None);
    }
    let before = night.triage.seen.clone();
    loop {
        crate::stop::check()?;
        if let Some(stop) = night.over(deadline, &loaded.fac) {
            return Ok(Some(Halt::Stop(stop.into())));
        }
        if night.triage.off {
            return Ok(None);
        }
        let done = night.triage.tried.union(&night.finished).copied().collect();
        let found = candidates(&repo.root, &loaded.fac, &done)?.issues;
        night
            .triage
            .seen
            .get_or_insert_with(BTreeSet::new)
            .extend(found.iter().map(|(i, _)| i.number));
        let ready = || {
            Ok(!queue(&repo.root, &loaded.fac, &night.finished)?
                .ready
                .is_empty())
        };
        let retry = night.triage.retry.take();
        let again = found.iter().find(|(i, _)| Some(i.number) == retry);
        let picked = match again {
            Some((i, _)) => Some(i.clone()),
            None => next(found, before.as_ref(), ready)?,
        };
        let Some(issue) = picked else {
            return Ok(None);
        };
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
            Err(e) if crate::stop::requested().is_some() => return Err(e),
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
                return Ok(Some(Halt::Stop(PAUSED_PAST_UNTIL.into())));
            }
            // The limit stopped the run before it triaged: run it again after the reset.
            night.triage.tried.remove(&issue.number);
            night.triage.retry = Some(issue.number);
            return Ok(Some(Halt::Paused(reset)));
        }
        rec["state"] = json!(labels(&repo.root, &issue.number.to_string())
            .ok()
            .map(|ls| ls
                .into_iter()
                .filter(|l| is_status(q, l))
                .collect::<Vec<_>>()));
        night.triage.records.push(rec);
        if failing && night.harness_fails >= HARNESS_FAIL_LIMIT {
            return Ok(Some(Halt::Stop("harness failing".into())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(ns: &[u64]) -> Vec<(Issue, String)> {
        ns.iter()
            .map(|&number| {
                let i = Issue {
                    number,
                    title: String::new(),
                    labels: Vec::new(),
                    body: String::new(),
                    team: true,
                };
                (i, String::new())
            })
            .collect()
    }

    fn pick(ns: &[u64], before: Option<&[u64]>, ready: Option<bool>) -> Option<u64> {
        let before: Option<BTreeSet<u64>> = before.map(|b| b.iter().copied().collect());
        let ready = || {
            ready
                .map(Ok)
                .expect("the queue was read with a new candidate left")
        };
        next(found(ns), before.as_ref(), ready)
            .unwrap()
            .map(|i| i.number)
    }

    #[test]
    fn a_new_candidate_goes_first_whatever_the_queue_holds() {
        // Candidates arrive in triage order; #9 is the only one no earlier pass listed.
        assert_eq!(pick(&[4, 9, 5], Some(&[4, 5]), None), Some(9));
        assert_eq!(pick(&[9, 8], Some(&[]), None), Some(9));
    }

    #[test]
    fn the_backlog_runs_only_while_no_issue_is_ready() {
        assert_eq!(pick(&[4, 5], Some(&[4, 5]), Some(false)), Some(4));
        assert_eq!(pick(&[4, 5], Some(&[4, 5]), Some(true)), None);
        // The night's first pass has no earlier one: every candidate is backlog.
        assert_eq!(pick(&[4, 5], None, Some(false)), Some(4));
        assert_eq!(pick(&[4, 5], None, Some(true)), None);
    }

    #[test]
    fn no_candidate_needs_no_queue_read() {
        assert_eq!(pick(&[], Some(&[4]), None), None);
        assert_eq!(pick(&[], None, None), None);
    }

    #[test]
    fn a_queue_that_cannot_be_read_fails_the_pick() {
        let err = next(found(&[4]), None, || anyhow::bail!("gh down")).unwrap_err();
        assert_eq!(err.to_string(), "gh down");
    }

    #[test]
    fn triage_order_is_priority_then_number_not_category() {
        let q = QueueConfig::default();
        let issue = |number, labels: &[&str]| Issue {
            number,
            title: String::new(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            body: String::new(),
            team: true,
        };
        let mut is = [
            issue(3, &["type:fix"]),
            issue(2, &["type:docs"]),
            issue(6, &["priority:high", "type:fix"]),
            issue(5, &["priority:high", "type:docs"]),
            issue(4, &["priority:low"]),
        ];
        is.sort_by_key(|i| triage_order(&q, i));
        let ns: Vec<u64> = is.iter().map(|i| i.number).collect();
        assert_eq!(ns, [5, 6, 4, 2, 3]);
    }

    #[test]
    fn needs_triage_is_the_triage_label_or_no_state_at_all() {
        let q = QueueConfig {
            triage_label: "triage-me".into(),
            ready_label: "go".into(),
            split_label: "tracking".into(),
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
        // A split parent waits on its children: the pass leaves it alone.
        assert_eq!(needs_triage(&q, &l(&["tracking"])), None);
        // A custom triage label is a state, so starting a unit removes it like any other.
        assert!(is_status(&q, "triage-me"));
        assert!(!is_status(&q, "type:fix"));
    }
}
