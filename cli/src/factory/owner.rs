//! The GitHub owner type `factory validate` reads to warn when build-only meets a personal account.

use std::process::{Command, Stdio};

use crate::forge::{output_within, safe_host, PROBE_TIMEOUT};

/// `owner.type` of GitHub `repo` (`User` or `Organization`), from `gh`'s own login, asked of the
/// host `origin_url` is on (gh's default host when it is github.com); `None` for a host that fails `safe_host`. `None`
/// when gh is missing, fails or times out.
pub fn github_owner_type(repo: &str, origin_url: Option<&str>) -> Option<String> {
    let host = origin_url
        .and_then(crate::watch::forge::host)
        .filter(|h| !h.eq_ignore_ascii_case("github.com"));
    if host.is_some_and(|h| !safe_host(h)) {
        return None;
    }
    let mut cmd = Command::new("gh");
    crate::git::scrub(&mut cmd);
    cmd.arg("api");
    if let Some(h) = host {
        cmd.args(["--hostname", h]);
    }
    cmd.args([&format!("repos/{repo}"), "--jq", ".owner.type"])
        .stdin(Stdio::null());
    let out = output_within(cmd, PROBE_TIMEOUT).ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    out.status.success().then_some(text)
}
