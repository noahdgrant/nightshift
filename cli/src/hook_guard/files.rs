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

/// A command that goes up from a phase's `TMPDIR` to the unit's `phase-settings.json`.
/// `TMPDIR` is `.git/ns/units/<unit>/tmp/<phase>-<n>`, so two `..` after it land on that file.
/// The lexer drops variable names and does not follow `cd $TMPDIR`, so this reads the command's
/// words. It denies a word that has `TMPDIR` and a `..` part, and, after a `cd` or `pushd` whose
/// argument has `TMPDIR`, any later word with a `..` part or named like the settings file. A
/// `..` elsewhere in the command (`cd .. && cargo test > $TMPDIR/out.log`) is fine.
pub(super) fn tmpdir_reaches_settings(command: &str) -> Option<String> {
    let mut in_tmp = false;
    for segment in segments(command) {
        let segment = segment.replace(['\'', '"', '\\'], "");
        let words: Vec<&str> = segment
            .split(|c: char| c.is_whitespace() || "<>=".contains(c))
            .filter(|w| !w.is_empty())
            .collect();
        for (i, word) in words.iter().enumerate() {
            let up = word.split('/').any(|p| p == "..");
            if up && word.contains("TMPDIR") {
                return Some(format!(
                    "{word} goes up from $TMPDIR, a phase's scratch directory ({PROTECTED})"
                ));
            }
            if in_tmp && (up || word.rsplit('/').next() == Some(pin::SETTINGS_FILE)) {
                return Some(format!(
                    "{word} after a cd into $TMPDIR could reach the phase settings ({PROTECTED})"
                ));
            }
            if matches!(word.trim_start_matches('('), "cd" | "pushd")
                && words[i + 1..].iter().any(|w| w.contains("TMPDIR"))
            {
                in_tmp = true;
            }
        }
    }
    None
}

/// The parts of `command` between `;`, `&`, `|`, newlines and backticks.
fn segments(command: &str) -> impl Iterator<Item = &str> {
    command.split([';', '&', '|', '\n', '`'])
}

/// Words that run the command after them.
const WRAPPERS: &[&str] = &[
    "command", "env", "xargs", "sudo", "exec", "nohup", "time", "nice", "builtin", "then", "do",
    "else", "!", "doas", "setsid", "timeout", "stdbuf",
];

/// Short option letters and long options with which `cp` makes a link or copies one as a link.
const CP_LINK_SHORT: &str = "slarRdP";
const CP_LINK_LONG: &[&str] = &[
    "--symbolic-link",
    "--link",
    "--archive",
    "--recursive",
    "--no-dereference",
];
const RSYNC_LINK_SHORT: &str = "laH";
const RSYNC_LINK_LONG: &[&str] = &["--links", "--archive", "--hard-links", "--safe-links"];

/// A command that can plant a link, with an argument that is in `ns/units` or mentions
/// `TMPDIR`. A link in a phase's `TMPDIR` that points at the unit's `phase-settings.json`, written
/// through later, would reach past the `tmp` exception, whatever the link-maker is called
/// (`ln`, `link`, `cp -s`, `install`, `rsync -a`) and however it is reached (`/bin/ln`, `\ln`,
/// `command ln`, `env ln`, `xargs ln`). A script the phase writes is out of reach.
pub(super) fn links_into_unit(command: &str) -> Option<String> {
    let units = format!("{}/{}", pin::COMMON_NS, pin::COMMON_UNITS);
    for segment in segments(command) {
        let tokens: Vec<String> = segment
            .split_whitespace()
            .map(|t| t.replace(['\'', '"', '\\'], ""))
            .collect();
        let (mut at_command, mut wrapped) = (true, false);
        for (i, token) in tokens.iter().enumerate() {
            let bare = token.trim_start_matches(['(', '$', '{']);
            if bare.len() != token.len() {
                at_command = true;
            }
            let name = bare.rsplit('/').next().unwrap_or(bare);
            if at_command || wrapped {
                let rest = &tokens[i + 1..];
                if makes_links(name, rest) {
                    if let Some(arg) = rest.iter().find(|a| {
                        a.contains("TMPDIR") || a.contains(&units) || a.contains(pin::SETTINGS_FILE)
                    }) {
                        return Some(format!(
                            "{name} {arg}: a link out of a phase's scratch directory could reach {PROTECTED}"
                        ));
                    }
                }
                if WRAPPERS.contains(&name) {
                    wrapped = true;
                } else if !(bare.starts_with('-') || bare.contains('=')) {
                    at_command = false;
                }
            }
        }
    }
    None
}

/// `name` run with `args` makes a link.
fn makes_links(name: &str, args: &[String]) -> bool {
    let has_flag = |short: &str, long: &[&str]| {
        args.iter().any(|a| {
            if a.starts_with("--") {
                long.iter().any(|l| a.split('=').next() == Some(l))
            } else {
                a.starts_with('-') && a.chars().skip(1).any(|c| short.contains(c))
            }
        })
    };
    match name {
        "ln" | "link" | "install" => true,
        "cp" => has_flag(CP_LINK_SHORT, CP_LINK_LONG),
        "rsync" => has_flag(RSYNC_LINK_SHORT, RSYNC_LINK_LONG),
        _ => false,
    }
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
        "rm" | "ln" | "rmdir" | "unlink" | "shred" | "mv" | "chmod" | "chown" | "truncate"
        | "tee" | "touch" => args.iter().find(hit),
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
