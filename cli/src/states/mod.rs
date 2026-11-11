//! What each state does. A state passes by returning `Ok` and fails by returning `Err`.

pub mod build;
mod check;
pub mod clean_up;
pub mod retrospective;
pub mod review;
mod rounds;
mod setup;
pub mod ship;
pub mod verdict;
pub mod verify;

pub use verify::unproven_note;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local};

use crate::forge::Forge;
use crate::harness::{self, Harness, Reply, Task, TimedOut};
use crate::manifest::{self, Place};
use crate::os::Os;
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
}

impl Context {
    /// The model a session at `place` uses.
    pub(crate) fn model_find(&self, place: Place) -> Result<Option<String>> {
        let root = self.vcs.root_find(&self.from)?;
        manifest::model_read(&root, place, self.model.as_deref())
    }
}

/// Runs one state. `Err` means the state itself went wrong, which `Outcome::Redo` repeats.
pub fn run(ctx: &mut Context, state: State) -> Result<Outcome> {
    match state {
        State::Setup => setup::run(ctx),
        State::Build => build::run(ctx),
        State::Verify => verify::run(ctx),
        State::Review => review::run(ctx),
        State::Ship => ship::run(ctx),
        State::Retrospective => retrospective::run(ctx),
        State::CleanUp => clean_up::run(ctx),
        _ => Ok(Outcome::Pass),
    }
}

/// Runs a headless session of `state` and keeps its transcript, also when the session ran out of
/// time, which is when it is most wanted.
pub(crate) fn session_run(ctx: &Context, state: State, task: &Task) -> Result<Reply> {
    let root = ctx.vcs.root_find(&ctx.from)?;
    let kept = Transcripts::open(ctx.vcs.as_ref(), &root)?;
    let unit = ctx.unit.to_string();
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
        fn process_run(&self, _: Command, _: &[u8], _: Duration) -> Result<Finished> {
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
    struct Limited(Rc<Cell<u32>>);

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
        fn task_run(&self, _: &Task) -> Result<Reply> {
            self.0.set(self.0.get() + 1);
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
        let ctx = Context {
            vcs: crate::vcs::get(),
            forge: crate::forge::get(),
            harness: Box::new(Limited(calls.clone())),
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
        };
        let task = Task {
            dir: repo.path().to_path_buf(),
            prompt: String::new(),
            write: false,
            model: None,
            timeout: Duration::from_secs(1),
            resume: None,
        };
        let reply = session_run(&ctx, State::Build, &task).unwrap();
        assert_eq!(reply.text, "done");
        assert_eq!(calls.get(), 2);
        assert_eq!(*slept.borrow(), [Duration::from_secs(3 * 3600)]);
    }
}
