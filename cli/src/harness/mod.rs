//! Coding-agent harnesses behind one trait. Everything specific to one lives in its own file.

mod claude;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::{DateTime, Local};

/// Marks an error as the harness session itself failing (it did not start, finish or answer in
/// time), not a model answer `ns` cannot use.
#[derive(Debug)]
struct SessionFailed;

impl std::fmt::Display for SessionFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the harness session failed")
    }
}

/// Marks `error` as a failed session.
pub fn session_failed(error: anyhow::Error) -> anyhow::Error {
    error.context(SessionFailed)
}

/// Whether `error` was marked by [`session_failed`].
pub fn is_session_failure(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SessionFailed>().is_some()
}

/// The error of a session the harness refused because the user's usage limit is reached. It
/// lasts hours and clears when its window ends, so it is waited out or stopped on, never repeated
/// after seconds, and it never costs a unit an attempt.
#[derive(Debug)]
pub struct UsageLimit {
    /// When the limit ends, when the harness said so in a way that could be read.
    pub resets: Option<DateTime<Local>>,
}

impl std::fmt::Display for UsageLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.resets {
            Some(at) => write!(
                f,
                "the harness usage limit is reached and resets at {}",
                at.format("%Y-%m-%d %H:%M")
            ),
            None => {
                f.write_str("the harness usage limit is reached and it does not say when it resets")
            }
        }
    }
}

impl std::error::Error for UsageLimit {}

/// The usage limit `error` is, however much context is on top of it.
pub fn usage_limit_of(error: &anyhow::Error) -> Option<&UsageLimit> {
    error.downcast_ref::<UsageLimit>()
}

/// The longest `ns` sleeps for a reset that is already past, so a limit that still holds is not
/// asked again in a tight loop.
const MIN_WAIT: Duration = Duration::from_secs(60);

/// Makes `call`, and while it fails on a usage limit that says when it resets, prints that it is
/// waiting, `sleep`s until then and makes it again, so the state goes on where it was. A limit
/// with no reset time, or one that resets after `until`, is returned as the error.
pub fn usage_limit_wait<T>(
    until: Option<DateTime<Local>>,
    now: &dyn Fn() -> DateTime<Local>,
    sleep: &dyn Fn(Duration),
    mut call: impl FnMut() -> Result<T>,
) -> Result<T> {
    loop {
        let error = match call() {
            Err(e) => e,
            ok => return ok,
        };
        let Some(at) = usage_limit_of(&error).and_then(|limit| limit.resets) else {
            return Err(error);
        };
        if until.is_some_and(|deadline| at > deadline) {
            return Err(error.context("that is after the time `ns auto` stops"));
        }
        eprintln!(
            "ns: usage limit reached; waiting until {}",
            at.format("%Y-%m-%d %H:%M")
        );
        sleep((at - now()).to_std().unwrap_or_default().max(MIN_WAIT));
    }
}

pub trait Harness {
    /// The version the harness reports. An error when it cannot be started.
    fn version_get(&self) -> Result<String>;

    /// Where, relative to a repository's top, this harness looks for skills.
    fn skills_dir(&self) -> &'static str;

    /// Makes the harness run `command` whenever a session starts in the repository at `root`, by
    /// a file meant to be committed. Does nothing when it already does. Returns whether it wrote.
    fn session_start_hook_install(&self, root: &Path, command: &str) -> Result<bool>;

    /// Runs one headless session and waits for it to finish.
    fn task_run(&self, task: &Task) -> Result<Reply>;

    /// A saved session transcript as plain text: what the agent said, each tool call and the
    /// start of each tool result, in order.
    fn transcript_render(&self, transcript: &str) -> String;

    /// The model and the tokens a saved session transcript records.
    fn transcript_usage(&self, transcript: &str) -> Usage;

    /// Starts a session the user takes part in, on their terminal, that begins with `prompt`.
    /// Returns when the user ends it, and whether it ended normally.
    fn session_interact(&self, dir: &Path, prompt: &str) -> Result<bool>;
}

/// The four token totals of a session, kept apart: they cost differently and are not summed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// What a session's transcript says about its model and its tokens. `None` is what it does not say.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Usage {
    pub model: Option<String>,
    pub tokens: Option<Tokens>,
}

pub struct Task {
    /// Where the session works.
    pub dir: PathBuf,
    pub prompt: String,
    /// Whether the session may change files. Without it the session only reads.
    pub write: bool,
    /// The model to use. `None` takes the harness's own default.
    pub model: Option<String>,
    /// How long the session may take. A session still running then is killed and is an error.
    pub timeout: Duration,
    /// The session to carry on, by the id its reply gave. `None` starts a new one.
    pub resume: Option<String>,
}

/// How a batch of sessions runs.
pub struct Sessions<'a> {
    /// How long one session may take.
    pub timeout: Duration,
    /// The model each session uses. `None` takes the harness's own default.
    pub model: Option<&'a str>,
}

#[derive(Debug)]
pub struct Reply {
    pub text: String,
    /// Whether the session ended normally.
    pub success: bool,
    /// The id to resume this session by. `None` when the harness gave none.
    pub session: Option<String>,
    /// Everything the session printed, as the harness prints it: what was read and run, not only
    /// the final `text`.
    pub transcript: Vec<u8>,
    /// The transcripts of the sessions started again before this one, oldest first.
    pub earlier: Vec<Vec<u8>>,
}

/// The error of a session that ran out of time and was killed. It keeps what the session
/// produced up to then.
#[derive(Debug)]
pub struct TimedOut {
    pub seconds: u64,
    pub transcript: Vec<u8>,
    /// The transcripts of the sessions started again before this one, oldest first.
    pub earlier: Vec<Vec<u8>>,
}

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the harness did not finish within {} seconds",
            self.seconds
        )
    }
}

impl std::error::Error for TimedOut {}

/// The harnesses a repository can name in its manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Claude,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
        }
    }

    pub fn parse(name: &str) -> Result<Kind> {
        match name {
            "claude" => Ok(Kind::Claude),
            "codex" => bail!("the codex harness is not supported yet"),
            other => bail!("there is no harness named {other}"),
        }
    }
}

/// The harness `ns` uses. The only place one is chosen.
pub fn get() -> Box<dyn Harness> {
    of(Kind::Claude)
}

/// The harness of `kind`.
pub fn of(kind: Kind) -> Box<dyn Harness> {
    match kind {
        Kind::Claude => Box::new(claude::Claude::new(crate::os::get())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u32) -> DateTime<Local> {
        use chrono::TimeZone;
        Local.with_ymd_and_hms(2026, 3, 10, hour, 0, 0).unwrap()
    }

    /// Calls `usage_limit_wait` with a call that hits `limit` `times` times, then passes. Returns
    /// the result, how many calls were made and how long it slept each time.
    fn waited(
        until: Option<DateTime<Local>>,
        resets: Option<DateTime<Local>>,
        times: u32,
    ) -> (Result<&'static str>, u32, Vec<Duration>) {
        let calls = std::cell::Cell::new(0);
        let slept = std::cell::RefCell::new(Vec::new());
        let result = usage_limit_wait(until, &|| at(10), &|d| slept.borrow_mut().push(d), || {
            calls.set(calls.get() + 1);
            if calls.get() <= times {
                return Err(UsageLimit { resets }.into());
            }
            Ok("done")
        });
        (result, calls.get(), slept.into_inner())
    }

    #[test]
    fn a_limit_with_a_reset_is_slept_through_and_the_call_made_again() {
        let (result, calls, slept) = waited(None, Some(at(13)), 1);
        assert_eq!(result.unwrap(), "done");
        assert_eq!(calls, 2);
        assert_eq!(slept, [Duration::from_secs(3 * 3600)]);
    }

    #[test]
    fn a_reset_already_past_is_waited_a_minute_not_none() {
        let (result, _, slept) = waited(None, Some(at(9)), 1);
        assert!(result.is_ok());
        assert_eq!(slept, [Duration::from_secs(60)]);
    }

    #[test]
    fn a_limit_with_no_reset_is_returned_without_a_sleep() {
        let (result, calls, slept) = waited(None, None, 1);
        assert!(usage_limit_of(&result.unwrap_err()).is_some());
        assert_eq!(calls, 1);
        assert!(slept.is_empty());
    }

    #[test]
    fn a_reset_after_the_deadline_is_returned_without_a_sleep() {
        let (result, calls, slept) = waited(Some(at(12)), Some(at(13)), 1);
        let error = result.unwrap_err();
        assert!(usage_limit_of(&error).is_some());
        assert!(format!("{error:#}").contains("after the time `ns auto` stops"));
        assert_eq!(calls, 1);
        assert!(slept.is_empty());
        assert!(waited(Some(at(13)), Some(at(13)), 1).0.is_ok());
    }

    #[test]
    fn any_other_error_is_returned_at_once() {
        let mut calls = 0;
        let result: Result<()> = usage_limit_wait(None, &|| at(10), &|_| panic!("slept"), || {
            calls += 1;
            Err(anyhow::anyhow!("no"))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn a_marked_error_is_a_session_failure_even_with_more_context_on_top() {
        let marked = session_failed(anyhow::anyhow!("not logged in")).context("unit 4");
        assert!(is_session_failure(&marked));
        assert_eq!(
            format!("{marked:#}"),
            "unit 4: the harness session failed: not logged in"
        );
    }

    #[test]
    fn an_unmarked_error_is_not_a_session_failure() {
        assert!(!is_session_failure(&anyhow::anyhow!("no ROUTE line")));
    }
}
