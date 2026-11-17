//! What each state does. A state passes by returning `Ok` and fails by returning `Err`.

pub mod build;
mod check;
pub mod clean_up;
pub mod findings;
pub mod retrospective;
pub mod review;
mod rounds;
mod setup;
pub mod ship;
pub mod verdict;
pub mod verify;

pub use verify::{unproven_list, unproven_note};

/// How many runs of the check called `kind` (`verify`, `review`) the unit has kept in `dir`.
pub fn rounds_so_far(dir: &std::path::Path, unit: &str, kind: &str) -> usize {
    rounds::Rounds::new(dir, unit, kind).so_far()
}

use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local};

use crate::forge::Forge;
use crate::harness::{self, Events, Harness, Reply, Task, TimedOut};
use crate::manifest::{self, Place};
use crate::os::Os;
use crate::runlog::RunLog;
use crate::state::{Outcome, State};
use crate::transcripts::Transcripts;
use crate::vcs::Vcs;
use crate::workspace::Workspace;

/// The artifact `build` leaves with a snapshot of the workspace as it ended the build.
pub(crate) const BUILT_SNAPSHOT: &str = "built.snapshot";

/// Everything a state works with.
pub struct Context {
    pub vcs: Box<dyn Vcs>,
    pub forge: Box<dyn Forge>,
    pub harness: Box<dyn Harness>,
    pub os: Box<dyn Os>,
    /// Where `ns` was started, inside the repository.
    pub from: PathBuf,
    pub unit: u64,
    /// Set by `setup`.
    pub workspace: Option<Workspace>,
    pub ship: ship::Options,
    /// How long a harness session may take.
    pub harness_timeout: Duration,
    /// How long the repository's checks may take.
    pub checks_timeout: Duration,
    /// The model the person asked for, which beats every key of `[model]`.
    pub model: Option<String>,
    /// The moment `ns auto` stops, when it was told one. A usage limit that resets after it is
    /// not waited for.
    pub until: Option<DateTime<Local>>,
    /// How often a running session says it is alive. Zero says nothing until it ends.
    pub heartbeat: Duration,
}

impl Context {
    /// The model a session at `place` uses.
    pub(crate) fn model_find(&self, place: Place) -> Result<Option<String>> {
        let root = self.vcs.root_find(&self.from)?;
        manifest::model_read(&root, place, self.model.as_deref())
    }
}

/// What a state produced besides its outcome, as fields of the event of the move out of it. A
/// state sets only the facts it has, and only these fields can ride on an event.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Facts {
    /// How a check ended.
    pub verdict: Option<verdict::Verdict>,
    /// How many findings a check made.
    pub findings: Option<usize>,
    /// The path in the workspace of the notes the state kept.
    pub artifact: Option<std::path::PathBuf>,
    /// The name and verdict of each review session.
    pub sessions: Option<Vec<SessionVerdict>>,
}

/// How one review session ended.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionVerdict {
    pub name: String,
    pub verdict: Option<verdict::Verdict>,
}

/// The verdict an event may carry: `PASS`, `FAIL` or `INCONCLUSIVE`. A harness can write
/// `VERDICT: ERROR` itself, and that word never reaches an event.
pub(crate) fn reported(verdict: Option<verdict::Verdict>) -> Option<verdict::Verdict> {
    verdict.filter(|v| *v != verdict::Verdict::Error)
}

impl Facts {
    /// The facts of a check: its `verdict` when the evidence has one, its `findings` count and
    /// the path of the `artifact` it kept.
    pub(crate) fn check(
        verdict: Option<verdict::Verdict>,
        findings: usize,
        artifact: &std::path::Path,
    ) -> Facts {
        Facts {
            verdict: reported(verdict),
            findings: Some(findings),
            artifact: Some(artifact.to_path_buf()),
            sessions: None,
        }
    }

    /// Adds the facts that are set to `event`, an object, as fields. The one place the names of
    /// the fields are spelled, and no other field of the event is touched.
    pub fn onto(&self, event: &mut serde_json::Value) {
        if let Some(v) = &self.verdict {
            event["verdict"] = v.word().into();
        }
        if let Some(n) = self.findings {
            event["findings"] = n.into();
        }
        if let Some(path) = &self.artifact {
            event["artifact"] = path.display().to_string().into();
        }
        if let Some(sessions) = &self.sessions {
            event["sessions"] = sessions
                .iter()
                .map(|s| {
                    let mut entry = serde_json::json!({ "name": s.name });
                    if let Some(v) = &s.verdict {
                        entry["verdict"] = v.word().into();
                    }
                    entry
                })
                .collect::<Vec<_>>()
                .into();
        }
    }
}

/// How a state ended and the facts it left.
pub struct Done {
    pub outcome: Outcome,
    pub facts: Facts,
}

impl From<Outcome> for Done {
    fn from(outcome: Outcome) -> Self {
        Done {
            outcome,
            facts: Facts::default(),
        }
    }
}

/// Runs one state. `Err` means the state itself went wrong, which `Outcome::Redo` repeats.
pub fn run(ctx: &mut Context, state: State) -> Result<Done> {
    match state {
        State::Setup => setup::run(ctx).map(Done::from),
        State::Build => build::run(ctx),
        State::Verify => verify::run(ctx),
        State::Review => review::run(ctx),
        State::Ship => ship::run(ctx).map(Done::from),
        State::Retrospective => retrospective::run(ctx).map(Done::from),
        State::CleanUp => clean_up::run(ctx).map(Done::from),
        _ => Ok(Outcome::Pass.into()),
    }
}

/// Runs a headless session of `state` and keeps its transcript, also when the session ran out of
/// time, which is when it is most wanted.
pub(crate) fn session_run(ctx: &Context, state: State, task: &Task) -> Result<Reply> {
    let root = ctx.vcs.root_find(&ctx.from)?;
    let kept = Transcripts::open(ctx.vcs.as_ref(), &root)?;
    let unit = ctx.unit.to_string();
    // What a session says is printed as it happens and kept in the run log like a move.
    let log = RunLog::open(ctx.vcs.as_ref(), &root)?;
    let first = kept.next(&unit, state)?;
    let mut task = task.clone();
    task.events = Some(Events {
        unit: unit.clone(),
        state: state.to_string(),
        first,
        heartbeat: ctx.heartbeat,
        emit: Rc::new(move |event| {
            // A reader that went away must not stop the session, so a failed write is ignored.
            let _ = writeln!(std::io::stdout(), "{event}");
            if let Err(e) = log.append(&event) {
                eprintln!("ns: cannot log an event: {e:#}");
            }
        }),
    });
    let task = &task;
    // A usage limit that says when it resets is waited out here, so the state goes on in place.
    let ran = harness::usage_limit_wait(
        ctx.until,
        &|| ctx.os.time_now(),
        &|d| ctx.os.sleep(d),
        || ctx.harness.task_run(task),
    );
    match ran {
        Ok(reply) => {
            for raw in reply.earlier.iter().chain([&reply.transcript]) {
                kept.keep(&unit, state, raw, task.model.as_deref())?;
            }
            Ok(reply)
        }
        Err(e) => {
            if let Some(timed_out) = e.downcast_ref::<TimedOut>() {
                for raw in timed_out.earlier.iter().chain([&timed_out.transcript]) {
                    kept.keep(&unit, state, raw, task.model.as_deref())?;
                }
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_verdict_adds_no_verdict_field() {
        let facts = Facts::check(
            Some(verdict::Verdict::Error),
            0,
            std::path::Path::new("a.md"),
        );
        let mut event = serde_json::json!({"unit": "u"});
        facts.onto(&mut event);
        assert_eq!(
            event,
            serde_json::json!({"unit": "u", "findings": 0, "artifact": "a.md"})
        );
    }

    use crate::harness::UsageLimit;
    use crate::os::Finished;
    use chrono::TimeZone;
    use std::cell::{Cell, RefCell};
    use std::process::Command;
    use std::rc::Rc;

    fn at(hour: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 3, 10, hour, 0, 0)
            .earliest()
            .unwrap()
    }

    /// An operating system at 10:00 that records how long it was asked to sleep.
    struct Clock(Rc<RefCell<Vec<Duration>>>);

    impl Os for Clock {
        fn process_stream(
            &self,
            _: Command,
            _: &[u8],
            _: Duration,
            _: Duration,
            _: &mut dyn FnMut(crate::os::Streamed),
        ) -> Result<Finished> {
            unreachable!()
        }
        fn process_interact(&self, _: Command) -> Result<bool> {
            unreachable!()
        }
        fn process_replace(&self, _: Command) -> anyhow::Error {
            unreachable!()
        }
        fn shell_command(&self, _: &str) -> Command {
            unreachable!()
        }
        fn time_now(&self) -> DateTime<Local> {
            at(10)
        }
        fn sleep(&self, d: Duration) {
            self.0.borrow_mut().push(d);
        }
        fn runner_install(&self, _: &crate::os::Runner) -> Result<crate::os::Installed> {
            unreachable!()
        }
        fn runner_remove(&self, _: &std::path::Path) -> Result<bool> {
            unreachable!()
        }
        fn runner_check(&self, _: &crate::os::Runner) -> Result<crate::os::RunnerState> {
            unreachable!()
        }
    }

    /// A harness that hits a usage limit resetting at 13:00 once, then passes.
    struct Limited(Rc<Cell<u32>>, Rc<RefCell<Vec<usize>>>);

    impl Harness for Limited {
        fn version_get(&self) -> Result<String> {
            unreachable!()
        }
        fn skills_dir(&self) -> &'static str {
            unreachable!()
        }
        fn session_start_hook_install(&self, _: &std::path::Path, _: &str) -> Result<bool> {
            unreachable!()
        }
        fn task_run(&self, task: &Task) -> Result<Reply> {
            self.0.set(self.0.get() + 1);
            let ev = task.events.as_ref().expect("a state's session has events");
            (ev.emit)(
                serde_json::json!({"unit": ev.unit, "session": "start", "state": ev.state, "n": ev.first}),
            );
            // What the run log holds while the session is still running.
            let log = RunLog::open(crate::vcs::get().as_ref(), &task.dir).unwrap();
            self.1.borrow_mut().push(log.events().unwrap().len());
            if self.0.get() == 1 {
                return Err(UsageLimit {
                    resets: Some(at(13)),
                }
                .into());
            }
            Ok(Reply {
                text: "done".into(),
                success: true,
                session: None,
                transcript: b"{}".to_vec(),
                earlier: Vec::new(),
            })
        }
        fn transcript_render(&self, _: &str) -> String {
            unreachable!()
        }
        fn transcript_messages(&self, _: &str) -> Vec<crate::harness::Message> {
            unreachable!()
        }
        fn transcript_usage(&self, _: &str) -> crate::harness::Usage {
            unreachable!()
        }
        fn session_interact(&self, _: &std::path::Path, _: &str) -> Result<bool> {
            unreachable!()
        }
    }

    #[test]
    fn session_run_waits_out_a_usage_limit_with_a_reset_and_calls_again() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(out.status.success());
        };
        git(&["init", "-q"]);
        let slept = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::new(Cell::new(0));
        let logged = Rc::new(RefCell::new(Vec::new()));
        // Two sessions of the state are already kept, so the next is number 3.
        let kept = Transcripts::open(crate::vcs::get().as_ref(), repo.path()).unwrap();
        kept.keep("7", State::Build, b"{}", None).unwrap();
        kept.keep("7", State::Build, b"{}", None).unwrap();
        let ctx = Context {
            vcs: crate::vcs::get(),
            forge: crate::forge::get(),
            harness: Box::new(Limited(calls.clone(), logged.clone())),
            os: Box::new(Clock(slept.clone())),
            from: repo.path().to_path_buf(),
            unit: 7,
            workspace: None,
            ship: ship::Options {
                merge: false,
                poll: Duration::ZERO,
                timeout: Duration::ZERO,
                start: Duration::ZERO,
                queue: false,
                max_base_moves: 0,
            },
            harness_timeout: Duration::from_secs(1),
            checks_timeout: Duration::from_secs(1),
            model: None,
            until: None,
            heartbeat: Duration::ZERO,
        };
        let task = Task {
            dir: repo.path().to_path_buf(),
            prompt: String::new(),
            write: false,
            model: None,
            timeout: Duration::from_secs(1),
            resume: None,
            events: None,
        };
        let reply = session_run(&ctx, State::Build, &task).unwrap();
        assert_eq!(reply.text, "done");
        assert_eq!(calls.get(), 2);
        assert_eq!(*slept.borrow(), [Duration::from_secs(3 * 3600)]);
        // Each session's start was in the run log while the session still ran, not after it.
        assert_eq!(*logged.borrow(), [1, 2]);
        let log = RunLog::open(ctx.vcs.as_ref(), repo.path()).unwrap();
        let events = log.events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["session"], "start");
        assert_eq!(events[0]["state"], "build");
        assert_eq!(events[0]["unit"], "7");
        assert_eq!(events[0]["n"], 3);
    }
}
