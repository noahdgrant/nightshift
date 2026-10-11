//! The paths the guard protects and the file commands that write to them.

use super::*;

/// `path` is in `.githooks/`, `.git/hooks` or `.git/config`, or is a Claude Code settings file
/// (`.claude/settings*.json`), which can turn the hook off.
pub(super) fn is_protected(path: &str) -> bool {
    let parts = components(path);
    parts.contains(&".githooks")
        || parts.windows(2).any(|p| {
            (p[0] == ".git" && matches!(p[1], "hooks" | "config"))
                || (p[0] == ".claude" && p[1].starts_with("settings") && p[1].ends_with(".json"))
        })
}

/// A working directory whose relative paths are protected: inside `.githooks/` or the `.git`
/// directory itself.
pub(super) fn is_protected_directory(dir: &str) -> bool {
    let parts = components(dir);
    is_protected(dir) || parts.contains(&".git")
}

/// How many directories below a protected path the directory `dir` is, or `None` when it is
/// not in one.
pub(super) fn protected_depth(dir: &str) -> Option<usize> {
    if !is_protected_directory(dir) {
        return None;
    }
    let parts = components(dir);
    let root =
        (0..parts.len()).find(|&i| parts[i] == ".git" || is_protected(&parts[..=i].join("/")))?;
    Some(parts.len() - root - 1)
}

/// The relative `path` from `depth` directories below a protected path stays in it.
pub(super) fn stays_inside(mut depth: usize, path: &str) -> bool {
    for part in components(path) {
        if part == ".." {
            match depth.checked_sub(1) {
                Some(up) => depth = up,
                None => return false,
            }
        }
    }
    true
}

/// The parts of `path` between slashes, without the empty and `.` ones.
pub(super) fn components(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|p| !matches!(*p, "" | "."))
        .collect()
}

/// The value of `--option=value`, or `arg` itself.
pub(super) fn option_value(arg: &str) -> &str {
    match arg.strip_prefix("--").and_then(|_| arg.split_once('=')) {
        Some((_, value)) => value,
        None => arg,
    }
}

/// A file command that edits, moves or deletes something in `.githooks/`.
pub(super) fn file_tool(prog: &str, args: &[Word], scan: &mut Scan) -> Option<String> {
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

pub(super) fn has_target_directory_flag(args: &[Word]) -> bool {
    args.iter().any(|a| {
        a.text.starts_with("--target-directory")
            || (!a.text.starts_with("--") && a.text.starts_with('-') && a.text.contains('t'))
    })
}

pub(super) fn edits_in_place(prog: &str, args: &[Word]) -> bool {
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

fn names_hooks(pattern: &str) -> bool {
    pattern.contains("githooks") || pattern.contains("git/hooks")
}

pub(super) fn find(args: &[Word], scan: &mut Scan) -> Option<String> {
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
        if let Some(pattern) = args.windows(2).find_map(|p| {
            let matches_path = matches!(
                p[0].text.as_str(),
                "-path" | "-ipath" | "-wholename" | "-iwholename" | "-regex" | "-iregex"
            );
            (matches_path && names_hooks(&p[1].text)).then_some(&p[1])
        }) {
            return Some(format!("find {} in {PROTECTED}", pattern.text));
        }
    }
    let mut rest = args;
    while let Some(at) = rest
        .iter()
        .position(|a| matches!(a.text.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
    {
        let command = &rest[at + 1..];
        let end = command
            .iter()
            .position(|a| matches!(a.text.as_str(), ";" | "+"))
            .unwrap_or(command.len());
        if let Some(why) = check_segment(&command[..end], scan) {
            return Some(why);
        }
        rest = command.get(end + 1..).unwrap_or(&[]);
    }
    None
}
