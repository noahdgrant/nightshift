//! `ns guard-hooks`: the `PreToolUse` hook `ns run` gives every claude phase. It reads the
//! tool call claude is about to make on stdin and denies one that would skip the repo's git
//! hooks, naming the rule (`skills/ns-contract`, Git hooks). Every other call passes untouched.

use std::collections::HashMap;
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
use lexer::{tokens, Sep, Tok, Word};
use wrappers::*;

const RULE: &str = "The factory contract's Git hooks rule (skills/ns-contract/SKILL.md, \"Git hooks\") says a phase never skips the repo's hooks. Fix what the hook reports and commit or push again; when the cause lies outside the unit, stop with status: blocked.";

/// What a denial says when only a word the guard couldn't read caused it.
const UNREAD_RULE: &str = "The guard could not read a word a shell expansion builds, so it could not rule out a protected path or a flag that skips the hooks. Write the word out literally, or write the file with the Write tool, and carry on: this denial is no reason to stop the phase.";

/// The phase's variables the guard reads from its own environment.
const PHASE_VARS: [&str; 2] = ["NS_UNIT", "NS_WORKTREE"];

/// What stands in for each expansion when the guard asks whether a denial came only from the
/// words it couldn't read: a name no protected path or flag has.
const FILLER: &str = "unread";

/// Variables the guard knows the value of, by name.
type Vars = HashMap<String, String>;

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
    if let Some(denial) = judge(&call, &phase_vars(|name| std::env::var(name).ok())) {
        let out = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": denial.reason(),
            },
        });
        println!("{out}");
    }
    Ok(())
}

/// A call the guard denies.
struct Denial {
    why: String,
    /// The call passes once each word the guard couldn't read is written out as a plain name:
    /// rewriting the word, not stopping, is the fix.
    unread: bool,
}

impl Denial {
    fn reason(&self) -> String {
        let why = &self.why;
        if self.unread {
            format!("ns: denied: {why}. {UNREAD_RULE}")
        } else {
            format!("ns: blocked: {why}. {RULE}")
        }
    }
}

/// The `PHASE_VARS` that `env` holds a literal value for.
fn phase_vars(env: impl Fn(&str) -> Option<String>) -> Vars {
    PHASE_VARS
        .iter()
        .filter_map(|name| Some((name.to_string(), env(name)?)))
        .filter(|(_, value)| is_literal_value(value))
        .collect()
}

/// Whether `call` would skip the git hooks, with `vars` known to hold their values.
fn judge(call: &Value, vars: &Vars) -> Option<Denial> {
    let why = verdict(call, vars, false)?;
    let unread = verdict(call, vars, true).is_none();
    Some(Denial { why, unread })
}

/// Why `call` would skip the git hooks, with `vars` known to hold their values and, when
/// `fill`, every other expansion read as `FILLER`.
fn verdict(call: &Value, vars: &Vars, fill: bool) -> Option<String> {
    let input = &call["tool_input"];
    match call["tool_name"].as_str()? {
        BASH_TOOL => {
            let mut scan = Scan {
                cwd: call["cwd"].as_str().map(Cwd::of).unwrap_or_default(),
                vars: vars.clone(),
                fill,
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
    /// Variables whose value the top-level command is known to hold at this point.
    vars: Vars,
    /// Read every expansion not in `vars` as `FILLER`.
    fill: bool,
}

impl Scan {
    /// `w` with what the scan knows written in: at the top level of the command (`top`) each
    /// variable in `vars`, and under `fill` every other expansion.
    fn read(&self, w: Word, top: bool) -> Word {
        let w = if top {
            w.resolved(|e| Some(self.vars.get(e.name.as_ref()?)?.as_str()))
        } else {
            w
        };
        if self.fill {
            w.resolved(|_| Some(FILLER))
        } else {
            w
        }
    }

    /// After the top-level segment `words`, ended by `sep`, learn the variables it assigns a
    /// literal value for certain and forget those it may change. `flow` says whether the
    /// segment ran for certain, in this shell.
    fn settle(&mut self, words: &[Word], sep: Sep, flow: &mut Flow) {
        let command = words.iter().find(|w| env_assignment(&w.text).is_none());
        let command = command.map(|w| basename(&w.text));
        if command.is_some_and(|c| COMPOUND_WORDS.contains(&c)) {
            flow.compound = true;
        }
        let may_set_any = command.is_some_and(|c| {
            matches!(c, "eval" | "source" | "." | "declare" | "typeset" | "local")
                || (SETTERS.contains(&c) && words.iter().any(|w| !w.expansions.is_empty()))
        });
        if may_set_any {
            self.vars.clear();
        }
        if words.iter().any(|w| may_set(&w.text, "IFS")) {
            self.vars.clear();
            flow.compound = true;
        }
        if flow.runs && !flow.compound && matches!(sep, Sep::Seq | Sep::And) {
            self.vars.extend(literal_assignments(words));
        }
        flow.runs = sep == Sep::Seq;
        flow.compound |= sep == Sep::Group;
    }

    /// Forget each variable the word `w` may set: it names it, or assigns it.
    fn forget(&mut self, w: &Word) {
        self.vars.retain(|name, _| !may_set(&w.text, name));
    }

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
    let top = scan.depth == 1 && !lexed.assigns;
    let why = check_tokens(lexed.main, scan, top).or_else(|| {
        lexed
            .substitutions
            .into_iter()
            .find_map(|toks| check_tokens(toks, scan, false))
    });
    scan.depth -= 1;
    why
}

/// How far the top-level command so far is known to run, for `Scan::settle`.
struct Flow {
    /// The next segment runs for certain, in this shell, once the ones before it have.
    runs: bool,
    /// A compound command (a brace, a subshell, a function, a reserved word) began: what runs
    /// after it is no longer followed.
    compound: bool,
}

/// Check the tokens of one command. `top` is the command the phase ran, not one nested in an
/// `eval`, a `sh -c` or a substitution, and it has no arithmetic or `${name=value}` that could
/// set a variable unseen: only there are its variables followed.
fn check_tokens(toks: Vec<Tok>, scan: &mut Scan, top: bool) -> Option<String> {
    let is_word = |t: &Tok| matches!(t, Tok::Word(_));
    let mut words: Vec<Word> = Vec::new();
    let mut stdin_is_text = false;
    let mut flow = Flow {
        runs: true,
        compound: false,
    };
    let mut toks = toks.into_iter().peekable();
    while let Some(tok) = toks.next() {
        let tok = match tok {
            Tok::Word(w) => Tok::Word(scan.read(w, top)),
            tok => tok,
        };
        match tok {
            Tok::Word(w) => {
                if top {
                    scan.forget(&w);
                }
                words.push(w);
            }
            Tok::Herestring => {
                toks.next_if(is_word);
                stdin_is_text = true;
            }
            Tok::Heredoc => stdin_is_text = true,
            Tok::RedirectOut => {
                if let Some(Tok::Word(f)) = toks.next_if(is_word) {
                    let f = scan.read(f, top);
                    if scan.touches_hooks(&path(&f)) {
                        return Some(format!("write to {} in {PROTECTED}", f.text));
                    }
                }
            }
            Tok::RedirectIn => {
                toks.next_if(is_word);
            }
            Tok::CommandEnd(_) | Tok::SubshellOpen | Tok::SubshellClose => {
                if let Some(why) = check_fed_segment(&words, stdin_is_text, scan) {
                    return Some(why);
                }
                if top {
                    let sep = match tok {
                        Tok::CommandEnd(sep) => sep,
                        _ => Sep::Group,
                    };
                    scan.settle(&words, sep, &mut flow);
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

/// Command words that begin or belong to a compound command.
const COMPOUND_WORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "for", "select", "case",
    "esac", "function", "!", "coproc",
];

/// Builtins that can set a variable named by one of their arguments.
const SETTERS: &[&str] = &[
    "read",
    "mapfile",
    "readarray",
    "printf",
    "getopts",
    "export",
    "readonly",
    "unset",
    "let",
    "builtin",
    "command",
];

/// The word `text` names the variable `name`, assigns it, or ends with it as an option's
/// value (`-vU`), so the command may set it.
fn may_set(text: &str, name: &str) -> bool {
    text.strip_prefix(name)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['=', '+', '[']))
        || (text.starts_with('-') && text.ends_with(name))
}

/// A value a variable can be resolved to: the same text wherever it is expanded, quoted or
/// not, so no blank, glob, quote or other character the shell reads.
fn is_literal_value(value: &str) -> bool {
    value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_./:@%+,=-".contains(c))
}

/// The variables a segment that only assigns (`U=lit`, `export U=lit`) sets to a literal
/// value. Any other segment sets none: a prefix assignment (`U=lit cmd`) lasts only for its
/// command.
fn literal_assignments(words: &[Word]) -> Vec<(String, String)> {
    let assignments = match words {
        [first, rest @ ..] if first.text == "export" && first.expansions.is_empty() => rest,
        _ => words,
    };
    let literal = |w: &Word| {
        let (name, value) = env_assignment(&w.text)?;
        (w.expansions.is_empty() && is_literal_value(value))
            .then(|| (name.to_string(), value.to_string()))
    };
    assignments
        .iter()
        .map(literal)
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
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
