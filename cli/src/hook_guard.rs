//! `ns guard-hooks`: the `PreToolUse` hook `ns run` gives every claude phase. It reads the
//! tool call claude is about to make on stdin and denies one that would skip the repo's git
//! hooks, naming the rule (`skills/ns-contract`, Git hooks). Every other call passes untouched.

use std::io::Read;

use crate::pin;
use serde_json::{json, Value};

mod files;
mod git;
mod lexer;
#[cfg(test)]
mod tests;
mod wrappers;

use files::*;
use git::*;
use lexer::{tokens, Tok, Word};
use wrappers::*;

const RULE: &str = "The factory contract's Git hooks rule (skills/ns-contract/SKILL.md, \"Git hooks\") says a phase never skips the repo's hooks. Fix what the hook reports and commit or push again; when the cause lies outside the unit, stop with status: blocked.";

/// How deep `eval`, `sh -c` and aliases may nest before the guard gives up and denies.
const MAX_NESTING: usize = 8;

/// What a protected path is called in a denial.
const PROTECTED: &str = ".githooks/, .git/hooks, .git/config, .claude/settings*.json, .ns/<unit> or .ns/<unit>/skills, or a skills link into it";

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
    if let Some(why) = check(&call) {
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

/// Why `call` would skip the git hooks, or `None` when it wouldn't.
pub fn check(call: &Value) -> Option<String> {
    let input = &call["tool_input"];
    match call["tool_name"].as_str()? {
        BASH_TOOL => {
            let mut scan = Scan {
                cwd: call["cwd"].as_str().map(Cwd::of).unwrap_or_default(),
                ..Scan::default()
            };
            let command = input["command"].as_str()?;
            if command.contains(pin::SETTINGS_VAR) {
                return Some(format!(
                    "a command naming ${} in {PROTECTED}",
                    pin::SETTINGS_VAR
                ));
            }
            check_command(command, &mut scan)
        }
        tool if FILE_TOOLS.contains(&tool) => ["file_path", "notebook_path"]
            .iter()
            .filter_map(|k| input[k].as_str())
            .find(|p| is_protected(p))
            .map(|p| format!("edit of {p} in {PROTECTED}")),
        _ => None,
    }
}

fn unreadable(what: &str) -> String {
    format!("{what} takes a word built by a shell expansion that could be a flag that skips the hooks; write the word out literally")
}

#[derive(Default)]
struct Scan {
    /// An earlier `cd` or `pushd`, or this git's `-C`, went into a protected path: how many
    /// directories below it the working directory is.
    cwd: Cwd,
    depth: usize,
    /// `cwd` outside each subshell the scan is in.
    outer_cwds: Vec<Cwd>,
    /// The command's stdin is a heredoc or herestring, which a shell would run.
    stdin_is_text: bool,
}

impl Scan {
    /// `path` is protected, or is relative to a working directory that is.
    fn touches_hooks(&self, path: &str) -> bool {
        let path = option_value(path);
        if path.contains(UNREAD) {
            let erased = path.replace(UNREAD, "");
            return self.touches_hooks(&erased) || self.unread_under_ns(path);
        }
        is_protected(path)
            || (!path.starts_with(['-', '/'])
                && (self
                    .cwd
                    .hooks
                    .is_some_and(|depth| stays_inside(depth, path))
                    || self
                        .cwd
                        .ns
                        .is_some_and(|depth| reaches_unit_file(depth, path))))
    }

    /// An expansion of unknown depth sits below a `.ns` part of `path`, or in a relative path
    /// from a working directory below `.ns`, so it may reach a unit's directory or `skills`.
    fn unread_under_ns(&self, path: &str) -> bool {
        unread_below_protected(path, &[pin::NS_DIR], ['-', '/'], self.cwd)
    }
}

fn check_command(s: &str, scan: &mut Scan) -> Option<String> {
    if scan.depth >= MAX_NESTING {
        return Some(format!(
            "a command nested more than {MAX_NESTING} levels deep in eval, sh -c or aliases"
        ));
    }
    scan.depth += 1;
    let lexed = tokens(s);
    let why = check_tokens(lexed.main, scan).or_else(|| {
        lexed
            .substitutions
            .into_iter()
            .find_map(|toks| check_tokens(toks, scan))
    });
    scan.depth -= 1;
    why
}

fn check_tokens(toks: Vec<Tok>, scan: &mut Scan) -> Option<String> {
    let is_word = |t: &Tok| matches!(t, Tok::Word(_));
    let mut words: Vec<Word> = Vec::new();
    let mut stdin_is_text = false;
    let mut toks = toks.into_iter().peekable();
    while let Some(tok) = toks.next() {
        match tok {
            Tok::Word(w) => words.push(w),
            Tok::Herestring => {
                toks.next_if(is_word);
                stdin_is_text = true;
            }
            Tok::Heredoc => stdin_is_text = true,
            Tok::RedirectOut => {
                if let Some(Tok::Word(f)) = toks.next_if(is_word) {
                    if scan.touches_hooks(&path(&f)) {
                        return Some(format!("write to {} in {PROTECTED}", f.text));
                    }
                }
            }
            Tok::RedirectIn => {
                toks.next_if(is_word);
            }
            Tok::CommandEnd | Tok::SubshellOpen | Tok::SubshellClose => {
                if let Some(why) = check_fed_segment(&words, stdin_is_text, scan) {
                    return Some(why);
                }
                words.clear();
                stdin_is_text = false;
                match tok {
                    Tok::SubshellOpen => scan.outer_cwds.push(scan.cwd),
                    Tok::SubshellClose => {
                        if let Some(outer) = scan.outer_cwds.pop() {
                            scan.cwd = outer;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    check_fed_segment(&words, stdin_is_text, scan)
}

/// `check_segment` for a command whose stdin may be a heredoc or herestring. A command it
/// runs inherits that stdin, so the flag holds through `eval`, `sh -c` and wrappers.
fn check_fed_segment(words: &[Word], stdin_is_text: bool, scan: &mut Scan) -> Option<String> {
    let outer = scan.stdin_is_text;
    scan.stdin_is_text |= stdin_is_text;
    let why = check_segment(words, scan);
    scan.stdin_is_text = outer;
    why
}

/// Why one simple command (no operators) would skip the git hooks.
fn check_segment(words: &[Word], scan: &mut Scan) -> Option<String> {
    let mut w = words;
    loop {
        let first = w.first()?;
        if first.opaque {
            return opaque_command(w);
        }
        let text = first.text.as_str();
        if let Some((name, value)) = env_assignment(text) {
            if let Some(why) = env_bypass(name, value) {
                return Some(why);
            }
            w = &w[1..];
            continue;
        }
        if RESERVED_WORDS.contains(&text) {
            w = &w[1..];
            continue;
        }
        let prog = basename(text);
        if let Some(wrap) = WRAPPERS.iter().find(|x| x.name == prog) {
            if prog == "env" {
                if let Some(cmd) = env_split_string(&w[1..]) {
                    return check_command(cmd, scan);
                }
            }
            w = skip_options(&w[1..], wrap).get(wrap.positional..)?;
            continue;
        }
        return match prog {
            "cd" | "pushd" => change_directory(&w[1..], scan),
            "popd" => {
                scan.cwd = Cwd::default();
                None
            }
            "eval" => check_words(&w[1..], scan),
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" => match shell_dash_c_script(&w[1..]) {
                Some(script) => check_words(std::slice::from_ref(script), scan),
                None if scan.stdin_is_text => Some(format!(
                    "{prog} reading a heredoc or herestring as its script; use {prog} -c"
                )),
                None => None,
            },
            "git" => git(&w[1..], scan),
            prog => file_tool(prog, &w[1..], scan),
        };
    }
}

fn check_words(words: &[Word], scan: &mut Scan) -> Option<String> {
    if words.iter().any(|w| w.opaque) {
        return Some(unreadable("a shell run through eval or sh -c"));
    }
    let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    check_command(&text.join(" "), scan)
}

/// A command named by an expansion is unreadable; deny it when git or the hooks are near.
fn opaque_command(words: &[Word]) -> Option<String> {
    let names_git = words.first().is_some_and(|w| w.text.starts_with("git"));
    let near_git = names_git
        || words.iter().any(|w| {
            w.text.split(['{', '}', ',']).any(|p| basename(p) == "git")
                || is_guarded_subcommand(&w.text)
                || is_protected(&w.text)
        });
    near_git.then(|| "a command named by a shell expansion next to git or .githooks".to_string())
}

fn change_directory(args: &[Word], scan: &mut Scan) -> Option<String> {
    let dir = args
        .iter()
        .find(|a| a.text == "-" || !a.text.starts_with('-'));
    if let Some(d) = dir {
        if cd_loses_protected_cwd(d, scan.cwd) {
            return Some(format!(
                "cd {} into a directory of unknown depth in a protected path",
                d.text
            ));
        }
    }
    scan.cwd = dir.map_or_else(Cwd::default, |d| cd_to(&dir_path(d), scan.cwd));
    None
}

/// The working directory after `cd dir` (or `git -C dir`) from `from`.
fn cd_to(dir: &str, from: Cwd) -> Cwd {
    match dir {
        d if is_protected_directory(d) => Cwd::of(d),
        d if !d.starts_with(['/', '~']) => Cwd::relative(from, d),
        d => Cwd::of(d),
    }
}

fn env_assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let ok = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    ok.then_some((name, value))
}

/// Variables that make git skip a hook, move `core.hooksPath` or read another config or repo. A command that sets one is
/// denied whether or not it runs git: an `export` lasts past the call.
fn env_bypass(name: &str, value: &str) -> Option<String> {
    let hit = match name {
        "HUSKY" => value == "0",
        "SKIP"
        | "GIT_CONFIG_COUNT"
        | "GIT_CONFIG_PARAMETERS"
        | "GIT_CONFIG_GLOBAL"
        | "GIT_CONFIG_SYSTEM"
        | "GIT_DIR" => true,
        n => n.starts_with("GIT_CONFIG_KEY_") || n.starts_with("GIT_CONFIG_VALUE_"),
    };
    hit.then(|| {
        format!("{name}= makes git hooks skip, or moves core.hooksPath or the config git reads")
    })
}

fn basename(prog: &str) -> &str {
    prog.rsplit('/').next().unwrap_or(prog)
}
