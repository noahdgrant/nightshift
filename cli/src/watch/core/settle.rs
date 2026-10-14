//! Settling a unit that ended: the label and comment its outcome earns, and whether its issue
//! returns to the queue, the night stops or every unit pauses (docs/factory/watch.md, "Status
//! labels"). The scheduler reads the unit's result and the facts below, and does what
//! [`settle`] returns, in order.

use std::path::Path;

use crate::run::{Outcome, RunResult};
use crate::watch::transitions::Event;
use crate::watch::{ended_cleanly, open_findings, HARNESS_FAIL_LIMIT};
use crate::worktree::BRANCH_PREFIX;

/// What the scheduler knows of the night as a unit ends.
#[derive(Clone, Copy, Debug, Default)]
pub struct SettleFacts<'a> {
    /// A stop was asked for: a signal, passed on or not.
    pub stopping: bool,
    /// The run's every attempt failed at once: the harness, not the work ([`interrupted`] runs
    /// count nothing).
    pub failing: bool,
    /// Runs in a row the harness failed, this one counted.
    pub harness_fails: u32,
    /// Why the night is over, `--until` or the budget, when it is.
    pub over: Option<&'static str>,
    /// The fix unit ended unfixed and main is still red; read only when [`checks_main`] says.
    pub unfixed: bool,
    /// When a paused unit resumes.
    pub resume_at: i64,
    /// The text of the unit's `review.md`, when its last artifact is one ([`review_path`]).
    pub review: Option<&'a str>,
}

/// What settling a unit does, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettleAct {
    /// Move the issue's status label on this event. A failure fails the settle: at once, or with
    /// `defer` after the acts that follow.
    Label { event: Event, defer: bool },
    /// Close the issue; GitHub may have closed it already, so a failure is only reported.
    Close,
    /// Take the human-merge label off this PR, set by a past night; a failure is reported.
    UnflagPr(u64),
    /// Label this PR for a human merge and comment `body` on it; a failure is reported.
    FlagPr { pr: u64, body: String },
    /// Comment on the issue; a failure fails the settle after the acts that follow.
    Comment(String),
    /// The fix issue's unit ended without fixing main, still red: the night stops.
    Unfixed,
    /// Start nothing new from here on, for this reason.
    StopAll(String),
    /// Keep the issue claimed to resume, and pause every unit until this time.
    Pause(i64),
    /// Tonight's record of the unit shows `outcome`, and `reset_at` for a pause. `finished` puts
    /// the issue out of tonight's queue. A deferred failure before it fails the settle here.
    Record {
        outcome: &'static str,
        reset_at: Option<i64>,
        finished: bool,
    },
}

/// Whether a stop cut the unit short: under a stop, an outcome other than a clean end may come
/// from the stop itself, so the issue goes back to the queue.
pub fn interrupted(r: &RunResult, stopping: bool) -> bool {
    stopping && !ended_cleanly(r)
}

/// A run cut short by `--until` ends `budget`; the night stops as `until`, not unfixed.
fn cut_short(r: &RunResult, f: &SettleFacts) -> bool {
    r.outcome == Outcome::Budget && f.over == Some("until")
}

/// Whether the settle asks if main is still red ([`SettleFacts::unfixed`]): a unit that ended on
/// its own, not through the harness or `--until`.
pub fn checks_main(r: &RunResult, f: &SettleFacts) -> bool {
    !interrupted(r, f.stopping) && !f.failing && !cut_short(r, f)
}

/// What to do for the unit that ended with `r`.
pub fn settle(r: &RunResult, f: SettleFacts) -> Vec<SettleAct> {
    use SettleAct::{Close, Comment, FlagPr, Label, Record, StopAll, UnflagPr};
    let record = |outcome| Record {
        outcome,
        reset_at: None,
        finished: true,
    };
    let label = |event, defer| Label { event, defer };
    if interrupted(r, f.stopping) {
        return vec![
            label(Event::Interrupted, false),
            Record {
                outcome: "interrupted",
                reset_at: None,
                finished: false,
            },
        ];
    }
    let mut acts = Vec::new();
    if f.unfixed && checks_main(r, &f) {
        acts.push(SettleAct::Unfixed);
    }
    let pr = r.json["pr"].as_u64();
    let flag = |acts: &mut Vec<SettleAct>| {
        if let Some(pr) = pr {
            acts.push(FlagPr {
                pr,
                body: human_merge_comment(r),
            });
        }
    };
    match r.outcome {
        Outcome::Merged => {
            acts.push(label(Event::Merged, true));
            acts.push(Close);
            acts.extend(pr.map(UnflagPr));
        }
        Outcome::Split => acts.push(label(Event::Split, false)),
        Outcome::Done if !r.needs_human => {
            acts.push(label(Event::AwaitingHumanMerge, false));
            flag(&mut acts);
        }
        Outcome::Stuck if f.failing => {
            acts.push(label(Event::HarnessFailing, false));
            if f.harness_fails >= HARNESS_FAIL_LIMIT {
                acts.push(StopAll("harness failing".into()));
            }
            acts.push(record("harness_failing"));
            return acts;
        }
        Outcome::Done | Outcome::Stuck => {
            acts.push(label(Event::NeedsHuman, true));
            acts.push(Comment(stuck_comment(r, f.review)));
            if r.outcome == Outcome::Done {
                flag(&mut acts);
            }
        }
        Outcome::Budget => {
            acts.push(label(Event::Budget, false));
            // A runner lock wait cut short by `--until` ends the run as `budget` too.
            acts.push(StopAll(f.over.unwrap_or("budget").into()));
        }
        Outcome::Paused => {
            // Recorded first, so a hold that can't be written still leaves it to give back.
            acts.push(Record {
                outcome: r.outcome.label(),
                reset_at: Some(f.resume_at),
                finished: false,
            });
            acts.push(SettleAct::Pause(f.resume_at));
            return acts;
        }
    }
    acts.push(record(r.outcome.label()));
    acts
}

/// Why a `done` unit's PR needs a human merge, as its comment says.
fn human_merge_comment(r: &RunResult) -> String {
    let why = if r.needs_human {
        r.reason.as_str()
    } else {
        "merge.policy is \"human\", so ns never merges; needs a human merge"
    };
    format!("nightshift unit `{}`: {why}", r.unit)
}

/// The unit's last artifact when it is a `review.md`, whose open findings the stuck comment
/// quotes.
pub fn review_path(r: &RunResult) -> Option<&str> {
    r.artifact
        .as_deref()
        .filter(|a| Path::new(a).file_name().is_some_and(|f| f == "review.md"))
}

/// The comment on an issue whose unit got stuck or needs a human; `review` is its `review.md`.
pub fn stuck_comment(r: &RunResult, review: Option<&str>) -> String {
    let what = if r.outcome == Outcome::Stuck {
        "got stuck"
    } else {
        "needs a human"
    };
    let artifact = r
        .artifact
        .as_deref()
        .and_then(|a| Path::new(a).file_name())
        .map_or("none".to_string(), |f| {
            format!(
                "`.ns/{0}/{1}` on branch `{BRANCH_PREFIX}{0}`",
                r.unit,
                f.to_string_lossy()
            )
        });
    let findings = review
        .filter(|_| review_path(r).is_some())
        .map(open_findings)
        .filter(|f| !f.is_empty())
        .map_or(String::new(), |f| {
            format!(
                "\n\nOpen findings in `review.md`:\n```text\n{}\n```",
                f.join("\n")
            )
        });
    format!(
        "nightshift {what} on unit `{}`: {}{findings}\n\nLast artifact: {artifact}",
        r.unit, r.reason
    )
}

#[cfg(test)]
mod tests;
