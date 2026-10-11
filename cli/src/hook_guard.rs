//! `ns guard-hooks`: the `PreToolUse` hook `ns run` gives every claude phase. It reads the
//! tool call claude is about to make on stdin and denies one that would skip the repo's git
//! hooks, naming the rule (`skills/ns-contract`, Git hooks). Every other call passes untouched.

use std::io::Read;
use std::path::Path;

use serde_json::{json, Value};

use crate::shell::shell_quote;

mod lexer;
#[cfg(test)]
mod tests;

use lexer::{tokens, Tok, Word};

const RULE: &str = "The factory contract's Git hooks rule (skills/ns-contract/SKILL.md, \"Git hooks\") says a phase never skips the repo's hooks. Fix what the hook reports and commit or push again; when the cause lies outside the unit, stop with status: blocked.";

/// How deep `eval`, `sh -c` and aliases may nest before the guard gives up and denies.
const MAX_NESTING: usize = 8;

/// Git subcommands that take `--no-verify` and need no other reading.
const NO_VERIFY_SUBCOMMANDS: [&str; 6] = ["push", "merge", "pull", "am", "cherry-pick", "revert"];

/// Subcommands the guard reads for a bypass.
fn is_guarded_subcommand(name: &str) -> bool {
    NO_VERIFY_SUBCOMMANDS.contains(&name) || matches!(name, "commit" | "rebase" | "config")
}

/// What a protected path is called in a denial.
const PROTECTED: &str = ".githooks/, .git/hooks or .git/config";

const BASH_TOOL: &str = "Bash";

/// Tools that write a file at `file_path` or `notebook_path`.
const FILE_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The `--settings` value for a claude phase: one `PreToolUse` hook that runs `ns guard-hooks`.
/// Claude Code lets a call through when a hook exits with anything but 2, so a missing or
/// crashing `ns` must exit 2 to block it.
pub fn settings(ns: &Path) -> String {
    let command = format!(
        "{} guard-hooks || exit 2",
        shell_quote(&ns.to_string_lossy())
    );
    let matcher = [BASH_TOOL]
        .into_iter()
        .chain(FILE_TOOLS)
        .collect::<Vec<_>>()
        .join("|");
    json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": matcher,
                "hooks": [{"type": "command", "command": command}],
            }],
        },
    })
    .to_string()
}

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
        BASH_TOOL => check_command(input["command"].as_str()?, &mut Scan::default()),
        tool if FILE_TOOLS.contains(&tool) => ["file_path", "notebook_path"]
            .iter()
            .filter_map(|k| input[k].as_str())
            .find(|p| is_protected(p))
            .map(|p| format!("edit of {p} in {PROTECTED}")),
        _ => None,
    }
}

/// `path` is in `.githooks/`, `.git/hooks` or `.git/config`.
fn is_protected(path: &str) -> bool {
    let parts = components(path);
    parts.contains(&".githooks")
        || parts
            .windows(2)
            .any(|p| p[0] == ".git" && matches!(p[1], "hooks" | "config"))
}

/// The parts of `path` between slashes, without the empty and `.` ones.
fn components(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|p| !matches!(*p, "" | "."))
        .collect()
}

/// The value of `--option=value`, or `arg` itself.
fn option_value(arg: &str) -> &str {
    match arg.strip_prefix("--").and_then(|_| arg.split_once('=')) {
        Some((_, value)) => value,
        None => arg,
    }
}

fn unreadable(what: &str) -> String {
    format!("{what} takes a word built by a shell expansion that could be a flag that skips the hooks; write the word out literally")
}

// --------------------------------------------------------------- analysis

#[derive(Default)]
struct Scan {
    /// An earlier `cd` or `pushd`, or this git's `-C`, went into a protected path.
    cwd_is_hooks: bool,
    depth: usize,
    /// `cwd_is_hooks` outside each subshell the scan is in.
    outer_cwds: Vec<bool>,
}

impl Scan {
    /// `path` is protected, or is relative to a working directory that is.
    fn touches_hooks(&self, path: &str) -> bool {
        let path = option_value(path);
        is_protected(path)
            || (self.cwd_is_hooks && !path.starts_with(['-', '/']) && !path.starts_with(".."))
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
    let mut toks = toks.into_iter().peekable();
    while let Some(tok) = toks.next() {
        match tok {
            Tok::Word(w) => words.push(w),
            Tok::RedirectOut => {
                if let Some(Tok::Word(f)) = toks.next_if(is_word) {
                    if scan.touches_hooks(&f.text) {
                        return Some(format!("write to {} in {PROTECTED}", f.text));
                    }
                }
            }
            Tok::RedirectIn => {
                toks.next_if(is_word);
            }
            Tok::CommandEnd | Tok::SubshellOpen | Tok::SubshellClose => {
                if let Some(why) = check_segment(&words, scan) {
                    return Some(why);
                }
                words.clear();
                match tok {
                    Tok::SubshellOpen => scan.outer_cwds.push(scan.cwd_is_hooks),
                    Tok::SubshellClose => {
                        if let Some(outer) = scan.outer_cwds.pop() {
                            scan.cwd_is_hooks = outer;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    check_segment(&words, scan)
}

const RESERVED_WORDS: [&str; 9] = [
    "if", "then", "else", "elif", "do", "while", "until", "!", "coproc",
];

/// A command that runs the command after its own options. It lists the options that take a
/// value, so the value is not mistaken for the program.
struct Wrapper {
    name: &'static str,
    /// Letters of the short options that take a value.
    short_values: &'static str,
    long_values: &'static [&'static str],
    /// Positional arguments before the command (the duration of `timeout`).
    positional: usize,
}

const fn wrapper(name: &'static str, short_values: &'static str) -> Wrapper {
    Wrapper {
        name,
        short_values,
        long_values: &[],
        positional: 0,
    }
}

const WRAPPERS: [Wrapper; 19] = [
    wrapper("export", ""),
    wrapper("declare", ""),
    wrapper("typeset", ""),
    wrapper("readonly", ""),
    wrapper("local", ""),
    wrapper("command", ""),
    wrapper("nohup", ""),
    wrapper("builtin", ""),
    wrapper("setsid", ""),
    wrapper("doas", "uC"),
    wrapper("exec", "a"),
    Wrapper {
        long_values: &["--unset", "--chdir"],
        ..wrapper("env", "uC")
    },
    Wrapper {
        long_values: &[
            "--user",
            "--group",
            "--host",
            "--prompt",
            "--close-from",
            "--chdir",
            "--role",
            "--type",
            "--other-user",
            "--command-timeout",
            "--chroot",
        ],
        ..wrapper("sudo", "ughpCrtTUDR")
    },
    Wrapper {
        long_values: &["--adjustment"],
        ..wrapper("nice", "n")
    },
    Wrapper {
        long_values: &["--format", "--output"],
        ..wrapper("time", "fo")
    },
    Wrapper {
        long_values: &["--input", "--output", "--error"],
        ..wrapper("stdbuf", "ioe")
    },
    Wrapper {
        long_values: &["--class", "--classdata", "--pid", "--pgid", "--uid"],
        ..wrapper("ionice", "cnpPu")
    },
    Wrapper {
        long_values: &[
            "--max-args",
            "--max-lines",
            "--max-procs",
            "--max-chars",
            "--delimiter",
            "--arg-file",
            "--process-slot-var",
        ],
        ..wrapper("xargs", "nLPIsdEa")
    },
    Wrapper {
        long_values: &["--kill-after", "--signal"],
        positional: 1,
        ..wrapper("timeout", "ks")
    },
];

/// The words after a command's leading options, skipping the value of each option that takes one.
fn skip_options<'a>(mut words: &'a [Word], wrap: &Wrapper) -> &'a [Word] {
    while let Some(first) = words.first() {
        let arg = first.text.as_str();
        let width = if arg == "--" {
            return &words[1..];
        } else if arg.starts_with("--") {
            if !arg.contains('=') && wrap.long_values.contains(&arg) {
                2
            } else {
                1
            }
        } else if let Some(cluster) = arg.strip_prefix('-').filter(|c| !c.is_empty()) {
            let value_flag = cluster
                .char_indices()
                .find(|(_, c)| wrap.short_values.contains(*c));
            match value_flag {
                Some((i, c)) if i + c.len_utf8() == cluster.len() => 2,
                _ => 1,
            }
        } else {
            return words;
        };
        words = words.get(width..).unwrap_or(&[]);
    }
    words
}

/// The command string of `env -S`, which env splits and runs. Walks env's options with their
/// values, so `-iS`, `-u FOO -S` and `-C /x -S` all reach it.
fn env_split_string(options: &[Word]) -> Option<&str> {
    let mut args = options.iter().map(|w| w.text.as_str());
    while let Some(arg) = args.next() {
        match arg {
            "--" => return None,
            "--split-string" => return args.next(),
            "--unset" | "--chdir" => {
                args.next();
            }
            _ if arg.starts_with("--") => {
                if let Some(v) = arg.strip_prefix("--split-string=") {
                    return Some(v);
                }
            }
            _ if arg.starts_with('-') && arg.len() > 1 => {
                for (i, c) in arg.char_indices().skip(1) {
                    let attached = &arg[i + c.len_utf8()..];
                    match c {
                        'S' if attached.is_empty() => return args.next(),
                        'S' => return Some(attached),
                        'u' | 'C' => {
                            if attached.is_empty() {
                                args.next();
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            }
            _ => return None,
        }
    }
    None
}

/// The script word of `sh -c SCRIPT`: the word after the first option cluster with a `c`.
fn shell_dash_c_script(args: &[Word]) -> Option<&Word> {
    let at = args.iter().position(|a| {
        a.text.starts_with('-') && !a.text.starts_with("--") && a.text.contains('c')
    })?;
    args.get(at + 1)
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
            "cd" | "pushd" => {
                change_directory(&w[1..], scan);
                None
            }
            "popd" => {
                scan.cwd_is_hooks = false;
                None
            }
            "eval" => check_words(&w[1..], scan),
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" => shell_dash_c_script(&w[1..])
                .and_then(|script| check_words(std::slice::from_ref(script), scan)),
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
    let near_git = words.iter().any(|w| {
        w.text.split(['{', '}', ',']).any(|p| basename(p) == "git")
            || is_guarded_subcommand(&w.text)
            || is_protected(&w.text)
    });
    near_git.then(|| "a command named by a shell expansion next to git or .githooks".to_string())
}

fn change_directory(args: &[Word], scan: &mut Scan) {
    let dir = args
        .iter()
        .map(|a| a.text.as_str())
        .find(|a| !a.starts_with('-'));
    match dir {
        Some(d) if is_protected(d) || components(d).last() == Some(&".git") => {
            scan.cwd_is_hooks = true;
        }
        Some(d) if !d.starts_with("..") && !d.starts_with(['/', '~']) => {}
        _ => scan.cwd_is_hooks = false,
    }
}

fn env_assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let ok = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    ok.then_some((name, value))
}

/// Variables that make git skip a hook or move `core.hooksPath`. A command that sets one is
/// denied whether or not it runs git: an `export` lasts past the call.
fn env_bypass(name: &str, value: &str) -> Option<String> {
    let hit = match name {
        "HUSKY" => value == "0",
        "SKIP" | "GIT_CONFIG_COUNT" | "GIT_CONFIG_PARAMETERS" => true,
        n => n.starts_with("GIT_CONFIG_KEY_") || n.starts_with("GIT_CONFIG_VALUE_"),
    };
    hit.then(|| format!("{name}= makes git hooks skip or moves core.hooksPath"))
}

fn basename(prog: &str) -> &str {
    prog.rsplit('/').next().unwrap_or(prog)
}

fn is_hooks_path(key: &str) -> bool {
    key.eq_ignore_ascii_case("core.hooksPath")
}

/// `--no-verify` or an abbreviation git accepts for it.
fn no_verify(arg: &str) -> bool {
    let name = arg.split('=').next().unwrap_or(arg);
    name.len() >= "--no-v".len() && "--no-verify".starts_with(name)
}

/// Options before the git subcommand that take a value in the next word.
const GLOBAL_VALUE_OPTIONS: [&str; 8] = [
    "-c",
    "-C",
    "--config-env",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--super-prefix",
    "--attr-source",
];

/// What `git` options before the subcommand set up.
struct GlobalOptions {
    /// Index of the subcommand in the arguments.
    subcommand: usize,
    /// `alias.NAME=VALUE` pairs from `-c`.
    aliases: Vec<(String, String)>,
}

/// `--config-env=KV`, `-cKV` or `-Cdir` as the option and its value.
fn attached_option(arg: &str) -> Option<(&str, &str)> {
    if let Some(kv) = arg.strip_prefix("--config-env=") {
        return Some(("--config-env", kv));
    }
    if arg.starts_with("--") {
        return None;
    }
    let value = |prefix| arg.strip_prefix(prefix).filter(|v| !v.is_empty());
    value("-c")
        .map(|v| ("-c", v))
        .or_else(|| value("-C").map(|v| ("-C", v)))
}

/// Read the options before the subcommand. `Err` is a denial; `Ok(None)` means no subcommand.
fn git_global_options(args: &[Word], scan: &mut Scan) -> Result<Option<GlobalOptions>, String> {
    let mut aliases = Vec::new();
    let mut i = 0;
    while let Some(a) = args.get(i) {
        if a.opaque {
            return Err(unreadable("a git option"));
        }
        let arg = a.text.as_str();
        let step = if let Some((option, value)) = attached_option(arg) {
            global_option(option, value, scan, &mut aliases)?;
            1
        } else if GLOBAL_VALUE_OPTIONS.contains(&arg) {
            let Some(value) = args.get(i + 1) else {
                return Ok(None);
            };
            global_option(arg, &value.text, scan, &mut aliases)?;
            2
        } else if arg.starts_with('-') {
            1
        } else {
            return Ok(Some(GlobalOptions {
                subcommand: i,
                aliases,
            }));
        };
        i += step;
    }
    Ok(None)
}

/// Apply one global option with its value; `Err` is a denial.
fn global_option(
    option: &str,
    value: &str,
    scan: &mut Scan,
    aliases: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let denial = match option {
        "-c" => config_pair(value, scan, aliases),
        "-C" => {
            scan.cwd_is_hooks |= is_protected(value);
            None
        }
        "--config-env" => config_env(value),
        _ => None,
    };
    denial.map_or(Ok(()), Err)
}

fn git(args: &[Word], scan: &mut Scan) -> Option<String> {
    let outer_cwd = scan.cwd_is_hooks;
    let why = git_command(args, scan);
    scan.cwd_is_hooks = outer_cwd;
    why
}

fn git_command(args: &[Word], scan: &mut Scan) -> Option<String> {
    let options = match git_global_options(args, scan) {
        Ok(Some(options)) => options,
        Ok(None) => return None,
        Err(why) => return Some(why),
    };
    let sub = &args[options.subcommand];
    if sub.opaque {
        return Some(unreadable("the git subcommand"));
    }
    let rest = &args[options.subcommand + 1..];
    let name = sub.text.as_str();
    let alias = options
        .aliases
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name));
    if let Some((_, expansion)) = alias {
        if let Some(why) = alias_command(expansion, rest, scan) {
            return Some(why);
        }
    }
    match name {
        "commit" => commit(rest),
        "rebase" => skips_hooks(name, rest).or_else(|| rebase_exec(rest, scan)),
        n if NO_VERIFY_SUBCOMMANDS.contains(&n) => skips_hooks(name, rest),
        "submodule" => submodule_foreach(rest, scan),
        "config" => config(rest, scan),
        "rm" | "mv" => rest
            .iter()
            .find(|a| scan.touches_hooks(&a.text))
            .map(|a| format!("git {name} {} in {PROTECTED}", a.text)),
        _ => None,
    }
}

/// An opaque word is let through where a flag can't be told from an argument only if it is
/// quoted whole (`"$branch"`) or follows a literal argument (`origin $branch`).
fn skips_hooks(name: &str, rest: &[Word]) -> Option<String> {
    let mut positional = false;
    for a in rest {
        if no_verify(&a.text) && !a.opaque {
            return Some(format!("git {name} {} skips the hooks", a.text));
        }
        if a.opaque {
            if a.text.starts_with('-') || !(positional || a.quoted_var) {
                return Some(unreadable(&format!("git {name}")));
            }
        } else if !a.text.starts_with('-') {
            positional = true;
        }
    }
    None
}

/// `git rebase -x CMD`, `--exec CMD` and `--exec=CMD` run CMD in a shell.
fn rebase_exec(rest: &[Word], scan: &mut Scan) -> Option<String> {
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let text = a.text.as_str();
        let script = match text {
            "-x" | "--exec" => {
                let script = it.next()?;
                if script.opaque {
                    return Some(unreadable("git rebase --exec"));
                }
                script.text.as_str()
            }
            _ => match text.strip_prefix("--exec=") {
                Some(script) => script,
                None => match text.strip_prefix("-x").filter(|_| !text.starts_with("--")) {
                    Some(script) => script,
                    None => continue,
                },
            },
        };
        if let Some(why) = check_command(script, scan) {
            return Some(why);
        }
    }
    None
}

/// `git submodule foreach [--recursive] CMD` runs CMD in a shell.
fn submodule_foreach(rest: &[Word], scan: &mut Scan) -> Option<String> {
    let [first, options @ ..] = rest else {
        return None;
    };
    if first.text != "foreach" {
        return None;
    }
    let script = options.iter().find(|a| !a.text.starts_with('-'))?;
    if script.opaque {
        return Some(unreadable("git submodule foreach"));
    }
    check_command(&script.text, scan)
}

/// What a `git config` key can do to the hooks.
enum ConfigKey<'a> {
    HooksPath,
    /// `alias.NAME`, with the name.
    Alias(&'a str),
    /// `include.path` or `includeIf.*.path`: a file that can set `core.hooksPath`.
    Include,
}

fn config_key(key: &str) -> Option<ConfigKey<'_>> {
    let lower = key.to_ascii_lowercase();
    if lower == "core.hookspath" {
        Some(ConfigKey::HooksPath)
    } else if lower == "include.path" || lower.starts_with("includeif.") {
        Some(ConfigKey::Include)
    } else if lower.starts_with("alias.") {
        Some(ConfigKey::Alias(&key["alias.".len()..]))
    } else {
        None
    }
}

/// `git -c KEY=VALUE`: a hooks path override, an include, or an alias that hides a bypass.
/// An alias is also recorded so the subcommand that uses it is read with its arguments.
fn config_pair(kv: &str, scan: &mut Scan, aliases: &mut Vec<(String, String)>) -> Option<String> {
    let (key, value) = kv.split_once('=').unwrap_or((kv, ""));
    match config_key(key)? {
        ConfigKey::HooksPath => Some(format!("git -c {kv} overrides core.hooksPath")),
        ConfigKey::Include => Some(format!(
            "git -c {kv} includes a file that could override core.hooksPath"
        )),
        ConfigKey::Alias(name) => {
            aliases.push((name.to_string(), value.to_string()));
            alias_command(value, &[], scan)
        }
    }
}

/// An alias expanded with the arguments given after it: `!` text is a shell command that
/// gets them appended, anything else is git arguments.
fn alias_command(expansion: &str, rest: &[Word], scan: &mut Scan) -> Option<String> {
    if let Some(shell) = expansion.strip_prefix('!') {
        if rest.iter().any(|w| w.opaque) {
            return Some(unreadable("a git alias"));
        }
        let args: Vec<String> = rest.iter().map(|w| shell_quote(&w.text)).collect();
        return check_command(&format!("{shell} {}", args.join(" ")), scan);
    }
    let mut words: Vec<Word> = expansion.split_whitespace().map(Word::literal).collect();
    words.extend_from_slice(rest);
    git(&words, scan)
}

/// `git config --get core.hooksPath` and `git config core.hooksPath` read; a write flag, or
/// a value after the key in any position, writes.
fn config_writes(args: &[Word], key_at: usize) -> bool {
    const WRITES: [&str; 8] = [
        "set",
        "unset",
        "--unset",
        "--unset-all",
        "--add",
        "--replace-all",
        "--edit",
        "-e",
    ];
    const READS: [&str; 6] = [
        "--get",
        "--get-all",
        "--get-regexp",
        "--get-urlmatch",
        "--list",
        "-l",
    ];
    let has = |names: &[&str]| args.iter().any(|a| names.contains(&a.text.as_str()));
    if has(&WRITES) {
        return true;
    }
    !has(&READS) && args[key_at + 1..].iter().any(|a| !a.text.starts_with('-'))
}

fn config(args: &[Word], scan: &mut Scan) -> Option<String> {
    let (at, key) = args
        .iter()
        .enumerate()
        .find_map(|(i, a)| config_key(&a.text).map(|k| (i, k)))?;
    if !config_writes(args, at) {
        return None;
    }
    match key {
        ConfigKey::HooksPath => Some("git config core.hooksPath moves the hooks".to_string()),
        ConfigKey::Include => Some(format!(
            "git config {} includes a file that could move core.hooksPath",
            args[at].text
        )),
        ConfigKey::Alias(_) => {
            let value = args[at + 1..].iter().find(|a| !a.text.starts_with('-'))?;
            if value.opaque {
                return Some(unreadable("a git alias"));
            }
            if value.text.starts_with('!') {
                return Some(format!(
                    "git config {} stores a shell command that could skip the hooks",
                    args[at].text
                ));
            }
            alias_command(&value.text, &[], scan)
                .map(|why| format!("git config {} stores an alias: {why}", args[at].text))
        }
    }
}

/// `git --config-env=KEY=ENVVAR`.
fn config_env(kv: &str) -> Option<String> {
    let key = kv.split('=').next().unwrap_or("");
    is_hooks_path(key).then(|| format!("git --config-env {kv} overrides core.hooksPath"))
}

fn commit(args: &[Word]) -> Option<String> {
    const LONG_VALUES: [&str; 12] = [
        "--message",
        "--file",
        "--reuse-message",
        "--reedit-message",
        "--template",
        "--author",
        "--date",
        "--fixup",
        "--squash",
        "--trailer",
        "--cleanup",
        "--pathspec-from-file",
    ];
    let mut it = args.iter();
    while let Some(w) = it.next() {
        let a = w.text.as_str();
        if a == "--" {
            return None;
        }
        if w.opaque {
            return Some(unreadable("git commit"));
        }
        if a.starts_with("--") {
            if no_verify(a) {
                return Some(format!("git commit {a} skips the hooks"));
            }
            if !a.contains('=') && LONG_VALUES.contains(&a) {
                it.next();
            }
            continue;
        }
        let Some(flags) = a.strip_prefix('-') else {
            continue;
        };
        for (n, c) in flags.chars().enumerate() {
            match c {
                'n' => return Some(format!("git commit {a} skips the hooks")),
                'm' | 'F' | 'c' | 'C' | 't' => {
                    if n + 1 == flags.len() {
                        it.next();
                    }
                    break;
                }
                'u' | 'S' => break,
                _ => {}
            }
        }
    }
    None
}

/// A file command that edits, moves or deletes something in `.githooks/`.
fn file_tool(prog: &str, args: &[Word], scan: &mut Scan) -> Option<String> {
    let hit = |a: &&Word| scan.touches_hooks(&a.text);
    let found = match prog {
        "rm" | "rmdir" | "unlink" | "shred" | "mv" | "chmod" | "chown" | "truncate" | "tee"
        | "ln" | "touch" => args.iter().find(hit),
        "cp" | "install" if has_target_directory_flag(args) => args.iter().find(hit),
        "cp" | "install" | "rsync" => args.last().filter(hit),
        "dd" => args.iter().find(|a| {
            a.text
                .strip_prefix("of=")
                .is_some_and(|f| scan.touches_hooks(f))
        }),
        "sed" | "perl" | "awk" | "gawk" if edits_in_place(prog, args) => args.iter().find(hit),
        "find" => return find(args, scan),
        _ => None,
    }?;
    Some(format!("{prog} {} in {PROTECTED}", found.text))
}

fn has_target_directory_flag(args: &[Word]) -> bool {
    args.iter().any(|a| {
        a.text.starts_with("--target-directory")
            || (!a.text.starts_with("--") && a.text.starts_with('-') && a.text.contains('t'))
    })
}

fn edits_in_place(prog: &str, args: &[Word]) -> bool {
    if prog == "awk" || prog == "gawk" {
        return args
            .windows(2)
            .any(|p| p[0].text == "-i" && p[1].text == "inplace");
    }
    let stops = if prog == "perl" { "eEMmIxdDCVF" } else { "ef" };
    args.iter().any(|a| {
        let t = a.text.as_str();
        if t == "--in-place" || t.starts_with("--in-place=") {
            return true;
        }
        let Some(cluster) = t.strip_prefix('-').filter(|c| !c.starts_with('-')) else {
            return false;
        };
        cluster
            .chars()
            .find(|c| *c == 'i' || stops.contains(*c))
            .is_some_and(|c| c == 'i')
    })
}

fn find(args: &[Word], scan: &mut Scan) -> Option<String> {
    let acts = |a: &Word| {
        matches!(
            a.text.as_str(),
            "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
        )
    };
    if args.iter().any(acts) {
        if let Some(dir) = args.iter().find(|a| scan.touches_hooks(&a.text)) {
            return Some(format!("find {} in {PROTECTED}", dir.text));
        }
    }
    args.iter()
        .enumerate()
        .filter(|(_, a)| matches!(a.text.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
        .find_map(|(i, _)| check_segment(&args[i + 1..], scan))
}
