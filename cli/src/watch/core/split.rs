//! Split parents: what to do with a split issue from its children's states, read by the shell
//! first (docs/factory/watch.md, "Split parents"). `watch/split.rs` finds the parents and their
//! children and does what these decide.

use serde_json::{json, Value};

use crate::watch::forge::IssueState;
use crate::watch::Issue;

#[derive(Debug, PartialEq)]
pub enum Action {
    /// Every child is closed as completed: close the parent.
    Close,
    /// These children were closed as not planned: a human decides.
    Dropped(Vec<u64>),
    /// A swept parent with this many children open: move it to `split_label`.
    Relabel(usize),
    /// This many children are still open.
    Wait(usize),
    /// No children found: left alone.
    NoChildren,
}

pub struct Parent {
    pub issue: Issue,
    pub children: Vec<u64>,
    pub action: Action,
    pub swept: bool,
}

impl Parent {
    pub fn to_json(&self) -> Value {
        let (action, extra) = match &self.action {
            Action::Close => ("close", json!({})),
            Action::Dropped(d) => ("ready_for_human", json!({"dropped": d})),
            Action::Relabel(open) => ("relabel", json!({"open": open})),
            Action::Wait(open) => ("wait", json!({"open": open})),
            Action::NoChildren => ("no_children", json!({})),
        };
        let mut v = json!({
            "number": self.issue.number,
            "title": self.issue.title,
            "action": action,
            "children": self.children,
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    }
}

/// What to do with a parent whose children are in `states`, in the order its children were
/// found; `swept` for a parent under the old split label.
pub fn action(states: &[(u64, IssueState)], swept: bool) -> Action {
    if states.is_empty() {
        return Action::NoChildren;
    }
    let open = states
        .iter()
        .filter(|(_, s)| *s == IssueState::Open)
        .count();
    let dropped: Vec<u64> = states
        .iter()
        .filter(|(_, s)| *s == IssueState::NotPlanned)
        .map(|(c, _)| *c)
        .collect();
    if !dropped.is_empty() {
        Action::Dropped(dropped)
    } else if open == 0 {
        Action::Close
    } else if swept {
        Action::Relabel(open)
    } else {
        Action::Wait(open)
    }
}

/// Whether the sweep is finished: every old-label parent was read (`all_read`) and `done` has
/// each one `parents` planned a change for.
pub fn sweep_finished(all_read: bool, parents: &[Parent], done: &[Value]) -> bool {
    all_read
        && parents
            .iter()
            .filter(|p| p.swept && !matches!(p.action, Action::Wait(_) | Action::NoChildren))
            .all(|p| done.iter().any(|d| d["number"] == p.issue.number))
}

fn list(ns: &[u64]) -> String {
    ns.iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The comment on a parent closed because every child in `children` is done.
pub fn closed_comment(children: &[u64]) -> String {
    format!(
        "Every child of this split is closed as completed: {}. Closing it.",
        list(children)
    )
}

/// The comment on a parent sent to a human because the children in `dropped` were not planned.
pub fn dropped_comment(dropped: &[u64]) -> String {
    format!(
        "A child of this split was closed as not planned: {}. The parent stays open for a human to decide what is left.",
        list(dropped)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use IssueState::{Completed, NotPlanned, Open};

    #[test]
    fn the_action_follows_the_childrens_states() {
        assert_eq!(action(&[], false), Action::NoChildren);
        assert_eq!(
            action(&[(2, Completed), (3, Completed)], false),
            Action::Close
        );
        assert_eq!(action(&[(2, Open), (3, Completed)], false), Action::Wait(1));
        assert_eq!(action(&[(2, Open), (3, Open)], true), Action::Relabel(2));
        // A dropped child goes to a human even beside an open one, swept or not.
        for swept in [false, true] {
            let states = [(2, Open), (3, NotPlanned), (4, NotPlanned)];
            assert_eq!(action(&states, swept), Action::Dropped(vec![3, 4]));
        }
        // A swept parent whose children are all done closes as any other.
        assert_eq!(action(&[(2, Completed)], true), Action::Close);
    }

    #[test]
    fn the_comments_list_the_children_they_name() {
        assert_eq!(
            closed_comment(&[2, 3]),
            "Every child of this split is closed as completed: #2, #3. Closing it."
        );
        assert!(dropped_comment(&[3])
            .starts_with("A child of this split was closed as not planned: #3."));
    }
}
