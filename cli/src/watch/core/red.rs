//! A red default branch under `[queue] fix_main_first` (docs/factory/watch.md, "Main's CI"):
//! which issues a fill may start, what the night says of it once per head, and when a fix unit
//! that ended unfixed stops the night. Finding or filing the fix issue is the shell's
//! (`watch/fix_main.rs`); these decide from what it found.

use std::collections::BTreeSet;

use crate::run::Outcome;
use crate::watch::ci::MainCi;
use crate::watch::Issue;

/// Why the night stopped when the fix issue's unit ended without fixing main.
pub const MAIN_RED_UNFIXED: &str = "main_red_unfixed";

/// The issue that fixes red `main`, and where it came from.
#[derive(Debug, Clone)]
pub enum Fix {
    /// Found open: the one used earlier tonight, or one the forge lists.
    Listed(Issue),
    /// Filed just now, which the forge may not list yet.
    JustFiled(Issue),
}

impl Fix {
    pub fn issue(&self) -> &Issue {
        match self {
            Fix::Listed(i) | Fix::JustFiled(i) => i,
        }
    }

    /// What to warn about when the queue will not start this fix issue: only one found open can
    /// be in that state, since one filed now carries the ready label.
    pub fn warning(&self, startable: &dyn Fn(&Issue) -> bool) -> Option<String> {
        match self {
            Fix::Listed(i) if !startable(i) => Some(format!(
                "ns watch: warning: fix issue #{} is open but the queue will not start it; \
                 check its labels and blockers",
                i.number
            )),
            _ => None,
        }
    }
}

/// What red main allows a fill to start.
#[derive(Debug, Clone)]
pub enum Gate {
    /// Main is not red: the whole queue.
    Open,
    /// Only the fix issue.
    Only(Fix),
    /// Nothing: the fix issue for this head was closed and main is still red.
    Shut,
}

impl Gate {
    /// The issues of `ready`, the queue in order, this gate lets start.
    pub fn admit(&self, mut ready: Vec<Issue>) -> Vec<Issue> {
        match self {
            // GitHub may not list an issue filed a moment ago.
            Gate::Only(Fix::JustFiled(issue)) => vec![issue.clone()],
            Gate::Only(Fix::Listed(issue)) => {
                ready.retain(|i| i.number == issue.number);
                ready
            }
            Gate::Shut => Vec::new(),
            Gate::Open => ready,
        }
    }
}

/// What the night says of main's CI after a fill read it, each said once per head.
#[derive(Debug, PartialEq, Eq)]
pub enum Note {
    /// Main is red at this head; only this fix issue starts.
    Red,
    /// Main was red and is not any more.
    Green,
    /// Main is still red at this head and its fix issue is closed: the queue stays held.
    Shut,
}

/// What a night remembers of red main between fills.
#[derive(Default)]
pub struct Red {
    /// The head main was last found red at, until it is found not red.
    pub red_at: Option<String>,
    /// The fix issue this night used, kept while it stays open: GitHub may not list a new one yet.
    pub fix: Option<Issue>,
    /// The heads this night filed a fix issue for, so a fix issue closed with main still red there
    /// is not filed again.
    pub filed: BTreeSet<String>,
    /// The head the night last warned it holds for, a closed fix issue and main still red.
    pub shut_at: Option<String>,
}

impl Red {
    /// Main read not red: the whole queue starts, and a night that saw it red says so.
    pub fn not_red(&mut self) -> (Gate, Option<Note>) {
        let note = self.red_at.take().map(|_| Note::Green);
        (Gate::Open, note)
    }

    /// Main read red at `main`, with `found` the fix issue the shell found or filed, or `None`
    /// when the one filed for this head is closed. Records what the next fill needs.
    pub fn red(&mut self, main: &MainCi, found: Option<Fix>) -> (Gate, Option<Note>) {
        let Some(fix) = found else {
            let note = (self.shut_at != main.sha).then_some(Note::Shut);
            self.shut_at = main.sha.clone();
            return (Gate::Shut, note);
        };
        if let (Fix::JustFiled(_), Some(sha)) = (&fix, &main.sha) {
            self.filed.insert(sha.clone());
        }
        self.fix = Some(fix.issue().clone());
        let note = (self.red_at != main.sha).then_some(Note::Red);
        self.red_at = main.sha.clone();
        (Gate::Only(fix), note)
    }

    /// Whether `issue`'s unit, ended `outcome`, is the fix unit ending unfixed if main is still
    /// red: then the shell reads main's CI and [`Red::unfixed`] decides.
    pub fn may_be_unfixed(&self, issue: u64, outcome: Outcome) -> bool {
        matches!(outcome, Outcome::Stuck | Outcome::Budget | Outcome::Split)
            && self.fix.as_ref().is_some_and(|f| f.number == issue)
    }
}

/// Whether the night stops as [`MAIN_RED_UNFIXED`]: the fix unit ended without fixing main
/// (`candidate`, from [`Red::may_be_unfixed`]) and `main`, read after, still holds the queue.
/// Another attempt would fail the same way, and nothing else may start. A state that couldn't
/// be read (`None`) stops nothing.
pub fn unfixed(candidate: bool, main: Option<&MainCi>) -> bool {
    candidate && main.is_some_and(MainCi::holds_queue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::ci::State;

    fn main(state: State, sha: &str) -> MainCi {
        MainCi {
            state,
            sha: Some(sha.into()),
            failing: Vec::new(),
        }
    }

    fn issue(n: u64) -> Issue {
        Issue {
            number: n,
            title: format!("Issue {n}"),
            labels: Vec::new(),
            body: String::new(),
            team: true,
        }
    }

    #[test]
    fn red_is_said_once_per_head_and_green_once_after_it() {
        let mut red = Red::default();
        assert_eq!(red.not_red().1, None);
        let found = || Some(Fix::Listed(issue(100)));
        assert_eq!(red.red(&main(State::Red, "a"), found()).1, Some(Note::Red));
        assert_eq!(red.red(&main(State::Red, "a"), found()).1, None);
        assert_eq!(red.red(&main(State::Red, "b"), found()).1, Some(Note::Red));
        assert_eq!(red.not_red().1, Some(Note::Green));
        assert_eq!(red.not_red().1, None);
        assert_eq!(red.red(&main(State::Red, "b"), found()).1, Some(Note::Red));
    }

    #[test]
    fn a_closed_fix_issue_shuts_the_queue_and_warns_once_per_head() {
        let mut red = Red::default();
        let (gate, note) = red.red(&main(State::Red, "a"), None);
        assert!(matches!(gate, Gate::Shut));
        assert_eq!(note, Some(Note::Shut));
        assert_eq!(red.red(&main(State::Red, "a"), None).1, None);
        assert_eq!(red.red(&main(State::Red, "b"), None).1, Some(Note::Shut));
        assert!(gate.admit(vec![issue(2)]).is_empty());
    }

    #[test]
    fn a_fix_filed_now_is_remembered_by_head_and_starts_unlisted() {
        let mut red = Red::default();
        let (gate, _) = red.red(&main(State::Red, "a"), Some(Fix::JustFiled(issue(100))));
        assert!(red.filed.contains("a"));
        assert_eq!(red.fix.as_ref().map(|i| i.number), Some(100));
        let admitted: Vec<u64> = gate
            .admit(vec![issue(2)])
            .iter()
            .map(|i| i.number)
            .collect();
        assert_eq!(admitted, [100]);
        // One found listed starts only if the queue lists it.
        let (gate, _) = red.red(&main(State::Red, "a"), Some(Fix::Listed(issue(5))));
        assert!(!red.filed.contains("b"));
        let admitted: Vec<u64> = gate
            .admit(vec![issue(2), issue(5)])
            .iter()
            .map(|i| i.number)
            .collect();
        assert_eq!(admitted, [5]);
        assert!(gate.admit(vec![issue(2)]).is_empty());
    }

    #[test]
    fn only_the_fix_unit_ending_stuck_budget_or_split_while_main_is_red_is_unfixed() {
        let mut red = Red::default();
        red.red(&main(State::Red, "a"), Some(Fix::Listed(issue(100))));
        for o in [Outcome::Stuck, Outcome::Budget, Outcome::Split] {
            assert!(red.may_be_unfixed(100, o));
            assert!(!red.may_be_unfixed(2, o));
        }
        for o in [Outcome::Merged, Outcome::Done, Outcome::Paused] {
            assert!(!red.may_be_unfixed(100, o));
        }
        assert!(unfixed(true, Some(&main(State::Red, "a"))));
        assert!(!unfixed(true, Some(&main(State::Green, "a"))));
        assert!(!unfixed(true, Some(&main(State::Pending, "a"))));
        assert!(!unfixed(true, None));
        assert!(!unfixed(false, Some(&main(State::Red, "a"))));
    }
}
