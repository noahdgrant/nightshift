//! Settling units: the labels, comments and PR edits each outcome earns. Moved from
//! tests/factory/watch_nights_and_splits.rs.

use serde_json::json;

use super::{Sim, READY};
use crate::watch::forge::DISCLOSURE;

/// A night with #2 and #3 ready, one unit at most.
fn once() -> Sim {
    let mut sim = Sim::new();
    sim.ready(2, "Fix a", &["type:fix"]);
    sim.max_units = Some(1);
    sim
}

#[test]
fn watch_once_swaps_labels_on_done() {
    let mut sim = once();
    sim.ready(3, "Fix b", &["type:fix"]);
    let stopped = sim.run(|_, _| json!({"outcome": "done"})).unwrap();
    assert_eq!(sim.units, [(2, "done".to_string())]);
    assert_eq!(stopped, "max_units");
    assert_eq!(sim.claimed, [2]);
    assert_eq!(sim.labels(2), ["type:fix", "status:in-review"]);
    assert_eq!(sim.labels(3), ["type:fix", READY]);
}

#[test]
fn watch_stuck_swaps_label_and_comments() {
    let mut sim = once();
    let reason = "brief.md is missing: triage wrote no brief";
    sim.run(|_, _| json!({"outcome": "stuck", "reason": reason}))
        .unwrap();
    assert_eq!(sim.units, [(2, "stuck".to_string())]);
    assert_eq!(sim.labels(2), ["type:fix", "status:ready-for-human"]);
    let comments = sim.comments(2);
    assert_eq!(comments.len(), 1, "{comments:?}");
    let all = &comments[0];
    assert!(all.starts_with(&format!("{DISCLOSURE}\n\n")), "{all}");
    assert!(all.contains("triage wrote no brief"), "{all}");
    assert!(all.contains("Last artifact"));
}

/// The PR labels added or removed and the PR comments, after a night.
fn pr_edits(sim: &Sim) -> (usize, usize) {
    (
        sim.forge.pr_labels.borrow().len(),
        sim.forge.pr_comments.borrow().len(),
    )
}

#[test]
fn watch_done_without_a_pr_leaves_prs_alone() {
    let mut sim = once();
    sim.run(|_, _| json!({"outcome": "done"})).unwrap();
    assert_eq!(sim.units, [(2, "done".to_string())]);
    assert_eq!(pr_edits(&sim), (0, 0));
}

#[test]
fn watch_stuck_after_ship_leaves_the_pr_alone() {
    let mut sim = once();
    let blocked = "pr.md is blocked: ship said blocked CI never started";
    sim.run(|_, _| json!({"outcome": "stuck", "pr": 12, "reason": blocked}))
        .unwrap();
    assert_eq!(sim.units, [(2, "stuck".to_string())]);
    assert_eq!(pr_edits(&sim), (0, 0));
}

#[test]
fn watch_paused_at_ship_leaves_the_pr_alone() {
    let mut sim = once();
    let reset = sim.now + 36_000;
    sim.deadline = Some(sim.now + 3600);
    let stopped = sim
        .run(|_, _| json!({"outcome": "paused", "pr": 12, "reset_at": reset}))
        .unwrap();
    assert_eq!(sim.units, [(2, "paused".to_string())]);
    assert_eq!(stopped, "usage limit resets after --until");
    assert_eq!(pr_edits(&sim), (0, 0));
}

#[test]
fn watch_stuck_comment_quotes_the_blocker_and_a_relative_path() {
    let mut sim = once();
    let base = tempfile::tempdir().unwrap();
    let unit = "2-fix-a";
    let artifact = base.path().join(format!("wt/.ns/{unit}/evidence.md"));
    let reason = "evidence.md is blocked: verify said blocked needs a board";
    let fields = json!({"outcome": "stuck", "reason": reason, "artifact": artifact});
    sim.run(|_, _| fields.clone()).unwrap();
    assert_eq!(sim.units, [(2, "stuck".to_string())]);
    let comment = &sim.comments(2)[0];
    assert!(comment.contains(reason), "{comment}");
    assert!(
        comment.contains(&format!("`.ns/{unit}/evidence.md` on branch `ns/{unit}`")),
        "{comment}"
    );
    assert!(
        !comment.contains(base.path().to_str().unwrap()),
        "{comment}"
    );
}

#[test]
fn a_blocked_review_comment_lists_its_open_findings() {
    let mut sim = once();
    let dir = tempfile::tempdir().unwrap();
    let review = dir.path().join("review.md");
    std::fs::write(
        &review,
        "---\nstatus: blocked\n---\n\n## Critical\n### C1. cancel_all drops every SKU\n- Location: `src/w.py:119`\n- Status: open\n\
         \n## Important\n### I1. Fixed thing\n- Location: `src/w.py:3`\n- Status: fixed (cycle 1, abc)\n\
         ### I2. No test for UnknownItem\n- Location: `src/w.py:118`\n- Status: open. The fix did not land.\n\
         ### I3. Old code smell\n- Location: `src/old.py:9`\n- Status: deferred: #77\n\
         \n## Suggestion\n### S1. Rename x\n- Location: `src/w.py:5`\n- Status: open\n",
    )
    .unwrap();
    let reason =
        "review.md is blocked: review said blocked Open after at least 2 fix attempts: C1, I2.";
    let fields = json!({"outcome": "stuck", "reason": reason, "artifact": review});
    sim.run(|_, _| fields.clone()).unwrap();
    assert_eq!(sim.units, [(2, "stuck".to_string())]);
    let comment = &sim.comments(2)[0];
    assert!(
        comment.contains(
            "Open findings in `review.md`:\n```text\nC1. cancel_all drops every SKU (src/w.py:119)\nI2. No test for UnknownItem (src/w.py:118)\n```"
        ),
        "{comment}"
    );
    for gone in ["I1.", "I3.", "S1."] {
        assert!(!comment.contains(gone), "{gone} in {comment}");
    }
}

/// A night on #2 whose unit's triage adds `stray`, a status label, before the unit ends with
/// `fields`: #2's labels after.
fn labels_after_triage_adds(stray: &str, fields: serde_json::Value) -> Vec<String> {
    let mut sim = once();
    sim.fill().unwrap();
    sim.forge
        .labels
        .borrow_mut()
        .get_mut(&2)
        .unwrap()
        .push(stray.into());
    sim.end(2, fields).unwrap();
    let labels = sim.labels(2);
    assert!(labels.contains(&"type:fix".to_string()), "{labels:?}");
    labels
}

#[test]
fn watch_done_leaves_only_the_done_label() {
    let labels = labels_after_triage_adds(READY, json!({"outcome": "done"}));
    assert_eq!(labels, ["type:fix", "status:in-review"]);
}

#[test]
fn watch_stuck_leaves_only_the_stuck_label() {
    let labels = labels_after_triage_adds(READY, json!({"outcome": "stuck"}));
    assert_eq!(labels, ["type:fix", "status:ready-for-human"]);
}
