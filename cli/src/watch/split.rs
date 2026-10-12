//! Split parents: issues triage split into child issues, which wait under `[queue] split_label`
//! until `ns watch` closes them once their children are done (docs/FACTORY.md, "Split parents").

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::forge::IssueState;
use super::{set_status, Forge, Issue, Post};
use crate::factory::Queue as QueueConfig;

/// The description `split_label` gets when `ns watch` creates it.
const DESCRIPTION: &str = "Split into child issues; closes when they're done";
/// Where a split parent waited before `split_label` had a label of its own (D35).
const OLD_LABEL: &str = "status:needs-define";
/// Under `<common>/ns/`: the sweep has run in this clone.
const SWEPT: &str = "split-swept";

#[derive(Debug, PartialEq)]
pub(super) enum Action {
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

pub(super) struct Parent {
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

/// Move issue `n` to `split_label`, creating the label first when the repo lacks it.
pub(super) fn mark(forge: &dyn Forge, q: &QueueConfig, n: u64) -> Result<()> {
    if let Err(e) = forge.ensure_label(&q.split_label, DESCRIPTION) {
        eprintln!("ns watch: cannot create label {}: {e:#}", q.split_label);
    }
    set_status(forge, q, n, Some(&q.split_label))
}

/// The `#n` children a team note lists: each list item under a `Children:` line, by the first
/// `#n` in the item, and any `#n` on that line itself. An item with no `#n` is an error: the
/// note names a child this can't read, so the parent must not close without it.
fn listed(text: &str) -> Result<Vec<u64>> {
    let num = regex::Regex::new(r"#(\d+)\b").unwrap();
    let item_start = regex::Regex::new(r"^(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s*)?").unwrap();
    let mut found: Vec<u64> = Vec::new();
    let mut in_list = false;
    let mut indent: Option<usize> = None;
    for line in text.lines() {
        let plain = line.trim().replace(['*', '_'], "");
        if let Some(rest) = plain
            .get(..9)
            .filter(|h| h.eq_ignore_ascii_case("children:"))
            .map(|_| &plain[9..])
        {
            in_list = true;
            indent = None;
            found.extend(
                num.captures_iter(rest)
                    .filter_map(|c| c[1].parse::<u64>().ok()),
            );
            continue;
        }
        if !in_list {
            continue;
        }
        let item = line.trim_start();
        let depth = line.len() - item.len();
        if item.is_empty() {
            continue;
        }
        if indent.is_some_and(|i| depth > i) {
            continue;
        }
        match item_start.captures(item) {
            Some(m) => {
                indent = Some(depth);
                let title = item[m.get(0).unwrap().end()..].trim();
                if title.is_empty() {
                    continue;
                }
                match num.captures(title).and_then(|c| c[1].parse::<u64>().ok()) {
                    Some(n) => found.push(n),
                    None => anyhow::bail!("a Children: list item names no issue: {title}"),
                }
            }
            None => in_list = false,
        }
    }
    Ok(found)
}

/// Whether `body` has a `Part of #parent` line.
fn part_of(body: &str, parent: u64) -> bool {
    let re = regex::Regex::new(r"(?im)^\s*part of #(\d+)\b").unwrap();
    let linked = re
        .captures_iter(body)
        .any(|c| c[1].parse::<u64>().ok() == Some(parent));
    linked
}

/// `parent`'s children: its sub-issues and those its team notes list, else with `search` the team
/// issues whose body says `Part of #parent`.
fn children(forge: &dyn Forge, parent: &Issue, search: bool) -> Result<Vec<u64>> {
    let n = parent.number;
    let mut found: BTreeSet<u64> = forge.sub_issues(n)?.into_iter().collect();
    let mut notes = forge.team_comments(n)?;
    if parent.team {
        notes.push(parent.body.clone());
    }
    for note in &notes {
        found.extend(listed(note)?);
    }
    if found.is_empty() && search {
        found.extend(
            forge
                .part_of(n)?
                .iter()
                .filter(|i| i.team && part_of(&i.body, n))
                .map(|i| i.number),
        );
    }
    found.remove(&n);
    Ok(found.into_iter().collect())
}

/// What to do with `parent`, from the states of its `children`.
fn action(forge: &dyn Forge, children: &[u64], swept: bool) -> Result<Action> {
    if children.is_empty() {
        return Ok(Action::NoChildren);
    }
    let mut open = 0;
    let mut dropped = Vec::new();
    for &c in children {
        match forge.state(c)? {
            IssueState::Open => open += 1,
            IssueState::NotPlanned => dropped.push(c),
            IssueState::Completed => {}
        }
    }
    Ok(if !dropped.is_empty() {
        Action::Dropped(dropped)
    } else if open == 0 {
        Action::Close
    } else if swept {
        Action::Relabel(open)
    } else {
        Action::Wait(open)
    })
}

/// Every open issue under `split_label` and what to do with it, and with `sweep`, each one under
/// the old split label (D35) whose children can be found. A parent whose children can't be read
/// is reported and left out.
pub(super) fn plan(forge: &dyn Forge, q: &QueueConfig, sweep: bool) -> Result<Vec<Parent>> {
    plan_read(forge, q, sweep).map(|(parents, _)| parents)
}

/// `plan`, and whether every parent under the old split label was read.
pub(super) fn plan_read(
    forge: &dyn Forge,
    q: &QueueConfig,
    sweep: bool,
) -> Result<(Vec<Parent>, bool)> {
    let mut issues: Vec<(Issue, bool)> = forge
        .open_issues(Some(&q.split_label))?
        .into_iter()
        .map(|i| (i, false))
        .collect();
    if sweep {
        let split: BTreeSet<u64> = issues.iter().map(|(i, _)| i.number).collect();
        issues.extend(
            forge
                .open_issues(Some(OLD_LABEL))?
                .into_iter()
                .filter(|i| !split.contains(&i.number))
                .map(|i| (i, true)),
        );
    }
    let mut parents = Vec::new();
    let mut all_read = true;
    for (issue, swept) in issues {
        let read =
            children(forge, &issue, !swept).and_then(|c| action(forge, &c, swept).map(|a| (c, a)));
        match read {
            Ok((_, Action::NoChildren)) if swept => {}
            Ok((children, action)) => parents.push(Parent {
                issue,
                children,
                action,
                swept,
            }),
            Err(e) => {
                all_read &= !swept;
                eprintln!("ns watch: split parent #{} left alone: {e:#}", issue.number);
            }
        }
    }
    Ok((parents, all_read))
}

/// Whether the sweep is finished: every old-label parent was read (`all_read`) and `done` has
/// each one `parents` planned a change for.
pub(super) fn sweep_finished(all_read: bool, parents: &[Parent], done: &[Value]) -> bool {
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

/// Do what `parents` plan, and say what was done, one record per parent changed. A change that
/// fails is reported and the parent left for the next pass. A parent with no children is
/// reported once a night, `warned` holding those already reported.
pub(super) fn apply(
    forge: &dyn Forge,
    q: &QueueConfig,
    parents: &[Parent],
    warned: &mut BTreeSet<u64>,
) -> Vec<Value> {
    let mut done = Vec::new();
    for p in parents {
        let n = p.issue.number;
        let changed = match &p.action {
            Action::Wait(_) => continue,
            Action::NoChildren => {
                if warned.insert(n) {
                    eprintln!(
                        "ns watch: #{n} is labelled {} but has no children ns watch can find; left alone",
                        q.split_label
                    );
                }
                continue;
            }
            // Closed first: an open parent stripped of its label would go back to triage.
            Action::Close => forge.close(n).map(|()| {
                let body = Post::new(&format!(
                    "Every child of this split is closed as completed: {}. Closing it.",
                    list(&p.children)
                ));
                if let Err(e) = forge.comment(n, &body) {
                    eprintln!("ns watch: #{n} closed without its comment: {e:#}");
                }
                if let Err(e) = set_status(forge, q, n, None) {
                    eprintln!("ns watch: #{n} closed but its {} label was not cleared: {e:#}", q.split_label);
                }
            }),
            // Moved first: a comment posted on a parent still under `split_label` would repeat.
            Action::Dropped(d) => set_status(forge, q, n, Some(&q.stuck_label)).map(|()| {
                let body = Post::new(&format!(
                    "A child of this split was closed as not planned: {}. The parent stays open for a human to decide what is left.",
                    list(d)
                ));
                if let Err(e) = forge.comment(n, &body) {
                    eprintln!("ns watch: #{n} moved without its comment: {e:#}");
                }
            }),
            Action::Relabel(_) => mark(forge, q, n),
        };
        match changed {
            Ok(()) => done.push(p.to_json()),
            Err(e) => eprintln!("ns watch: split parent #{n} not moved on: {e:#}"),
        }
    }
    done
}

/// Whether the clone whose common dir is `common` has yet to run the sweep.
pub(super) fn sweep_due(common: &Path) -> bool {
    !common.join("ns").join(SWEPT).exists()
}

/// Record that the sweep ran in the clone whose common dir is `common`.
pub(super) fn swept(common: &Path) -> Result<()> {
    let dir = common.join("ns");
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    std::fs::write(dir.join(SWEPT), "")
        .with_context(|| format!("cannot write {}", dir.join(SWEPT).display()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::forge::DISCLOSURE;
    use super::super::tests::Fake;
    use super::*;

    const SPLIT: &str = "status:split";

    fn listed(text: &str) -> Vec<u64> {
        super::listed(text).unwrap()
    }

    fn issue(number: u64, labels: &[&str], body: &str) -> Issue {
        Issue {
            number,
            title: format!("Issue {number}"),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            body: body.into(),
            team: true,
        }
    }

    /// A forge with split parent 1 whose sub-issues are 2 and 3.
    fn forge(closed: &[u64], not_planned: &[u64]) -> Fake {
        let f = Fake {
            issues: vec![issue(1, &[SPLIT], "")],
            closed: closed.iter().copied().collect(),
            not_planned: not_planned.iter().copied().collect(),
            subs: BTreeMap::from([(1, vec![2, 3])]),
            ..Fake::default()
        };
        f.labels
            .borrow_mut()
            .insert(1, vec!["type:feat".into(), SPLIT.into()]);
        f
    }

    fn run(f: &Fake, sweep: bool) -> Vec<Value> {
        let q = QueueConfig::default();
        let parents = plan(f, &q, sweep).unwrap();
        apply(f, &q, &parents, &mut BTreeSet::new())
    }

    #[test]
    fn the_default_split_label_is_its_own_state() {
        let q = QueueConfig::default();
        assert_eq!(q.split_label, SPLIT);
        assert!(super::super::is_status(&q, SPLIT));
    }

    #[test]
    fn all_children_completed_closes_the_parent_with_a_comment() {
        let f = forge(&[2, 3], &[]);
        let done = run(&f, false);
        assert_eq!(*f.closes.borrow(), [1]);
        let posted = f.posted.borrow();
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].0, 1);
        let lead = format!("{DISCLOSURE}\n\nEvery child");
        assert!(posted[0].1.starts_with(&lead), "{}", posted[0].1);
        assert!(posted[0].1.contains("#2, #3"), "{}", posted[0].1);
        assert_eq!(f.labels(1).unwrap(), ["type:feat"]);
        assert_eq!(done[0]["action"], "close");
        assert_eq!(done[0]["children"], json!([2, 3]));
    }

    #[test]
    fn a_not_planned_child_moves_the_parent_to_ready_for_human() {
        let f = forge(&[2, 3], &[3]);
        let done = run(&f, false);
        assert!(f.closes.borrow().is_empty());
        assert_eq!(
            f.labels(1).unwrap(),
            ["type:feat", "status:ready-for-human"]
        );
        let posted = f.posted.borrow();
        assert!(posted[0].1.contains("not planned: #3"), "{}", posted[0].1);
        assert_eq!(done[0]["action"], "ready_for_human");
        assert_eq!(done[0]["dropped"], json!([3]));
    }

    #[test]
    fn a_parent_with_an_open_child_is_left_alone() {
        let f = forge(&[2], &[]);
        assert!(run(&f, false).is_empty());
        assert!(f.closes.borrow().is_empty() && f.posted.borrow().is_empty());
        assert_eq!(f.labels(1).unwrap(), ["type:feat", SPLIT]);
        let q = QueueConfig::default();
        assert_eq!(plan(&f, &q, false).unwrap()[0].action, Action::Wait(1));
    }

    #[test]
    fn a_parent_with_no_children_is_left_alone_and_reported_once() {
        let f = Fake {
            issues: vec![issue(1, &[SPLIT], "")],
            ..Fake::default()
        };
        let q = QueueConfig::default();
        let parents = plan(&f, &q, false).unwrap();
        assert_eq!(parents[0].action, Action::NoChildren);
        let mut warned = BTreeSet::new();
        assert!(apply(&f, &q, &parents, &mut warned).is_empty());
        assert_eq!(warned, BTreeSet::from([1]));
        assert!(f.closes.borrow().is_empty() && f.posted.borrow().is_empty());
    }

    #[test]
    fn children_listed_in_team_notes_count() {
        let notes = "## Triage Notes\n\nSplit.\n\n**Children:**\n- #8 first, after #3\n* #9 second\n\nMore text #10\n";
        assert_eq!(listed(notes), [8, 9]);
        assert_eq!(listed("Children: #4, #5 and #6\nnot #7"), [4, 5, 6]);
        assert_eq!(listed("Children:\n\n- #4\n- see #5\n"), [4, 5]);
        assert_eq!(listed("Children:\n- #4\n\n- #5\n"), [4, 5]);
        assert_eq!(listed("No list here: #4\n- #5\n"), Vec::<u64>::new());
        assert_eq!(listed("children:\n- #4\ntext\n- #5\n"), [4]);

        let f = Fake {
            issues: vec![issue(1, &[SPLIT], "")],
            comments: BTreeMap::from([(1, vec!["Children:\n- #2\n".to_string()])]),
            closed: BTreeSet::from([2]),
            ..Fake::default()
        };
        run(&f, false);
        assert_eq!(*f.closes.borrow(), [1]);
    }

    #[test]
    fn ordered_and_checkbox_items_list_children_too() {
        assert_eq!(listed("Children:\n1. #8\n2) #9\n"), [8, 9]);
        assert_eq!(
            listed("Children:\n- [ ] #8\n- [x] #9\n- [X] #10\n"),
            [8, 9, 10]
        );
        assert_eq!(
            listed("Children:\n1. [ ] #8\n- #9\n* [x] #10\n"),
            [8, 9, 10]
        );
        assert_eq!(listed("Children:\n1. #8\n- see #5\n- #9\n"), [8, 5, 9]);
        assert_eq!(listed("Children:\n1. #8\ntext\n- #9\n"), [8]);
    }

    #[test]
    fn a_wrapped_item_keeps_the_list_going() {
        assert_eq!(
            listed("Children:\n- #8 Add the long\n  widget thing\n- #9 second\n"),
            [8, 9]
        );
    }

    #[test]
    fn nested_lines_under_an_item_are_not_children() {
        assert_eq!(
            listed("Children:\n- #8 Add widget\n  - acceptance: foo\n- #9 two\n"),
            [8, 9]
        );
        assert_eq!(
            listed("Children:\n- #8 Add widget\n  - blocked by #5\n"),
            [8]
        );
        assert_eq!(
            listed("Children:\n  - #8\n    - blocked by #5\n  - #9\n"),
            [8, 9]
        );
        assert_eq!(listed("Children:\n- #8\n  more\ntext\n- #9\n"), [8]);
        assert!(super::listed("Children:\n- #8\n- no number\n").is_err());
    }

    #[test]
    fn a_failed_issue_list_is_an_error_not_an_empty_plan() {
        let f = Fake {
            issues: vec![issue(1, &[SPLIT], "")],
            fails: BTreeSet::from(["open_issues"]),
            ..Fake::default()
        };
        let q = QueueConfig::default();
        assert!(plan(&f, &q, false).is_err());
        assert!(plan_read(&f, &q, true).is_err());
    }

    #[test]
    fn an_item_counts_by_its_first_issue_number_wherever_it_sits() {
        let notes = "Children:\n- #8\n- Add widget (#9)\n- [#10](https://example.com/x#3) title\n1. Title #11 and #12\n- [ ] Do (#13)\n";
        assert_eq!(listed(notes), [8, 9, 10, 11, 13]);
    }

    #[test]
    fn an_item_with_no_issue_number_makes_the_parent_unreadable() {
        for text in [
            "Children:\n- #8\n- Add widget\n",
            "Children:\n- Add widget\n- #8\n",
            "Children:\n1. Add widget\n",
            "Children:\n- [ ] Add widget\n",
        ] {
            assert!(super::listed(text).is_err(), "{text}");
        }
        assert_eq!(listed("Children:\n- #8\n- \n- [ ]\n\n"), [8]);

        // Child #8 is done; the item naming no issue must not let the parent close.
        let f = Fake {
            issues: vec![issue(1, &[SPLIT], "")],
            comments: BTreeMap::from([(1, vec!["Children:\n- #8\n- Add widget\n".to_string()])]),
            closed: BTreeSet::from([8]),
            ..Fake::default()
        };
        assert!(run(&f, false).is_empty());
        assert!(f.closes.borrow().is_empty());
        assert!(plan(&f, &QueueConfig::default(), false).unwrap().is_empty());
    }

    #[test]
    fn an_open_child_beside_a_not_planned_one_still_goes_to_a_human() {
        for sweep in [false, true] {
            let mut f = forge(&[3], &[3]);
            if sweep {
                f.issues = vec![issue(1, &[OLD_LABEL], "")];
            }
            let p = plan(&f, &QueueConfig::default(), sweep).unwrap();
            assert_eq!(p[0].action, Action::Dropped(vec![3]), "sweep {sweep}");
            let done = run(&f, sweep);
            assert_eq!(done[0]["action"], "ready_for_human");
            assert!(f.closes.borrow().is_empty());
            assert_eq!(
                f.labels(1).unwrap(),
                ["type:feat", "status:ready-for-human"]
            );
        }
    }

    #[test]
    fn a_swept_parent_that_cannot_be_read_or_moved_leaves_the_sweep_unfinished() {
        let q = QueueConfig::default();
        let sweep_forge = |subs: Vec<u64>| {
            let mut f = forge(&[2, 3], &[]);
            f.issues = vec![issue(1, &[OLD_LABEL], "")];
            f.subs.insert(1, subs);
            f
        };

        let f = sweep_forge(vec![2, 99]);
        let (parents, all_read) = plan_read(&f, &q, true).unwrap();
        assert!(!all_read);
        let done = apply(&f, &q, &parents, &mut BTreeSet::new());
        assert!(!sweep_finished(all_read, &parents, &done));

        let mut f = sweep_forge(vec![2, 3]);
        f.fails.insert("close");
        let (parents, all_read) = plan_read(&f, &q, true).unwrap();
        let done = apply(&f, &q, &parents, &mut BTreeSet::new());
        assert!(all_read && done.is_empty());
        assert!(!sweep_finished(all_read, &parents, &done));

        let f = sweep_forge(vec![2, 3]);
        let (parents, all_read) = plan_read(&f, &q, true).unwrap();
        let done = apply(&f, &q, &parents, &mut BTreeSet::new());
        assert!(sweep_finished(all_read, &parents, &done));
    }

    #[test]
    fn a_failed_label_move_posts_no_comment_and_leaves_the_parent_for_the_next_pass() {
        let mut f = forge(&[2, 3], &[3]);
        f.fails.insert("edit_labels");
        let done = run(&f, false);
        assert!(done.is_empty());
        assert!(f.posted.borrow().is_empty());
        assert!(f.closes.borrow().is_empty());
        assert_eq!(f.labels(1).unwrap(), ["type:feat", SPLIT]);
        let q = QueueConfig::default();
        assert_eq!(
            plan(&f, &q, false).unwrap()[0].action,
            Action::Dropped(vec![3])
        );
    }

    #[test]
    fn a_failed_relabel_is_not_reported_and_leaves_the_sweep_unfinished() {
        let q = QueueConfig::default();
        let mut f = forge(&[2], &[]);
        f.issues = vec![issue(1, &[OLD_LABEL], "")];
        f.labels
            .borrow_mut()
            .insert(1, vec!["type:feat".into(), OLD_LABEL.into()]);
        f.fails.insert("edit_labels");
        let (parents, all_read) = plan_read(&f, &q, true).unwrap();
        assert_eq!(parents[0].action, Action::Relabel(1));
        let done = apply(&f, &q, &parents, &mut BTreeSet::new());
        assert!(done.is_empty());
        assert!(!sweep_finished(all_read, &parents, &done));
        assert_eq!(f.labels(1).unwrap(), ["type:feat", OLD_LABEL]);
    }

    #[test]
    fn a_failed_label_strip_after_the_close_still_reports_the_parent() {
        let mut f = forge(&[2, 3], &[]);
        f.fails.insert("edit_labels");
        let done = run(&f, false);
        assert_eq!(*f.closes.borrow(), [1]);
        assert_eq!(f.posted.borrow().len(), 1);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0]["action"], "close");
        assert_eq!(f.labels(1).unwrap(), ["type:feat", SPLIT]);
    }

    #[test]
    fn a_list_ends_at_a_line_that_only_looks_like_an_item() {
        assert_eq!(listed("Children:\n- #4\n**Notes:**\n- #12"), [4]);
        assert_eq!(listed("Children:\n- #4\n---\n- #12"), [4]);
        assert_eq!(listed("Children:\n- #4\n-foo\n- #12"), [4]);
        assert_eq!(listed("Children:\n- #4\n*emph* text\n- #12"), [4]);
    }

    #[test]
    fn a_swept_parent_is_read_from_sub_issues_and_notes_only() {
        let q = QueueConfig::default();
        let linked = BTreeMap::from([(1, vec![issue(2, &[], "Part of #1")])]);
        let f = Fake {
            issues: vec![issue(1, &[OLD_LABEL], "")],
            linked: linked.clone(),
            closed: BTreeSet::from([2]),
            ..Fake::default()
        };
        assert!(plan(&f, &q, true).unwrap().is_empty());
        assert_eq!(f.searches.get(), 0);

        let f = Fake {
            issues: vec![issue(1, &[SPLIT], "")],
            linked,
            closed: BTreeSet::from([2]),
            ..Fake::default()
        };
        assert_eq!(plan(&f, &q, true).unwrap()[0].children, [2]);
        assert_eq!(f.searches.get(), 1);
    }

    #[test]
    fn an_unreadable_split_parent_does_not_hold_the_sweep_open() {
        let q = QueueConfig::default();
        let mut f = forge(&[2, 3], &[]);
        f.issues = vec![issue(1, &[SPLIT], ""), issue(4, &[OLD_LABEL], "")];
        f.subs.insert(1, vec![99]);
        f.subs.insert(4, vec![2, 3]);
        let (parents, all_read) = plan_read(&f, &q, true).unwrap();
        assert!(all_read);
        assert_eq!(parents.len(), 1);
        let done = apply(&f, &q, &parents, &mut BTreeSet::new());
        assert!(sweep_finished(all_read, &parents, &done));
        assert_eq!(*f.closes.borrow(), [4]);
    }

    #[test]
    fn sub_issues_and_listed_children_together_must_all_be_done() {
        let mut f = forge(&[2, 3], &[]);
        f.comments.insert(1, vec!["Children:\n- #4\n".into()]);
        let q = QueueConfig::default();
        let p = plan(&f, &q, false).unwrap();
        assert_eq!(p[0].children, [2, 3, 4]);
        assert_eq!(p[0].action, Action::Wait(1));
    }

    #[test]
    fn a_body_lists_children_only_for_a_team_parent() {
        let mut parent = issue(1, &[SPLIT], "Children: #2");
        let mut f = Fake {
            issues: vec![parent.clone()],
            closed: BTreeSet::from([2]),
            ..Fake::default()
        };
        let q = QueueConfig::default();
        assert_eq!(plan(&f, &q, false).unwrap()[0].children, [2]);
        parent.team = false;
        f.issues = vec![parent];
        assert_eq!(plan(&f, &q, false).unwrap()[0].action, Action::NoChildren);
    }

    #[test]
    fn part_of_lines_from_team_issues_count_when_nothing_else_names_children() {
        assert!(part_of("Part of #13\n\nbody", 13));
        assert!(part_of("Blocked by: #2\n  part of #13", 13));
        assert!(!part_of("Part of #133", 13));
        assert!(!part_of("Not part of #13", 13));
        assert!(!part_of("Part of #12", 13));

        let mut outsider = issue(4, &[], "Part of #1");
        outsider.team = false;
        let f = Fake {
            issues: vec![issue(1, &[SPLIT], "")],
            linked: BTreeMap::from([(
                1,
                vec![
                    issue(2, &[], "Part of #1"),
                    issue(3, &[], "Part of #11"),
                    outsider,
                ],
            )]),
            closed: BTreeSet::from([2]),
            ..Fake::default()
        };
        let q = QueueConfig::default();
        assert_eq!(plan(&f, &q, false).unwrap()[0].children, [2]);
        run(&f, false);
        assert_eq!(*f.closes.borrow(), [1]);
    }

    #[test]
    fn the_sweep_moves_old_split_parents_and_skips_other_needs_define_issues() {
        let mut f = forge(&[2], &[]);
        f.issues = vec![
            issue(1, &[OLD_LABEL], ""),
            issue(5, &[OLD_LABEL], ""),
            issue(6, &[OLD_LABEL], ""),
        ];
        f.subs.insert(6, vec![7]);
        f.closed.insert(7);
        for n in [1, 5, 6] {
            f.labels.borrow_mut().insert(n, vec![OLD_LABEL.into()]);
        }
        let planned: Vec<u64> = plan(&f, &QueueConfig::default(), true)
            .unwrap()
            .iter()
            .map(|p| p.issue.number)
            .collect();
        assert_eq!(planned, [1, 6]);
        let done = run(&f, true);
        assert_eq!(f.labels(1).unwrap(), [SPLIT]);
        assert_eq!(*f.created.borrow(), [SPLIT]);
        assert_eq!(f.labels(5).unwrap(), [OLD_LABEL]);
        assert_eq!(*f.closes.borrow(), [6]);
        let actions: Vec<_> = done
            .iter()
            .map(|d| (d["number"].clone(), d["action"].clone()))
            .collect();
        assert_eq!(
            actions,
            [(json!(1), json!("relabel")), (json!(6), json!("close"))]
        );
        assert_eq!(done[0]["open"], 1);

        // Without the sweep, the old label is someone else's state.
        let f = forge(&[2], &[]);
        let mut g = f;
        g.issues = vec![issue(1, &[OLD_LABEL], "")];
        assert!(plan(&g, &QueueConfig::default(), false).unwrap().is_empty());
    }

    #[test]
    fn a_split_label_set_to_the_old_one_sweeps_nothing_twice() {
        let f = forge(&[2, 3], &[]);
        let q = QueueConfig {
            split_label: OLD_LABEL.into(),
            ..QueueConfig::default()
        };
        let mut g = f;
        g.issues = vec![issue(1, &[OLD_LABEL], "")];
        assert_eq!(plan(&g, &q, true).unwrap().len(), 1);
    }

    #[test]
    fn the_sweep_runs_once_per_clone() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(sweep_due(tmp.path()));
        swept(tmp.path()).unwrap();
        assert!(!sweep_due(tmp.path()));
    }

    #[test]
    fn a_parent_never_counts_as_its_own_child() {
        let mut f = forge(&[2, 3], &[]);
        f.subs.insert(1, vec![1, 2, 3]);
        let p = plan(&f, &QueueConfig::default(), false).unwrap();
        assert_eq!(p[0].children, [2, 3]);
    }

    #[test]
    fn a_parent_that_does_not_close_keeps_its_label_and_gets_no_comment() {
        let mut f = forge(&[2, 3], &[]);
        f.fails.insert("close");
        assert!(run(&f, false).is_empty());
        assert!(f.posted.borrow().is_empty());
        assert_eq!(f.labels(1).unwrap(), ["type:feat", SPLIT]);
    }

    #[test]
    fn a_comment_that_fails_still_closes_and_clears_the_parent() {
        let mut f = forge(&[2, 3], &[]);
        f.fails.insert("comment");
        assert_eq!(run(&f, false)[0]["action"], "close");
        assert_eq!(*f.closes.borrow(), [1]);
        assert_eq!(f.labels(1).unwrap(), ["type:feat"]);

        let mut f = forge(&[2, 3], &[2]);
        f.fails.insert("comment");
        assert_eq!(run(&f, false)[0]["action"], "ready_for_human");
        assert_eq!(
            f.labels(1).unwrap(),
            ["type:feat", "status:ready-for-human"]
        );
    }

    #[test]
    fn a_parent_whose_child_cannot_be_read_is_left_out() {
        let mut f = forge(&[2, 3], &[]);
        f.subs.insert(1, vec![2, 99]);
        assert!(plan(&f, &QueueConfig::default(), false).unwrap().is_empty());
        assert!(run(&f, false).is_empty());
        assert!(f.closes.borrow().is_empty());
    }

    #[test]
    fn part_of_lines_are_not_read_when_the_parent_names_its_children() {
        let mut f = forge(&[2, 3], &[]);
        f.linked.insert(1, vec![issue(4, &[], "Part of #1")]);
        let p = plan(&f, &QueueConfig::default(), false).unwrap();
        assert_eq!(p[0].children, [2, 3]);
        assert_eq!(p[0].action, Action::Close);
    }

    #[test]
    fn a_parent_under_both_labels_is_planned_once() {
        let mut f = forge(&[2], &[]);
        f.issues = vec![issue(1, &[SPLIT, OLD_LABEL], "")];
        let p = plan(&f, &QueueConfig::default(), true).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].action, Action::Wait(1));
    }

    #[test]
    fn mark_moves_the_issue_even_when_the_label_cannot_be_created() {
        let mut f = Fake::default();
        f.fails.insert("ensure_label");
        mark(&f, &QueueConfig::default(), 4).unwrap();
        assert_eq!(f.labels(4).unwrap(), [SPLIT]);
    }

    #[test]
    fn mark_creates_the_split_label_and_moves_the_issue() {
        let f = Fake::default();
        f.labels
            .borrow_mut()
            .insert(4, vec!["status:in-progress".into()]);
        mark(&f, &QueueConfig::default(), 4).unwrap();
        assert_eq!(f.labels(4).unwrap(), [SPLIT]);
        assert_eq!(*f.created.borrow(), [SPLIT]);
    }
}
