//! Self-update and the skills drain: whether a merged `cli/` change means new units start on a
//! newer binary, when a build has ended, when the night hands off to it or syncs the skills it
//! drained for, and which in-progress issues go back to the queue at the night's start
//! (docs/factory/starting-a-night.md, docs/design/factory-core/v1.md). Self-update never drains
//! the night: running units finish on their binary and the hand-off waits until none runs. The
//! shell (`watch/self_update.rs`, `watch/scheduler/update.rs`, `watch.rs`) reads git, the run
//! locks and the clock, and acts on these answers.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::watch::Issue;

/// The commit a build stamped into `ns`, or `None` for a build from outside a git checkout
/// (`unknown`) or an empty stamp.
pub fn build_commit(stamp: &str) -> Option<&str> {
    (!stamp.is_empty() && stamp != "unknown").then_some(stamp)
}

/// Whether the night updates its own binary, and from which commit.
#[derive(Debug, PartialEq, Eq)]
pub enum Enabled<'a> {
    /// `--no-self-update`, or the watched repo is not the nightshift source.
    Off,
    /// The nightshift source, but this `ns` has no build commit to count changes from.
    NoBuildCommit,
    /// On, counting `cli/` changes from this commit.
    From(&'a str),
}

/// Self-update is on for the nightshift source unless `off`, with a build commit in `stamp`.
pub fn enabled(off: bool, is_nightshift: bool, stamp: &str) -> Enabled<'_> {
    if off || !is_nightshift {
        return Enabled::Off;
    }
    build_commit(stamp).map_or(Enabled::NoBuildCommit, Enabled::From)
}

/// Whether the fill's look at origin needs a fetch at all: a self-update or a skills drain is on.
pub fn checks_base(updating: bool, skills_drain: bool) -> bool {
    updating || skills_drain
}

/// Whether a path a commit touched is under `cli/`, the source `ns` is built from.
fn in_cli(path: &str) -> bool {
    path == "cli" || path.starts_with("cli/")
}

/// The commit to build: `to`, origin's default branch, when no build runs, it hasn't failed
/// tonight, and a commit between the one new units run and it touched `cli/`. `to` is `None`
/// when git couldn't read the base, `touched` (each path a commit in that range touched) when it
/// couldn't read the range, as for a build commit the repo lacks: neither builds.
pub fn to_build(
    building: bool,
    failed: &BTreeSet<String>,
    to: Option<&str>,
    touched: Option<&[String]>,
) -> Option<String> {
    if building {
        return None;
    }
    let to = to.filter(|t| !failed.contains(*t))?;
    touched?.iter().any(|p| in_cli(p)).then(|| to.to_string())
}

/// The ceiling in milliseconds: `over` (the test override, milliseconds) when it reads as a
/// whole number of them, else `default_ms`.
pub fn ceiling_ms(over: Option<&str>, default_ms: i64) -> i64 {
    over.and_then(|v| v.parse::<u32>().ok())
        .map_or(default_ms, i64::from)
}

/// What the build's process looked like when polled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Running,
    /// It ended, with success or not.
    Ended {
        success: bool,
    },
    /// Its state couldn't be read.
    Unreadable,
}

/// What to do about a running build, polled at `now`.
#[derive(Debug, PartialEq, Eq)]
pub enum BuildStep {
    /// It runs on; poll again later.
    Wait,
    /// A stop was requested: kill it, and the night winds down.
    Stop,
    /// It built: check and adopt the binary.
    Built,
    /// It ended with an error.
    Failed,
    /// Its state couldn't be read: give it up.
    Unreadable,
    /// It outlived its ceiling: kill it and give it up.
    TimedOut,
}

/// A stop wins over everything; then an ended build; then the ceiling, which passes at
/// `deadline` (the same clock as `now`).
pub fn build_step(stop: bool, exit: Exit, now: i64, deadline: i64) -> BuildStep {
    if stop {
        return BuildStep::Stop;
    }
    match exit {
        Exit::Ended { success: true } => BuildStep::Built,
        Exit::Ended { success: false } => BuildStep::Failed,
        Exit::Unreadable => BuildStep::Unreadable,
        Exit::Running if now >= deadline => BuildStep::TimedOut,
        Exit::Running => BuildStep::Wait,
    }
}

/// What the scheduler knows when it asks whether to hand off.
#[derive(Debug, Default, Clone, Copy)]
pub struct Idle {
    /// A skills drain waits for its sync.
    pub draining: bool,
    /// A checked binary waits for the night to hand off to it.
    pub adopted: bool,
    pub running: usize,
    /// Units paused on a usage limit, waiting to resume.
    pub resuming: usize,
    /// The night sleeps until a usage limit resets.
    pub paused: bool,
    /// The night is winding down: a stop, the budget or the deadline.
    pub stopping: bool,
}

/// A drain to sync or a binary to hand off to, with nothing running, waiting to resume or
/// paused, and the night not stopping: running and paused units finish on the binary they
/// started on first.
pub fn ready_to_hand_off(i: &Idle) -> bool {
    (i.draining || i.adopted) && i.running == 0 && i.resuming == 0 && !i.paused && !i.stopping
}

/// What a ready hand-off does.
#[derive(Debug, PartialEq, Eq)]
pub enum HandOffStep {
    /// Drop the drain unsynced: a stop came first.
    DropDrain,
    /// Sync the skills drained for; a hand-off to an adopted binary comes on the next pass.
    Sync,
    /// Exec the adopted binary.
    Exec,
    Nothing,
}

/// The skills drain is served first, so the new binary starts on synced skills. `stop`: a stop
/// was requested since the scheduler last looked.
pub fn hand_off_step(i: &Idle, stop: bool) -> HandOffStep {
    match (i.draining, stop, i.adopted) {
        (true, true, _) => HandOffStep::DropDrain,
        (true, false, _) => HandOffStep::Sync,
        (false, _, true) => HandOffStep::Exec,
        (false, _, false) => HandOffStep::Nothing,
    }
}

/// Whether the night drains for a skills sync instead of syncing before each unit: a phase's
/// harness reads the installed skills, which a sync changes under it (codex). `reads` holds one
/// answer per phase.
pub fn drains_for_skills(reads: impl IntoIterator<Item = bool>) -> bool {
    reads.into_iter().any(|r| r)
}

/// Whether a unit's start syncs the skills first: only when the night doesn't drain for them.
pub fn syncs_before_unit(skills_drain: bool) -> bool {
    !skills_drain
}

/// The commit to drain for: `pending`'s `(from, to)`, the skills checkout's commit and origin's,
/// when origin has skill changes the checkout lacks and `to` wasn't synced or tried tonight.
pub fn skills_drain(
    pending: Option<(String, String)>,
    tried: &BTreeSet<String>,
) -> Option<(String, String)> {
    pending.filter(|(_, to)| !tried.contains(to))
}

/// Whether a checkout that can't move for `reason` warns: not when the last warning said the
/// same, so a checkout that stays stuck warns once.
pub fn warns_stale(warned: Option<&str>, reason: &str) -> bool {
    warned != Some(reason)
}

/// Whether a run lock's `holder` is running issue `n`'s unit: by its issue, or by a unit id of
/// `n` or `n-<slug>`.
pub fn holds(holder: &Value, n: u64) -> bool {
    if holder["issue"].as_u64() == Some(n) {
        return true;
    }
    holder["unit"]
        .as_str()
        .is_some_and(|u| u == n.to_string() || u.starts_with(&format!("{n}-")))
}

/// Open issues carrying the in-progress label: those no live `ns run` holds, left so by a night
/// that ended mid-unit, and those one does, each with its run lock's holder.
#[derive(Debug, Default)]
pub struct InProgress {
    /// Issues to requeue, lowest number first.
    pub stale: Vec<Issue>,
    pub held: Vec<(Issue, Value)>,
}

/// Sort `issues`, as listed under `label`, by the run locks' `holders`. An issue that no longer
/// carries `label` is left alone.
pub fn in_progress(issues: Vec<Issue>, label: &str, holders: &[Value]) -> InProgress {
    let mut found = InProgress::default();
    for i in issues {
        if !i.labels.iter().any(|l| l == label) {
            continue;
        }
        match holders.iter().find(|h| holds(h, i.number)) {
            Some(h) => found.held.push((i, h.clone())),
            None => found.stale.push(i),
        }
    }
    found.stale.sort_by_key(|i| i.number);
    found
}

#[cfg(test)]
mod tests;
