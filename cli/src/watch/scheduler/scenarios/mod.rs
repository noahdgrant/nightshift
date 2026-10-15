//! Nights in process: a serial scheduler over the same `core::fill`, `core::red` and
//! `core::settle` decisions the scheduler drives, with a [`FakeForge`] for the tracker and the
//! units' results given by each test instead of a child `ns run`. Each area's scenarios, moved
//! from the factory end-to-end tests under the same names, are in the files beside this one.

use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{json, Value};

use super::overlap;
use super::settle::forge_part;
use crate::factory::Factory;
use crate::git::Repo;
use crate::run::RunResult;
use crate::touches::Overlap;
use crate::watch::ci::{self, Check, HeadChecks, MainCi, Outcome as CheckOutcome};
use crate::watch::core::fill::{
    open_fill, Claims, FillAct, FillInput, Holds, Kind, Open, OpenAct, Stopping,
};
use crate::watch::core::red::{self as core_red, Gate, Note, Red, MAIN_RED_UNFIXED};
use crate::watch::core::settle::{self as core_settle, SettleAct, SettleFacts};
use crate::watch::forge::fake::FakeForge;
use crate::watch::transitions::{self, Event, State};
use crate::watch::{fix_main, queue, triage, unit_id, Issue, Night, PAUSED_PAST_UNTIL};

mod fill;
mod red;
mod settle;
mod triage_pass;

/// The ready label of the default queue.
pub(super) const READY: &str = "status:ready-for-agent";

/// What a night said that a test checks: the run log's main-CI and overlap events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Said {
    MainRed {
        sha: Option<String>,
        failing: Vec<String>,
        issue: u64,
    },
    MainGreen {
        sha: Option<String>,
        state: ci::State,
    },
    Unfixed {
        sha: Option<String>,
        issue: u64,
        outcome: &'static str,
    },
    /// The warning that the fix issue for this head is closed and the queue stays held.
    Shut,
}

/// A triage-only run of an issue, given the forge to edit as the run's phase would: the fields
/// of its result, or why it failed to start.
pub(super) type TriageRun = Box<dyn FnMut(u64, &FakeForge) -> Result<Value>>;

/// One night of `ns watch`, run serially in process.
pub(super) struct Sim {
    pub forge: FakeForge,
    pub fac: Factory,
    _dir: tempfile::TempDir,
    repo: Repo,
    pub night: Night,
    pub parallel: usize,
    pub max_units: Option<u32>,
    /// Why the night is over, `--until` or the budget, when it is.
    pub over: Option<&'static str>,
    /// `--until`: a pause that lasts past it stops the night.
    pub deadline: Option<i64>,
    /// A stop was asked for.
    pub signal: bool,
    pub now: i64,
    pub running: Vec<Issue>,
    pub resume: Vec<Issue>,
    pub stopping: Option<Stopping>,
    pub paused_until: Option<i64>,
    pub started: u32,
    pub red: Red,
    pub holds: Holds,
    /// Each unit's record: its issue and outcome.
    pub units: Vec<(u64, String)>,
    /// Each paused unit's record: its issue and when it resumes.
    pub resets: Vec<(u64, i64)>,
    pub said: Vec<Said>,
    /// What the night warned of on stderr about the fix issue.
    pub warnings: Vec<String>,
    /// The issues claimed, in order.
    pub claimed: Vec<u64>,
    /// The night's triage-only runs; `None` runs no triage pass.
    pub triage: Option<TriageRun>,
    /// What happened, in order: `triage <n>`, `claim <n>` and `end <n>`.
    pub timeline: Vec<String>,
}

impl Sim {
    /// A night on a live fake forge, one unit at a time, main's CI unread.
    pub fn new() -> Sim {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo {
            root: dir.path().to_path_buf(),
            common_dir: dir.path().join(".git"),
        };
        let night = Night::new();
        Sim {
            now: night.shared.clock.now(),
            forge: FakeForge {
                live: true,
                ..FakeForge::default()
            },
            fac: Factory::default(),
            _dir: dir,
            repo,
            night,
            parallel: 1,
            max_units: None,
            over: None,
            deadline: None,
            signal: false,
            running: Vec::new(),
            resume: Vec::new(),
            stopping: None,
            paused_until: None,
            started: 0,
            red: Red::default(),
            holds: Holds::default(),
            units: Vec::new(),
            resets: Vec::new(),
            said: Vec::new(),
            warnings: Vec::new(),
            claimed: Vec::new(),
            triage: None,
            timeline: Vec::new(),
        }
    }

    /// A team issue `n` under the ready label and `labels`.
    pub fn ready(&mut self, n: u64, title: &str, labels: &[&str]) {
        let mut ls: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
        ls.push(READY.into());
        self.forge.labels.borrow_mut().insert(n, ls.clone());
        self.forge.issues.push(Issue {
            number: n,
            title: title.into(),
            labels: ls,
            body: String::new(),
            team: true,
        });
    }

    /// A team issue `n` under `labels` alone, none of them the ready label.
    pub fn untriaged(&mut self, n: u64, title: &str, labels: &[&str]) {
        let ls: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
        self.forge.labels.borrow_mut().insert(n, ls.clone());
        self.forge.issues.push(Issue {
            number: n,
            title: title.into(),
            labels: ls,
            body: String::new(),
            team: true,
        });
    }

    /// The issue and outcome of each triage-only run, in order.
    pub fn triaged(&self) -> Vec<(u64, String)> {
        let recs = &self.night.triage.records;
        let num = |r: &Value| r["issue"].as_u64().unwrap();
        let out = |r: &Value| r["outcome"].as_str().unwrap().to_string();
        recs.iter().map(|r| (num(r), out(r))).collect()
    }

    /// Issue `n`'s labels now.
    pub fn labels(&self, n: u64) -> Vec<String> {
        self.forge
            .labels
            .borrow()
            .get(&n)
            .cloned()
            .unwrap_or_default()
    }

    /// The comments posted on issue `n`.
    pub fn comments(&self, n: u64) -> Vec<String> {
        let posted = self.forge.posted.borrow();
        posted
            .iter()
            .filter(|(i, _)| *i == n)
            .map(|(_, c)| c.clone())
            .collect()
    }

    /// Fill the free slots as the scheduler does, without the housekeeping, the update check
    /// and the triage pass.
    pub fn fill(&mut self) -> Result<()> {
        let open = Open {
            kind: Kind::Full,
            paused: self.paused_until.is_some(),
            free: self.parallel - self.running.len(),
            resume: self.resume.len(),
            stopping: self.stopping.as_ref(),
            claims: self.claims(),
            draining: false,
        };
        for act in open_fill(&open) {
            match act {
                OpenAct::Housekeep | OpenAct::CheckBase => {}
                OpenAct::Resume => {
                    let issue = self.resume.remove(0);
                    self.running.push(issue);
                }
                OpenAct::Stop(why) => self.stopping = Some(why),
                OpenAct::Triage => {
                    match self.triage_pass()? {
                        Some(triage::Halt::Stop(why)) => self.stop_all(&why),
                        Some(triage::Halt::Paused(reset)) => self.pause(reset),
                        None => return self.claim_ready(),
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// The triage pass, its runs given by [`Sim::triage`].
    fn triage_pass(&mut self) -> Result<Option<triage::Halt>> {
        let Some(given) = self.triage.as_mut() else {
            return Ok(None);
        };
        let (forge, fac, timeline) = (&self.forge, &self.fac, &mut self.timeline);
        let held = || ci::read(forge, fac).is_ok_and(|m| m.holds_queue());
        let mut run = |n: u64, night: &mut Night| -> Result<RunResult> {
            timeline.push(format!("triage {n}"));
            let mut v = json!({"unit": format!("{n}-triage"), "reason": "why"});
            let fields = given(n, forge)?;
            v.as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let r = RunResult::from_json(v).expect("a run result");
            night.shared.add_spend(&r.unit, r.cost_usd)?;
            Ok(r)
        };
        triage::pass(forge, fac, self.deadline, &mut self.night, &held, &mut run)
    }

    /// Pause every unit until `reset`, or the latest reset tonight; past `--until`, stop.
    fn pause(&mut self, reset: i64) {
        let (until, past) = core_settle::pause_until(self.paused_until, reset, self.deadline);
        self.paused_until = Some(until);
        if past {
            self.stop_all(PAUSED_PAST_UNTIL);
        }
    }

    fn claims(&self) -> Claims {
        Claims {
            started: self.started,
            max_units: self.max_units,
            over: self.over,
        }
    }

    /// Read the queue and main's CI, and claim what `core::fill::plan_fill` picks.
    fn claim_ready(&mut self) -> Result<()> {
        let mut taken: BTreeSet<u64> = self.night.finished.clone();
        taken.extend(self.running.iter().chain(&self.resume).map(|i| i.number));
        let ready = queue(&self.forge, &self.fac, &taken)?.ready;
        let gate = match ci::read(&self.forge, &self.fac).ok() {
            Some(m) => self.gate(&m, &ready, &taken)?,
            None => Gate::Open,
        };
        let taken_now = taken.clone();
        let input = FillInput {
            ready: &ready,
            gate: &gate,
            taken: &taken_now,
            free: self.parallel - self.running.len(),
            claims: self.claims(),
            holds: &self.holds,
        };
        let running = || {
            let units = self.running.iter().map(|i| (i, None));
            overlap::running_units(&self.repo, &self.forge, units)
        };
        let ignore = self.fac.queue.overlap_ignore.clone();
        let rule = || Overlap::new(Vec::new(), &ignore);
        let acts = overlap::plan(&self.forge, &input, self.now, running, rule);
        for act in acts {
            match act {
                FillAct::Held { issue, .. } => {
                    self.holds.hold(issue, self.now);
                }
                FillAct::Forced { .. } => {}
                FillAct::Claim(issue) => self.claim(issue)?,
                FillAct::Stop(why) => self.stopping = Some(why),
            }
        }
        Ok(())
    }

    /// What red main lets start, as `Scheduler::main_fix` decides it.
    fn gate(&mut self, main: &MainCi, ready: &[Issue], taken: &BTreeSet<u64>) -> Result<Gate> {
        let (gate, note) = if main.holds_queue() {
            let red = &self.red;
            let found = fix_main::find_or_file(
                &self.repo,
                &self.forge,
                &self.fac,
                main,
                red.fix.as_ref(),
                &red.filed,
            )?;
            let startable =
                |i: &Issue| taken.contains(&i.number) || ready.iter().any(|r| r.number == i.number);
            self.warnings
                .extend(found.as_ref().and_then(|f| f.warning(&startable)));
            self.red.red(main, found)
        } else {
            self.red.not_red()
        };
        match (note, &gate) {
            (Some(Note::Red), Gate::Only(fix)) => self.said.push(Said::MainRed {
                sha: main.sha.clone(),
                failing: main.failing.iter().map(|c| c.name.clone()).collect(),
                issue: fix.issue().number,
            }),
            (Some(Note::Green), _) => self.said.push(Said::MainGreen {
                sha: main.sha.clone(),
                state: main.state,
            }),
            (Some(Note::Shut), _) => self.said.push(Said::Shut),
            _ => {}
        }
        Ok(gate)
    }

    fn claim(&mut self, issue: Issue) -> Result<()> {
        let q = &self.fac.queue;
        let n = issue.number;
        if let Err(e) = transitions::apply(&self.forge, q, n, State::Ready, Event::Claim) {
            let _ = transitions::apply(&self.forge, q, n, State::InProgress, Event::StartFailed);
            return Err(e);
        }
        self.claimed.push(n);
        self.timeline.push(format!("claim {n}"));
        self.running.push(issue);
        self.started += 1;
        Ok(())
    }

    /// The unit running issue `n` ends with the result `fields` give, its unit named after the
    /// issue unless they name it: settle it as the scheduler does.
    pub fn end(&mut self, n: u64, fields: Value) -> Result<()> {
        let at = self
            .running
            .iter()
            .position(|i| i.number == n)
            .expect("running");
        let issue = self.running.remove(at);
        self.timeline.push(format!("end {n}"));
        let mut v = json!({"unit": unit_id(&issue), "reason": "why"});
        if let (Some(v), Value::Object(fields)) = (v.as_object_mut(), fields) {
            v.extend(fields);
        }
        let r = RunResult::from_json(v).expect("a run result");
        let review = core_settle::review_path(&r).and_then(|a| std::fs::read_to_string(a).ok());
        let mut facts = SettleFacts {
            stopping: self.signal,
            over: self.over,
            resume_at: r.reset_at.unwrap_or(self.now + 1800),
            review: review.as_deref(),
            ..SettleFacts::default()
        };
        let mut main = None;
        if !core_settle::interrupted(&r, facts.stopping) {
            facts.failing = self.night.harness(&r);
            facts.harness_fails = self.night.harness_fails;
            if self.red.may_be_unfixed(n, r.outcome) && core_settle::checks_main(&r, &facts) {
                main = ci::read(&self.forge, &self.fac).ok();
                facts.unfixed = core_red::unfixed(true, main.as_ref());
            }
        }
        let acts = core_settle::settle(&r, facts);
        let (acts, failed) = forge_part(&self.forge, &self.fac.queue, n, acts);
        for act in acts {
            match act {
                SettleAct::Unfixed => {
                    self.said.push(Said::Unfixed {
                        sha: main.as_ref().and_then(|m| m.sha.clone()),
                        issue: n,
                        outcome: r.outcome.label(),
                    });
                    self.stop_all(MAIN_RED_UNFIXED);
                }
                SettleAct::StopAll(why) => self.stop_all(&why),
                SettleAct::Pause(reset) => {
                    self.resume.push(issue.clone());
                    self.pause(reset);
                }
                SettleAct::Record {
                    outcome,
                    finished,
                    reset_at,
                } => {
                    self.units.push((n, outcome.into()));
                    self.resets.extend(reset_at.map(|t| (n, t)));
                    if finished {
                        self.night.finished.insert(n);
                    }
                }
                other => unreachable!("forge_part does {other:?}"),
            }
        }
        failed
    }

    fn stop_all(&mut self, why: &str) {
        if !matches!(self.stopping, Some(Stopping::All(_))) {
            self.stopping = Some(Stopping::All(why.into()));
        }
    }

    /// Run the night to its end, each unit ending with what `result` gives for its issue, which
    /// may change the night too, as a phase or a person would: fill, end the oldest running unit,
    /// and again, sleeping through a pause unless the night stops. Why the night ended.
    pub fn run(&mut self, mut result: impl FnMut(&Issue, &mut Sim) -> Value) -> Result<String> {
        loop {
            if let Err(e) = self.fill() {
                self.stop_all("error");
                return Err(e);
            }
            let Some(issue) = self.running.first().cloned() else {
                let resumes = !matches!(self.stopping, Some(Stopping::All(_)));
                match self.paused_until.take().filter(|_| resumes) {
                    Some(reset) => {
                        self.now = self.now.max(reset);
                        continue;
                    }
                    None => break,
                }
            };
            let fields = result(&issue, self);
            self.end(issue.number, fields)?;
        }
        // The night ended before the reset: the units still paused go back to the queue.
        for issue in std::mem::take(&mut self.resume) {
            let n = issue.number;
            let q = &self.fac.queue;
            transitions::apply(&self.forge, q, n, State::InProgress, Event::PausedGivenBack)?;
        }
        Ok(self
            .stopping
            .as_ref()
            .map_or("queue empty".into(), |s| s.reason().to_string()))
    }
}

/// Main's head at `sha` with one check, `test`, ending `outcome`.
pub(super) fn main_at(sha: &str, outcome: CheckOutcome) -> HeadChecks {
    HeadChecks {
        sha: sha.into(),
        checks: vec![Check {
            name: "test".into(),
            outcome,
            required: true,
            log: None,
        }],
    }
}

/// The red head every main-red scenario starts from.
pub(super) const MAIN_SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

/// Main red at [`MAIN_SHA`]: check `test` failed.
pub(super) fn main_red() -> HeadChecks {
    main_at(MAIN_SHA, CheckOutcome::Fail)
}

/// Each unit record's issue and outcome, sorted by issue.
pub(super) fn by_issue(units: &[(u64, String)]) -> Vec<(u64, String)> {
    let mut o = units.to_vec();
    o.sort();
    o
}

pub(super) fn each(issues: &[u64], outcome: &str) -> Vec<(u64, String)> {
    issues.iter().map(|n| (*n, outcome.to_string())).collect()
}
