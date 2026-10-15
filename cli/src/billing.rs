//! Subscription billing guard for claude and codex runs, and usage-limit detection for claude.

use std::path::Path;

use serde_json::Value;

/// Variables that make claude bill an API key or a cloud provider instead of the subscription.
pub const API_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

/// Variables that make codex bill an OpenAI API key instead of the ChatGPT subscription.
pub const CODEX_API_ENV: &[&str] = &["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"];

fn runs(argv: &[String], bin: &str) -> bool {
    argv.first()
        .and_then(|a| Path::new(a).file_name())
        .is_some_and(|n| n == bin)
}

pub fn is_claude(argv: &[String]) -> bool {
    runs(argv, "claude")
}

pub fn is_codex(argv: &[String]) -> bool {
    runs(argv, "codex")
}

/// The variables that would make `argv`'s harness bill an API instead of the subscription.
pub fn api_env(argv: &[String]) -> &'static [&'static str] {
    if is_claude(argv) {
        API_ENV
    } else if is_codex(argv) {
        CODEX_API_ENV
    } else {
        &[]
    }
}

/// A usage-limit stop found in a claude stream-json transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    pub message: String,
    /// Unix seconds when the limit resets, when the message says.
    pub reset_at: Option<i64>,
}

/// A usage-limit stop: an error `result` event that claude marks structurally
/// (`api_error` names a limit, or `api_error_status` is 429), or whose text names one.
/// The reset time comes from any event's `rate_limit_info.resetsAt`, else from the text.
pub fn usage_limit(stdout: &str, now: i64) -> Option<Limit> {
    let mut found = None;
    let mut resets_at: Option<i64> = None;
    for line in stdout.lines() {
        let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let Some(t) = find_resets_at(&ev) {
            resets_at = Some(t);
        }
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
        let api_error = ev.get("api_error").and_then(Value::as_str).unwrap_or("");
        let structured = api_error.contains("limit")
            || ev.get("api_error_status").and_then(Value::as_i64) == Some(429);
        let lower = text.to_lowercase();
        let worded = [
            "usage limit",
            "rate limit",
            "limit reached",
            "session limit",
            "hit your limit",
            "5-hour limit",
            "weekly limit",
        ]
        .iter()
        .any(|k| lower.contains(k));
        if structured || worded {
            found = Some(Limit {
                reset_at: resets_at.or_else(|| reset_time(&text, now)),
                message: text,
            });
        }
    }
    found
}

/// The first `resetsAt` (unix seconds) anywhere in an event.
fn find_resets_at(v: &Value) -> Option<i64> {
    match v {
        Value::Object(m) => {
            if let Some(t) = m.get("resetsAt").and_then(Value::as_i64) {
                return Some(t);
            }
            m.values().find_map(find_resets_at)
        }
        Value::Array(a) => a.iter().find_map(find_resets_at),
        _ => None,
    }
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
    fn detects_a_session_limit_from_structured_fields() {
        let out = concat!(
            "{\"type\":\"assistant\",\"error\":\"rate_limit\",\"api_error_params\":{\"rate_limit_info\":{\"status\":\"rejected\",\"resetsAt\":1791529200,\"rateLimitType\":\"five_hour\"}}}\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":true,\"api_error_status\":429,\"api_error\":\"usage_limit_reached\",\"result\":\"You've hit your session limit \\u00b7 resets 3am (America/Toronto)\"}\n",
        );
        let l = usage_limit(out, 1_791_500_000).unwrap();
        assert_eq!(l.reset_at, Some(1_791_529_200));
        assert!(l.message.contains("session limit"));
        let bare =
            "{\"type\":\"result\",\"is_error\":true,\"api_error_status\":429,\"result\":\"\"}";
        assert!(usage_limit(bare, 0).is_some());
        let other = "{\"type\":\"result\",\"is_error\":true,\"api_error_status\":500,\"result\":\"overloaded\"}";
        assert!(usage_limit(other, 0).is_none());
    }

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
    fn spots_claude_and_codex() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_claude(&v(&["/usr/bin/claude"])));
        assert!(!is_claude(&v(&["codex"])));
        assert!(is_codex(&v(&["/usr/bin/codex", "exec"])));
        assert!(!is_codex(&v(&["claude"])));
        assert!(!is_codex(&v(&["codex-wrapper"])));
        assert!(!is_codex(&[]));
    }

    #[test]
    fn each_harness_scrubs_only_its_own_api_variables() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(api_env(&v(&["claude", "-p"])), API_ENV);
        assert_eq!(api_env(&v(&["codex", "exec"])), CODEX_API_ENV);
        assert!(api_env(&v(&["sh", "-c", "true"])).is_empty());
    }
}
