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
/// cannot be moved, removed or copied over to swap that file. Only writes are denied; reading
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
        || parts
            .windows(2)
            .any(|p| p[0] == ".git" && names_in_common_dir(p[1], pin::COMMON_NS))
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
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    loop {
        match pattern.get(p) {
            Some(b'[') => return true,
            Some(b'*') => {
                backtrack = Some((p, t));
                p += 1;
                continue;
            }
            Some(c) if t < text.len() && (*c == b'?' || *c == text[t]) => {
                p += 1;
                t += 1;
                continue;
            }
            None if t == text.len() => return true,
            _ => {}
        }
        match backtrack {
            Some((star, at)) if at < text.len() => {
                backtrack = Some((star, at + 1));
                p = star + 1;
                t = at + 1;
            }
            _ => return false,
        }
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

/// What a path holds where an expansion's depth can't be known.
pub(super) const UNREAD: char = '\0';

/// `w` read as a path. An expansion that is a whole part of it (`$X`, `${X}`, `"$X"`) is one
/// unknown directory, `*`: a glob for one part, so a check for `skills` or a unit's directory
/// still matches it. Any other expansion is `UNREAD`: two in one part, one beside other text,
/// a glob or a `..` part, one in a word with a brace, or one that isn't a plain parameter.
pub(super) fn path(w: &Word) -> String {
    let text = w.text.as_str();
    let mut parts: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for part in text.split('/') {
        if !matches!(part, "" | ".") {
            parts.push((start, start + part.len()));
        }
        start += part.len() + 1;
    }
    let is_dot_dot = |&(s, e): &(usize, usize)| &text[s..e] == "..";
    let differs = |j: Option<usize>, at: usize| {
        j.and_then(|j| w.expansions.get(j))
            .is_none_or(|o| o.at != at)
    };
    w.filled(|i| {
        let e = &w.expansions[i];
        let (before, after) = text.split_at(e.at);
        let alone = differs(i.checked_sub(1), e.at) && differs(Some(i + 1), e.at);
        let whole_part = (before.is_empty() || before.ends_with('/'))
            && (after.is_empty() || after.starts_with('/'));
        let prev = parts.partition_point(|&(_, end)| end <= e.at);
        let next = parts.partition_point(|&(start, _)| start < e.at);
        let beside_dot_dot = prev.checked_sub(1).is_some_and(|p| is_dot_dot(&parts[p]))
            || parts.get(next).is_some_and(is_dot_dot);
        if e.plain && alone && whole_part && !beside_dot_dot {
            '*'
        } else {
            UNREAD
        }
    })
}

/// `w` read as a directory to change to: an expansion of unknown depth is dropped, as if empty.
pub(super) fn dir_path(w: &Word) -> String {
    path(w).replace(UNREAD, "")
}

/// `path` has an unknown part (`UNREAD`) below one of the `roots` parts, or is a relative path
/// (one that doesn't start with a char of `absolute`) from a working directory in a protected
/// path: the shell or the write may land anywhere in it, or outside it.
pub(super) fn unread_below_protected(
    path: &str,
    roots: &[&str],
    absolute: [char; 2],
    from: Cwd,
) -> bool {
    if !path.contains(UNREAD) {
        return false;
    }
    components(path)
        .into_iter()
        .take_while(|p| !p.contains(UNREAD))
        .any(|p| roots.contains(&p))
        || (!path.starts_with(absolute) && (from.hooks.is_some() || from.ns.is_some()))
}

/// `dir` (the word `w` read as a path to change to) loses track of a protected working
/// directory: it has an unknown part below a protected path (`.ns`, `.githooks` or `.git`), or
/// it is relative to a working directory that is protected and has any expansion in it. An
/// expansion can be `.` or `..`, so even a whole-part one does not make a known directory
/// below a protected working directory.
pub(super) fn cd_loses_protected_cwd(w: &Word, from: Cwd) -> bool {
    let dir = path(w);
    let protected_cwd = from.hooks.is_some() || from.ns.is_some();
    (protected_cwd && !w.expansions.is_empty() && !dir.starts_with(['/', '~']))
        || unread_below_protected(&dir, &[".ns", ".githooks", ".git"], ['/', '~'], from)
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
    let hit = |a: &&Word| scan.touches_hooks(&path(a));
    let found = match prog {
        "rm" | "rmdir" | "unlink" | "shred" | "mv" | "chmod" | "chown" | "truncate" | "tee"
        | "ln" | "touch" => args.iter().find(hit),
        "cp" | "install" if has_target_directory_flag(args) => args.iter().find(hit),
        "cp" | "install" | "rsync" => args.last().filter(hit),
        "dd" => args.iter().find(|a| {
            path(a)
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
        if let Some(dir) = args.iter().find(|a| scan.touches_hooks(&path(a))) {
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
