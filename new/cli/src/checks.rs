//! The repository's own checks (format, lint, tests), run on the unit's workspace. A pass is
//! remembered by what the workspace holds, so checking the same work twice runs them once.

use std::os::fd::AsFd;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::vcs::Vcs;

/// The file holding the command that runs the repository's checks, read from the workspace. A
/// unit that changes it is held to its change; whether the change is good is for `review` to say.
const COMMAND_FILE: &str = ".nightshift/check";

pub struct Report {
    pub passed: bool,
    /// Whether the pass was remembered from an earlier run on the same work.
    pub cached: bool,
    /// What the checks printed. Empty when it was streamed or came from the cache.
    pub output: String,
}

/// The command that runs the repository's checks, if the workspace at `workspace` names one.
pub fn command_find(workspace: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(workspace.join(COMMAND_FILE)) {
        Ok(text) => Ok(Some(text.trim().to_string()).filter(|c| !c.is_empty())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {COMMAND_FILE}")),
    }
}

/// Runs the checks on the workspace at `workspace`, unless they already passed on exactly what it
/// holds. `None` when the repository has no checks. With `stream`, the checks print as they run
/// and the report's output is empty.
pub fn run(vcs: &dyn Vcs, root: &Path, workspace: &Path, stream: bool) -> Result<Option<Report>> {
    let Some(command) = command_find(workspace)? else {
        return Ok(None);
    };
    let remembered = vcs.state_dir_find(root)?.join("checks-passed");
    let entry = remembered.join(vcs.workspace_snapshot(workspace)?);
    if std::fs::read_to_string(&entry).is_ok_and(|c| c == command) {
        return Ok(Some(Report {
            passed: true,
            cached: true,
            output: String::new(),
        }));
    }
    let (passed, output) = execute(&command, workspace, stream)?;
    if passed {
        std::fs::create_dir_all(&remembered)?;
        std::fs::write(&entry, &command)?;
    }
    Ok(Some(Report {
        passed,
        cached: false,
        output,
    }))
}

fn execute(command: &str, dir: &Path, stream: bool) -> Result<(bool, String)> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").current_dir(dir);
    if stream {
        let status = cmd
            .arg(command)
            .stdout(Stdio::from(std::io::stderr().as_fd().try_clone_to_owned()?))
            .status()
            .context("cannot run the checks")?;
        return Ok((status.success(), String::new()));
    }
    let out = cmd
        .arg(format!("{{ {command}\n}} 2>&1"))
        .output()
        .context("cannot run the checks")?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

/// The last `lines` lines of `output`, where a failure says what failed.
pub fn tail(output: &str, lines: usize) -> String {
    let all: Vec<&str> = output.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}
