use std::ffi::OsString;
use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use super::{Harness, Reply, Task};

pub struct Claude {
    program: OsString,
}

impl Claude {
    pub fn new() -> Claude {
        Claude::with_program("claude")
    }

    pub fn with_program(program: impl Into<OsString>) -> Claude {
        Claude {
            program: program.into(),
        }
    }
}

impl Harness for Claude {
    fn task_run(&self, task: &Task) -> Result<Reply> {
        let mut cmd = Command::new(&self.program);
        cmd.arg("-p").arg("--permission-mode").arg(if task.write {
            "bypassPermissions"
        } else {
            "plan"
        });
        if let Some(model) = &task.model {
            cmd.arg("--model").arg(model);
        }
        let mut child = cmd
            .current_dir(&task.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("cannot run claude")?;
        child
            .stdin
            .take()
            .context("claude has no stdin")?
            .write_all(task.prompt.as_bytes())
            .context("cannot send the prompt to claude")?;
        let out = child.wait_with_output().context("claude did not finish")?;
        Ok(Reply {
            text: String::from_utf8_lossy(&out.stdout).into_owned(),
            success: out.status.success(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    /// A stand-in for `claude` that prints its arguments, then the prompt it was sent.
    fn fake(dir: &Path, exit: i32) -> Claude {
        let path = dir.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\necho \"$@\"\ncat\nexit {exit}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Claude::with_program(path)
    }

    fn task(dir: &Path, write: bool) -> Task {
        Task {
            dir: dir.to_path_buf(),
            prompt: "do it".into(),
            write,
            model: Some("opus".into()),
        }
    }

    #[test]
    fn a_read_only_task_plans_and_a_write_task_accepts_edits() {
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
        let tmp = tempfile::tempdir().unwrap();
        let reply = fake(tmp.path(), 3)
            .task_run(&task(tmp.path(), true))
            .unwrap();
        assert!(!reply.success);
    }
}
