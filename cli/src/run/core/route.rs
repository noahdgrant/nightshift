//! Routing: which artifacts hold at HEAD, and the phase `ns run` drives next (the state table in
//! docs/factory/run.md). Pure: the shell reads the artifacts and the git facts into a [`State`],
//! and nothing here reads a file or runs git.

use std::collections::BTreeMap;

use super::Ledger;
use crate::factory::{artifact_of, LocalGate, PHASES};

/// `a` and `b` name the same commit: one is a prefix of the other. The same rule as
/// `git::same_sha`, kept here so the core needs no git module.
pub fn same_sha(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

#[derive(Debug, Clone, Default)]
pub struct Art {
    pub status: String,
    pub sha: Option<String>,
    pub pr: Option<String>,
    pub body: String,
    /// What `ns artifact check` rejects in an `ns-artifact/2` file. An invalid artifact counts as
    /// absent: its phase runs again.
    pub invalid: Option<Vec<String>>,
    /// An `ns-artifact/2` file's `next`, which routes it; `None` for `/1`.
    pub next: Option<String>,
}

/// The factory settings a decision reads: plain data the shell copies from the factory definition
/// and the run's flags.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Each phase's `max_attempts`.
    pub max_attempts: BTreeMap<&'static str, u32>,
    /// `merge.policy = "auto"`: a done unit goes on to the merge step.
    pub merge_auto: bool,
    /// `--triage-only`: the run ends after triage, by its exit.
    pub triage_only: bool,
    /// `[gate] local`.
    pub local_gate: LocalGate,
}

impl Settings {
    /// `phase`'s `max_attempts`; 2, the factory default, for a phase the shell left out.
    pub fn max_attempts(&self, phase: &str) -> u32 {
        self.max_attempts.get(phase).copied().unwrap_or(2)
    }
}

/// The unit as the shell read it: its artifacts, HEAD and the git facts a decision needs, all
/// precomputed.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub arts: BTreeMap<&'static str, Art>,
    pub head: String,
    pub has_issue: bool,
    /// The unit's diff ids against the base, which tell a rebase from a change; `None` compares
    /// shas only.
    pub diffs: Option<Diffs>,
    /// For each verify or review sha [`facts_wanted`] names: what changed between it and HEAD.
    pub facts: BTreeMap<String, CarryFacts>,
    /// Verify and review artifacts that are not current, but only because ship took in the base
    /// and that changed few of the unit's own lines (CORE-1): evidence carries forward, and
    /// review runs over just that change. Filled by [`carry_forward`].
    pub carried: BTreeMap<&'static str, Carried>,
    pub settings: Settings,
}

/// The unit's diff ids (`git::diff_id`): HEAD's, and that of each sha an artifact names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diffs {
    /// HEAD's diff id; `None` when git could not compute it, which matches nothing.
    pub head: Option<String>,
    /// The diff id of every sha an artifact names that isn't HEAD.
    pub of: BTreeMap<String, Option<String>>,
}

/// What the base brought into the unit between an artifact's sha and HEAD.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CarryFacts {
    /// The unit's start moved: the change took in the base (`git::took_in_base`).
    pub took_in_base: bool,
    /// How many of the unit's own changed lines differ (`git::interdiff_lines`); read only when
    /// `took_in_base`.
    pub interdiff_lines: Option<usize>,
}

/// An artifact carried forward over a small change the base brought in.
#[derive(Debug, Clone, PartialEq)]
pub struct Carried {
    /// The sha the artifact was written at.
    pub from: String,
    /// How many of the unit's own changed lines differ since.
    pub lines: usize,
}

/// The most changed lines of the unit's own diff a base merge may bring in and still carry
/// evidence forward with only a delta review (CORE-1 in docs/design/factory-core/v1.md).
pub const DELTA_MAX_LINES: usize = 40;

impl State {
    /// `sha` is HEAD, or carries the same diff against the base as HEAD (a rebase).
    pub fn current(&self, sha: &str) -> bool {
        if same_sha(sha, &self.head) {
            return true;
        }
        let Some(d) = &self.diffs else {
            return false;
        };
        debug_assert!(
            sha.is_empty() || d.of.contains_key(sha),
            "the shell did not precompute the diff id of {sha}"
        );
        d.head.is_some() && d.of.get(sha) == Some(&d.head)
    }

    pub fn passes(&self, a: &Art) -> bool {
        a.status == "pass" && self.current(a.sha.as_deref().unwrap_or(""))
    }
}

/// The shas whose [`CarryFacts`] [`carry_forward`] reads: each verify or review sha that is not
/// current, where only ship moved HEAD since.
pub fn facts_wanted(l: &Ledger, s: &State) -> Vec<String> {
    ["verify", "review"]
        .iter()
        .filter_map(|p| s.arts.get(p)?.sha.clone())
        .filter(|sha| !s.current(sha) && l.only_ship_moved(sha, &s.head))
        .collect()
}

/// The verify and review artifacts that only a small base merge by ship made stale, so they carry
/// forward (CORE-1).
pub fn carry_forward(l: &Ledger, s: &State) -> BTreeMap<&'static str, Carried> {
    let mut out = BTreeMap::new();
    for phase in ["verify", "review"] {
        let Some(sha) = s.arts.get(phase).and_then(|a| a.sha.clone()) else {
            continue;
        };
        if s.current(&sha) || !l.only_ship_moved(&sha, &s.head) {
            continue;
        }
        debug_assert!(
            s.facts.contains_key(&sha),
            "the shell did not gather the carry facts of {sha}"
        );
        let f = s.facts.get(&sha).copied().unwrap_or_default();
        let Some(lines) = f.interdiff_lines.filter(|_| f.took_in_base) else {
            continue;
        };
        if lines <= DELTA_MAX_LINES {
            out.insert(phase, Carried { from: sha, lines });
        }
    }
    out
}

/// The phases whose artifacts are archived before `phase` runs: its own, and every downstream one
/// that is not `pass` and current, or follows one that is archived. Whatever is left is current,
/// so presence alone drives [`decide`].
pub fn to_archive(phase: &str, s: &State) -> Vec<&'static str> {
    let Some(i) = PHASES.iter().position(|p| *p == phase) else {
        return vec![];
    };
    let mut out = vec![PHASES[i]];
    let mut cascading = true;
    for p in &PHASES[i + 1..] {
        cascading = cascading && s.arts.get(p).is_some_and(|a| s.passes(a));
        if !cascading {
            out.push(p);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Run {
        phase: &'static str,
        feedback: String,
        why: String,
    },
    Done,
    Stuck(String),
    /// Triage split the issue into child issues, which replace the unit.
    Split(String),
}

pub fn run(phase: &'static str, feedback: &str, why: impl Into<String>) -> Decision {
    Decision::Run {
        phase,
        feedback: feedback.to_string(),
        why: why.into(),
    }
}

/// The body's first non-empty line, the contract's one-sentence reason. A heading marker is
/// dropped and its text kept.
pub fn first_line(body: &str) -> String {
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim_start())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

/// A NO-GO whose first line starts with `base moved`, after an optional `NO-GO:`: the branch only needs a rebase.
pub fn base_moved(body: &str) -> bool {
    let line = first_line(body).to_lowercase();
    let rest = line.strip_prefix("no-go:").unwrap_or(&line);
    rest.trim_start().starts_with("base moved")
}

/// `d` sends a passed build back only to rebase, because verify, review or ship said the base
/// moved.
pub fn rebases_for_base(s: &State, d: &Decision) -> bool {
    matches!(d, Decision::Run { phase: "build", feedback, .. } if base_moved(feedback))
        && s.arts.get("build").is_some_and(|b| b.status == "pass")
}

/// The phase a failed ship names first: `verify` or `review`.
fn named_phase(body: &str) -> Option<&'static str> {
    let lower = body.to_lowercase();
    let v = lower.find("verify");
    let r = lower.find("review");
    match (v, r) {
        (Some(a), Some(b)) => Some(if a < b { "verify" } else { "review" }),
        (Some(_), None) => Some("verify"),
        (None, Some(_)) => Some("review"),
        (None, None) => None,
    }
}

/// What a delta review is told: review only what the base merge changed in the unit's diff.
fn delta_feedback(c: &Carried, head: &str) -> String {
    format!(
        "Delta review. review.md passed at {from}. Since then only ship's merge of the base changed \
         this unit's own diff, by {n} lines, so verify's evidence carries forward. Review only \
         that change, not the whole unit: compare the unit's diff at {from} with its diff at \
         {head}, for example with `git range-diff $(git merge-base {from} origin/HEAD)..{from} \
         $(git merge-base {head} origin/HEAD)..{head}`. Keep the earlier review's findings for \
         everything else, and add `delta_of: {from}` to review.md's frontmatter.",
        from = c.from,
        n = c.lines,
    )
}

/// The state table in docs/factory/run.md.
pub fn decide(s: &State) -> Decision {
    if let Some(brief) = s.arts.get("triage").filter(|a| a.status == "split") {
        return Decision::Split(format!("brief.md is split: {}", first_line(&brief.body)));
    }
    if let Some(p) = blocked(s) {
        let a = &s.arts[p];
        return Decision::Stuck(format!(
            "{} is blocked: {}",
            artifact_of(p),
            first_line(&a.body)
        ));
    }
    let passes = |p: &str| s.arts.get(p).is_some_and(|a| s.passes(a));
    let verify_carried = s.carried.contains_key("verify")
        && s.arts.get("verify").is_some_and(|a| a.status == "pass");
    if passes("ship") && (passes("verify") || verify_carried) && passes("review") {
        return Decision::Done;
    }
    let Some(brief) = s.arts.get("triage") else {
        return if s.has_issue {
            run("triage", "", "no brief.md")
        } else {
            Decision::Stuck("no brief: no brief.md and no issue to triage".into())
        };
    };
    if let Some(n) = brief.next.as_deref().filter(|n| *n != "build") {
        return Decision::Stuck(format!(
            "triage routed the issue to {n}: {}",
            first_line(&brief.body)
        ));
    }
    if brief.status != "pass" {
        return Decision::Stuck(format!(
            "triage decided a human or define is needed (brief.md status {:?})",
            brief.status
        ));
    }
    let Some(b) = s.arts.get("build") else {
        return run("build", "", "no build.md");
    };
    match b.status.as_str() {
        "pass" => {}
        "fail" => return run("build", &b.body, "build.md failed; build retries"),
        other => return Decision::Stuck(format!("build.md has status {other:?}")),
    }
    if let Some(d) = decide_checks(s) {
        return d;
    }
    let Some(pr) = s.arts.get("ship") else {
        return run("ship", "", "no pr.md");
    };
    decide_ship(pr, stale_why(s, "pr.md", pr))
}

/// The first phase, in order, whose artifact is `blocked`.
pub fn blocked(s: &State) -> Option<&'static str> {
    PHASES
        .iter()
        .copied()
        .find(|p| s.arts.get(p).is_some_and(|a| a.status == "blocked"))
}

fn stale_why(s: &State, file: &str, a: &Art) -> String {
    format!(
        "{file} sha {} is not current at HEAD {}",
        a.sha.as_deref().unwrap_or("(none)"),
        s.head
    )
}

/// Verify's and review's rows of the state table: the decision when either doesn't hold.
fn decide_checks(s: &State) -> Option<Decision> {
    for (phase, file) in [("verify", "evidence.md"), ("review", "review.md")] {
        let Some(a) = s.arts.get(phase) else {
            return Some(run(phase, "", format!("no {file}")));
        };
        match s.carried.get(phase).filter(|_| a.status == "pass") {
            Some(_) if phase == "verify" => continue,
            Some(c) => {
                return Some(run(
                    "review",
                    &delta_feedback(c, &s.head),
                    format!(
                    "{file} sha {} is behind a {}-line change the base brought in: delta review",
                    c.from, c.lines
                ),
                ))
            }
            None => {}
        }
        if !s.current(a.sha.as_deref().unwrap_or("")) {
            return Some(run(phase, "", stale_why(s, file, a)));
        }
        match (a.status.as_str(), a.next.as_deref()) {
            ("pass", _) => {}
            ("fail", Some("human")) => {
                return Some(Decision::Stuck(format!(
                    "{file} failed and needs a human: {}",
                    first_line(&a.body)
                )))
            }
            ("fail", _) => return Some(run("build", &a.body, format!("{file} failed"))),
            (other, _) => return Some(Decision::Stuck(format!("{file} has status {other:?}"))),
        }
    }
    None
}

/// Where a `pr.md` sends the unit: by its `next` when it is `ns-artifact/2`, else by the `/1`
/// rules (a `base moved` first line to build, a named verify or review to that phase). `stale`
/// is why a passed one that isn't current reruns ship.
pub fn decide_ship(pr: &Art, stale: String) -> Decision {
    if pr.status == "fail" {
        if let Some(n) = pr.next.as_deref() {
            return match n {
                "build" => run(
                    "build",
                    &pr.body,
                    format!("pr.md failed and names build: {}", first_line(&pr.body)),
                ),
                "verify" | "review" => run(
                    if n == "verify" { "verify" } else { "review" },
                    &pr.body,
                    format!("pr.md failed and names {n}"),
                ),
                "ship" => run("ship", &pr.body, "pr.md is not merge-ready yet"),
                _ => Decision::Stuck(format!("ship failed: {}", first_line(&pr.body))),
            };
        }
    }
    match pr.status.as_str() {
        "pass" => run("ship", "", stale),
        "fail" if base_moved(&pr.body) => run("build", &pr.body, "pr.md failed: base moved"),
        "fail" => match named_phase(&pr.body) {
            Some(p) => run(
                if p == "verify" { "verify" } else { "review" },
                &pr.body,
                format!("pr.md failed and names {p}"),
            ),
            None => Decision::Stuck(format!("ship failed: {}", first_line(&pr.body))),
        },
        other => Decision::Stuck(format!("pr.md has status {other:?}")),
    }
}
