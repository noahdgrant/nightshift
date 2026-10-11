//! `ns guard-hooks`: the `PreToolUse` hook `ns run` gives every claude phase. It reads the
//! tool call claude is about to make on stdin and denies one that would skip the repo's git
//! hooks, naming the rule (`skills/ns-contract`, Git hooks). Every other call passes untouched.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{json, Value};

use crate::shell::shell_quote;

mod lexer;
#[cfg(test)]
mod tests;

use lexer::{tokens, Tok, Word};

const RULE: &str = "The factory contract's Git hooks rule (skills/ns-contract/SKILL.md, \"Git hooks\") says a phase never skips the repo's hooks. Fix what the hook reports and commit or push again; when the cause lies outside the unit, stop with status: blocked.";

/// How deep `eval`, `sh -c` and aliases may nest before the guard gives up and denies.
const MAX_NESTING: usize = 8;

const GIT_SUBCOMMANDS: [&str; 9] = [
    "commit",
    "push",
    "merge",
    "rebase",
    "pull",
    "am",
    "cherry-pick",
    "revert",
    "config",
];

/// The `--settings` value for a claude phase: one `PreToolUse` hook that runs `ns guard-hooks`.
/// Claude Code lets a call through when a hook exits with anything but 2, so a missing or
/// crashing `ns` must exit 2 to block it.
pub fn settings(ns: &Path) -> String {
    let command = format!(
        "{} guard-hooks || exit 2",
        shell_quote(&ns.to_string_lossy())
    );
    json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash|Edit|Write|MultiEdit|NotebookEdit",
                "hooks": [{"type": "command", "command": command}],
            }],
        },
    })
    .to_string()
}

/// The running `ns` binary, which must still exist for the hook to run it.
pub fn ns_exe() -> anyhow::Result<PathBuf> {
    let exe = strip_deleted(std::env::current_exe().context("cannot find the ns binary")?);
    anyhow::ensure!(
        exe.exists(),
        "the ns binary {} no longer exists; the hook guard could not run it. Rebuild or reinstall ns and run again",
        exe.display()
    );
    Ok(exe)
}

/// Linux appends ` (deleted)` to `current_exe` when the binary was replaced while running.
pub fn strip_deleted(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(live) => PathBuf::from(live),
        None => path,
    }
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
        "Bash" => check_command(input["command"].as_str()?, &mut Scan::default()),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => ["file_path", "notebook_path"]
            .iter()
            .filter_map(|k| input[k].as_str())
            .find(|p| in_hooks_dir(p))
            .map(|p| format!("edit of {p} in .githooks/")),
        _ => None,
    }
}

fn in_hooks_dir(path: &str) -> bool {
    path.split('/').any(|c| c == ".githooks")
}

/// `path` is in `.githooks/`, or is relative to a working directory that is.
fn touches_hooks(path: &str, cwd_is_hooks: bool) -> bool {
    let path = match path.strip_prefix("--").and_then(|_| path.split_once('=')) {
        Some((_, value)) => value,
        None => path,
    };
    in_hooks_dir(path) || (cwd_is_hooks && !path.starts_with(['-', '/']) && !path.starts_with(".."))
}

fn unreadable(what: &str) -> String {
    format!("{what} takes a word built by a shell expansion that could be a flag that skips the hooks; write the word out literally")
}

// --------------------------------------------------------------- analysis

#[derive(Default)]
struct Scan {
    /// An earlier `cd` or `pushd` went into `.githooks/`.
    cwd_is_hooks: bool,
    depth: usize,
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
                    if touches_hooks(&f.text, scan.cwd_is_hooks) {
                        return Some(format!("write to {} in .githooks/", f.text));
                    }
                }
            }
            Tok::RedirectIn => {
                toks.next_if(is_word);
            }
            Tok::CommandEnd => {
                if let Some(why) = check_segment(&words, scan) {
                    return Some(why);
                }
                words.clear();
            }
        }
    }
    check_segment(&words, scan)
}

const RESERVED_WORDS: [&str; 9] = [
    "if", "then", "else", "elif", "do", "while", "until", "!", "coproc",
];

struct Wrapper {
    short_values: &'static str,
    long_values: &'static [&'static str],
}

/// A command that runs the command after its own options. The tables hold the options that
/// take a value, so the value is not mistaken for the program.
fn wrapper(prog: &str) -> Option<Wrapper> {
    let (short_values, long_values): (&str, &[&str]) = match prog {
        "export" | "command" | "nohup" | "builtin" | "setsid" => ("", &[]),
        "env" => ("uC", &["--unset", "--chdir"]),
        "sudo" => (
            "ughpCrtTUDR",
            &[
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
        ),
        "doas" => ("uC", &[]),
        "nice" => ("n", &["--adjustment"]),
        "exec" => ("a", &[]),
        "time" => ("fo", &["--format", "--output"]),
        "stdbuf" => ("ioe", &["--input", "--output", "--error"]),
        "ionice" => (
            "cnpPu",
            &["--class", "--classdata", "--pid", "--pgid", "--uid"],
        ),
        "xargs" => (
            "nLPIsdEa",
            &[
                "--max-args",
                "--max-lines",
                "--max-procs",
                "--max-chars",
                "--delimiter",
                "--arg-file",
                "--process-slot-var",
            ],
        ),
        _ => return None,
    };
    Some(Wrapper {
        short_values,
        long_values,
    })
}

/// The words after a command's leading options, skipping the value of each option that takes one.
fn skip_options<'a>(mut words: &'a [Word], short_values: &str, long_values: &[&str]) -> &'a [Word] {
    while let Some(first) = words.first() {
        let arg = first.text.as_str();
        let width = if arg == "--" {
            return &words[1..];
        } else if arg.starts_with("--") {
            if !arg.contains('=') && long_values.contains(&arg) {
                2
            } else {
                1
            }
        } else if let Some(cluster) = arg.strip_prefix('-').filter(|c| !c.is_empty()) {
            let value_flag = cluster
                .char_indices()
                .find(|(_, c)| short_values.contains(*c));
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

/// The command string of `env -S`, which env splits and runs.
fn env_split_string(options: &[Word]) -> Option<&str> {
    let mut it = options.iter().map(|w| w.text.as_str());
    while let Some(arg) = it.next().filter(|a| a.starts_with('-')) {
        if arg == "-S" || arg == "--split-string" {
            return it.next();
        }
        if let Some(v) = arg.strip_prefix("--split-string=") {
            return Some(v);
        }
        if let Some(v) = arg.strip_prefix("-S").filter(|_| !arg.starts_with("--")) {
            return Some(v);
        }
    }
    None
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
        let prog = base(text);
        if let Some(wrap) = wrapper(prog) {
            if prog == "env" {
                if let Some(cmd) = env_split_string(&w[1..]) {
                    return check_command(cmd, scan);
                }
            }
            w = skip_options(&w[1..], wrap.short_values, wrap.long_values);
            continue;
        }
        match prog {
            "timeout" => {
                w = skip_options(&w[1..], "ks", &["--kill-after", "--signal"]);
                w = w.get(1..)?;
            }
            "cd" | "pushd" => {
                change_directory(&w[1..], scan);
                return None;
            }
            "popd" => {
                scan.cwd_is_hooks = false;
                return None;
            }
            "eval" => return check_words(&w[1..], scan),
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" => {
                let i = w[1..].iter().position(|a| {
                    a.text.starts_with('-') && !a.text.starts_with("--") && a.text.contains('c')
                })?;
                return check_words(w.get(i + 2..i + 3)?, scan);
            }
            "git" => return git(&w[1..], scan),
            prog => return file_tool(prog, &w[1..], scan),
        }
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
        w.text.contains("git")
            || GIT_SUBCOMMANDS.contains(&w.text.as_str())
            || in_hooks_dir(&w.text)
    });
    near_git.then(|| "a command named by a shell expansion next to git or .githooks".to_string())
}

fn change_directory(args: &[Word], scan: &mut Scan) {
    let dir = args
        .iter()
        .map(|a| a.text.as_str())
        .find(|a| !a.starts_with('-'));
    match dir {
        Some(d) if in_hooks_dir(d) => scan.cwd_is_hooks = true,
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

fn base(prog: &str) -> &str {
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

fn git(args: &[Word], scan: &mut Scan) -> Option<String> {
    let mut cwd_is_hooks = scan.cwd_is_hooks;
    let mut i = 0;
    while let Some(a) = args.get(i) {
        if a.opaque {
            return Some(unreadable("a git option"));
        }
        let arg = a.text.as_str();
        i += match arg {
            "-c" => {
                if let Some(why) = config_pair(&args.get(i + 1)?.text, scan) {
                    return Some(why);
                }
                2
            }
            "-C" => {
                cwd_is_hooks |= in_hooks_dir(&args.get(i + 1)?.text);
                2
            }
            "--config-env" => {
                if let Some(why) = config_env(&args.get(i + 1)?.text) {
                    return Some(why);
                }
                2
            }
            "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix" => 2,
            _ => {
                if let Some(kv) = arg.strip_prefix("--config-env=") {
                    if let Some(why) = config_env(kv) {
                        return Some(why);
                    }
                } else if let Some(kv) = arg.strip_prefix("-c").filter(|_| !arg.starts_with("--")) {
                    if let Some(why) = config_pair(kv, scan) {
                        return Some(why);
                    }
                } else if !arg.starts_with('-') {
                    break;
                }
                1
            }
        };
    }
    let sub = args.get(i)?;
    if sub.opaque {
        return Some(unreadable("the git subcommand"));
    }
    let rest = &args[i + 1..];
    let name = sub.text.as_str();
    match name {
        "commit" => commit(rest),
        "push" | "merge" | "rebase" | "pull" | "am" | "cherry-pick" | "revert" => rest
            .iter()
            .find(|a| a.opaque || no_verify(&a.text))
            .map(|a| {
                if a.opaque {
                    unreadable(&format!("git {name}"))
                } else {
                    format!("git {name} {} skips the hooks", a.text)
                }
            }),
        "config" => config(rest),
        "rm" | "mv" => rest
            .iter()
            .find(|a| touches_hooks(&a.text, cwd_is_hooks))
            .map(|a| format!("git {name} {} in .githooks/", a.text)),
        _ => None,
    }
}

/// `git -c KEY=VALUE`: a hooks path override, or an alias that hides a bypass.
fn config_pair(kv: &str, scan: &mut Scan) -> Option<String> {
    let (key, value) = kv.split_once('=').unwrap_or((kv, ""));
    if is_hooks_path(key) {
        return Some(format!("git -c {kv} overrides core.hooksPath"));
    }
    if key
        .get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case("alias."))
    {
        if let Some(shell) = value.strip_prefix('!') {
            return check_command(shell, scan);
        }
        let words: Vec<Word> = value.split_whitespace().map(Word::literal).collect();
        return git(&words, scan);
    }
    None
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

fn config(args: &[Word]) -> Option<String> {
    let key = args.iter().position(|a| is_hooks_path(&a.text))?;
    let writes = [
        "set",
        "unset",
        "--unset",
        "--unset-all",
        "--add",
        "--replace-all",
        "--edit",
        "-e",
    ];
    let read_only = key + 1 == args.len()
        && !args[..key]
            .iter()
            .any(|a| writes.contains(&a.text.as_str()));
    (!read_only).then(|| "git config core.hooksPath moves the hooks".to_string())
}

/// A file command that edits, moves or deletes something in `.githooks/`.
fn file_tool(prog: &str, args: &[Word], scan: &mut Scan) -> Option<String> {
    let cwd = scan.cwd_is_hooks;
    let hit = |a: &&Word| touches_hooks(&a.text, cwd);
    let found = match prog {
        "rm" | "rmdir" | "unlink" | "shred" | "mv" | "chmod" | "chown" | "truncate" | "tee"
        | "ln" | "touch" => args.iter().find(hit),
        "cp" | "install" if has_target_directory_flag(args) => args.iter().find(hit),
        "cp" | "install" | "rsync" => args.last().filter(hit),
        "dd" => args.iter().find(|a| {
            a.text
                .strip_prefix("of=")
                .is_some_and(|f| touches_hooks(f, cwd))
        }),
        "sed" | "perl" | "awk" | "gawk" if edits_in_place(prog, args) => args.iter().find(hit),
        "find" => return find(args, scan),
        _ => None,
    }?;
    Some(format!("{prog} {} in .githooks/", found.text))
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
    let cwd = scan.cwd_is_hooks;
    let acts = |a: &Word| {
        matches!(
            a.text.as_str(),
            "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
        )
    };
    if args.iter().any(acts) {
        if let Some(dir) = args.iter().find(|a| touches_hooks(&a.text, cwd)) {
            return Some(format!("find {} in .githooks/", dir.text));
        }
    }
    args.iter()
        .enumerate()
        .filter(|(_, a)| matches!(a.text.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
        .find_map(|(i, _)| check_segment(&args[i + 1..], scan))
}
