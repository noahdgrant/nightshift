//! Every status label `ns watch` writes comes from one table: the state an issue is in and what
//! happened to it give the label it moves to (docs/factory/watch.md, "Status labels"). Skills never
//! write a status label; a triage-only run reports its outcome in `brief.md`, read here.

use anyhow::Result;

use super::Forge;
use crate::error::SfError;
use crate::factory::Queue as QueueConfig;
use crate::frontmatter;

/// The state an issue is in when something happens to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    /// Not yet on the tracker: an issue `ns watch` files.
    New,
    /// Labelled ready, in the queue.
    Ready,
    /// Claimed: a unit runs it, is paused, or ran it and ended.
    InProgress,
    /// A split parent waiting on its children.
    Split,
    /// A split parent under the old split label, `status:needs-define` (D35).
    OldSplit,
    /// A triage candidate: the triage label, or no state at all.
    Triage,
}

/// Where a triage-only run routes its issue: `brief.md`'s `next:`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Next {
    Build,
    Define,
    Troubleshoot,
    Human,
    Split,
    Info,
    Wontfix,
}

impl Next {
    fn parse(s: &str) -> Option<Next> {
        Some(match s {
            "build" => Next::Build,
            "define" => Next::Define,
            "troubleshoot" => Next::Troubleshoot,
            "human" => Next::Human,
            "split" => Next::Split,
            "info" => Next::Info,
            "wontfix" => Next::Wontfix,
            _ => return None,
        })
    }

    /// The outcome a triage brief reports: its `next:`, else from its `status`: `pass` to build,
    /// `split` to split, `blocked` to define. An error names a value that routes nowhere.
    pub(super) fn from_brief(text: &str) -> Result<Next, String> {
        let Some((yaml, _)) = frontmatter::split(text) else {
            return Err("brief.md has no frontmatter".into());
        };
        if let Some(next) = frontmatter::raw_field(yaml, "next").filter(|n| n != "none") {
            return Next::parse(&next).ok_or(format!("brief.md says next: {next}"));
        }
        match frontmatter::raw_field(yaml, "status").as_deref() {
            Some("pass") => Ok(Next::Build),
            Some("split") => Ok(Next::Split),
            Some("blocked") => Ok(Next::Define),
            Some(s) => Err(format!("brief.md says status: {s} and no next")),
            None => Err("brief.md has no next and no status".into()),
        }
    }
}

/// What happened to an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// Filed as the queue's next work (the fix-main issue).
    Filed,
    /// Claimed by a fill, before its unit starts.
    Claim,
    /// The claim, or the unit's start or resume, failed.
    StartFailed,
    Merged,
    /// The unit's triage split the issue into children.
    Split,
    /// Done, its PR waiting on a human merge (`merge.policy = "human"`).
    AwaitingHumanMerge,
    /// Stuck, or done but needing a human.
    NeedsHuman,
    /// Stuck because the harness itself failed, not the work.
    HarnessFailing,
    /// A stop cut the unit short.
    Interrupted,
    /// `--until` or the budget cut the unit short.
    Budget,
    /// Paused on a usage limit when the night ended.
    PausedGivenBack,
    /// Left in progress by an earlier night, with no live `ns run`.
    StaleRequeue,
    /// Every child of a split parent closed as completed; the parent is closed.
    ChildrenDone,
    /// A child of a split parent closed as not planned.
    ChildDropped,
    /// A parent under the old split label found by the one-time sweep.
    SplitSwept,
    /// A triage-only run ended with this outcome.
    Triaged(Next),
}

/// A status label, by its role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Label {
    Ready,
    InProgress,
    InReview,
    Human,
    Split,
    NeedsDefine,
    NeedsRepro,
    NeedsInfo,
    Wontfix,
}

impl Label {
    /// The tracker's label for the role: the queue's own setting, else the role's `status:` label.
    pub(super) fn name(self, q: &QueueConfig) -> &str {
        match self {
            Label::Ready => &q.ready_label,
            Label::InProgress => &q.in_progress_label,
            Label::InReview => &q.done_label,
            Label::Human => &q.stuck_label,
            Label::Split => &q.split_label,
            Label::NeedsDefine => "status:needs-define",
            Label::NeedsRepro => "status:needs-repro",
            Label::NeedsInfo => "status:needs-info",
            Label::Wontfix => "status:wontfix",
        }
    }
}

/// `(from, event, to)`: an issue in `from` that `event` happens to ends with label `to`, or with
/// no status label for `None`.
const TABLE: &[(State, Event, Option<Label>)] = &[
    (State::New, Event::Filed, Some(Label::Ready)),
    (State::Ready, Event::Claim, Some(Label::InProgress)),
    (State::InProgress, Event::StartFailed, Some(Label::Ready)),
    (State::InProgress, Event::Merged, None),
    (State::InProgress, Event::Split, Some(Label::Split)),
    (
        State::InProgress,
        Event::AwaitingHumanMerge,
        Some(Label::InReview),
    ),
    (State::InProgress, Event::NeedsHuman, Some(Label::Human)),
    (State::InProgress, Event::HarnessFailing, Some(Label::Ready)),
    (State::InProgress, Event::Interrupted, Some(Label::Ready)),
    (State::InProgress, Event::Budget, Some(Label::Ready)),
    (
        State::InProgress,
        Event::PausedGivenBack,
        Some(Label::Ready),
    ),
    (State::InProgress, Event::StaleRequeue, Some(Label::Ready)),
    (State::Split, Event::ChildrenDone, None),
    (State::Split, Event::ChildDropped, Some(Label::Human)),
    (State::OldSplit, Event::SplitSwept, Some(Label::Split)),
    (
        State::Triage,
        Event::Triaged(Next::Build),
        Some(Label::Ready),
    ),
    (
        State::Triage,
        Event::Triaged(Next::Define),
        Some(Label::NeedsDefine),
    ),
    (
        State::Triage,
        Event::Triaged(Next::Troubleshoot),
        Some(Label::NeedsRepro),
    ),
    (
        State::Triage,
        Event::Triaged(Next::Human),
        Some(Label::Human),
    ),
    (
        State::Triage,
        Event::Triaged(Next::Split),
        Some(Label::Split),
    ),
    (
        State::Triage,
        Event::Triaged(Next::Info),
        Some(Label::NeedsInfo),
    ),
    (
        State::Triage,
        Event::Triaged(Next::Wontfix),
        Some(Label::Wontfix),
    ),
];

/// The description `split_label` gets when `ns watch` creates it.
const SPLIT_DESCRIPTION: &str = "Split into child issues; closes when they're done";

/// The label `event` moves an issue in `from` to: `Ok(None)` for no status label, an error for a
/// move the table has no row for.
pub(super) fn to(from: State, event: Event) -> Result<Option<Label>> {
    TABLE
        .iter()
        .find(|(f, e, _)| *f == from && *e == event)
        .map(|(_, _, to)| *to)
        .ok_or_else(|| {
            SfError::general(format!("no status move from {from:?} on {event:?}")).into()
        })
}

/// The label an issue `ns watch` files, the fix-main issue, starts with.
pub(super) fn filed(q: &QueueConfig) -> &str {
    match to(State::New, Event::Filed) {
        Ok(Some(l)) => l.name(q),
        Ok(None) | Err(_) => &q.ready_label,
    }
}

/// Move issue `n`, in `from`, to the label `event` leads to, removing every other status label.
/// The split label is created first when the repo lacks it.
pub(super) fn apply(
    forge: &dyn Forge,
    q: &QueueConfig,
    n: u64,
    from: State,
    event: Event,
) -> Result<()> {
    let label = to(from, event)?;
    if label == Some(Label::Split) {
        if let Err(e) = forge.ensure_label(&q.split_label, SPLIT_DESCRIPTION) {
            eprintln!("ns watch: cannot create label {}: {e:#}", q.split_label);
        }
    }
    set_status(forge, q, n, label.map(|l| l.name(q)))
}

const LABEL_EDIT_ATTEMPTS: u32 = 2;

struct StatusDrift {
    stray: Vec<String>,
    missing: bool,
}

impl StatusDrift {
    fn is_clean(&self) -> bool {
        self.stray.is_empty() && !self.missing
    }
}

fn status_drift(
    forge: &dyn Forge,
    q: &QueueConfig,
    n: u64,
    end: Option<&str>,
) -> Result<StatusDrift> {
    let labels = forge.labels(n)?;
    let stray = labels
        .iter()
        .filter(|l| Some(l.as_str()) != end && super::is_status(q, l))
        .cloned()
        .collect();
    Ok(StatusDrift {
        stray,
        missing: end.is_some_and(|e| !labels.iter().any(|l| l == e)),
    })
}

fn set_status(forge: &dyn Forge, q: &QueueConfig, n: u64, end: Option<&str>) -> Result<()> {
    for _ in 0..LABEL_EDIT_ATTEMPTS {
        let drift = status_drift(forge, q, n, end)?;
        if drift.is_clean() {
            return Ok(());
        }
        forge.edit_labels(n, &drift.stray, end.filter(|_| drift.missing))?;
    }
    let drift = status_drift(forge, q, n, end)?;
    if drift.is_clean() {
        return Ok(());
    }
    let mut problems = Vec::new();
    if !drift.stray.is_empty() {
        problems.push(format!(
            "has stray status labels {}",
            drift.stray.join(", ")
        ));
    }
    if let Some(e) = end.filter(|_| drift.missing) {
        problems.push(format!("is missing {e}"));
    }
    Err(SfError::general(format!(
        "#{n} {} after {LABEL_EDIT_ATTEMPTS} label edits (want {})",
        problems.join(" and "),
        end.unwrap_or("no status label")
    ))
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::tests::Fake;

    /// Every row, with the label it must end on under the default queue.
    const ROWS: &[(State, Event, Option<&str>)] = &[
        (State::New, Event::Filed, Some("status:ready-for-agent")),
        (State::Ready, Event::Claim, Some("status:in-progress")),
        (
            State::InProgress,
            Event::StartFailed,
            Some("status:ready-for-agent"),
        ),
        (State::InProgress, Event::Merged, None),
        (State::InProgress, Event::Split, Some("status:split")),
        (
            State::InProgress,
            Event::AwaitingHumanMerge,
            Some("status:in-review"),
        ),
        (
            State::InProgress,
            Event::NeedsHuman,
            Some("status:ready-for-human"),
        ),
        (
            State::InProgress,
            Event::HarnessFailing,
            Some("status:ready-for-agent"),
        ),
        (
            State::InProgress,
            Event::Interrupted,
            Some("status:ready-for-agent"),
        ),
        (
            State::InProgress,
            Event::Budget,
            Some("status:ready-for-agent"),
        ),
        (
            State::InProgress,
            Event::PausedGivenBack,
            Some("status:ready-for-agent"),
        ),
        (
            State::InProgress,
            Event::StaleRequeue,
            Some("status:ready-for-agent"),
        ),
        (State::Split, Event::ChildrenDone, None),
        (
            State::Split,
            Event::ChildDropped,
            Some("status:ready-for-human"),
        ),
        (State::OldSplit, Event::SplitSwept, Some("status:split")),
        (
            State::Triage,
            Event::Triaged(Next::Build),
            Some("status:ready-for-agent"),
        ),
        (
            State::Triage,
            Event::Triaged(Next::Define),
            Some("status:needs-define"),
        ),
        (
            State::Triage,
            Event::Triaged(Next::Troubleshoot),
            Some("status:needs-repro"),
        ),
        (
            State::Triage,
            Event::Triaged(Next::Human),
            Some("status:ready-for-human"),
        ),
        (
            State::Triage,
            Event::Triaged(Next::Split),
            Some("status:split"),
        ),
        (
            State::Triage,
            Event::Triaged(Next::Info),
            Some("status:needs-info"),
        ),
        (
            State::Triage,
            Event::Triaged(Next::Wontfix),
            Some("status:wontfix"),
        ),
    ];

    #[test]
    fn every_row_moves_the_issue_to_its_label_and_clears_the_rest() {
        assert_eq!(ROWS.len(), TABLE.len(), "a row has no test");
        let q = QueueConfig::default();
        for &(from, event, want) in ROWS {
            let f = Fake::default();
            let start = vec![
                "type:fix".to_string(),
                "status:in-progress".into(),
                "status:needs-triage".into(),
            ];
            f.labels.borrow_mut().insert(7, start);
            apply(&f, &q, 7, from, event).unwrap();
            let mut want_labels = vec!["type:fix".to_string()];
            want_labels.extend(want.map(String::from));
            assert_eq!(f.labels(7).unwrap(), want_labels, "{from:?} {event:?}");
            let created = f.created.borrow().clone();
            let split = want == Some("status:split");
            assert_eq!(created == ["status:split"], split, "{from:?} {event:?}");
        }
    }

    #[test]
    fn a_move_with_no_row_is_an_error_and_edits_nothing() {
        let f = Fake::default();
        f.labels
            .borrow_mut()
            .insert(7, vec!["status:ready-for-agent".into()]);
        let q = QueueConfig::default();
        for (from, event) in [
            (State::Ready, Event::Merged),
            (State::Triage, Event::Claim),
            (State::InProgress, Event::Triaged(Next::Build)),
            (State::Split, Event::SplitSwept),
        ] {
            let e = apply(&f, &q, 7, from, event).unwrap_err();
            assert!(e.to_string().contains("no status move"), "{e}");
        }
        assert_eq!(f.labels(7).unwrap(), ["status:ready-for-agent"]);
    }

    #[test]
    fn a_label_the_queue_renames_is_the_one_written() {
        let q = QueueConfig {
            ready_label: "go".into(),
            split_label: "tracking".into(),
            ..QueueConfig::default()
        };
        let f = Fake::default();
        apply(&f, &q, 3, State::InProgress, Event::Budget).unwrap();
        assert_eq!(f.labels(3).unwrap(), ["go"]);
        apply(&f, &q, 3, State::Triage, Event::Triaged(Next::Split)).unwrap();
        assert_eq!(f.labels(3).unwrap(), ["tracking"]);
        assert_eq!(*f.created.borrow(), ["tracking"]);
    }

    #[test]
    fn a_split_label_that_cannot_be_created_still_moves_the_issue() {
        let mut f = Fake::default();
        f.fails.insert("ensure_label");
        apply(
            &f,
            &QueueConfig::default(),
            4,
            State::InProgress,
            Event::Split,
        )
        .unwrap();
        assert_eq!(f.labels(4).unwrap(), ["status:split"]);
    }

    #[test]
    fn a_forge_that_cannot_edit_labels_fails_the_move() {
        let mut f = Fake::default();
        f.labels.borrow_mut().insert(2, vec!["status:x".into()]);
        f.fails.insert("edit_labels");
        let e = apply(&f, &QueueConfig::default(), 2, State::Ready, Event::Claim).unwrap_err();
        assert!(e.to_string().contains("HTTP 502"), "{e}");
    }

    #[test]
    fn a_brief_routes_by_next_else_by_status() {
        let brief = |fm: &str| format!("---\nunit: 5-a\n{fm}---\nbody\n");
        for (fm, want) in [
            ("next: build\nstatus: blocked\n", Ok(Next::Build)),
            ("next: define\n", Ok(Next::Define)),
            ("next: troubleshoot\n", Ok(Next::Troubleshoot)),
            ("next: human\n", Ok(Next::Human)),
            ("next: split\n", Ok(Next::Split)),
            ("next: info\n", Ok(Next::Info)),
            ("next: wontfix  # duplicate\n", Ok(Next::Wontfix)),
            ("next: \"human\"\n", Ok(Next::Human)),
            ("status: pass\n", Ok(Next::Build)),
            ("status: split\n", Ok(Next::Split)),
            ("status: blocked\n", Ok(Next::Define)),
            (
                "next: ship\nstatus: pass\n",
                Err("brief.md says next: ship"),
            ),
            (
                "status: fail\n",
                Err("brief.md says status: fail and no next"),
            ),
            ("", Err("brief.md has no next and no status")),
        ] {
            let got = Next::from_brief(&brief(fm));
            assert_eq!(got, want.map_err(String::from), "{fm:?}");
        }
        assert_eq!(
            Next::from_brief("no frontmatter"),
            Err("brief.md has no frontmatter".into())
        );
    }
}
