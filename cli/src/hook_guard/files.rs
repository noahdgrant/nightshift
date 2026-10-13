//! The paths the guard protects and the file commands that write to them.

use std::cell::RefCell;
use std::path::Path;

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
/// (`in_phase_tmp`). Only writes are denied; reading
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
            && !in_phase_tmp(path))
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

thread_local! {
    /// The running phase's `TMPDIR` as given and with its parent resolved, while `check` runs.
    static PHASE_TMP: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Run `f` with `tmpdir` as the phase's own `TMPDIR` for `in_phase_tmp`. A `tmpdir` not of the
/// shape `ns run` gives (`is_phase_tmpdir`) exempts nothing.
pub(super) fn with_phase_tmp<T>(tmpdir: Option<&str>, f: impl FnOnce() -> T) -> T {
    let dirs: Vec<String> = tmpdir
        .filter(|d| is_phase_tmpdir(d))
        .map(|d| {
            let dir = Path::new(d);
            let real = dir
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .zip(dir.file_name())
                .map(|(p, name)| p.join(name).to_string_lossy().into_owned())
                .filter(|r| is_phase_tmpdir(r));
            std::iter::once(d.to_string()).chain(real).collect()
        })
        .unwrap_or_default();
    let outer = PHASE_TMP.replace(dirs);
    let out = f();
    PHASE_TMP.set(outer);
    out
}

/// `dir` is a plain absolute `.../ns/units/<unit>/tmp/<phase>-<n>`, where `ns run` puts a
/// phase's `TMPDIR`.
fn is_phase_tmpdir(dir: &str) -> bool {
    let parts = components(dir);
    let numbered = |name: &str| {
        name.rsplit_once('-').is_some_and(|(phase, n)| {
            !phase.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
        })
    };
    dir.starts_with('/')
        && is_plain(&parts)
        && matches!(
            parts.as_slice(),
            [.., ns, units, _, tmp, name]
                if *ns == pin::COMMON_NS && *units == pin::COMMON_UNITS
                    && *tmp == pin::TMP_DIR && numbered(name)
        )
}

/// `path` is the running phase's own `TMPDIR` or in it, by an absolute path with no `..` or
/// glob, which could reach past it.
pub(super) fn in_phase_tmp(path: &str) -> bool {
    let parts = components(path);
    path.starts_with('/')
        && is_plain(&parts)
        && PHASE_TMP.with_borrow(|dirs| dirs.iter().any(|d| parts.starts_with(&components(d))))
}

/// No part of a path is `..` or a glob.
fn is_plain(parts: &[&str]) -> bool {
    !parts
        .iter()
        .any(|p| *p == ".." || p.contains(['*', '?', '[']))
}

/// `path` (from `cwd` when relative) with its longest existing ancestor resolved, symlinks
/// followed, so a write through a link out of `TMPDIR` is checked where it lands. A path with
/// no existing ancestor comes back as given.
pub(super) fn resolved(path: &str, cwd: Option<&str>) -> String {
    let full = Path::new(cwd.unwrap_or("")).join(path);
    let mut base = full.as_path();
    let mut tail = Vec::new();
    loop {
        if let Ok(real) = base.canonicalize() {
            let real = tail.iter().rev().fold(real, |p, name| p.join(name));
            return real.to_string_lossy().into_owned();
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                base = parent;
            }
            _ => return full.to_string_lossy().into_owned(),
        }
    }
}

/// A command that goes up from a phase's `TMPDIR` to the unit's `phase-settings.json`.
/// `TMPDIR` is `.git/ns/units/<unit>/tmp/<phase>-<n>`, so two `..` after it land on that file.
/// The lexer drops variable names and does not follow `cd $TMPDIR`, so this reads the command's
/// words. It denies a word that has `TMPDIR` and a `..` part, and, while the shell may be in
/// `TMPDIR` (after a `cd` or `pushd` whose first non-option argument has `TMPDIR`), any word with
/// a `..` part or named like the settings file. A `cd` to a literal absolute or home path, or to
/// home, ends that; a `cd` to a relative path or an expansion (`cd .`, `cd "$PWD"`) may stay in
/// `TMPDIR`, so it does not. `cd -` goes back to where the `cd` before it left, and `popd` may.
/// The `)` that closes the `(` the `cd` was inside ends it too, whether a subshell or a `$(...)`.
/// A `..` elsewhere (`cd .. && cargo test > $TMPDIR/out.log`) is fine.
pub(super) fn tmpdir_reaches_settings(command: &str) -> Option<String> {
    let mut in_tmp = false;
    let mut was_in_tmp = false;
    let mut scopes: Vec<bool> = Vec::new();
    let mut cd_pending = false;
    for segment in segments(command) {
        let segment = segment.replace(['\'', '"', '\\'], "");
        let words: Vec<&str> = segment
            .split(|c: char| c.is_whitespace() || c == '=')
            .filter(|w| !w.is_empty())
            .collect();
        for word in &words {
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
            if cd_pending && (*word == "-" || !word.starts_with('-')) {
                let next = if word.contains("TMPDIR") {
                    true
                } else if *word == "-" {
                    was_in_tmp
                } else if word.starts_with(['/', '~']) && !word.contains(['$', '`']) {
                    false
                } else {
                    in_tmp
                };
                was_in_tmp = std::mem::replace(&mut in_tmp, next);
                cd_pending = false;
            }
            for c in word.chars() {
                match c {
                    '(' => scopes.push(in_tmp),
                    ')' => in_tmp = scopes.pop().unwrap_or(in_tmp),
                    _ => {}
                }
            }
            match word.trim_start_matches(['(', '$']) {
                "cd" | "pushd" => cd_pending = true,
                "popd" => in_tmp |= was_in_tmp,
                _ => {}
            }
        }
        if cd_pending {
            was_in_tmp = std::mem::replace(&mut in_tmp, false);
            cd_pending = false;
        }
    }
    None
}

/// The parts of `command` between `;`, `&`, `|`, newlines and backticks.
fn segments(command: &str) -> impl Iterator<Item = &str> {
    command.split([';', '&', '|', '\n', '`'])
}

/// A command that can plant a link or copy the phase settings. A link in a phase's `TMPDIR` that
/// points at the unit's `phase-settings.json`, written through later, would reach past the `tmp`
/// exception. Per segment, the guard fails closed: a token whose basename is a verb (`ln`, `link`,
/// `cp`, `rsync`, `install`) counts wherever it stands, so wrappers it does not know (`timeout 5
/// ln`), `sh -c 'ln ..'`, redirections and shell constructs cannot hide it. The one exception is
/// a segment whose command word (after assignments, keywords and redirections) is in
/// `NON_LINKING`: there `cargo test link` names a verb as a plain argument. Such a segment still
/// counts a verb that opens a `(` or `$(`. The other tokens of the segment are the arguments, so
/// a verb-named token is never its own argument (`touch $TMPDIR/link` passes):
/// - `ln` and `link` are denied when an argument mentions `TMPDIR`, an `ns/units` path component
///   or `phase-settings.json`. That includes `ln -s a $TMPDIR/l`, whose target is harmless: the
///   guard does not tell a safe target from an unsafe one, so it denies them all.
/// - `cp`, `rsync` and `install` are denied when an argument names an `ns/units` path component
///   or `phase-settings.json`, unless it is a plain path in the phase's own `TMPDIR`
///   (`in_phase_tmp`); `cp` also when it has a link flag (`-s`, `-l`, `--symbolic-link`,
///   `--link`) and an argument mentions `TMPDIR` or names a unit path.
/// - After a segment with a `cd` or `pushd` and a later `TMPDIR` token, `ln`, `link` and `cp` with
///   a link flag are denied whatever their arguments: a later `cd .` or `cd -` could still stand
///   in `TMPDIR`, so the guard does not follow where the shell is.
///
/// Out of reach: a script the phase writes (`python -c os.symlink`) and a path a shell builds
/// (`$(dirname $TMPDIR)`). A false deny remains for an unknown command with a verb-named argument
/// and `TMPDIR` (`mytool link $TMPDIR/x`), and for a link anywhere after a `cd` into `TMPDIR`.
pub(super) fn links_into_unit(command: &str) -> Option<String> {
    let mut entered_tmp = false;
    for segment in segments(command) {
        let tokens: Vec<String> = segment
            .split_whitespace()
            .map(|t| t.replace(['\'', '"', '\\'], ""))
            .collect();
        let safe = command_word(&tokens).is_some_and(|w| NON_LINKING.contains(&w));
        for (i, token) in tokens.iter().enumerate() {
            let bare = token.trim_start_matches(['(', '$']);
            let verb = bare.rsplit('/').next().unwrap_or(bare);
            let opens = token.starts_with(['(', '$']) || (i > 0 && tokens[i - 1].ends_with('('));
            if !matches!(verb, "ln" | "link" | "cp" | "rsync" | "install") || (safe && !opens) {
                continue;
            }
            let links = matches!(verb, "ln" | "link")
                || (verb == "cp" && tokens[i + 1..].iter().any(|a| is_link_flag(a)));
            if links && entered_tmp {
                return Some(format!(
                    "{verb} after a cd into $TMPDIR: a link out of a phase's scratch directory could reach {PROTECTED}"
                ));
            }
            let mut args = tokens
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, a)| a);
            let hit = if links {
                args.find(|a| a.contains("TMPDIR") || names_unit_or_settings(a))
            } else {
                args.find(|a| names_unit_or_settings(a) && !in_phase_tmp(a))
            };
            if let Some(arg) = hit {
                return Some(format!(
                    "{verb} {arg}: a link out of a phase's scratch directory could reach {PROTECTED}"
                ));
            }
        }
        entered_tmp |= tokens
            .iter()
            .position(|t| matches!(t.trim_start_matches(['(', '$']), "cd" | "pushd"))
            .is_some_and(|at| tokens[at + 1..].iter().any(|t| t.contains("TMPDIR")));
    }
    None
}

/// Commands that never make a link from their arguments, so a verb-named argument is plain text.
const NON_LINKING: [&str; 25] = [
    "cargo", "grep", "egrep", "fgrep", "rg", "git", "echo", "printf", "cat", "make", "npm", "npx",
    "pnpm", "yarn", "pytest", "ls", "head", "tail", "wc", "sort", "diff", "gh", "test", "which",
    "type",
];

/// The basename of the segment's command word: the first token that is not an assignment, a
/// shell keyword or brace, or a redirection with its target. `None` when the segment has none.
fn command_word(tokens: &[String]) -> Option<&str> {
    let mut i = 0;
    while let Some(token) = tokens.get(i) {
        i += 1;
        let op = token.trim_start_matches(|c: char| c.is_ascii_digit());
        if op.starts_with(['>', '<']) {
            if op.trim_start_matches(['>', '<']).is_empty() {
                i += 1;
            }
        } else if !is_command_prefix(token) {
            let bare = token.trim_start_matches(['(', '$']);
            return Some(bare.rsplit('/').next().unwrap_or(bare));
        }
    }
    None
}

/// `token` can stand before a command word: an assignment (`FOO=1`) or a shell keyword or brace.
fn is_command_prefix(token: &str) -> bool {
    matches!(
        token,
        "{" | "!" | "if" | "then" | "else" | "elif" | "do" | "while" | "until"
    ) || (!token.starts_with('-')
        && token.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_')
        }))
}

/// `arg` has an `ns/units` path component pair (not `runs/units`) or names the settings file.
fn names_unit_or_settings(arg: &str) -> bool {
    arg.contains(pin::SETTINGS_FILE)
        || arg
            .split('/')
            .collect::<Vec<_>>()
            .windows(2)
            .any(|p| p[0] == pin::COMMON_NS && p[1] == pin::COMMON_UNITS)
}

/// `arg` is a `cp` option that makes a symbolic or hard link: `-s`, `-l` (alone or among other
/// short letters, as in `-rs`), `--symbolic-link` or `--link`.
fn is_link_flag(arg: &str) -> bool {
    match arg.strip_prefix("--") {
        Some(long) => matches!(long, "symbolic-link" | "link"),
        None => arg.starts_with('-') && arg.chars().any(|c| matches!(c, 's' | 'l')),
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
        "rm" | "ln" | "rmdir" | "unlink" | "shred" | "mv" | "chmod" | "chown" | "truncate"
        | "tee" | "touch" => args.iter().find(hit),
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
