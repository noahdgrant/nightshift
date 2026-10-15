//! An in-memory driver over [`step`], for routing scenarios: the same transition function
//! `run/drive.rs` drives, with a fake unit in place of the worktree, the harness, the gate, the
//! forge and the merge step's GitHub (`sim/merge.rs`). It mirrors the fake harness of
//! `cli/tests/fixtures/factory/claude`: each phase pops its next action from a queue,
//! `status[:extra]`, where `status` is `pass`, `fail`, `blocked`, `none` (no artifact, exit 0) or
//! `crash` (no artifact, exit 1), and `extra` is `+`-joined words: `commit` moves HEAD,
//! `next=<phase>` and `find=<id>,<id>` set the artifact's `next` and `findings` (a fix cycle's
//! `fix.md` lists the ids it was handed, and the pass after it counts an attempt for each),
//! `movemain` moves the default branch, `sleep` times the phase out, and any other text lands in
//! the artifact's body (`base moved` as its first line). The guards' words: `pushmain` pushes
//! HEAD to the default branch, `landmain` lands [`Sim::land_pr`] there and rebases onto it,
//! `merge` merges the unit's PR, `noorigin` loses the origin, and `touch=<path>` makes the
//! phase's commit change `path`, for a red gate's culprit. The merge step's: `dirty` and `clean`
//! set the PR's merge state on GitHub.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

use serde_json::{json, Value};

use super::gate::{culprit, Suspect};
use super::guard::{branch_moved, moved_by_others, pr_merged, Unread};
use super::merge::Ask;
use super::route::{
    facts_wanted, fix_cycle, review_run, to_archive, Art, ReviewLoop, Settings, State,
};
use super::step::{step, Action, Event, GateRan, GateRed, GuardFacts, PhaseRan};
use super::{Ledger, Rebase};
use crate::factory::RUN_PHASES;
use crate::run::Outcome;
use crate::watch::forge::CommitPr;

mod merge;

pub use merge::{check, FakeMerge};

/// A fake unit and the scripts of its phases.
pub struct Sim {
    pub settings: Settings,
    pub has_issue: bool,
    /// The default branch's name and the number of times it moved, or `None` without an origin.
    pub remote: Option<(String, u32)>,
    /// Each gate's verdict in order, `Err` carrying a red gate's output; `None` sets no gate
    /// command. An empty queue goes green. The merge step's gate takes from it too.
    pub gates: Option<VecDeque<Result<(), String>>>,
    /// The unit's name, whose branch is `ns/<unit>`.
    pub unit: String,
    /// The PR `landmain` lands on the default branch.
    pub land_pr: CommitPr,
    /// The forge's query for a commit's PRs fails with this.
    pub forge_fails: Option<String>,
    /// The merge step's GitHub.
    pub merge: FakeMerge,
    queues: BTreeMap<&'static str, VecDeque<String>>,
    arts: BTreeMap<&'static str, Art>,
    commits: u32,
    red: Option<String>,
    /// The move of the default branch the unit's HEAD holds.
    held: u32,
    /// The PRs the forge links to each move's commit; a move with none came from no PR.
    main_prs: BTreeMap<u32, Vec<CommitPr>>,
    /// The files each commit changes, by its number.
    touched: BTreeMap<u32, Vec<String>>,
    /// The move of the default branch the guards saw last.
    baseline: u32,
    /// The time, in seconds; the merge step's sleeps move it.
    now: i64,
    /// A phase merged the unit's PR.
    pr_merged: bool,
    /// The review loop as `ns`'s record and `review.md` would give it.
    review: ReviewLoop,
}

/// One phase run the core admitted.
#[derive(Debug, Clone)]
pub struct Run {
    pub phase: &'static str,
    pub attempt: u32,
    /// The feedback its prompt carries.
    pub feedback: String,
    pub why: String,
    pub rebase: Option<(Rebase, String, String)>,
    pub follows_base: bool,
}

/// How a scenario ended, and what ran on the way.
#[derive(Debug)]
pub struct Report {
    pub outcome: Outcome,
    pub reason: String,
    pub phase: Option<&'static str>,
    /// The merge step ended the unit for a human to merge.
    pub needs_human: bool,
    pub runs: Vec<Run>,
    /// The run-log events the core asked for, and those the shell writes from its actions.
    pub logs: Vec<Value>,
    /// Each gate the core asked for: the phase it followed (`merge` for the merge step's) and
    /// whether it went green.
    pub gates: Vec<(&'static str, bool)>,
    /// What the merge step asked the shell for, in order.
    pub asks: Vec<Ask>,
}

impl Report {
    /// The phases run, in order: the e2e tests' `calls()`.
    pub fn calls(&self) -> Vec<&'static str> {
        self.runs.iter().map(|r| r.phase).collect()
    }

    /// The runs that took in the base for free: the e2e tests' `rebase_after_base_moved` events.
    pub fn rebases(&self) -> Vec<&Run> {
        self.runs.iter().filter(|r| r.rebase.is_some()).collect()
    }

    pub fn logged(&self, event: &str) -> bool {
        self.logs.iter().any(|l| l["event"] == event)
    }

    /// The run-log events named `event`.
    pub fn events(&self, event: &str) -> Vec<&Value> {
        self.logs.iter().filter(|l| l["event"] == event).collect()
    }

    /// How many times the merge step asked for `ask`, matched by its variant.
    pub fn asked(&self, ask: &Ask) -> usize {
        let d = std::mem::discriminant(ask);
        self.asks
            .iter()
            .filter(|a| std::mem::discriminant(*a) == d)
            .count()
    }

    /// The heads the merge step squash-merged at.
    pub fn merged_at(&self) -> Vec<&str> {
        self.asks
            .iter()
            .filter_map(|a| match a {
                Ask::Merge { head } => Some(head.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// The most steps a scenario takes before the sim calls it a loop.
const MAX_STEPS: usize = 1000;

/// What one scenario collects on its way.
#[derive(Default)]
struct Trace {
    runs: Vec<Run>,
    logs: Vec<Value>,
    gates: Vec<(&'static str, bool)>,
    asks: Vec<Ask>,
}

impl Sim {
    /// A unit with an issue, an origin whose default branch is `main`, no gate, and the factory
    /// defaults.
    pub fn new() -> Self {
        Self {
            settings: Settings {
                max_attempts: RUN_PHASES.iter().map(|p| (*p, 2)).collect(),
                ..Settings::default()
            },
            has_issue: true,
            remote: Some(("main".into(), 0)),
            gates: None,
            unit: "7-fix-the-thing".into(),
            land_pr: CommitPr {
                number: 3,
                head: "ns/3-fix-b".into(),
                merged: true,
            },
            forge_fails: None,
            merge: FakeMerge::default(),
            queues: BTreeMap::new(),
            arts: BTreeMap::new(),
            commits: 0,
            red: None,
            held: 0,
            main_prs: BTreeMap::new(),
            touched: BTreeMap::new(),
            baseline: 0,
            now: 1_000,
            pr_merged: false,
            review: ReviewLoop::default(),
        }
    }

    /// The fake harness's `e.queue(phase, actions)`.
    pub fn queue(&mut self, phase: &'static str, actions: &[&str]) {
        self.queues
            .entry(phase)
            .or_default()
            .extend(actions.iter().map(|a| a.to_string()));
    }

    pub fn max_attempts(&mut self, phase: &'static str, n: u32) {
        self.settings.max_attempts.insert(phase, n);
    }

    /// A gate whose verdicts are `verdicts`, then green.
    pub fn gate(&mut self, verdicts: &[Result<(), &str>]) {
        let v = verdicts.iter().map(|r| r.map_err(String::from));
        self.gates = Some(v.collect());
    }

    fn head(&self) -> String {
        format!("c{:06}", self.commits)
    }

    fn main_sha(n: u32) -> String {
        format!("m{n:06}")
    }

    fn remote_now(&self) -> Option<(String, String)> {
        self.remote
            .as_ref()
            .map(|(b, n)| (b.clone(), Self::main_sha(*n)))
    }

    fn state(&self, l: &Ledger) -> State {
        let mut s = State {
            arts: self.arts.clone(),
            head: self.head(),
            has_issue: self.has_issue,
            settings: self.settings.clone(),
            review: self.review.clone(),
            ..State::default()
        };
        for sha in facts_wanted(l, &s) {
            s.facts.insert(sha, Default::default());
        }
        s
    }

    /// Drive the unit from a fresh ledger to an outcome.
    pub fn run(&mut self) -> Report {
        self.run_from(Ledger::new(None))
    }

    pub fn run_from(&mut self, mut ledger: Ledger) -> Report {
        let mut t = Trace::default();
        let mut state = self.state(&ledger);
        let mut action = Action::Decide;
        self.baseline = self.remote.as_ref().map_or(0, |r| r.1);
        for _ in 0..MAX_STEPS {
            let ev = match action {
                Action::Decide => {
                    state = self.state(&ledger);
                    Event::Start
                }
                Action::ReadRemote => Event::Remote(self.remote_now()),
                Action::RunPhase(a) => {
                    if a.follows_base {
                        t.logs
                            .push(json!({"event": "ship_after_base_moved", "attempt": a.attempt}));
                    }
                    t.runs.push(Run {
                        phase: a.phase,
                        attempt: a.attempt,
                        feedback: a.prompt_feedback(),
                        why: a.why.clone(),
                        rebase: a.rebase.clone(),
                        follows_base: a.follows_base,
                    });
                    Event::PhaseRan(self.run_phase(a.phase, &state))
                }
                Action::CheckGuards => {
                    let phase = t.runs.last().map_or("", |r| r.phase);
                    Event::GuardFacts(self.guard(phase, &mut t.logs))
                }
                Action::SetAside { phase, .. } => {
                    self.arts.remove(phase);
                    Event::Archived
                }
                Action::RunGate(call) => {
                    let phase = t.runs.last().map_or("", |r| r.phase);
                    let g = self.gate_after(call.rebased_onto, &ledger);
                    t.gates.push((phase, !matches!(g, GateRan::Red(_))));
                    Event::GateRan(g)
                }
                Action::Merge { .. } => match self.merge_step(&state, &mut t) {
                    Ok(back) => Event::MergeEnded(back),
                    Err((outcome, reason, needs_human)) => {
                        return t.report(outcome, reason, Some("merge"), needs_human)
                    }
                },
                Action::Finish(e) => {
                    t.logs.extend(e.log);
                    return t.report(e.outcome, e.reason, e.phase, false);
                }
            };
            action = step(&mut ledger, &state, ev);
        }
        panic!("sim: no outcome after {MAX_STEPS} steps; ran {:?}", t.runs)
    }

    /// Run `phase` as the fake harness would, in the unit at `state`.
    fn run_phase(&mut self, phase: &'static str, state: &State) -> PhaseRan {
        let saved: Vec<(&'static str, Art)> = to_archive(phase, state)
            .into_iter()
            .filter_map(|p| Some((p, self.arts.remove(p)?)))
            .collect();
        let action = self
            .queues
            .get_mut(phase)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| "pass".into());
        let (status, extra) = action.split_once(':').unwrap_or((&action, ""));
        let words: Vec<&str> = extra.split('+').collect();
        let head_before = self.head();
        self.act(&words);
        let timed_out = words.contains(&"sleep");
        let written = !matches!(status, "none" | "crash") && !timed_out;
        if written {
            let body = if extra.starts_with("base moved") {
                extra.to_string()
            } else {
                format!("{phase} said {status} {extra}")
            };
            let word = |k: &str| words.iter().find_map(|w| w.strip_prefix(k));
            let mut findings: Vec<String> = word("find=")
                .map(|ids| ids.split(',').map(String::from).collect())
                .unwrap_or_default();
            match review_run(phase, state) {
                Some((_, _, true)) => self.review = ReviewLoop::default(),
                Some(_) if phase == "review-fix" => {
                    let review = state.arts.get("review").cloned().unwrap_or_default();
                    findings = fix_cycle(&review, state).map(|c| c.2).unwrap_or_default();
                    for id in &findings {
                        *self.review.attempts.entry(id.clone()).or_default() += 1;
                    }
                    self.review.fixes += 1;
                }
                _ => {}
            }
            let art = Art {
                status: status.into(),
                sha: Some(self.head()),
                body,
                next: word("next=").map(String::from),
                findings,
                ..Art::default()
            };
            self.arts.insert(phase, art);
        } else {
            for (p, a) in saved {
                self.arts.entry(p).or_insert(a);
            }
        }
        PhaseRan {
            artifact: written.then(|| status.to_string()),
            invalid: None,
            present: self.arts.contains_key(phase),
            timed_out,
            killed: None,
            timeout_text: "timed out after 1 s".into(),
            exit: if status == "crash" { Some(1) } else { Some(0) },
            head_before,
            head_after: self.head(),
            red_feedback: self.red.clone(),
        }
    }

    /// What a phase's words do to the unit and the default branch, in order.
    fn act(&mut self, words: &[&str]) {
        for w in words {
            match *w {
                "commit" => self.commits += 1,
                "merge" => self.pr_merged = true,
                "noorigin" => self.remote = None,
                "dirty" => self.merge.merge_state = "DIRTY",
                "clean" => self.merge.merge_state = "CLEAN",
                "movemain" | "pushmain" | "landmain" => {
                    let Some((_, n)) = self.remote.as_mut() else {
                        continue;
                    };
                    *n += 1;
                    let n = *n;
                    if *w != "movemain" {
                        self.held = n;
                    }
                    if *w == "landmain" {
                        self.main_prs.insert(n, vec![self.land_pr.clone()]);
                    }
                }
                w => {
                    if let Some(path) = w.strip_prefix("touch=") {
                        let at = self.commits;
                        self.touched.entry(at).or_default().push(path.into());
                    }
                }
            }
        }
    }

    /// The hard rules after `phase`, as `run/post_phase.rs` checks them: the default branch moved
    /// only by others, and no phase merged the PR.
    fn guard(&mut self, phase: &str, logs: &mut Vec<Value>) -> GuardFacts {
        let moved = self.branch_guard(phase, logs);
        if moved.breach.is_some() {
            return moved;
        }
        let pr = self.merge.pr.filter(|_| self.arts.contains_key("ship"));
        match pr.filter(|_| self.pr_merged) {
            Some(n) => {
                let (ev, reason) = pr_merged(phase, n);
                logs.push(ev);
                GuardFacts {
                    breach: Some(reason),
                }
            }
            None => GuardFacts::default(),
        }
    }

    fn branch_guard(&mut self, phase: &str, logs: &mut Vec<Value>) -> GuardFacts {
        let Some((branch, n)) = self.remote.clone() else {
            return GuardFacts::default();
        };
        if n == self.baseline {
            return GuardFacts::default();
        }
        let (from, now) = (Self::main_sha(self.baseline), Self::main_sha(n));
        let attributed = (self.held == n).then(|| {
            let commits = (self.baseline + 1..=n).rev().map(Self::main_sha).collect();
            let own = format!("ns/{}", self.unit);
            moved_by_others(Ok(commits), None, &own, |c| match &self.forge_fails {
                Some(e) => Err(Unread::Commit(e.clone())),
                None => {
                    let k: u32 = c[1..].parse().expect("a sim main sha");
                    Ok(self.main_prs.get(&k).cloned().unwrap_or_default())
                }
            })
        });
        let (ev, breach) = branch_moved(phase, (&branch, &from, &now), attributed);
        logs.push(ev);
        if breach.is_none() {
            self.baseline = n;
        }
        GuardFacts { breach }
    }

    /// The gate after a phase: the next verdict, and on red the phase run its output blames.
    fn gate_after(&mut self, rebased_onto: Option<(String, String)>, l: &Ledger) -> GateRan {
        let Some(gates) = self.gates.as_mut() else {
            return GateRan::NoCommand;
        };
        match gates.pop_front().unwrap_or(Ok(())) {
            Ok(()) => {
                self.red = None;
                GateRan::Green
            }
            Err(output) => {
                self.red = Some(output.clone());
                let now = self.remote_now();
                let failing = crate::gate::failing_paths(&output);
                let suspects: Vec<Suspect> = l
                    .suspects()
                    .map(|(index, m)| Suspect {
                        index,
                        phase: m.phase.clone(),
                        changed: Some(self.changed(&m.from, &m.to)),
                    })
                    .collect();
                let tracked: Vec<String> = self.touched.values().flatten().cloned().collect();
                let culprit = culprit(&failing, &suspects, Path::new("/sim"), &tracked);
                GateRan::Red(GateRed {
                    feedback: output,
                    reason: "the CI gate failed".into(),
                    stuck_reason: None,
                    moved_since: rebased_onto
                        .is_some_and(|(_, onto)| now.is_some_and(|(_, sha)| sha != onto)),
                    culprit: culprit.map(|c| c.0),
                })
            }
        }
    }

    /// The files the commits after `from` up to `to` change.
    fn changed(&self, from: &str, to: &str) -> Vec<String> {
        let num = |sha: &str| sha[1..].parse::<u32>().expect("a sim commit");
        self.touched
            .range(num(from) + 1..=num(to))
            .flat_map(|(_, f)| f.clone())
            .collect()
    }
}

impl Trace {
    fn report(
        self,
        outcome: Outcome,
        reason: String,
        phase: Option<&'static str>,
        needs_human: bool,
    ) -> Report {
        Report {
            outcome,
            reason,
            phase,
            needs_human,
            runs: self.runs,
            logs: self.logs,
            gates: self.gates,
            asks: self.asks,
        }
    }
}
