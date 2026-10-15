//! The merge step in the sim: [`super::super::merge::next`] driven against a fake GitHub, as
//! `run/merge.rs` drives it against `gh`, the way the e2e tests' fake `gh` answers.

use std::collections::{BTreeSet, VecDeque};

use super::super::merge::{
    next, Ask, Check, Gated, MergeConfig, MergeEnd, MergeFact, MergeState, Overlap, Red,
};
use super::super::route::State;
use super::super::MergeBack;
use super::{Sim, Trace};
use crate::factory::LocalGate;
use crate::run::Outcome;

/// The merge step's GitHub: an open PR #12 at the unit's HEAD that GitHub sees as clean, whose
/// checks register at once and pass, on a green main, and that merges.
#[derive(Debug, Clone)]
pub struct FakeMerge {
    /// The PR pr.md names.
    pub pr: Option<u64>,
    /// The PR's state on the forge.
    pub state: &'static str,
    /// The artifacts are current at the PR head.
    pub current: bool,
    /// `[merge] local_queue`.
    pub local_queue: bool,
    /// The PR's `mergeStateStatus` on each read, then [`FakeMerge::merge_state`].
    pub merge_states: VecDeque<&'static str>,
    pub merge_state: &'static str,
    /// How many fetches of the default branch fail next.
    pub tip_fails: u32,
    /// The error update-branch fails with each time.
    pub update_error: Option<String>,
    /// The git fallback's results in order, then a clean merge.
    pub git_merges: VecDeque<Result<bool, String>>,
    /// The head the first update-branch gives the PR; later ones give others.
    pub updated_head: String,
    /// How many head views still show the old head after update-branch.
    pub update_lag: u32,
    /// The merge state a head view reports.
    pub head_state: &'static str,
    /// How many polls find no check run, and no status, before each registers; `u32::MAX` for
    /// never.
    pub register_after: [u32; 2],
    /// The check-run and the status queries fail with these.
    pub count_errors: [Option<String>; 2],
    /// `[merge] ci_register_timeout`.
    pub register_minutes: u64,
    /// `[merge] ci_timeout_minutes`.
    pub ci_minutes: u64,
    pub ci_times_out: bool,
    /// The tail of each failed check's log.
    pub failed_log: String,
    /// The checks each CI wait ends with, then one green check.
    pub checks: VecDeque<Vec<Check>>,
    /// How many CI waits, from the first, move the default branch.
    pub moves_on_watch: u32,
    /// The default branch's moves change a file the PR changes.
    pub moves_overlap: bool,
    /// How many polls find main's CI red before it goes green; `u32::MAX` for never.
    pub main_red_polls: u32,
    pub main_running: bool,
    /// The head main's CI ran at.
    pub main_sha: String,
    /// The wait for the merge lock gives up with this.
    pub lock_gives_up: Option<String>,
    /// The PR head after CI passed, when another push moved it.
    pub head_after_ci: Option<String>,
    pub review_hits: Vec<String>,
    pub regions: Result<Vec<String>, String>,
    /// `gh pr merge` fails with this, and leaves the PR open.
    pub merge_error: Option<String>,
    /// The local merge queue's merge result conflicts.
    pub queue_conflicts: bool,
    /// The gate cannot run, for this reason.
    pub gate_human: Option<String>,
    watches: u32,
    updates: u32,
    head_polls: u32,
    count_polls: u32,
    main_polls: u32,
}

impl Default for FakeMerge {
    fn default() -> Self {
        Self {
            pr: Some(12),
            state: "OPEN",
            current: true,
            local_queue: true,
            merge_states: VecDeque::new(),
            merge_state: "CLEAN",
            tip_fails: 0,
            update_error: None,
            git_merges: VecDeque::new(),
            updated_head: "0123456789abcdef0123456789abcdef01234567".into(),
            update_lag: 0,
            head_state: "CLEAN",
            register_after: [0, 0],
            count_errors: [None, None],
            register_minutes: 3,
            ci_minutes: 1,
            ci_times_out: false,
            failed_log: "assert 1 == 2".into(),
            checks: VecDeque::new(),
            moves_on_watch: 0,
            moves_overlap: true,
            main_red_polls: 0,
            main_running: false,
            main_sha: "abcdef0123456789abcdef0123456789abcdef01".into(),
            lock_gives_up: None,
            head_after_ci: None,
            review_hits: vec![],
            regions: Ok(vec![]),
            merge_error: None,
            queue_conflicts: false,
            gate_human: None,
            watches: 0,
            updates: 0,
            head_polls: 0,
            count_polls: 0,
            main_polls: 0,
        }
    }
}

/// A check named `name` in `bucket`, with a GitHub Actions run link.
pub fn check(name: &str, bucket: &str) -> Check {
    Check {
        name: Some(name.into()),
        state: Some(bucket.to_uppercase()),
        bucket: Some(bucket.into()),
        link: Some("https://github.com/o/r/actions/runs/99/job/2".into()),
    }
}

/// How the merge step ended the unit: its outcome, reason, and whether a human merges.
pub type Ended = (Outcome, String, bool);

impl Sim {
    /// Run the merge step at `state` to its end: back to a phase, or the unit's end.
    pub(super) fn merge_step(&mut self, state: &State, t: &mut Trace) -> Result<MergeBack, Ended> {
        let cfg = MergeConfig {
            build_only: self.settings.local_gate == LocalGate::BuildOnly,
            local_queue: self.merge.local_queue,
            gate: self.gates.is_some(),
            register_minutes: self.merge.register_minutes,
            ci_minutes: self.merge.ci_minutes,
            fixes_main: false,
            branch: format!("ns/{}", self.unit),
        };
        let mut m = MergeState::new(cfg);
        let mut fact = MergeFact::Start;
        let (mut pr_head, mut lock) = (self.head(), false);
        loop {
            let act = next(&mut m, fact, self.now);
            t.logs.extend(act.log);
            t.asks.push(act.ask.clone());
            fact = match act.ask {
                Ask::End(end) => return ended(end),
                Ask::TakeLock => {
                    assert!(!lock, "sim: the merge lock taken twice");
                    lock = self.merge.lock_gives_up.is_none();
                    MergeFact::Locked(self.merge.lock_gives_up.clone())
                }
                Ask::Unlock => {
                    assert!(lock, "sim: the merge lock released unheld");
                    lock = false;
                    MergeFact::Done
                }
                Ask::Gate { .. } => {
                    let g = self.merge_gate();
                    t.gates.push(("merge", !matches!(g, Gated::Red(_))));
                    MergeFact::Gated(g)
                }
                ask => self.answer(ask, state, &mut pr_head),
            };
        }
    }

    /// What the fake GitHub, git and clock answer `ask` with.
    fn answer(&mut self, ask: Ask, state: &State, pr_head: &mut String) -> MergeFact {
        let n = self.remote.as_ref().map_or(0, |r| r.1);
        let (head, holds) = (self.head(), self.held == n);
        let f = &mut self.merge;
        match ask {
            Ask::ReadPr => MergeFact::Pr {
                ship: state.arts.contains_key("ship"),
                number: f.pr,
            },
            Ask::ViewPr => MergeFact::View {
                state: Some(f.state.into()),
                head: pr_head.clone(),
            },
            Ask::Currency { .. } => MergeFact::Currency {
                head,
                current: f.current,
                review_passes: state.arts.get("review").is_some_and(|r| r.status == "pass"),
            },
            Ask::ReadDefault => MergeFact::Default(self.remote.as_ref().map(|r| r.0.clone())),
            Ask::FetchTip if f.tip_fails > 0 || self.remote.is_none() => {
                f.tip_fails = f.tip_fails.saturating_sub(1);
                MergeFact::Tip(Err("git fetch origin main failed: no remote".into()))
            }
            Ask::FetchTip => MergeFact::Tip(Ok(Self::main_sha(n))),
            Ask::MergeState => MergeFact::MergeState(Some(
                f.merge_states.pop_front().unwrap_or(f.merge_state).into(),
            )),
            Ask::IsAncestor { .. } => MergeFact::Ancestor(holds),
            Ask::Overlap { .. } => {
                let pr = BTreeSet::from(["work.txt".to_string()]);
                let file = if f.moves_overlap {
                    "work.txt"
                } else {
                    "other.txt"
                };
                let base = BTreeSet::from([file.to_string()]);
                MergeFact::Overlap(Overlap::Files { pr, base })
            }
            Ask::UpdateBranch => {
                f.head_polls = 0;
                f.updates += 1;
                MergeFact::Updated(f.update_error.clone().map_or(Ok(()), Err))
            }
            Ask::MergeWithGit { .. } => {
                f.head_polls = 0;
                f.updates += 1;
                MergeFact::GitMerged(f.git_merges.pop_front().unwrap_or(Ok(true)))
            }
            Ask::ViewHead => {
                f.head_polls += 1;
                let moved = f.head_polls > f.update_lag;
                if moved {
                    // Each update merges the base in anew: a new head each time.
                    *pr_head = match f.updates {
                        1 => f.updated_head.clone(),
                        n => format!("{n:040}"),
                    };
                }
                MergeFact::HeadView {
                    head: Some(pr_head.clone()),
                    merge_state: f.head_state.into(),
                }
            }
            ask => self.answer_ci(ask, pr_head),
        }
    }

    /// The CI, lock-time and merge answers.
    fn answer_ci(&mut self, ask: Ask, pr_head: &mut String) -> MergeFact {
        let f = &mut self.merge;
        match ask {
            Ask::CountChecks { .. } => {
                f.count_polls += 1;
                let count = |i: usize| match &f.count_errors[i] {
                    Some(e) => Err(e.clone()),
                    None => Ok(u64::from(f.count_polls > f.register_after[i])),
                };
                MergeFact::CheckCounts([count(0), count(1)])
            }
            Ask::WatchChecks { .. } => {
                f.watches += 1;
                let moves = f.watches <= f.moves_on_watch;
                let timed_out = f.ci_times_out;
                if let (true, Some((_, n))) = (moves, self.remote.as_mut()) {
                    *n += 1;
                }
                if let Some(h) = self.merge.head_after_ci.take() {
                    *pr_head = h;
                }
                MergeFact::Watched { timed_out }
            }
            Ask::ReadChecks => MergeFact::Checks(
                f.checks
                    .pop_front()
                    .unwrap_or_else(|| vec![check("ci", "pass")]),
            ),
            Ask::FailedLogs(ids) => {
                MergeFact::FailedLogs(ids.iter().map(|_| Some(f.failed_log.clone())).collect())
            }
            Ask::MainChecks => {
                f.main_polls += 1;
                MergeFact::MainCi(Some(super::super::merge::MainHead {
                    sha: f.main_sha.clone(),
                    red: f.main_polls <= f.main_red_polls,
                    running: f.main_running,
                }))
            }
            Ask::ViewHeadRef => MergeFact::HeadRef(Some(pr_head.clone())),
            Ask::ReviewHits => MergeFact::ReviewHits(f.review_hits.clone()),
            Ask::Regions => MergeFact::Regions(f.regions.clone()),
            Ask::Merge { .. } => MergeFact::Merged {
                state: Some(
                    if f.merge_error.is_some() {
                        "OPEN"
                    } else {
                        "MERGED"
                    }
                    .into(),
                ),
                error: f.merge_error.clone(),
                spent_usd: 0.0,
            },
            Ask::DeleteBranch => MergeFact::BranchDeleted(Ok(())),
            Ask::Sleep(until) => {
                self.now = self.now.max(until);
                MergeFact::Done
            }
            ask => panic!("sim: the merge step asked for {ask:?}, which the sim does not answer"),
        }
    }

    /// The gate before the merge: the next verdict of [`Sim::gates`].
    fn merge_gate(&mut self) -> Gated {
        if let Some(r) = self.merge.gate_human.clone() {
            return Gated::Human(r);
        }
        if self.merge.queue_conflicts {
            return Gated::Conflict;
        }
        let verdict = self.gates.as_mut().and_then(VecDeque::pop_front);
        match verdict.unwrap_or(Ok(())) {
            Ok(()) => Gated::Green,
            Err(output) => Gated::Red(Red {
                feedback: output,
                reason: "the CI gate failed".into(),
                killed: None,
                ratchets: None,
            }),
        }
    }
}

/// The merge step's end as `run/drive.rs` takes it.
fn ended(end: MergeEnd) -> Result<MergeBack, Ended> {
    match end {
        MergeEnd::Finish {
            outcome,
            reason,
            needs_human,
        } => Err((outcome, reason, needs_human)),
        MergeEnd::Rebuild(red) => Ok(MergeBack::Rebuild {
            stuck_reason: red
                .killed
                .or_else(|| red.ratchets.map(|r| format!("the CI gate failed on {r}"))),
            feedback: red.feedback,
            reason: red.reason,
        }),
        MergeEnd::Reship(feedback) => Ok(MergeBack::Reship(feedback)),
        MergeEnd::Rebase { feedback, why } => Ok(MergeBack::Rebase { feedback, why }),
    }
}
