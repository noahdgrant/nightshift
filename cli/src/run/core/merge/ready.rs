//! A merge round without the lock: bring the PR up to date with its base when it is behind,
//! gate it when the config calls for a gate here, and wait for its CI.

use serde_json::json;

use super::{
    main_ci, needs_update, out_of_turn, ready, workflow_refusal, Ask, Check, Gated, MergeAct,
    MergeEnd, MergeFact, MergeState, Out, Polled, Red, Stage, Wait,
};
use crate::run::Outcome;

/// Begin a round: read the tip first, so a merge that lands during the update counts as a move.
pub(super) fn begin(s: &mut MergeState, o: Out) -> MergeAct {
    o.ask(s, Wait::Base(Stage::Ready), Ask::FetchTip)
}

/// The facts of a round that are not polled.
pub(super) fn step(s: &mut MergeState, mut o: Out, fact: MergeFact, now: i64) -> MergeAct {
    let n = s.pr;
    let default = s.default.clone();
    match (s.wait.clone(), fact) {
        (Wait::Base(Stage::Ready), MergeFact::Tip(tip)) => {
            s.base = tip.ok();
            s.merge_head.clone_from(&s.pr_head);
            let minutes = s.cfg.register_minutes;
            o.poll(s, Polled::MergeState, now, minutes)
        }
        (Wait::BuildOnlyTip, MergeFact::Tip(Err(e))) => o.human(
            s,
            format!("cannot fetch {default} to update PR #{n}: {e}; needs a human merge"),
        ),
        (Wait::BuildOnlyTip, MergeFact::Tip(Ok(tip))) => {
            let head = s.pr_head.clone();
            o.ask(s, Wait::Ancestor, Ask::IsAncestor { tip, head })
        }
        (Wait::Ancestor, MergeFact::Ancestor(holds)) => behind(s, o, !holds, now),
        (Wait::Overlap(Stage::Ready, tip), MergeFact::Overlap(ov)) => {
            let (ev, need) = needs_update(n, &s.pr_head, &tip, &ov);
            o.log.push(ev);
            if need {
                return o.ask(s, Wait::Update, Ask::UpdateBranch);
            }
            gate_or_ci(s, o, now)
        }
        (Wait::Update, MergeFact::Updated(Ok(()))) => {
            s.via = "api";
            let minutes = s.cfg.register_minutes;
            o.poll(s, Polled::Head, now, minutes)
        }
        (Wait::Update, MergeFact::Updated(Err(error))) => {
            let scope = workflow_refusal(&error);
            let mut ev = json!({"event": "update_branch_failed", "pr": n, "error": error});
            if scope {
                ev["workflow_scope"] = json!(true);
            }
            o.log.push(ev);
            if !scope {
                return conflict(s, o);
            }
            let pr_head = s.pr_head.clone();
            o.ask(s, Wait::GitMerge, Ask::MergeWithGit { pr_head })
        }
        (Wait::GitMerge, MergeFact::GitMerged(Ok(true))) => {
            s.via = "git";
            let minutes = s.cfg.register_minutes;
            o.poll(s, Polled::Head, now, minutes)
        }
        (Wait::GitMerge, MergeFact::GitMerged(Ok(false))) => conflict(s, o),
        (Wait::GitMerge, MergeFact::GitMerged(Err(e))) => o.human(
            s,
            format!("PR #{n}: GitHub refused update-branch without the `workflow` scope, and merging {default} with git failed: {e}; needs a human merge"),
        ),
        (Wait::Gate, MergeFact::Gated(g)) => match g {
            Gated::Green => ci(s, o, now),
            Gated::Red(red) => o.end(s, MergeEnd::Rebuild(red)),
            Gated::Conflict => conflict(s, o),
            Gated::Human(r) => o.human(s, r),
        },
        (Wait::Watch, MergeFact::Watched { timed_out: true }) => {
            let mins = s.cfg.ci_minutes;
            let r = format!("CI on PR #{n} did not finish within {mins} min");
            o.finish(s, Outcome::Stuck, r)
        }
        (Wait::Watch, MergeFact::Watched { timed_out: false }) => {
            o.ask(s, Wait::Checks, Ask::ReadChecks)
        }
        (Wait::Checks, MergeFact::Checks(checks)) => checked(s, o, &checks, now),
        (Wait::Logs(names, logged), MergeFact::FailedLogs(logs)) => {
            let mut fb = format!("CI failed on PR #{n}: {}\n", names.join(", "));
            for (name, log) in logged.iter().zip(logs) {
                if let Some(log) = log {
                    fb.push_str(&format!("\nLog tail ({name}):\n{}\n", log_tail(&log)));
                }
            }
            o.log.push(json!({"event": "ci_failed", "pr": n, "checks": names, "where": "github"}));
            let red = Red {
                feedback: fb,
                reason: "CI failed on the PR".into(),
                killed: None,
                ratchets: None,
            };
            o.end(s, MergeEnd::Rebuild(red))
        }
        (w, f) => out_of_turn(s, o, &w, &f),
    }
}

/// A polled fact: settled, or read again after a backoff, or past its deadline.
pub(super) fn polled(s: &mut MergeState, o: Out, fact: MergeFact, now: i64) -> MergeAct {
    let what = s.poll.expect("a poll in progress").what;
    match (what, fact) {
        (Polled::MergeState, MergeFact::MergeState(state)) => {
            if state.as_deref() == Some("UNKNOWN") {
                match o.backoff(s, now) {
                    Ok(act) => act,
                    Err(o) => merge_state(s, o, state.as_deref(), now),
                }
            } else {
                merge_state(s, o, state.as_deref(), now)
            }
        }
        (Polled::Head, MergeFact::HeadView { head, merge_state }) => {
            if merge_state.as_str() == Some("DIRTY") {
                return conflict(s, o);
            }
            match head {
                Some(h) if h == s.pr_head => {}
                h => return updated(s, o, h.unwrap_or_else(|| s.pr_head.clone()), now),
            }
            match o.backoff(s, now) {
                Ok(act) => act,
                Err(mut o) => {
                    let (n, mins) = (s.pr, s.cfg.register_minutes);
                    let error = format!(
                        "PR #{n} head {} did not change after update-branch within {mins} min",
                        s.pr_head
                    );
                    o.log.push(json!({"event": "update_branch_failed", "pr": n, "error": error, "merge_state": merge_state}));
                    o.human(s, format!("{error}; needs a human merge"))
                }
            }
        }
        (Polled::Counts, MergeFact::CheckCounts(counts)) => registered(s, o, counts, now),
        (Polled::MainRed(why), MergeFact::MainCi(head)) => main_ci(s, o, why, head, now),
        (_, f) => out_of_turn(s, o, &Wait::Polling, &f),
    }
}

/// GitHub worked out the PR's merge state (or stopped trying). Strict required checks: a PR
/// behind its base gets the base merged in, then fresh checks, unless the local merge queue gates
/// the merge result instead. Under build-only GitHub's checks on the PR head with the base merged
/// in are the merge queue, so the PR is behind whenever it lacks the tip.
fn merge_state(s: &mut MergeState, o: Out, state: Option<&str>, now: i64) -> MergeAct {
    match state {
        Some("DIRTY") => conflict(s, o),
        Some("BEHIND") => {
            let behind_ = !s.cfg.queued();
            behind(s, o, behind_, now)
        }
        _ if s.cfg.build_only => o.ask(s, Wait::BuildOnlyTip, Ask::FetchTip),
        _ => behind(s, o, false, now),
    }
}

/// Only commits that touch the PR's files call for the base merged in and CI again.
fn behind(s: &mut MergeState, o: Out, behind: bool, now: i64) -> MergeAct {
    if !behind {
        return gate_or_ci(s, o, now);
    }
    match s.base.clone() {
        None => o.ask(s, Wait::Update, Ask::UpdateBranch),
        Some(tip) => {
            let head = s.pr_head.clone();
            o.ask(
                s,
                Wait::Overlap(Stage::Ready, tip.clone()),
                Ask::Overlap { head, tip },
            )
        }
    }
}

/// update-branch gave the PR the head `h`.
fn updated(s: &mut MergeState, mut o: Out, h: String, now: i64) -> MergeAct {
    o.log
        .push(json!({"event": "update_branch", "pr": s.pr, "via": s.via}));
    s.merge_head = h;
    gate_or_ci(s, o, now)
}

/// The gate, when one is set and the PR's GitHub checks are not the merge queue, then CI.
fn gate_or_ci(s: &mut MergeState, o: Out, now: i64) -> MergeAct {
    if !s.cfg.gate || s.cfg.build_only {
        return ci(s, o, now);
    }
    let queued = s.cfg.queued();
    let head = if queued {
        s.pr_head.clone()
    } else {
        s.merge_head.clone()
    };
    o.ask(s, Wait::Gate, Ask::Gate { queued, head })
}

/// Wait, bounded, for GitHub to register a check run or status on the merge head.
fn ci(s: &mut MergeState, o: Out, now: i64) -> MergeAct {
    let minutes = s.cfg.register_minutes;
    o.poll(s, Polled::Counts, now, minutes)
}

fn registered(s: &mut MergeState, o: Out, counts: [Result<u64, String>; 2], now: i64) -> MergeAct {
    if counts.iter().any(|c| matches!(c, Ok(n) if *n > 0)) {
        let minutes = s.cfg.ci_minutes;
        return o.ask(s, Wait::Watch, Ask::WatchChecks { minutes });
    }
    match o.backoff(s, now) {
        Ok(act) => act,
        Err(o) => {
            let (n, mins) = (s.pr, s.cfg.register_minutes);
            let r = match counts.into_iter().find_map(|c| c.err()) {
                Some(err) => {
                    format!("could not query CI checks on PR #{n}: {err}; needs a human merge")
                }
                None => format!(
                    "no CI checks registered within {mins} min on PR #{n}; needs a human merge"
                ),
            };
            o.human(s, r)
        }
    }
}

/// CI ended: every check passed or skipped, or back to build with the failed ones' log tails.
fn checked(s: &mut MergeState, mut o: Out, checks: &[Check], now: i64) -> MergeAct {
    let n = s.pr;
    if checks.is_empty() {
        return o.human(
            s,
            format!("no CI checks reported on PR #{n}; needs a human merge"),
        );
    }
    let failed: Vec<&Check> = checks
        .iter()
        .filter(|c| !matches!(c.bucket.as_deref(), Some("pass") | Some("skipping")))
        .collect();
    if failed.is_empty() {
        o.log
            .push(json!({"event": "ci_passed", "pr": n, "sha": s.merge_head, "where": "github"}));
        return ready(s, o, now);
    }
    let name = |c: &Check| c.name.clone().unwrap_or_else(|| "?".into());
    let names: Vec<String> = failed
        .iter()
        .map(|c| {
            let how = c.bucket.as_deref().or(c.state.as_deref()).unwrap_or("?");
            format!("{} ({how})", name(c))
        })
        .collect();
    let run_re = regex::Regex::new(r"/actions/runs/(\d+)").expect("valid regex");
    let (logged, ids): (Vec<String>, Vec<String>) = failed
        .iter()
        .filter_map(|c| {
            let id = run_re.captures(c.link.as_deref()?)?[1].to_string();
            Some((name(c), id))
        })
        .unzip();
    o.ask(s, Wait::Logs(names, logged), Ask::FailedLogs(ids))
}

/// The last 3000 bytes of `log`, from a character boundary.
fn log_tail(log: &str) -> &str {
    let start = log.len().saturating_sub(3000);
    let start = (start..log.len())
        .find(|i| log.is_char_boundary(*i))
        .unwrap_or(0);
    &log[start..]
}

/// Back to build to rebase PR onto the default branch, which it conflicts with, or to ship under
/// `[gate] local = "build-only"`.
pub(super) fn conflict(s: &mut MergeState, mut o: Out) -> MergeAct {
    let (n, default) = (s.pr, s.default.clone());
    o.log.push(json!({"event": "conflict", "pr": n}));
    let feedback = format!(
        "PR #{n} conflicts with {default}: rebase onto {default} and resolve conflicts, then force-push with --force-with-lease"
    );
    let end = if s.cfg.build_only {
        MergeEnd::Reship(feedback)
    } else {
        MergeEnd::Rebase {
            feedback,
            why: format!("PR #{n} conflicts with {default}"),
        }
    };
    o.end(s, end)
}
