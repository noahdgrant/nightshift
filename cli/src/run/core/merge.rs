//! The merge step's decisions over plain data: a state machine `run/merge.rs` drives. [`next`]
//! takes what the shell observed (a [`MergeFact`]) and the time, and returns the [`MergeAct`] to
//! perform: run-log events to write, then one [`Ask`], a read or a deed whose answer is the next
//! fact, or the end. The shell keeps the `gh` and git calls, the merge lock and the clock.
//!
//! The step brings the PR up to date with its base and waits for CI without the merge lock, then
//! takes the lock and squash-merges it if the base has not moved since (`merge.policy = "auto"`).
//! A base that moved in files the PR changes sends the PR round again: past [`BASE_MOVE_CAP`]
//! moves with the lock held through the update and CI wait, and stuck past
//! [`BASE_MOVE_HARD_CAP`].

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::route::same_sha;
use crate::run::Outcome;

mod ready;

/// How many times the default branch may move between a merge step's CI wait and its merge
/// before the merge step holds the merge lock through the next update and CI wait, as it did
/// before merges were optimistic, so no other unit's merge can move it again.
pub const BASE_MOVE_CAP: u32 = 5;

/// How many moves end the unit stuck even with the lock held: only a merge from outside `ns`
/// can move the default branch then.
pub const BASE_MOVE_HARD_CAP: u32 = 2 * BASE_MOVE_CAP;

/// The first and the longest wait between two polls of GitHub.
const BACKOFF_START: i64 = 5;
const BACKOFF_MAX: i64 = 30;

/// What the merge step decides with, from the factory and the unit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MergeConfig {
    /// `[gate] local = "build-only"`: GitHub's checks on the PR head are the merge queue.
    pub build_only: bool,
    /// `[merge] local_queue`.
    pub local_queue: bool,
    /// A gate command is set for the steps after build.
    pub gate: bool,
    /// `[merge] ci_register_timeout`, in minutes.
    pub register_minutes: u64,
    /// `[merge] ci_timeout_minutes`.
    pub ci_minutes: u64,
    /// The unit works an issue `ns watch` filed to fix main's red CI: never held for it.
    pub fixes_main: bool,
    /// The unit's branch, deleted after the merge.
    pub branch: String,
}

impl MergeConfig {
    /// The local merge queue gates the merge of the PR head with the default branch: on with a
    /// gate, never under `build-only`.
    fn queued(&self) -> bool {
        !self.build_only && self.local_queue && self.gate
    }
}

/// What the shell observed, answering the last [`Ask`].
#[derive(Debug, Clone, PartialEq)]
pub enum MergeFact {
    /// Begin the merge step.
    Start,
    /// Whether pr.md exists, and the PR number it names.
    Pr {
        ship: bool,
        number: Option<u64>,
    },
    /// The PR's state and head on the forge.
    View {
        state: Option<String>,
        head: String,
    },
    /// HEAD, whether the artifacts are current at the PR head, and whether review.md passed.
    Currency {
        head: String,
        current: bool,
        review_passes: bool,
    },
    /// The default branch's name, `None` without one.
    Default(Option<String>),
    /// The freshly fetched tip of the default branch, or why it could not be fetched.
    Tip(Result<String, String>),
    /// The PR's `mergeStateStatus`.
    MergeState(Option<String>),
    /// The tip is an ancestor of the PR head.
    Ancestor(bool),
    Overlap(Overlap),
    /// `gh pr update-branch` ran, or the error it failed with.
    Updated(Result<(), String>),
    /// The default branch merged in with git and pushed (`false`: a conflict), or the error.
    GitMerged(Result<bool, String>),
    /// The PR's head and merge state, read while waiting for update-branch's head.
    HeadView {
        head: Option<String>,
        merge_state: Value,
    },
    Gated(Gated),
    /// The commit's check-run and status counts, or why each query failed.
    CheckCounts([Result<u64, String>; 2]),
    /// `gh pr checks --watch` ended, or was cut off at the CI timeout.
    Watched {
        timed_out: bool,
    },
    Checks(Vec<Check>),
    /// The failed log of each run [`Ask::FailedLogs`] named, `None` when it could not be read.
    FailedLogs(Vec<Option<String>>),
    /// The default branch's CI, `None` when it could not be read.
    MainCi(Option<MainHead>),
    /// The merge lock is taken, or the reason the wait for it gave up.
    Locked(Option<String>),
    /// The PR's head as the forge reports it now, `None` when the view failed or named none.
    HeadRef(Option<String>),
    /// The PR's changed files that match a `human_review` glob.
    ReviewHits(Vec<String>),
    /// The human-review regions HEAD's changes touch, or why they could not be read.
    Regions(Result<Vec<String>, String>),
    /// `gh pr merge` ran: the PR's state after it, the merge's error, and the run's spend.
    Merged {
        state: Option<String>,
        error: Option<String>,
        spent_usd: f64,
    },
    /// The PR's branch is deleted, or the error.
    BranchDeleted(Result<(), String>),
    /// A sleep or an unlock is done.
    Done,
}

/// What the commits a behind PR lacks change, against the PR's own changes.
#[derive(Debug, Clone, PartialEq)]
pub enum Overlap {
    /// The PR head already holds the tip.
    HasTip,
    /// The paths each side changed since their merge base, a rename as both of its paths.
    Files {
        pr: BTreeSet<String>,
        base: BTreeSet<String>,
    },
    /// Either list could not be read.
    Unreadable(String),
}

/// A red gate or CI run, as `gate::Red` holds it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Red {
    pub feedback: String,
    pub reason: String,
    /// How the gate was killed, when it was.
    pub killed: Option<String>,
    /// The ratchets it failed.
    pub ratchets: Option<String>,
}

/// How the gate before the merge came out.
#[derive(Debug, Clone, PartialEq)]
pub enum Gated {
    Green,
    Red(Red),
    /// The merge result conflicts.
    Conflict,
    /// The gate could not run: a human merges.
    Human(String),
}

/// One CI check on the PR.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Check {
    pub name: Option<String>,
    pub state: Option<String>,
    pub bucket: Option<String>,
    pub link: Option<String>,
}

/// The default branch's CI at its head.
#[derive(Debug, Clone, PartialEq)]
pub struct MainHead {
    pub sha: String,
    pub red: bool,
    pub running: bool,
}

/// What the shell does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// Read pr.md: send [`MergeFact::Pr`].
    ReadPr,
    /// `gh pr view` the PR: [`MergeFact::View`].
    ViewPr,
    /// [`MergeFact::Currency`] at this PR head.
    Currency {
        pr_head: String,
    },
    ReadDefault,
    /// Fetch the default branch: [`MergeFact::Tip`].
    FetchTip,
    MergeState,
    IsAncestor {
        tip: String,
        head: String,
    },
    /// [`MergeFact::Overlap`] of the PR at `head` and the default branch at `tip`.
    Overlap {
        head: String,
        tip: String,
    },
    UpdateBranch,
    /// Merge the default branch into the PR at `pr_head` with git, and push.
    MergeWithGit {
        pr_head: String,
    },
    /// [`MergeFact::HeadView`].
    ViewHead,
    /// Run the gate on the merge of `head` with the default branch (`queued`), or at `head`.
    Gate {
        queued: bool,
        head: String,
    },
    CountChecks {
        sha: String,
    },
    /// Watch the PR's checks, cut off after `minutes`.
    WatchChecks {
        minutes: u64,
    },
    ReadChecks,
    /// The failed logs of these GitHub Actions run ids.
    FailedLogs(Vec<String>),
    MainChecks,
    TakeLock,
    /// Release the merge lock.
    Unlock,
    ViewHeadRef,
    ReviewHits,
    Regions,
    /// Squash-merge the PR, matching this head.
    Merge {
        head: String,
    },
    DeleteBranch,
    /// Sleep until this time.
    Sleep(i64),
    End(MergeEnd),
}

/// How the merge step ends.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeEnd {
    Finish {
        outcome: Outcome,
        reason: String,
        needs_human: bool,
    },
    /// Back to build: CI or the gate went red.
    Rebuild(Red),
    /// Back to ship, only to bring in the base, with this feedback.
    Reship(String),
    /// Back to build only to rebase.
    Rebase { feedback: String, why: String },
}

/// What [`next`] returns: run-log events to write, then `ask`. A polled read carries how many
/// polls of it came before (the shell bounds a pinned clock's polls with it), else 0.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeAct {
    pub log: Vec<Value>,
    pub poll: u32,
    pub ask: Ask,
}

/// A read the step repeats with backoff until it settles or its deadline passes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Polled {
    MergeState,
    Head,
    Counts,
    MainRed(LockFor),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Poll {
    what: Polled,
    deadline: i64,
    wait: i64,
    polls: u32,
}

/// Why the merge lock is taken: past the cap, through the update and CI wait; else for the final
/// check and the merge.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LockFor {
    ThroughCi,
    Final,
}

/// Where the base read for: the start of a round, or the check under the lock.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Ready,
    Final,
}

/// The fact the step waits for.
#[derive(Debug, Clone, PartialEq)]
enum Wait {
    Start,
    Pr,
    View,
    Currency,
    Default,
    Base(Stage),
    Polling,
    Sleeping,
    BuildOnlyTip,
    Ancestor,
    Overlap(Stage, String),
    Update,
    GitMerge,
    Gate,
    Watch,
    Checks,
    /// The failed checks' names, for the feedback and the log.
    Logs(Vec<String>, Vec<String>),
    Lock(LockFor),
    Unlock,
    HeadRef,
    Hits,
    Regions,
    Merge,
    Delete,
    Ended,
}

/// The merge step's state across its facts.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeState {
    cfg: MergeConfig,
    wait: Wait,
    pr: u64,
    /// The worktree's HEAD, for the merged event.
    head: String,
    /// The PR head the step goes ahead with.
    pr_head: String,
    default: String,
    moves: u32,
    held: bool,
    /// The default branch's tip read at the start of the round, before the update and CI wait.
    base: Option<String>,
    /// The PR head this round merges: the update's when it made one.
    merge_head: String,
    poll: Option<Poll>,
    via: &'static str,
    red_logged: Option<String>,
}

impl MergeState {
    pub fn new(cfg: MergeConfig) -> Self {
        Self {
            cfg,
            wait: Wait::Start,
            pr: 0,
            head: String::new(),
            pr_head: String::new(),
            default: String::new(),
            moves: 0,
            held: false,
            base: None,
            merge_head: String::new(),
            poll: None,
            via: "api",
            red_logged: None,
        }
    }

    /// The PR the step merges, once read.
    pub fn pr(&self) -> u64 {
        self.pr
    }

    /// The default branch, once read.
    pub fn default_branch(&self) -> &str {
        &self.default
    }
}

/// The one act for a fact: the events `log` holds, then `ask`.
struct Out {
    log: Vec<Value>,
}

impl Out {
    fn new() -> Self {
        Self { log: vec![] }
    }

    fn ask(self, s: &mut MergeState, wait: Wait, ask: Ask) -> MergeAct {
        s.wait = wait;
        MergeAct {
            log: self.log,
            poll: 0,
            ask,
        }
    }

    fn end(self, s: &mut MergeState, end: MergeEnd) -> MergeAct {
        self.ask(s, Wait::Ended, Ask::End(end))
    }

    fn finish(self, s: &mut MergeState, outcome: Outcome, reason: String) -> MergeAct {
        let end = MergeEnd::Finish {
            outcome,
            reason,
            needs_human: false,
        };
        self.end(s, end)
    }

    fn human(self, s: &mut MergeState, reason: String) -> MergeAct {
        let end = MergeEnd::Finish {
            outcome: Outcome::Done,
            reason,
            needs_human: true,
        };
        self.end(s, end)
    }

    /// Start polling `what`, with a deadline `minutes` from `now`.
    fn poll(self, s: &mut MergeState, what: Polled, now: i64, minutes: u64) -> MergeAct {
        s.poll = Some(Poll {
            what,
            deadline: now + (minutes * 60) as i64,
            wait: BACKOFF_START,
            polls: 0,
        });
        self.repoll(s)
    }

    /// Read the polled fact again.
    fn repoll(self, s: &mut MergeState) -> MergeAct {
        let p = s.poll.as_mut().expect("a poll in progress");
        let polls = p.polls;
        p.polls += 1;
        let ask = match p.what {
            Polled::MergeState => Ask::MergeState,
            Polled::Head => Ask::ViewHead,
            Polled::Counts => Ask::CountChecks {
                sha: s.merge_head.clone(),
            },
            Polled::MainRed(_) => Ask::MainChecks,
        };
        MergeAct {
            poll: polls,
            ..self.ask(s, Wait::Polling, ask)
        }
    }

    /// The polled read did not settle: sleep before the next, or `None` past the deadline.
    fn backoff(self, s: &mut MergeState, now: i64) -> Result<MergeAct, Out> {
        let p = s.poll.as_mut().expect("a poll in progress");
        if now >= p.deadline {
            return Err(self);
        }
        let until = (now + p.wait).min(p.deadline);
        p.wait = (p.wait * 2).min(BACKOFF_MAX);
        Ok(self.ask(s, Wait::Sleeping, Ask::Sleep(until)))
    }
}

/// Take `fact` at `now` and return what to do next.
pub fn next(s: &mut MergeState, fact: MergeFact, now: i64) -> MergeAct {
    let o = Out::new();
    match (s.wait.clone(), fact) {
        (Wait::Start, MergeFact::Start) => o.ask(s, Wait::Pr, Ask::ReadPr),
        (Wait::Pr, MergeFact::Pr { ship, number }) => match (ship, number) {
            (false, _) => o.human(s, "no pr.md".into()),
            (true, None) => o.human(s, "pr.md has no pr: number; needs a human merge".into()),
            (true, Some(n)) => {
                s.pr = n;
                o.ask(s, Wait::View, Ask::ViewPr)
            }
        },
        (Wait::View, MergeFact::View { state, head }) => viewed(s, o, state.as_deref(), head),
        (
            Wait::Currency,
            MergeFact::Currency {
                head,
                current,
                review_passes,
            },
        ) => {
            let pr_head = s.pr_head.clone();
            if !current {
                let r = format!(
                    "PR head {pr_head} is not current with HEAD {head}; needs a human merge"
                );
                return o.human(s, r);
            }
            if !review_passes {
                return o.human(
                    s,
                    "review.md is not pass and current; needs a human merge".into(),
                );
            }
            s.head = head;
            o.ask(s, Wait::Default, Ask::ReadDefault)
        }
        (Wait::Default, MergeFact::Default(d)) => {
            s.default = d.unwrap_or_else(|| "main".into());
            round(s, o, now)
        }
        (Wait::Sleeping, MergeFact::Done) => o.repoll(s),
        (Wait::Polling, f) => ready::polled(s, o, f, now),
        (Wait::Lock(why), MergeFact::Locked(gave_up)) => locked(s, o, why, gave_up),
        (Wait::Unlock, MergeFact::Done) => round(s, o, now),
        (Wait::Base(Stage::Final), MergeFact::Tip(tip)) => final_check(s, o, tip.ok(), now),
        (Wait::Overlap(Stage::Final, tip), MergeFact::Overlap(ov)) => {
            let (ev, need) = needs_update(s.pr, &s.merge_head, &tip, &ov);
            let mut o = o;
            o.log.push(ev);
            if need {
                moved_on(s, o, &tip, now)
            } else {
                o.ask(s, Wait::HeadRef, Ask::ViewHeadRef)
            }
        }
        (Wait::HeadRef | Wait::Hits | Wait::Regions | Wait::Merge | Wait::Delete, f) => {
            merging(s, o, f)
        }
        (_, f) => ready::step(s, o, f, now),
    }
}

/// The PR's state on the forge: open goes on to the currency check.
fn viewed(s: &mut MergeState, o: Out, state: Option<&str>, head: String) -> MergeAct {
    let n = s.pr;
    match state {
        Some("OPEN") => {
            s.pr_head.clone_from(&head);
            o.ask(s, Wait::Currency, Ask::Currency { pr_head: head })
        }
        Some("MERGED") => o.finish(
            s,
            Outcome::Stuck,
            format!("PR merged by run: #{n} was merged before ns run's merge step"),
        ),
        other => o.finish(
            s,
            Outcome::Stuck,
            format!("PR #{n} is {}", other.unwrap_or("unknown")),
        ),
    }
}

/// The top of a round: past the cap, take the lock for the rest of the step first.
fn round(s: &mut MergeState, o: Out, now: i64) -> MergeAct {
    if s.moves >= BASE_MOVE_CAP && !s.held {
        return take(s, o, LockFor::ThroughCi, now);
    }
    ready::begin(s, o)
}

/// Take the merge lock, first waiting while main's CI is red, outside the local merge queue.
fn take(s: &mut MergeState, o: Out, why: LockFor, now: i64) -> MergeAct {
    if !s.cfg.queued() && !s.cfg.fixes_main {
        s.red_logged = None;
        let minutes = s.cfg.ci_minutes;
        return o.poll(s, Polled::MainRed(why), now, minutes);
    }
    o.ask(s, Wait::Lock(why), Ask::TakeLock)
}

/// Main's CI read while the merge waits for it: red with nothing running holds the merge.
fn main_ci(
    s: &mut MergeState,
    mut o: Out,
    why: LockFor,
    head: Option<MainHead>,
    now: i64,
) -> MergeAct {
    let Some(sha) = head.filter(|h| h.red && !h.running).map(|h| h.sha) else {
        return o.ask(s, Wait::Lock(why), Ask::TakeLock);
    };
    if s.red_logged.as_ref() != Some(&sha) {
        o.log
            .push(json!({"event": "merge_held_main_red", "pr": s.pr, "sha": sha}));
        s.red_logged = Some(sha.clone());
    }
    match o.backoff(s, now) {
        Ok(act) => act,
        Err(o) => o.finish(
            s,
            Outcome::Stuck,
            format!("main's CI is red at {sha}; not merging until it is fixed"),
        ),
    }
}

fn locked(s: &mut MergeState, mut o: Out, why: LockFor, gave_up: Option<String>) -> MergeAct {
    if let Some(reason) = gave_up {
        let end = MergeEnd::Finish {
            outcome: Outcome::Budget,
            reason,
            needs_human: false,
        };
        return o.end(s, end);
    }
    match why {
        LockFor::ThroughCi => {
            s.held = true;
            o.log
                .push(json!({"event": "merge_lock_held_through_ci", "pr": s.pr, "moves": s.moves}));
            ready::begin(s, o)
        }
        LockFor::Final => o.ask(s, Wait::Base(Stage::Final), Ask::FetchTip),
    }
}

/// CI passed at `merge_head`: under the lock, check whether the base moved since the round began.
fn ready(s: &mut MergeState, o: Out, now: i64) -> MergeAct {
    s.pr_head.clone_from(&s.merge_head);
    if s.held {
        return o.ask(s, Wait::Base(Stage::Final), Ask::FetchTip);
    }
    take(s, o, LockFor::Final, now)
}

/// The tip under the lock. A base that moved only in files the PR doesn't touch leaves CI on
/// the PR head standing, unless the local merge queue gated the merge with the old base.
fn final_check(s: &mut MergeState, o: Out, tip: Option<String>, now: i64) -> MergeAct {
    match tip.filter(|t| Some(t) != s.base.as_ref()) {
        Some(tip) if s.cfg.queued() => moved_on(s, o, &tip, now),
        Some(tip) => {
            let head = s.merge_head.clone();
            o.ask(
                s,
                Wait::Overlap(Stage::Final, tip.clone()),
                Ask::Overlap { head, tip },
            )
        }
        None => o.ask(s, Wait::HeadRef, Ask::ViewHeadRef),
    }
}

/// The base moved to `tip` during the round: go round again, or end stuck past the hard cap.
fn moved_on(s: &mut MergeState, mut o: Out, tip: &str, now: i64) -> MergeAct {
    s.moves += 1;
    o.log.push(json!({
        "event": "merge_base_moved",
        "pr": s.pr,
        "from": s.base,
        "to": tip,
        "attempt": s.moves,
    }));
    if s.moves > BASE_MOVE_HARD_CAP {
        return o.finish(
            s,
            Outcome::Stuck,
            format!(
                "the default branch moved {BASE_MOVE_HARD_CAP} times during this PR's CI, the last {} with the merge lock held (cap {BASE_MOVE_HARD_CAP})",
                BASE_MOVE_HARD_CAP - BASE_MOVE_CAP
            ),
        );
    }
    if !s.held {
        return o.ask(s, Wait::Unlock, Ask::Unlock);
    }
    round(s, o, now)
}

/// Under the lock, with the base unmoved: the head check, the human-review checks, the merge.
fn merging(s: &mut MergeState, mut o: Out, fact: MergeFact) -> MergeAct {
    let n = s.pr;
    match (s.wait.clone(), fact) {
        (Wait::HeadRef, MergeFact::HeadRef(now)) => match now {
            Some(now) if !same_sha(&now, &s.merge_head) => {
                let r = format!(
                    "PR #{n} head moved from {} to {now} after CI passed on it; needs a human merge",
                    s.merge_head
                );
                o.human(s, r)
            }
            _ => o.ask(s, Wait::Hits, Ask::ReviewHits),
        },
        (Wait::Hits, MergeFact::ReviewHits(hits)) if !hits.is_empty() => {
            o.log
                .push(json!({"event": "human_review", "pr": n, "paths": hits}));
            let r = format!(
                "changes files that need human review; needs a human merge ({})",
                hits.join(", ")
            );
            o.human(s, r)
        }
        (Wait::Hits, MergeFact::ReviewHits(_)) => o.ask(s, Wait::Regions, Ask::Regions),
        (Wait::Regions, MergeFact::Regions(Ok(regions))) if regions.is_empty() => {
            let head = s.merge_head.clone();
            o.ask(s, Wait::Merge, Ask::Merge { head })
        }
        (Wait::Regions, MergeFact::Regions(Ok(regions))) => {
            o.log
                .push(json!({"event": "human_review", "pr": n, "regions": regions}));
            let r = format!(
                "changes code in a human-review region; needs a human merge ({})",
                regions.join(", ")
            );
            o.human(s, r)
        }
        (Wait::Regions, MergeFact::Regions(Err(e))) => o.human(
            s,
            format!("cannot check human-review regions: {e}; needs a human merge"),
        ),
        (
            Wait::Merge,
            MergeFact::Merged {
                state,
                error,
                spent_usd,
            },
        ) => {
            if state.as_deref() != Some("MERGED") {
                let r = format!(
                    "gh pr merge #{n} did not merge it: {}",
                    error.unwrap_or_default()
                );
                return o.finish(s, Outcome::Stuck, r);
            }
            o.log
                .push(json!({"event": "merged", "pr": n, "sha": s.head, "spent_usd": spent_usd}));
            o.ask(s, Wait::Delete, Ask::DeleteBranch)
        }
        (Wait::Delete, MergeFact::BranchDeleted(r)) => {
            if let Err(e) = r {
                o.log.push(json!({"event": "branch_not_deleted", "pr": n, "branch": s.cfg.branch, "error": e}));
            }
            o.finish(s, Outcome::Merged, format!("squash-merged PR #{n}"))
        }
        (w, f) => out_of_turn(s, o, &w, &f),
    }
}

/// A fact that doesn't answer the last ask: a bug in the shell.
fn out_of_turn(s: &mut MergeState, o: Out, wait: &Wait, fact: &MergeFact) -> MergeAct {
    o.finish(
        s,
        Outcome::Stuck,
        format!("the merge step got {fact:?} while waiting for {wait:?}; this is a bug in ns"),
    )
}

/// Whether PR #`n` at `head` must take in `tip` before it merges: the commits it lacks change one
/// of its files, or either list can't be read. Returns the event that says which, and the lists
/// when it need not.
pub fn needs_update(n: u64, head: &str, tip: &str, overlap: &Overlap) -> (Value, bool) {
    match overlap {
        Overlap::HasTip => (
            json!({
                "event": "merge_update_needed",
                "pr": n,
                "overlap": null,
                "reason": format!("{tip} is already in the PR head {head}"),
            }),
            true,
        ),
        Overlap::Files { pr, base } => {
            let overlap: Vec<&String> = pr.intersection(base).collect();
            if overlap.is_empty() {
                let ev = json!({
                    "event": "merge_update_skipped",
                    "pr": n,
                    "base": tip,
                    "pr_files": pr,
                    "base_files": base,
                });
                return (ev, false);
            }
            (
                json!({"event": "merge_update_needed", "pr": n, "overlap": overlap}),
                true,
            )
        }
        Overlap::Unreadable(e) => (
            json!({"event": "merge_update_needed", "pr": n, "overlap": null, "error": e}),
            true,
        ),
    }
}

/// Whether a failed `gh pr update-branch` was GitHub refusing to write a workflow file: the
/// token lacks the `workflow` scope (an OAuth or personal token) or the `workflows` permission
/// (a GitHub App).
pub fn workflow_refusal(error: &str) -> bool {
    error.contains("refusing to allow") && error.contains("workflow")
}

#[cfg(test)]
mod tests;
