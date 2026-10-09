//! Subscription billing guard and usage-limit detection for claude runs.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::error::SfError;

/// Variables that make claude bill an API key or a cloud provider instead of the subscription.
pub const API_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

pub fn is_claude(argv: &[String]) -> bool {
    argv.first()
        .and_then(|a| Path::new(a).file_name())
        .is_some_and(|n| n == "claude")
}

/// Headless claude kills background tasks this long after its last turn (default 10 minutes).
pub const BG_WAIT_ENV: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";

/// Lift the ceiling so the phase timeout is the only bound, unless the user set one.
pub fn wait_for_bg_tasks(cmd: &mut Command) {
    let set_here = cmd.get_envs().any(|(k, v)| k == BG_WAIT_ENV && v.is_some());
    if !set_here && std::env::var_os(BG_WAIT_ENV).is_none() {
        cmd.env(BG_WAIT_ENV, "0");
    }
}

/// Drop `--bare`, which forces API-key auth.
pub fn strip_bare(argv: Vec<String>) -> Vec<String> {
    argv.into_iter().filter(|a| a != "--bare").collect()
}

pub fn scrub(cmd: &mut Command) {
    for k in API_ENV {
        cmd.env_remove(k);
    }
}

fn credentials_path() -> PathBuf {
    crate::config::expand_tilde("~/.claude/.credentials.json")
}

/// Refuse to start without a subscription login: `~/.claude/.credentials.json` or `CLAUDE_CODE_OAUTH_TOKEN`.
pub fn check_login() -> anyhow::Result<()> {
    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok_and(|v| !v.is_empty());
    if token || credentials_path().is_file() {
        return Ok(());
    }
    Err(SfError::usage(
        format!(
            "billing = \"subscription\" but {} is missing, so claude would fall back to API credits",
            credentials_path().display()
        ),
        "log in with your Claude subscription first:\n  claude /login\nor opt into API billing in .nightshift/nightshift.toml:\n  [defaults]\n  billing = \"api\"",
    )
    .into())
}

/// A usage-limit stop found in a claude stream-json transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    pub message: String,
    /// Unix seconds when the limit resets, when the message says.
    pub reset_at: Option<i64>,
}

/// An error `result` event whose text mentions a usage limit, rate limit, or "limit reached".
pub fn usage_limit(stdout: &str, now: i64) -> Option<Limit> {
    let mut found = None;
    for line in stdout.lines() {
        let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if ev.get("type").and_then(Value::as_str) != Some("result") {
            continue;
        }
        if ev.get("is_error").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let text = ev
            .get("result")
            .and_then(Value::as_str)
            .or_else(|| ev.get("error").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let lower = text.to_lowercase();
        if ["usage limit", "rate limit", "limit reached"]
            .iter()
            .any(|k| lower.contains(k))
        {
            found = Some(Limit {
                reset_at: reset_time(&text, now),
                message: text,
            });
        }
    }
    found
}

/// `...|1760000000` (unix seconds), or `resets 3am` / `resets at 10:30pm` in local time.
fn reset_time(text: &str, now: i64) -> Option<i64> {
    let epoch = regex::Regex::new(r"\|\s*(\d{10})\b").unwrap();
    if let Some(c) = epoch.captures(text) {
        return c[1].parse().ok();
    }
    let re =
        regex::Regex::new(r"(?i)resets?\s+(?:at\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?").unwrap();
    let c = re.captures(text)?;
    let mut h: u32 = c[1].parse().ok()?;
    let m: u32 = c.get(2).map_or(Some(0), |m| m.as_str().parse().ok())?;
    match c.get(3).map(|s| s.as_str().to_lowercase()) {
        Some(ap) if ap == "pm" && h < 12 => h += 12,
        Some(ap) if ap == "am" && h == 12 => h = 0,
        _ => {}
    }
    (h < 24 && m < 60).then(|| crate::clock::next_local(now, h, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_limits_and_reset_times() {
        let ev = |s: &str| {
            format!(
                "{{\"type\":\"result\",\"is_error\":true,\"result\":{}}}\n",
                serde_json::to_string(s).unwrap()
            )
        };
        let l = usage_limit(&ev("Claude AI usage limit reached|1760000000"), 0).unwrap();
        assert_eq!(l.reset_at, Some(1_760_000_000));
        let l = usage_limit(&ev("5-hour limit reached ∙ resets 3am"), 1_000).unwrap();
        assert!(l.reset_at.unwrap() > 1_000);
        let l = usage_limit(&ev("API Error: rate limit"), 0).unwrap();
        assert_eq!(l.reset_at, None);
        assert!(usage_limit(&ev("tests failed"), 0).is_none());
        assert!(usage_limit(
            "{\"type\":\"result\",\"is_error\":false,\"result\":\"usage limit docs\"}",
            0
        )
        .is_none());
    }

    #[test]
    fn strips_bare_and_spots_claude() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            strip_bare(v(&["claude", "--bare", "-p"])),
            v(&["claude", "-p"])
        );
        assert!(is_claude(&v(&["/usr/bin/claude"])));
        assert!(!is_claude(&v(&["codex"])));
    }

    #[test]
    fn a_bg_wait_ceiling_already_on_the_command_is_kept() {
        let mut cmd = Command::new("claude");
        cmd.env(BG_WAIT_ENV, "5");
        wait_for_bg_tasks(&mut cmd);
        let got: Vec<_> = cmd.get_envs().filter(|(k, _)| *k == BG_WAIT_ENV).collect();
        assert_eq!(got, [(BG_WAIT_ENV.as_ref(), Some("5".as_ref()))]);
    }
}
