//! Filling slots: which ready issues start next, given the running units, overlap and the hold
//! (docs/factory/watch.md, "Parallel units"). A fill has two decisions. [`open_fill`] decides,
//! before the queue is read, whether anything may start: a pause, paused units to resume, a
//! stop, the claims left, the update check and the triage pass. [`plan_fill`] decides, from the
//! queue read and main's CI, which issues to claim, in order, and which overlap holds back.
//!
//! `plan_fill` reads nothing itself. It asks the shell for what it lacks, one [`Need`] at a time
//! (the running units' `touches`, the overlap rule, one issue's brief), and is asked again with
//! it, so a fill reads only what its picks need: no brief when no running unit predicts any.

use std::collections::{BTreeMap, BTreeSet};

use crate::touches::Overlap;
use crate::watch::core::red::Gate;
use crate::watch::{unit_id, Issue};

/// How long a ready issue that overlaps a running unit waits before it starts anyway.
pub const HOLD_LIMIT_S: i64 = 2 * 60 * 60;

/// Why no new unit starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopping {
    /// No new claims, but a unit paused on a usage limit still resumes: `--max-units`.
    Claims(String),
    /// Nothing starts, and paused units go back to the queue: `--until`, the budget, the
    /// harness breaker, a reset past `--until`, an error or a signal.
    All(String),
}

impl Stopping {
    pub fn reason(&self) -> &str {
        match self {
            Stopping::Claims(r) | Stopping::All(r) => r,
        }
    }
}

/// Which steps a fill runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// At start and when a unit ends: the worktree cleanup and the update check run too.
    Full,
    /// The refill interval passing with a slot free: neither runs.
    Refill,
}

/// What bounds the claims a fill may make.
#[derive(Clone, Copy, Debug, Default)]
pub struct Claims {
    /// Units claimed tonight so far.
    pub started: u32,
    /// `--max-units`.
    pub max_units: Option<u32>,
    /// Why the night is over, `--until` or the budget, when it is.
    pub over: Option<&'static str>,
}

impl Claims {
    /// Why no further issue may be claimed: `--max-units`, `--until` or the budget.
    pub fn stop(&self) -> Option<Stopping> {
        if self.max_units.is_some_and(|m| self.started >= m) {
            return Some(Stopping::Claims("max_units".into()));
        }
        self.over.map(|why| Stopping::All(why.into()))
    }
}

/// What the scheduler knows as a fill opens.
#[derive(Clone, Copy, Debug)]
pub struct Open<'a> {
    pub kind: Kind,
    /// A usage limit's pause holds every unit.
    pub paused: bool,
    /// Free slots.
    pub free: usize,
    /// Units a usage limit paused, waiting to resume.
    pub resume: usize,
    pub stopping: Option<&'a Stopping>,
    pub claims: Claims,
    /// A skills drain waits for the running units to end.
    pub draining: bool,
}

/// What a fill does before it reads the queue, in order.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenAct {
    /// Remove merged units' worktrees and move split parents on.
    Housekeep,
    /// Start the first paused unit again.
    Resume,
    /// Start nothing new from here on.
    Stop(Stopping),
    /// Look at origin's default branch for a self-update or a skills drain; a drain it starts
    /// ends the fill.
    CheckBase,
    /// Run the triage pass, then read the queue and [`plan_fill`].
    Triage,
}

/// What a fill does before the queue is read: nothing under a pause; the housekeeping of a full
/// fill; paused units resumed into the free slots, unless the night stops all; then, with a slot
/// still free, nothing stopping and claims left, the update check of a full fill and the triage
/// pass.
pub fn open_fill(o: &Open) -> Vec<OpenAct> {
    let mut acts = Vec::new();
    if o.paused {
        return acts;
    }
    if o.kind == Kind::Full {
        acts.push(OpenAct::Housekeep);
    }
    let may_resume = !matches!(o.stopping, Some(Stopping::All(_)));
    let resumed = if may_resume { o.resume.min(o.free) } else { 0 };
    acts.extend((0..resumed).map(|_| OpenAct::Resume));
    if o.free == resumed || o.stopping.is_some() {
        return acts;
    }
    if let Some(why) = o.claims.stop() {
        acts.push(OpenAct::Stop(why));
        return acts;
    }
    if o.kind == Kind::Full && !o.draining {
        acts.push(OpenAct::CheckBase);
    }
    if !o.draining {
        acts.push(OpenAct::Triage);
    }
    acts
}

/// The ready issues held back tonight for overlapping a running unit, each with when it was first
/// held. An entry stays all night, so an issue is announced once and its wait counts from then.
#[derive(Clone, Debug, Default)]
pub struct Holds {
    since: BTreeMap<u64, i64>,
}

impl Holds {
    /// When `n` was first held, holding it from `now` if it wasn't yet: true for a new hold.
    pub fn hold(&mut self, n: u64, now: i64) -> (i64, bool) {
        match self.since.get(&n) {
            Some(t) => (*t, false),
            None => {
                self.since.insert(n, now);
                (now, true)
            }
        }
    }
}

/// What the shell has read for the overlap pick, this fill.
#[derive(Default)]
pub struct Overlaps {
    /// The running units that predict any `touches`, each as its unit id and globs.
    pub running: Option<Vec<(String, Vec<String>)>>,
    pub rule: Option<Overlap>,
    /// Each ready issue's predicted `touches`, by number.
    pub briefs: BTreeMap<u64, Vec<String>>,
}

/// What [`plan_fill`] needs read before it can decide.
#[derive(Debug, PartialEq, Eq)]
pub enum Need {
    /// The running units' `touches`.
    Running,
    /// The overlap rule.
    Rule,
    /// This ready issue's predicted `touches`.
    Brief(u64),
}

/// What the shell knows once it has read the queue and main's CI.
pub struct FillInput<'a> {
    /// The queue's ready issues, in order.
    pub ready: &'a [Issue],
    /// What red main lets start.
    pub gate: &'a Gate,
    /// Issues running, paused or ended tonight.
    pub taken: &'a BTreeSet<u64>,
    pub free: usize,
    pub claims: Claims,
    pub holds: &'a Holds,
}

/// What a fill does once it has read the queue, in order.
#[derive(Debug, Clone)]
pub enum FillAct {
    /// Held back for the first time tonight: its touches overlap these running units.
    Held { issue: u64, overlaps: Vec<String> },
    /// Starts beside these running units after waiting `waited` seconds.
    Forced {
        issue: u64,
        waited: i64,
        overlaps: Vec<String>,
    },
    /// Claim the issue and start its unit.
    Claim(Issue),
    /// Start nothing new from here on.
    Stop(Stopping),
}

/// Acts are equal when they do the same: a claim, of the same issue number.
impl PartialEq for FillAct {
    fn eq(&self, other: &Self) -> bool {
        use FillAct::{Claim, Forced, Held, Stop};
        match (self, other) {
            (Claim(a), Claim(b)) => a.number == b.number,
            (
                Held {
                    issue: a,
                    overlaps: x,
                },
                Held {
                    issue: b,
                    overlaps: y,
                },
            ) => a == b && x == y,
            (
                Forced {
                    issue: a,
                    waited: w,
                    overlaps: x,
                },
                Forced {
                    issue: b,
                    waited: v,
                    overlaps: y,
                },
            ) => a == b && w == v && x == y,
            (Stop(a), Stop(b)) => a == b,
            _ => false,
        }
    }
}

/// The issues to claim at `now`, from the ready ones red main admits, less those taken (a
/// listing can name an issue twice), until the slots or the claims run out: in queue order the
/// first whose predicted `touches` overlap no running unit's, else the first held for
/// [`HOLD_LIMIT_S`], which starts anyway. An issue claimed joins the running units for the next
/// pick. `overlaps` is what the shell has read for the pick; `Err` names what it must read
/// first.
pub fn plan_fill(input: &FillInput, overlaps: &Overlaps, now: i64) -> Result<Vec<FillAct>, Need> {
    let mut taken = input.taken.clone();
    let mut ready = input.gate.admit(input.ready.to_vec());
    ready.retain(|i| taken.insert(i.number));
    let mut pick = Pick {
        overlaps,
        holds: input.holds.clone(),
        running: None,
    };
    let (mut claims, mut free) = (input.claims, input.free);
    let mut acts = Vec::new();
    while !ready.is_empty() && free > 0 {
        if let Some(why) = claims.stop() {
            acts.push(FillAct::Stop(why));
            break;
        }
        // Every issue left overlaps a running unit and has not waited long enough.
        let Some(issue) = pick.take(&mut ready, now, &mut acts)? else {
            break;
        };
        acts.push(FillAct::Claim(issue));
        claims.started += 1;
        free -= 1;
    }
    Ok(acts)
}

/// One fill's overlap pick: the holds as this fill leaves them, and the running units, those
/// it claimed joining them.
struct Pick<'a> {
    overlaps: &'a Overlaps,
    holds: Holds,
    running: Option<Vec<(String, Vec<String>)>>,
}

/// One ready issue that overlaps a running unit: where it is in `ready`, and the units it hits.
struct Blocked {
    at: usize,
    overlaps: Vec<String>,
}

fn brief(overlaps: &Overlaps, n: u64) -> Result<&Vec<String>, Need> {
    overlaps.briefs.get(&n).ok_or(Need::Brief(n))
}

impl Pick<'_> {
    /// Remove from `ready` and return the issue to start next; `None` holds every one. Each new
    /// hold and each forced start goes on `acts`.
    fn take(
        &mut self,
        ready: &mut Vec<Issue>,
        now: i64,
        acts: &mut Vec<FillAct>,
    ) -> Result<Option<Issue>, Need> {
        let overlaps = self.overlaps;
        if self.running.is_none() {
            self.running = Some(overlaps.running.clone().ok_or(Need::Running)?);
        }
        let running = self.running.as_mut().expect("read above");
        let at = if running.is_empty() {
            0
        } else {
            let rule = overlaps.rule.as_ref().ok_or(Need::Rule)?;
            let (clear, blocked) = scan(rule, overlaps, ready, running)?;
            for b in &blocked {
                if self.holds.hold(ready[b.at].number, now).1 {
                    acts.push(FillAct::Held {
                        issue: ready[b.at].number,
                        overlaps: b.overlaps.clone(),
                    });
                }
            }
            match clear.or_else(|| forced(ready, &blocked, now, &mut self.holds, acts)) {
                Some(at) => at,
                None => return Ok(None),
            }
        };
        let issue = ready.remove(at);
        // The last one taken needs no prediction: no pick comes after it this fill.
        if ready.is_empty() {
            return Ok(Some(issue));
        }
        let mine = brief(overlaps, issue.number)?;
        if !mine.is_empty() {
            running.push((unit_id(&issue), mine.clone()));
        }
        Ok(Some(issue))
    }
}

/// The index in `ready` of the first issue clear of every `running` unit, if any, and each issue
/// before it that overlaps one.
fn scan(
    rule: &Overlap,
    overlaps: &Overlaps,
    ready: &[Issue],
    running: &[(String, Vec<String>)],
) -> Result<(Option<usize>, Vec<Blocked>), Need> {
    let mut blocked = Vec::new();
    for (at, i) in ready.iter().enumerate() {
        let hit = overlapped(rule, brief(overlaps, i.number)?, running);
        if hit.is_empty() {
            return Ok((Some(at), blocked));
        }
        blocked.push(Blocked {
            at,
            overlaps: hit.into_iter().map(String::from).collect(),
        });
    }
    Ok((None, blocked))
}

/// The first `blocked` issue held for [`HOLD_LIMIT_S`] by `now`, said as forced.
fn forced(
    ready: &[Issue],
    blocked: &[Blocked],
    now: i64,
    holds: &mut Holds,
    acts: &mut Vec<FillAct>,
) -> Option<usize> {
    let b = blocked.iter().find(|b| {
        let (since, _) = holds.hold(ready[b.at].number, now);
        now - since >= HOLD_LIMIT_S
    })?;
    let issue = ready[b.at].number;
    acts.push(FillAct::Forced {
        issue,
        waited: now - holds.hold(issue, now).0,
        overlaps: b.overlaps.clone(),
    });
    Some(b.at)
}

/// The unit id of each running unit `mine` overlaps.
pub fn overlapped<'a>(
    rule: &Overlap,
    mine: &[String],
    running: &'a [(String, Vec<String>)],
) -> Vec<&'a str> {
    running
        .iter()
        .filter(|(_, theirs)| rule.overlaps(mine, theirs))
        .map(|(unit, _)| unit.as_str())
        .collect()
}

#[cfg(test)]
mod tests;
