//! What the guard reads in a `git` command: skip flags, hook path overrides, aliases and
//! the commands git runs for you.

use super::*;
use crate::shell::shell_quote;

/// Git subcommands that take `--no-verify` and need no other reading.
pub(super) const NO_VERIFY_SUBCOMMANDS: &[&str] =
    &["push", "merge", "pull", "am", "cherry-pick", "revert"];

/// Subcommands the guard reads for a bypass.
pub(super) fn is_guarded_subcommand(name: &str) -> bool {
    NO_VERIFY_SUBCOMMANDS.contains(&name) || matches!(name, "commit" | "rebase" | "config")
}

/// `--no-verify` or an abbreviation git accepts for it.
pub(super) fn no_verify(arg: &str) -> bool {
    let name = arg.split('=').next().unwrap_or(arg);
    name.len() >= "--no-v".len() && "--no-verify".starts_with(name)
}

/// Options before the git subcommand that take a value in the next word.
pub(super) const GLOBAL_VALUE_OPTIONS: &[&str] = &[
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
pub(super) struct GlobalOptions {
    /// Index of the subcommand in the arguments.
    pub(super) subcommand: usize,
    /// `alias.NAME=VALUE` pairs from `-c`.
    pub(super) aliases: Vec<(String, String)>,
}

/// `--config-env=KV`, `-cKV` or `-Cdir` as the option and its value.
pub(super) fn attached_option(arg: &str) -> Option<(&str, &str)> {
    if let Some(kv) = arg.strip_prefix("--config-env=") {
        return Some(("--config-env", kv));
    }
    if let Some(dir) = arg.strip_prefix("--git-dir=") {
        return Some(("--git-dir", dir));
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
pub(super) fn git_global_options(
    args: &[Word],
    scan: &mut Scan,
) -> Result<Option<GlobalOptions>, String> {
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
            if value.opaque && matches!(arg, "-c" | "--config-env") {
                return Err(unreadable("a git config option"));
            }
            let value = if arg == "-C" {
                if cd_loses_protected_cwd(value, &scan.cwd) {
                    return Err(format!(
                        "git -C {} into a directory of unknown depth in a protected path",
                        value.text
                    ));
                }
                dir_path(value)
            } else {
                value.text.clone()
            };
            global_option(arg, &value, scan, &mut aliases)?;
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
pub(super) fn global_option(
    option: &str,
    value: &str,
    scan: &mut Scan,
    aliases: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let denial = match option {
        "-c" => config_pair(value, scan, aliases),
        "-C" => {
            scan.cwd = cd_to(value, &scan.cwd, scan.tmp);
            None
        }
        "--config-env" => config_env(value),
        "--git-dir" => Some(format!(
            "git --git-dir={value} makes git use other hooks and config"
        )),
        _ => None,
    };
    denial.map_or(Ok(()), Err)
}

pub(super) fn git(args: &[Word], scan: &mut Scan) -> Option<String> {
    let outer_cwd = scan.cwd.clone();
    let why = git_command(args, scan);
    scan.cwd = outer_cwd;
    why
}

pub(super) fn git_command(args: &[Word], scan: &mut Scan) -> Option<String> {
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
            .find(|a| scan.touches_word(a))
            .map(|a| format!("git {name} {} in {PROTECTED}", a.text)),
        _ => None,
    }
}

/// An opaque word is let through where a flag can't be told from an argument only if it
/// follows a literal argument (`origin $branch`) or a literal `--` that ends the options.
/// Outside `push` it may also be quoted whole (`"$branch"`). On `push` it may instead be a
/// word no expansion can turn into `--no-verify` (`"$V/remote.git"`), as a whole `"$R"` there
/// could hold it.
pub(super) fn skips_hooks(name: &str, rest: &[Word]) -> Option<String> {
    let mut positional = false;
    let mut before: Option<&Word> = None;
    for a in rest {
        if a.text == "--" && !a.opaque && before.is_none_or(leaves_double_dash_alone) {
            return None;
        }
        if no_verify(&a.text) && !a.opaque {
            return Some(format!("git {name} {} skips the hooks", a.text));
        }
        if a.opaque {
            let let_through = positional
                || if name == "push" {
                    !could_be_no_verify(a)
                } else {
                    a.quoted_var
                };
            if a.text.starts_with('-') || !let_through {
                return Some(unreadable(&format!("git {name}")));
            }
        } else if !a.text.starts_with('-') {
            positional = true;
        }
        before = Some(a);
    }
    None
}

/// The word before a `--` can't take it as its value: an argument, a long option with its
/// value attached, or flags known to take none. Any other option might, so the `--` ends
/// nothing.
fn leaves_double_dash_alone(w: &Word) -> bool {
    const LONG_FLAGS: &[&str] = &["--quiet", "--verbose", "--force", "--dry-run"];
    if w.opaque {
        return false;
    }
    match w.text.strip_prefix('-') {
        None => true,
        Some(long) if long.starts_with('-') => {
            LONG_FLAGS.contains(&w.text.as_str()) || w.text.contains('=')
        }
        Some(short) => !short.is_empty() && short.chars().all(|c| "qvfnu".contains(c)),
    }
}

/// `w`, with each expansion read as any text (`*`), could be a spelling of `--no-verify` git
/// accepts, or `--no-verify=` with a value, which `no_verify` reads as the flag: an `=`, or an
/// expansion at the end, could supply it. An expansion that splits the word could be anything.
fn could_be_no_verify(w: &Word) -> bool {
    if w.splits {
        return true;
    }
    let pattern = w.filled(|_| '*');
    let mut spellings = ("--no-v".len()..="--no-verify".len()).map(|n| &"--no-verify"[..n]);
    pattern.ends_with('*')
        || pattern.contains('=')
        || spellings.any(|s| glob_matches(pattern.as_bytes(), s.as_bytes()))
}

/// `git rebase -x CMD`, `--exec CMD` and `--exec=CMD` run CMD in a shell.
pub(super) fn rebase_exec(rest: &[Word], scan: &mut Scan) -> Option<String> {
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let text = a.text.as_str();
        let script = if matches!(text, "-x" | "--exec") {
            let script = it.next()?;
            if script.opaque {
                return Some(unreadable("git rebase --exec"));
            }
            script.text.as_str()
        } else if let Some(script) = text.strip_prefix("--exec=") {
            script
        } else if let Some(script) = text.strip_prefix("-x").filter(|_| !text.starts_with("--")) {
            script
        } else {
            continue;
        };
        if let Some(why) = check_command(script, scan) {
            return Some(why);
        }
    }
    None
}

/// `git submodule foreach [--recursive] CMD` runs CMD in a shell.
pub(super) fn submodule_foreach(rest: &[Word], scan: &mut Scan) -> Option<String> {
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
pub(super) enum ConfigKey<'a> {
    HooksPath,
    /// `alias.NAME`, with the name.
    Alias(&'a str),
    /// `include.path` or `includeIf.*.path`: a file that can set `core.hooksPath`.
    Include,
}

pub(super) fn config_key(key: &str) -> Option<ConfigKey<'_>> {
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
pub(super) fn config_pair(
    kv: &str,
    scan: &mut Scan,
    aliases: &mut Vec<(String, String)>,
) -> Option<String> {
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
pub(super) fn alias_command(expansion: &str, rest: &[Word], scan: &mut Scan) -> Option<String> {
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
pub(super) fn config_writes(args: &[Word], key_at: usize) -> bool {
    const WRITES: &[&str] = &[
        "set",
        "unset",
        "--unset",
        "--unset-all",
        "--add",
        "--replace-all",
    ];
    const READS: &[&str] = &[
        "--get",
        "--get-all",
        "--get-regexp",
        "--get-urlmatch",
        "--list",
        "-l",
    ];
    let has = |names: &[&str]| args.iter().any(|a| names.contains(&a.text.as_str()));
    if has(WRITES) {
        return true;
    }
    !has(READS) && args[key_at + 1..].iter().any(|a| !a.text.starts_with('-'))
}

/// `git config --edit`, or a section rename or removal that names `core`: each can drop or
/// move `core.hooksPath` without naming the key. The subcommand forms (`git config edit`)
/// count only as the first word that isn't an option.
pub(super) fn config_section_write(args: &[Word]) -> Option<String> {
    let text: Vec<&str> = args.iter().map(|a| a.text.as_str()).collect();
    let subcommand = text.iter().position(|a| !a.starts_with('-'));
    let names_core = |at: usize, count: usize| {
        text.iter()
            .skip(at + 1)
            .take(count)
            .any(|s| s.eq_ignore_ascii_case("core"))
    };
    text.iter().enumerate().find_map(|(i, arg)| {
        let operation = arg
            .strip_prefix("--")
            .or((Some(i) == subcommand).then_some(*arg));
        let hit = match operation {
            Some("edit") => true,
            Some("remove-section") => names_core(i, 1),
            Some("rename-section") => names_core(i, 2),
            _ => *arg == "-e",
        };
        hit.then(|| format!("git config {arg} can remove or move core.hooksPath"))
    })
}

pub(super) fn config(args: &[Word], scan: &mut Scan) -> Option<String> {
    if let Some(why) = config_section_write(args) {
        return Some(why);
    }
    let named = args.iter().find(|a| {
        !a.text.starts_with('-') && !matches!(a.text.as_str(), "set" | "unset" | "get" | "list")
    });
    if named.is_some_and(|a| a.opaque) {
        return Some(unreadable("a git config key"));
    }
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
pub(super) fn config_env(kv: &str) -> Option<String> {
    let key = kv.split('=').next().unwrap_or("");
    matches!(config_key(key), Some(ConfigKey::HooksPath))
        .then(|| format!("git --config-env {kv} overrides core.hooksPath"))
}

pub(super) fn commit(args: &[Word]) -> Option<String> {
    const LONG_VALUES: &[&str] = &[
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
        if w.opaque {
            return Some(unreadable("git commit"));
        }
        if a == "--" {
            return None;
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
