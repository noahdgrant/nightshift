//! `ns guard-hooks`: the `PreToolUse` hook `ns run` gives every claude phase. It reads the
//! tool call claude is about to make on stdin and denies one that names a hook bypass
//! literally, naming the rule (`skills/ns-contract`, Git hooks). Every other call passes
//! untouched.
//!
//! The guard is best-effort: it fails a few literal forms early, with a helpful message. It
//! follows no variable, alias or script. `ns run`'s tamper check (`run/tamper.rs`) puts back a
//! changed protected path whatever wrote it, and the gates and CI decide what reaches the
//! default branch (docs/decisions, "Hooks are a convenience").

use std::io::Read;

use serde_json::{json, Value};

mod files;
mod git;
#[cfg(test)]
mod tests;
mod words;

use files::{is_protected, writes_protected};
use words::{segments, Segment};

const RULE: &str = "The factory contract's Git hooks rule (skills/ns-contract/SKILL.md, \"Git hooks\") says a phase never skips the repo's hooks. Fix what the hook reports and commit or push again; when the cause lies outside the unit, stop with status: blocked.";

/// What a protected path is called in a denial.
const PROTECTED: &str = ".githooks/, .git/hooks, .git/config, config.worktree, .claude/settings*.json, .ns/<unit>/skills, the phase settings, or a skills link into a snapshot";

const BASH_TOOL: &str = "Bash";

/// Tools that write a file at `file_path` or `notebook_path`.
const FILE_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Read the hook input from stdin and print a deny decision for a bypass. Input that isn't a
/// tool call passes: the guard never blocks a call it can't read.
pub fn run() -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let Ok(call) = serde_json::from_str::<Value>(&input) else {
        return Ok(());
    };
    if let Some(why) = verdict(&call) {
        let out = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": format!("ns: blocked: {why}. {RULE}"),
            },
        });
        println!("{out}");
    }
    Ok(())
}

/// Why `call` would skip the git hooks, when it says so literally.
fn verdict(call: &Value) -> Option<String> {
    let input = &call["tool_input"];
    match call["tool_name"].as_str()? {
        BASH_TOOL => segments(input["command"].as_str()?)
            .iter()
            .find_map(check_segment),
        tool if FILE_TOOLS.contains(&tool) => ["file_path", "notebook_path"]
            .iter()
            .filter_map(|k| input[k].as_str())
            .find(|p| is_protected(p))
            .map(|p| format!("edit of {p} in {PROTECTED}")),
        _ => None,
    }
}

/// Words that come before a simple command's program.
const PREFIXES: &[&str] = &[
    "{", "!", "time", "sudo", "env", "command", "exec", "nohup", "nice",
];

/// Why one simple command (no operators) would skip the git hooks.
fn check_segment(seg: &Segment) -> Option<String> {
    if let Some(t) = seg.redirects.iter().find(|t| is_protected(&t.text)) {
        return Some(format!("write to {} in {PROTECTED}", t.text));
    }
    if seg
        .words
        .iter()
        .any(|w| !w.quoted && w.text.starts_with("GIT_DIR="))
    {
        return Some("GIT_DIR= makes git use other hooks and config".into());
    }
    let mut w = seg.words.as_slice();
    while let Some(first) = w.first() {
        let text = first.text.as_str();
        if is_assignment(text) || PREFIXES.contains(&text) || text.starts_with('-') {
            w = &w[1..];
        } else if text == "timeout" {
            let options = w[1..].iter().take_while(|a| a.text.starts_with('-'));
            w = w.get(2 + options.count()..).unwrap_or_default();
        } else {
            break;
        }
    }
    let (prog, args) = w.split_first()?;
    match basename(&prog.text) {
        "git" => git::check(args),
        prog => writes_protected(prog, args),
    }
}

/// `word` has the shape `NAME=value`.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn basename(prog: &str) -> &str {
    prog.rsplit('/').next().unwrap_or(prog)
}
