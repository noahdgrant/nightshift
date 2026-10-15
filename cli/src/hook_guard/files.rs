//! The paths the guard protects, named literally, and the file commands that write to them.

use super::words::Word;
use super::PROTECTED;
use crate::pin;

/// `path` names a protected path: something in `.githooks/` or `.git/hooks`, `.git/config` or a
/// `config.worktree`, a Claude Code settings file (`.claude/settings*.json`, which can turn the
/// hook off), the unit's skill snapshot `.ns/<unit>/skills`, the phase settings file (by name,
/// or as `$NS_PHASE_SETTINGS`), or a path through a `.claude/skills` or `.agents/skills` that
/// is a symlink into a snapshot. A relative path is read from where the guard runs.
pub(super) fn is_protected(path: &str) -> bool {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    parts.contains(&".githooks")
        || parts.windows(2).any(|p| {
            (p[0] == ".git" && matches!(p[1], "hooks" | "config"))
                || (p[0] == ".claude" && p[1].starts_with("settings") && p[1].ends_with(".json"))
        })
        || parts
            .last()
            .is_some_and(|l| *l == "config.worktree" || *l == pin::SETTINGS_FILE)
        || path.contains(pin::SETTINGS_VAR)
        || parts
            .windows(3)
            .any(|p| p[0] == pin::NS_DIR && p[2] == pin::SKILLS_DIR)
        || pin::LINKS
            .iter()
            .any(|link| is_under_snapshot_link(path, &parts, link))
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

/// Commands that write, move or delete every path they are given.
const WRITE_ALL: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "mv", "tee", "touch", "chmod", "chown", "truncate", "mkdir",
];

/// Commands that write only their last path: the others are read.
const WRITE_LAST: &[&str] = &["cp", "ln", "install"];

/// Why the command `prog args` writes a protected path it names.
pub(super) fn writes_protected(prog: &str, args: &[Word]) -> Option<String> {
    let paths: Vec<&Word> = args.iter().filter(|a| !a.text.starts_with('-')).collect();
    let written: Vec<&Word> = match prog {
        p if WRITE_ALL.contains(&p) => paths,
        p if WRITE_LAST.contains(&p) => paths.last().copied().into_iter().collect(),
        "sed" if args.iter().any(|a| in_place(&a.text)) => paths,
        _ => return None,
    };
    written
        .into_iter()
        .find(|a| is_protected(&a.text))
        .map(|a| format!("{prog} {} in {PROTECTED}", a.text))
}

/// `sed`'s `-i`, alone, with a suffix, in a cluster of short flags, or as `--in-place`.
fn in_place(arg: &str) -> bool {
    arg.starts_with("--in-place")
        || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('i'))
}
