//! The paths the guard protects and the file commands that write to them.

use super::*;
use crate::pin;

/// `path` is a protected path (`protected_paths`), or `.ns` or a unit's directory in it: move or
/// delete either, then recreate it, to swap the snapshot.
pub(super) fn is_protected(path: &str) -> bool {
    protected_paths(path) || is_ns_or_unit_dir(&components(path))
}

/// `path` is in `.githooks/`, `.git/hooks` or `.git/config`, or is a Claude Code settings file
/// (`.claude/settings*.json`), which can turn the hook off, or is a way in to a unit's skill
/// snapshot, which a later phase reads: `.ns/<unit>/skills`, and a `.claude/skills` or
/// `.agents/skills` that is a symlink into a snapshot. Also the phase settings: the file
/// `ns/units/<unit>/phase-settings.json` under any common dir, and the `.git` directory (or file,
/// in a worktree) itself with everything under `.git/ns`, so the `ns` directory and `ns/units`
/// cannot be moved, removed or copied over to swap that file, except a phase's own scratch space
/// (`in_unit_tmp`). Only writes are denied; reading
/// stays free, and `ns` commands a phase runs name none of these paths. A shell standing in a unit's directory
/// may write its artifacts, so the directory helpers call this and not `is_protected`.
fn protected_paths(path: &str) -> bool {
    let parts = components(path);
    parts.contains(&".githooks")
        || parts.windows(2).any(|p| {
            (p[0] == ".git" && matches!(p[1], "hooks" | "config"))
                || (p[0] == ".claude" && p[1].starts_with("settings") && p[1].ends_with(".json"))
        })
        || parts.last() == Some(&".git")
        || (parts
            .windows(2)
            .any(|p| p[0] == ".git" && names_in_common_dir(p[1], pin::COMMON_NS))
            && !in_unit_tmp(&parts))
        || parts.windows(4).any(|p| {
            p[0] == pin::COMMON_NS && p[1] == pin::COMMON_UNITS && p[3] == pin::SETTINGS_FILE
        })
        || pin::LINKS
            .iter()
            .any(|link| is_under_snapshot_link(path, &parts, link))
        || parts
            .windows(3)
            .any(|p| p[0] == pin::NS_DIR && names_skills(p[2]))
}

/// `parts` are an absolute path in `.git/ns/units/<unit>/tmp`, where `ns run` puts each phase's
/// `TMPDIR`. A `..` or a glob could reach past it, so either keeps the path protected.
fn in_unit_tmp(parts: &[&str]) -> bool {
    !parts
        .iter()
        .any(|p| *p == ".." || p.contains(['*', '?', '[']))
        && parts.windows(5).any(|p| {
            p[0] == ".git"
                && p[1] == pin::COMMON_NS
                && p[2] == pin::COMMON_UNITS
                && p[4] == pin::TMP_DIR
        })
}

/// The raw `word` goes up from `$TMPDIR`, which could land on the unit's `phase-settings.json`.
/// The lexer drops variable names, so this reads the command's own text.
pub(super) fn escapes_tmpdir(path: &str) -> bool {
    (path.contains("$TMPDIR") || path.contains("${TMPDIR")) && path.split('/').any(|p| p == "..")
}

/// A `ln` argument that could plant a link out of a phase's `TMPDIR`: one in the unit's `tmp`
/// directory or under `$TMPDIR`, or one naming the phase settings file. A link there, written
/// through later, would reach past the `tmp` exception.
fn links_through_tmp(path: &str) -> bool {
    let parts = components(path);
    path.contains("TMPDIR")
        || path.contains(pin::SETTINGS_FILE)
        || parts
            .windows(5)
            .any(|p| p[1] == pin::COMMON_NS && p[2] == pin::COMMON_UNITS && p[4] == pin::TMP_DIR)
}

/// `name` is `target`, or a glob that could match it.
fn names_in_common_dir(name: &str, target: &str) -> bool {
    if !name.contains(['*', '?', '[']) {
        return name == target;
    }
    glob_matches(name.as_bytes(), target.as_bytes())
}

/// `parts` end at `.ns` or at a direct child of it, the unit's directory.
fn is_ns_or_unit_dir(parts: &[&str]) -> bool {
    match parts {
        [.., last] if *last == pin::NS_DIR => true,
        [.., ns, _] => *ns == pin::NS_DIR,
        _ => false,
    }
}

/// `name` is `skills`, or a glob that could match it.
fn names_skills(name: &str) -> bool {
    if !name.contains(['*', '?', '[']) {
        return name == pin::SKILLS_DIR;
    }
    glob_matches(name.as_bytes(), pin::SKILLS_DIR.as_bytes())
}

/// `pattern` (`*` and `?`; a `[` set counts as a match) matches `text`.
pub(super) fn glob_matches(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|i| glob_matches(rest, &text[i..])),
        Some((b'[', _)) => true,
        Some((c, rest)) => text
            .split_first()
            .is_some_and(|(t, tail)| (*c == b'?' || c == t) && glob_matches(rest, tail)),
    }
}

/// `path` goes through `link` (`.claude/skills` or `.agents/skills`) and the link is a symlink
/// into a unit's snapshot. A real directory there is the repo's own and stays editable.
fn is_under_snapshot_link(path: &str, parts: &[&str], link: &str) -> bool {
    let link_parts: Vec<&str> = link.split('/').collect();
    let Some(i) = parts.windows(2).position(|p| p == link_parts.as_slice()) else {
        return false;
    };
    let root = if path.starts_with('/') { "/" } else { "" };
    let link = format!("{root}{}", parts[..i + 2].join("/"));
    std::fs::read_link(link).is_ok_and(|target| pin::points_into_a_snapshot(&target))
}

/// Where a shell is, as far as the guard follows it.
#[derive(Clone, Copy, Default)]
pub(super) struct Cwd {
    /// How many directories below a protected path, or `None` when not in one.
    pub hooks: Option<usize>,
    /// How many directories below `.ns/`: `.ns/<unit>` is 1. A relative `skills` there is the
    /// unit's snapshot, and the unit's directory is protected itself.
    pub ns: Option<usize>,
}

impl Cwd {
    /// The working directory `dir` names.
    pub fn of(dir: &str) -> Cwd {
        let parts = components(dir);
        Cwd {
            hooks: protected_depth(dir),
            ns: parts
                .iter()
                .rposition(|p| *p == pin::NS_DIR)
                .and_then(|i| walk(0, &parts[i + 1..])),
        }
    }

    /// The working directory after `cd` to the relative `dir` from `from`.
    pub fn relative(from: Cwd, dir: &str) -> Cwd {
        let parts = components(dir);
        let named = Cwd::of(dir);
        Cwd {
            hooks: named.hooks.or_else(|| {
                from.hooks.map(|depth| {
                    parts
                        .iter()
                        .fold(depth, |depth, part| step(depth, part).unwrap_or(0))
                })
            }),
            ns: named
                .ns
                .or_else(|| from.ns.and_then(|depth| walk(depth, &parts))),
        }
    }
}

/// The depth after one path `part` from `depth` directories below a root: `None` when `..`
/// leaves the root. Every depth walk here is this step. A walk through `walk` ends at `None`
/// once it leaves; the hooks directory (`Cwd::relative`) floors at the root instead, since
/// staying protected is the safe side; `reaches_unit_file` also looks at each part on the way.
fn step(depth: usize, part: &str) -> Option<usize> {
    match part {
        ".." => depth.checked_sub(1),
        _ => Some(depth + 1),
    }
}

/// The depth after walking `parts` from `depth`; `None` once it leaves the root.
fn walk(depth: usize, parts: &[&str]) -> Option<usize> {
    parts
        .iter()
        .try_fold(depth, |depth, part| step(depth, part))
}

/// The relative `path` from `depth` directories below `.ns/` reaches a unit's `skills`, or ends
/// at the unit's directory or `.ns` itself.
pub(super) fn reaches_unit_file(mut depth: usize, path: &str) -> bool {
    for part in components(path) {
        if depth == 1 && part != ".." && names_skills(part) {
            return true;
        }
        match step(depth, part) {
            Some(next) => depth = next,
            None => return false,
        }
    }
    depth <= 1
}

/// A working directory whose relative paths are protected: inside `.githooks/` or the `.git`
/// directory itself.
pub(super) fn is_protected_directory(dir: &str) -> bool {
    let parts = components(dir);
    protected_paths(dir) || parts.contains(&".git")
}

/// How many directories below a protected path the directory `dir` is, or `None` when it is
/// not in one.
pub(super) fn protected_depth(dir: &str) -> Option<usize> {
    if !is_protected_directory(dir) {
        return None;
    }
    let parts = components(dir);
    let root = (0..parts.len())
        .find(|&i| parts[i] == ".git" || protected_paths(&parts[..=i].join("/")))?;
    Some(parts.len() - root - 1)
}

/// The relative `path` from `depth` directories below a protected path stays in it.
pub(super) fn stays_inside(depth: usize, path: &str) -> bool {
    walk(depth, &components(path)).is_some()
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
        "ln" => args
            .iter()
            .find(|a| hit(a) || links_through_tmp(option_value(&a.text))),
        "rm" | "rmdir" | "unlink" | "shred" | "mv" | "chmod" | "chown" | "truncate" | "tee"
        | "touch" => args.iter().find(hit),
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
