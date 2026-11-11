use std::cell::RefCell;
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{Local, TimeZone};

use super::{Harness, Reply, Task, TimedOut, Usage, UsageLimit};
use crate::os::Os;
use crate::retry;

mod transcript;

/// What a session that ended badly prints when the service behind it had a bad moment. A session
/// out of time is not one of these: it is killed and is an error.
const TRANSIENT: [&str; 7] = [
    "overloaded",
    "rate limit",
    "api error: 5",
    "api error: 429",
    "econnreset",
    "etimedout",
    "socket hang up",
];

/// What a session that ended badly prints when the user's subscription has no usage left until a
/// window ends. This is not a bad moment, so it is never repeated after seconds.
///
/// The text comes from the installed `claude` CLI (2.1.321), which holds these phrases, and from
/// Claude Code's documentation of its usage limits. It has not been seen from a real limit: a
/// real one, once seen, replaces the fixtures in the tests.
const USAGE_LIMIT: [&str; 6] = [
    "usage limit reached",
    "usage limit has been reached",
    "reached your usage limit",
    "reached your weekly usage limit",
    "hit your usage limit",
    "hit your limit",
];

/// The usage limit `reply` says was reached, with its reset time when one can be read. Two forms
/// are read, both as epoch seconds: the older text `Claude AI usage limit reached|<epoch>`, and
/// the `resetsAt` of a `rate_limit_event` the stream printed with status `rejected`, which the
/// CLI emits in `stream-json` mode. A limit whose text gives only a clock time such as "resets
/// 3pm" has no reset that can be read.
fn usage_limit_find(reply: &Reply) -> Option<UsageLimit> {
    if reply.success || !retry::says(&reply.text, &USAGE_LIMIT) {
        return None;
    }
    let from_text = reply
        .text
        .to_lowercase()
        .split_once("limit reached|")
        .and_then(|(_, rest)| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<i64>().ok()
        });
    let from_stream = || {
        String::from_utf8_lossy(&reply.transcript)
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| event["type"] == "rate_limit_event")
            .map(|event| event["rate_limit_info"].clone())
            .filter(|info| info["status"] == "rejected")
            .filter_map(|info| info["resetsAt"].as_i64())
            .next_back()
    };
    let resets = from_text
        .or_else(from_stream)
        .map(|n| if n > 100_000_000_000 { n / 1000 } else { n })
        .and_then(|secs| Local.timestamp_opt(secs, 0).single());
    Some(UsageLimit { resets })
}

pub struct Claude {
    program: OsString,
    os: Box<dyn Os>,
    /// How long to wait before the second attempt of a session.
    retry_delay: Duration,
    /// Whether `NS_DEV` is set: `ns` is then the build being worked on, and a session must find
    /// that build and not an installed release.
    dev: bool,
}

impl Claude {
    pub fn new(os: Box<dyn Os>) -> Claude {
        Claude::with_program("claude", os)
    }

    pub fn with_program(program: impl Into<OsString>, os: Box<dyn Os>) -> Claude {
        Claude {
            program: program.into(),
            os,
            retry_delay: retry::DELAY,
            dev: std::env::var_os("NS_DEV").is_some(),
        }
    }

    /// The command that starts the harness, with the environment every session gets. Under
    /// `NS_DEV` the directory of the running `ns` comes first on `PATH`, so the hook a session
    /// runs and any `ns` it calls are this build. Otherwise `PATH` is left as it is, because the
    /// dispatcher already hands over to the pinned version.
    fn command(&self) -> Result<Command> {
        let mut cmd = Command::new(&self.program);
        if self.dev {
            let exe = std::env::current_exe().context("cannot find the running ns")?;
            let dir = exe.parent().context("the running ns has no directory")?;
            let mut dirs = vec![dir.to_path_buf()];
            dirs.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            cmd.env(
                "PATH",
                std::env::join_paths(dirs).context("cannot build PATH")?,
            );
        }
        Ok(cmd)
    }

    fn session_run(&self, task: &Task) -> Result<Reply> {
        let mut cmd = self.command()?;
        // The stream carries the tool calls and results; `--verbose` is what `claude` requires of
        // `stream-json` in a headless session.
        cmd.args(["-p", "--output-format", "stream-json", "--verbose"])
            .arg("--permission-mode")
            .arg(if task.write {
                "bypassPermissions"
            } else {
                "plan"
            });
        if let Some(session) = &task.resume {
            cmd.arg("--resume").arg(session);
        }
        if let Some(model) = &task.model {
            cmd.arg("--model").arg(model);
        }
        cmd.current_dir(&task.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let done = self
            .os
            .process_run(cmd, task.prompt.as_bytes(), task.timeout)
            .context("cannot run claude")?;
        if done.success.is_none() {
            return Err(TimedOut {
                seconds: task.timeout.as_secs(),
                transcript: done.stdout,
                earlier: Vec::new(),
            }
            .into());
        }
        let raw = String::from_utf8_lossy(&done.stdout).into_owned();
        let session = transcript::session_find(&raw);
        Ok(Reply {
            // A session that died before its result printed has only what it printed, which
            // still says why.
            text: transcript::result_find(&raw).unwrap_or(raw),
            success: done.success == Some(true),
            session,
            transcript: done.stdout,
            earlier: Vec::new(),
        })
    }
}

impl Harness for Claude {
    fn version_get(&self) -> Result<String> {
        let out = Command::new(&self.program)
            .arg("--version")
            .output()
            .context("cannot run claude")?;
        if !out.status.success() {
            bail!(
                "claude --version failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn skills_dir(&self) -> &'static str {
        ".claude/skills"
    }

    fn session_start_hook_install(&self, root: &Path, command: &str) -> Result<bool> {
        let file = root.join(".claude").join("settings.json");
        let mut settings: serde_json::Value = match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", file.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
        };
        let hooks = settings
            .as_object_mut()
            .context("settings.json is not an object")?
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .context("`hooks` in settings.json is not an object")?
            .entry("SessionStart")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .context("`SessionStart` in settings.json is not a list")?;
        let present = hooks.iter().any(|group| {
            group["hooks"]
                .as_array()
                .is_some_and(|list| list.iter().any(|hook| hook["command"] == command))
        });
        if present {
            return Ok(false);
        }
        hooks.push(serde_json::json!({ "hooks": [{ "type": "command", "command": command }] }));
        std::fs::create_dir_all(file.parent().context("settings.json has no directory")?)?;
        std::fs::write(&file, serde_json::to_string_pretty(&settings)? + "\n")
            .with_context(|| format!("cannot write {}", file.display()))?;
        Ok(true)
    }

    fn transcript_render(&self, raw: &str) -> String {
        transcript::render(raw)
    }

    fn transcript_usage(&self, raw: &str) -> Usage {
        transcript::usage(raw)
    }

    fn session_interact(&self, dir: &Path, prompt: &str) -> Result<bool> {
        let mut cmd = self.command()?;
        cmd.arg(prompt).current_dir(dir);
        self.os.process_interact(cmd).context("cannot run claude")
    }

    /// A session that ended on a usage limit is an error [`UsageLimit`], never started again here.
    /// A session that ended badly because the service had a bad moment is started again. What it
    /// changed before it ended stays, as it does when a state runs again.
    fn task_run(&self, task: &Task) -> Result<Reply> {
        let earlier = RefCell::new(Vec::new());
        let attempts = std::cell::Cell::new(0);
        let mut result = retry::retry(
            self.retry_delay,
            || self.session_run(task),
            |r| match r {
                // A usage limit is no bad moment, and its text may hold a word of `TRANSIENT`.
                Ok(reply) if usage_limit_find(reply).is_some() => None,
                Ok(reply) if !reply.success && retry::says(&reply.text, &TRANSIENT) => {
                    attempts.set(attempts.get() + 1);
                    // The last attempt is the reply itself, not an earlier session.
                    if attempts.get() < retry::ATTEMPTS {
                        earlier.borrow_mut().push(reply.transcript.clone());
                    }
                    Some(format!("claude ended with: {}", reply.text.trim()))
                }
                Ok(_) | Err(_) => None,
            },
        );
        let earlier = earlier.into_inner();
        // It is an error, not a failed reply, so no caller spends an attempt on it.
        if let Some(limit) = result.as_ref().ok().and_then(usage_limit_find) {
            return Err(limit.into());
        }
        match &mut result {
            Ok(reply) => reply.earlier = earlier,
            Err(e) => {
                if let Some(timed_out) = e.downcast_mut::<TimedOut>() {
                    timed_out.earlier = earlier;
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    use super::*;

    /// A stand-in for `claude` that prints its arguments, then the prompt it was sent.
    fn fake(dir: &Path, exit: i32) -> Claude {
        let path = dir.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\necho \"$@\"\ncat\nexit {exit}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Claude::with_program(path, crate::os::get())
    }

    fn task(dir: &Path, write: bool) -> Task {
        Task {
            dir: dir.to_path_buf(),
            prompt: "do it".into(),
            write,
            model: Some("opus".into()),
            timeout: Duration::from_secs(30),
            resume: None,
        }
    }

    #[test]
    fn a_read_only_task_plans_and_a_write_task_accepts_edits() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let claude = fake(tmp.path(), 0);
        let read = claude.task_run(&task(tmp.path(), false)).unwrap();
        assert_eq!(
            read.text,
            "-p --output-format stream-json --verbose --permission-mode plan --model opus\ndo it"
        );
        let write = claude.task_run(&task(tmp.path(), true)).unwrap();
        assert_eq!(
            write.text,
            "-p --output-format stream-json --verbose --permission-mode bypassPermissions --model opus\ndo it"
        );
        assert!(write.success);
    }

    /// A stand-in for `claude` that prints the `PATH` it was started with.
    fn path_printer(dir: &Path, dev: bool) -> Claude {
        let path = dir.join("claude");
        std::fs::write(&path, "#!/bin/sh\ncat >/dev/null\necho \"$PATH\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut claude = Claude::with_program(path, crate::os::get());
        claude.dev = dev;
        claude
    }

    #[test]
    fn under_ns_dev_the_running_ns_comes_first_on_path_and_otherwise_path_is_as_it_was() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let ours = std::env::current_exe().unwrap();
        let first = format!("{}:", ours.parent().unwrap().display());
        let outer = std::env::var("PATH").unwrap();

        let dev = path_printer(tmp.path(), true)
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert_eq!(dev.text.trim(), format!("{first}{outer}"));

        let plain = path_printer(tmp.path(), false)
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert_eq!(plain.text.trim(), outer);
    }

    #[test]
    fn a_failing_session_is_reported_not_raised() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let reply = fake(tmp.path(), 3)
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert!(!reply.success);
    }

    #[test]
    fn a_session_out_of_time_is_an_error() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("claude");
        std::fs::write(&path, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut late = task(tmp.path(), true);
        late.timeout = Duration::ZERO;
        let err = Claude::with_program(path, crate::os::get())
            .task_run(&late)
            .unwrap_err();
        assert!(err.to_string().contains("did not finish"));
    }

    /// A stand-in for `claude` that prints `stream` as it is, then ends with `tail`.
    fn streaming(dir: &Path, stream: &str, tail: &str) -> Claude {
        let path = dir.join("claude");
        std::fs::write(
            &path,
            format!("#!/bin/sh\ncat >/dev/null\ncat <<'EOF'\n{stream}\nEOF\n{tail}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Claude::with_program(path, crate::os::get())
    }

    const STREAM: &str = concat!(
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]}}"#,
        "\n",
        r#"{"type":"result","result":"All done."}"#
    );

    #[test]
    fn the_reply_is_the_final_result_and_the_transcript_is_the_whole_stream() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let reply = streaming(tmp.path(), STREAM, "")
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert_eq!(reply.text, "All done.");
        assert_eq!(
            String::from_utf8(reply.transcript).unwrap(),
            format!("{STREAM}\n")
        );
    }

    #[test]
    fn a_session_out_of_time_keeps_what_it_printed() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let mut late = task(tmp.path(), true);
        late.timeout = Duration::from_secs(2);
        let err = streaming(tmp.path(), STREAM, "exec sleep 30")
            .task_run(&late)
            .unwrap_err();
        let timed_out = err.downcast_ref::<TimedOut>().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&timed_out.transcript),
            format!("{STREAM}\n")
        );
    }

    #[test]
    fn a_saved_stream_renders_through_the_harness() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let claude = streaming(tmp.path(), STREAM, "");
        assert_eq!(claude.transcript_render(STREAM), "working\n");
    }

    /// A stand-in for `claude` that ends badly with `said` the first `failures` times, then
    /// answers. It counts its sessions in a file beside it.
    fn flaky(dir: &Path, failures: u32, said: &str) -> Claude {
        let path = dir.join("claude");
        let calls = dir.join("calls");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\ncat >/dev/null\necho x >> '{calls}'\nif [ $(wc -l < '{calls}') -le {failures} ]; then echo '{said}'; exit 1; fi\necho done\n",
                calls = calls.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut claude = Claude::with_program(path, crate::os::get());
        claude.retry_delay = Duration::ZERO;
        claude
    }

    fn sessions(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap()
            .lines()
            .count()
    }

    #[test]
    fn a_session_the_service_dropped_is_started_again() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let reply = flaky(tmp.path(), 2, "API Error: 529 Overloaded")
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert!(reply.success);
        assert_eq!(sessions(tmp.path()), 3);
    }

    #[test]
    fn when_every_attempt_fails_the_last_session_is_not_also_an_earlier_one() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let reply = flaky(tmp.path(), 99, "API Error: 529 Overloaded")
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert!(!reply.success);
        assert_eq!(sessions(tmp.path()), 3);
        assert_eq!(reply.earlier.len(), 2);
    }

    #[test]
    fn the_transcript_of_each_session_started_again_travels_with_the_reply() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let reply = flaky(tmp.path(), 2, "API Error: 529 Overloaded")
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert_eq!(reply.earlier.len(), 2);
        assert_eq!(reply.earlier[0], b"API Error: 529 Overloaded\n");
        assert_eq!(reply.transcript, b"done\n");
    }

    #[test]
    fn a_session_out_of_time_after_a_retry_keeps_the_earlier_transcripts() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("claude");
        let calls = tmp.path().join("calls");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\ncat >/dev/null\necho x >> '{calls}'\nif [ $(wc -l < '{calls}') -le 1 ]; then echo 'API Error: 529 Overloaded'; exit 1; fi\nexec sleep 30\n",
                calls = calls.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut claude = Claude::with_program(path, crate::os::get());
        claude.retry_delay = Duration::ZERO;
        let mut late = task(tmp.path(), true);
        late.timeout = Duration::from_secs(2);
        let err = claude.task_run(&late).unwrap_err();
        let timed_out = err.downcast_ref::<TimedOut>().unwrap();
        assert_eq!(
            timed_out.earlier,
            vec![b"API Error: 529 Overloaded\n".to_vec()]
        );
    }

    #[test]
    fn a_session_that_failed_for_any_other_reason_is_not_started_again() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let reply = flaky(tmp.path(), 99, "the tests do not pass")
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert!(!reply.success);
        assert_eq!(sessions(tmp.path()), 1);
    }

    #[test]
    fn an_interactive_session_starts_with_the_prompt_and_reports_how_it_ended() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("claude");
        std::fs::write(
            &path,
            "#!/bin/sh\necho \"$1\" > said\nexit \"${EXIT:-0}\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let claude = Claude::with_program(path, crate::os::get());
        assert!(claude.session_interact(tmp.path(), "grill me").unwrap());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("said")).unwrap(),
            "grill me\n"
        );
    }

    /// A stand-in for `claude` that prints `said` and ends badly, and counts its sessions.
    fn limited(dir: &Path, said: &str) -> Claude {
        flaky(dir, 99, said)
    }

    #[test]
    fn a_usage_limit_with_a_reset_is_an_error_that_is_not_started_again() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let err = limited(tmp.path(), "Claude AI usage limit reached|1773180000")
            .task_run(&task(tmp.path(), true))
            .unwrap_err();
        let limit = crate::harness::usage_limit_of(&err).unwrap();
        assert_eq!(limit.resets.unwrap().timestamp(), 1_773_180_000);
        assert_eq!(sessions(tmp.path()), 1);
    }

    #[test]
    fn a_usage_limit_in_a_rate_limit_event_gives_its_reset() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let stream = concat!(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1773180000,"rateLimitType":"five_hour"}}"#,
            "\n",
            r#"{"type":"result","is_error":true,"result":"You've hit your limit"}"#
        );
        let err = streaming(tmp.path(), stream, "exit 1")
            .task_run(&task(tmp.path(), true))
            .unwrap_err();
        let limit = crate::harness::usage_limit_of(&err).unwrap();
        assert_eq!(limit.resets.unwrap().timestamp(), 1_773_180_000);
    }

    #[test]
    fn a_reset_in_milliseconds_is_read_as_one() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let stream = concat!(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1773180000000}}"#,
            "\n",
            r#"{"type":"result","is_error":true,"result":"You've hit your limit"}"#
        );
        let err = streaming(tmp.path(), stream, "exit 1")
            .task_run(&task(tmp.path(), true))
            .unwrap_err();
        let limit = crate::harness::usage_limit_of(&err).unwrap();
        assert_eq!(limit.resets.unwrap().timestamp(), 1_773_180_000);
    }

    #[test]
    fn a_usage_limit_that_says_no_time_is_an_error_with_no_reset() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let err = limited(tmp.path(), "5-hour usage limit reached, resets 3pm")
            .task_run(&task(tmp.path(), true))
            .unwrap_err();
        assert!(crate::harness::usage_limit_of(&err)
            .unwrap()
            .resets
            .is_none());
        assert_eq!(sessions(tmp.path()), 1);
    }

    #[test]
    fn a_usage_limit_that_also_says_429_is_still_not_started_again() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let err = limited(
            tmp.path(),
            "API Error: 429 Claude AI usage limit reached|1773180000",
        )
        .task_run(&task(tmp.path(), true))
        .unwrap_err();
        assert!(crate::harness::usage_limit_of(&err).is_some());
        assert_eq!(sessions(tmp.path()), 1);
    }

    #[test]
    fn a_429_that_is_not_a_usage_limit_is_still_started_again() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let reply = flaky(tmp.path(), 2, "API Error: 429 rate limit exceeded")
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert!(reply.success);
        assert_eq!(sessions(tmp.path()), 3);
    }
}
