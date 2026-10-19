use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::{Harness, Reply, Task};
use crate::os::Os;
use crate::retry;

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
        cmd.arg("-p").arg("--permission-mode").arg(if task.write {
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
            bail!(
                "claude did not finish within {} seconds",
                task.timeout.as_secs()
            );
        }
        Ok(Reply {
            text: String::from_utf8_lossy(&done.stdout).into_owned(),
            success: done.success == Some(true),
        })
    }
}

impl Harness for Claude {
    fn skills_dir(&self) -> &'static str {
        ".claude/skills"
    }

    fn session_interact(&self, dir: &Path, prompt: &str) -> Result<bool> {
        let mut cmd = Command::new(&self.program);
        cmd.arg(prompt).current_dir(dir);
        self.os.process_interact(cmd).context("cannot run claude")
    }

    /// A session that ended badly because the service had a bad moment is started again. What it
    /// changed before it ended stays, as it does when a state runs again.
    fn task_run(&self, task: &Task) -> Result<Reply> {
        retry::retry(
            self.retry_delay,
            || self.session_run(task),
            |r| match r {
                Ok(reply) if !reply.success && retry::says(&reply.text, &TRANSIENT) => {
                    Some(format!("claude ended with: {}", reply.text.trim()))
                }
                Ok(_) | Err(_) => None,
            },
        )
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
        assert_eq!(read.text, "-p --permission-mode plan --model opus\ndo it");
        let write = claude.task_run(&task(tmp.path(), true)).unwrap();
        assert_eq!(
            write.text,
            "-p --permission-mode bypassPermissions --model opus\ndo it"
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
