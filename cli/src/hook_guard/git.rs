//! What the guard reads in a `git` command: `--no-verify` and `-n` on commit and push,
//! `-c core.hooksPath`, `--git-dir`, and `git rm` or `git mv` of a protected path.

use super::files::is_protected;
use super::words::Word;
use super::PROTECTED;

/// Options before the git subcommand that take a value in the next word.
const VALUE_OPTIONS: &[&str] = &[
    "-c",
    "-C",
    "--config-env",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--super-prefix",
    "--attr-source",
];

/// Why `git args` would skip the hooks.
pub(super) fn check(args: &[Word]) -> Option<String> {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        let arg = a.text.as_str();
        if arg == "--git-dir" || arg.starts_with("--git-dir=") {
            return Some(format!("git {arg} makes git use other hooks and config"));
        }
        let config = if let Some(kv) = arg.strip_prefix("--config-env=") {
            Some(kv)
        } else if let Some(kv) = arg.strip_prefix("-c").filter(|kv| !kv.is_empty()) {
            Some(kv)
        } else if matches!(arg, "-c" | "--config-env") {
            args.get(i + 1).map(|v| v.text.as_str())
        } else {
            None
        };
        if let Some(kv) = config {
            let key = kv.split('=').next().unwrap_or(kv);
            if key.eq_ignore_ascii_case("core.hooksPath") {
                return Some(format!("git {arg} {kv} overrides core.hooksPath"));
            }
        }
        if !arg.starts_with('-') {
            return subcommand(arg, &args[i + 1..]);
        }
        i += if VALUE_OPTIONS.contains(&arg) { 2 } else { 1 };
    }
    None
}

fn subcommand(name: &str, rest: &[Word]) -> Option<String> {
    match name {
        "commit" => commit(rest),
        "push" => rest
            .iter()
            .take_while(|a| a.text != "--")
            .find(|a| no_verify(&a.text))
            .map(|a| format!("git push {} skips the hooks", a.text)),
        "rm" | "mv" => rest
            .iter()
            .find(|a| is_protected(&a.text))
            .map(|a| format!("git {name} {} in {PROTECTED}", a.text)),
        _ => None,
    }
}

/// `--no-verify` or an abbreviation git accepts for it.
fn no_verify(arg: &str) -> bool {
    let name = arg.split('=').next().unwrap_or(arg);
    name.len() >= "--no-v".len() && "--no-verify".starts_with(name)
}

/// `git commit`'s `--no-verify`, or `-n` alone or in a cluster of short flags, but not as the
/// value of a flag that takes one (`-m -n`) or after `--`.
fn commit(args: &[Word]) -> Option<String> {
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
        if a == "--" {
            return None;
        }
        if a.starts_with("--") {
            if no_verify(a) {
                return Some(format!("git commit {a} skips the hooks"));
            }
            if LONG_VALUES.contains(&a) {
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
