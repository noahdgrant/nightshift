use std::cell::RefCell;
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::{Harness, Reply, Task, TimedOut};
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

pub struct Claude {
    program: OsString,
    os: Box<dyn Os>,
    /// How long to wait before the second attempt of a session.
    retry_delay: Duration,
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
        }
    }

    fn session_run(&self, task: &Task) -> Result<Reply> {
        let mut cmd = Command::new(&self.program);
        // The stream carries the tool calls and results; `--verbose` is what `claude` requires of
        // `stream-json` in a headless session.
        cmd.args(["-p", "--output-format", "stream-json", "--verbose"])
            .arg("--permission-mode")
            .arg(if task.write {
                "bypassPermissions"
            } else {
                "plan"
            });
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
        Ok(Reply {
            // A session that died before its result printed has only what it printed, which
            // still says why.
            text: transcript::result_find(&raw).unwrap_or(raw),
            success: done.success == Some(true),
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

    fn session_interact(&self, dir: &Path, prompt: &str) -> Result<bool> {
        let mut cmd = Command::new(&self.program);
        cmd.arg(prompt).current_dir(dir);
        self.os.process_interact(cmd).context("cannot run claude")
    }

    /// A session that ended badly because the service had a bad moment is started again. What it
    /// changed before it ended stays, as it does when a state runs again.
    fn task_run(&self, task: &Task) -> Result<Reply> {
        let earlier = RefCell::new(Vec::new());
        let attempts = std::cell::Cell::new(0);
        let mut result = retry::retry(
            self.retry_delay,
            || self.session_run(task),
            |r| match r {
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
}
