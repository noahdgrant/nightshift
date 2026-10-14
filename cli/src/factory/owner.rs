//! The GitHub owner type `factory validate` reads to warn when build-only meets a personal account.

use std::process::{Command, Stdio};

use crate::forge::{output_within, PROBE_TIMEOUT};

/// `owner.type` of GitHub `repo` (`User` or `Organization`), from `gh`'s own login. `None` when
/// gh is missing, fails or times out.
pub fn github_owner_type(repo: &str) -> Option<String> {
    let mut cmd = Command::new("gh");
    crate::git::scrub(&mut cmd);
    cmd.args(["api", &format!("repos/{repo}"), "--jq", ".owner.type"])
        .stdin(Stdio::null());
    let out = output_within(cmd, PROBE_TIMEOUT).ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    out.status.success().then_some(text)
}
