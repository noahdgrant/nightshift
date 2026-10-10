//! Human-review regions: lines fenced by a start marker (`ns:human-review start`, optionally
//! followed by `: <reason>`) and an end marker (`ns:human-review end`), in any comment syntax. A merge that changes a line inside one needs a
//! human, and `ns check-markers` rejects unbalanced or nested markers.

use std::collections::HashSet;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::json;

use crate::error::SfError;
use crate::git;

const TAG: &str = "ns:human-review";

/// Repo-root paths of the files at `rev` that hold a marker, from one `git grep` for the whole
/// tree. Exit 1 means no match. git grep reports an unreadable blob only on stderr, with exit 0
/// or 1, so any stderr output is an error too.
fn marked_at(dir: &Path, rev: &str) -> Result<HashSet<String>> {
    let out = git::command()
        .arg("-C")
        .arg(dir)
        .args([
            "--no-literal-pathspecs",
            "grep",
            "--full-name",
            "-l",
            "-z",
            "-F",
            "-e",
            TAG,
        ])
        .args([rev, "--", ":(top)"])
        .output()
        .context("failed to run git; is it installed and on PATH?")?;
    if !out.stderr.is_empty() || !matches!(out.status.code(), Some(0 | 1)) {
        anyhow::bail!(
            "git grep {TAG} {rev} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_grep_paths(rev, &out.stdout)
}

/// The paths in `git grep -l -z` output for `rev`, each printed as `<rev>:<path>`.
fn parse_grep_paths(rev: &str, stdout: &[u8]) -> Result<HashSet<String>> {
    let prefix = format!("{rev}:");
    String::from_utf8_lossy(stdout)
        .split_terminator('\0')
        .map(|p| {
            p.strip_prefix(&prefix)
                .map(String::from)
                .ok_or_else(|| anyhow::anyhow!("unexpected git grep output: {p}"))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Marker {
    Start(Option<String>),
    End,
}

/// Whether what follows `start` or `end` ends the word: the end of the line, or any character
/// except a letter, digit, `_`, `-`, a quote or a backtick. `-->` closes an HTML comment, so it is
/// allowed. Quoted prose and longer words (`endpoint`) are not markers.
fn follows_keyword(after: &str) -> bool {
    match after.chars().next() {
        None => true,
        Some('-') => after.starts_with("-->"),
        Some(c) => !(c.is_alphanumeric() || matches!(c, '_' | '`' | '\'' | '"')),
    }
}

/// The marker on a line, found by substring so any comment syntax works.
fn marker(line: &str) -> Option<Marker> {
    let mut rest = line;
    while let Some(i) = rest.find(TAG) {
        rest = &rest[i + TAG.len()..];
        let word = rest.trim_start_matches([' ', '\t']);
        for (kw, start) in [("start", true), ("end", false)] {
            let Some(after) = word.strip_prefix(kw) else {
                continue;
            };
            if !follows_keyword(after) {
                continue;
            }
            if !start {
                return Some(Marker::End);
            }
            let reason = after
                .trim_start()
                .strip_prefix(':')
                .map(|r| {
                    r.trim()
                        .trim_end_matches("*/")
                        .trim_end_matches("-->")
                        .trim()
                        .to_string()
                })
                .filter(|r| !r.is_empty());
            return Some(Marker::Start(reason));
        }
    }
    None
}

/// Lines `start..=end` (1-based), markers included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
    pub reason: Option<String>,
}

/// What one pass over a text's markers finds.
enum Event {
    Closed(Region),
    Nested { line: usize, open: usize },
    Stray(usize),
    Unclosed(Region),
}

/// How `scan` treats a start inside an open region.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nesting {
    /// The nested start deepens the open region, which closes at its own end.
    Deepen,
    /// The next end closes the open region.
    Flat,
}

/// Walk the lines once. An unclosed region runs to the last line.
fn scan(text: &str, nesting: Nesting) -> Vec<Event> {
    let mut out = Vec::new();
    let mut open: Option<(Region, usize)> = None;
    let mut last = 0;
    for (i, line) in text.lines().enumerate() {
        last = i + 1;
        match (marker(line), open.as_mut()) {
            (Some(Marker::Start(reason)), None) => {
                open = Some((
                    Region {
                        start: last,
                        end: last,
                        reason,
                    },
                    1,
                ))
            }
            (Some(Marker::Start(_)), Some((r, depth))) => {
                out.push(Event::Nested {
                    line: last,
                    open: r.start,
                });
                if nesting == Nesting::Deepen {
                    *depth += 1;
                }
            }
            (Some(Marker::End), None) => out.push(Event::Stray(last)),
            (Some(Marker::End), Some((r, depth))) => {
                *depth -= 1;
                if *depth == 0 {
                    r.end = last;
                    out.extend(open.take().map(|(r, _)| Event::Closed(r)));
                }
            }
            (None, _) => {}
        }
    }
    if let Some((mut r, _)) = open {
        r.end = last;
        out.push(Event::Unclosed(r));
    }
    out
}

/// Regions in a text, read leniently so a bad file still guards its code: a nested start
/// deepens the open region, which closes when its own end is reached, a stray end is ignored,
/// and an unclosed start runs to the last line.
pub fn regions(text: &str) -> Vec<Region> {
    scan(text, Nesting::Deepen)
        .into_iter()
        .filter_map(|e| match e {
            Event::Closed(r) | Event::Unclosed(r) => Some(r),
            Event::Nested { .. } | Event::Stray(_) => None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarkerError {
    pub line: usize,
    pub message: String,
}

/// Unbalanced or nested markers.
pub fn check(text: &str) -> Vec<MarkerError> {
    scan(text, Nesting::Flat)
        .into_iter()
        .filter_map(|e| {
            let (line, message) = match e {
                Event::Closed(_) => return None,
                Event::Nested { line, open } => (
                    line,
                    format!("{TAG} start nested inside the region opened on line {open}"),
                ),
                Event::Stray(line) => (line, format!("{TAG} end with no start")),
                Event::Unclosed(r) => (r.start, format!("{TAG} start with no end")),
            };
            Some(MarkerError { line, message })
        })
        .collect()
}

/// One `@@ -old_start,old_len +new_start,new_len @@` hunk header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
}

/// Hunk headers from a unified diff of one file.
pub fn hunks(diff: &str) -> Vec<Hunk> {
    let range = |s: &str| -> Option<(usize, usize)> {
        let (a, b) = s.split_once(',').unwrap_or((s, "1"));
        Some((a.parse().ok()?, b.parse().ok()?))
    };
    diff.lines()
        .filter_map(|l| {
            let mut parts = l.strip_prefix("@@ -")?.split_whitespace();
            let (old_start, old_len) = range(parts.next()?)?;
            let (new_start, new_len) = range(parts.next()?.strip_prefix('+')?)?;
            Some(Hunk {
                old_start,
                old_len,
                new_start,
                new_len,
            })
        })
        .collect()
}

/// The first region a diff touches: a removed line inside a base region, an added line inside a
/// head region, or an added or removed marker line.
pub fn touched(base: &str, head: &str, hunks: &[Hunk]) -> Option<Region> {
    let old = hunks
        .iter()
        .flat_map(|h| h.old_start..h.old_start + h.old_len);
    let new = hunks
        .iter()
        .flat_map(|h| h.new_start..h.new_start + h.new_len);
    touched_lines(base, old).or_else(|| touched_lines(head, new))
}

fn touched_lines(text: &str, mut changed: impl Iterator<Item = usize>) -> Option<Region> {
    let rs = regions(text);
    let lines: Vec<&str> = text.lines().collect();
    changed.find_map(|n| {
        if let Some(r) = rs.iter().find(|r| (r.start..=r.end).contains(&n)) {
            return Some(r.clone());
        }
        let reason = match marker(lines.get(n - 1)?)? {
            Marker::Start(reason) => reason,
            Marker::End => None,
        };
        Some(Region {
            start: n,
            end: n,
            reason,
        })
    })
}

/// Whether a text holds any marker; files without one are skipped cheaply.
pub fn has_markers(text: &str) -> bool {
    text.contains(TAG)
}

/// `<path>:<start>-<end>`, with the reason after a colon when there is one.
pub fn describe(path: &str, r: &Region) -> String {
    match &r.reason {
        Some(reason) => format!("{path}:{}-{}: {reason}", r.start, r.end),
        None => format!("{path}:{}-{}", r.start, r.end),
    }
}

/// One changed file between two commits: its path and whether each side holds a blob. A side
/// that is absent (`000000`) or a submodule commit (`160000`) holds no text, so no markers.
struct Change {
    path: String,
    old: bool,
    new: bool,
}

fn changes(dir: &Path, base: &str, head: &str) -> Result<Vec<Change>> {
    let out = git::run(
        dir,
        &[
            "--literal-pathspecs",
            "diff",
            "--no-renames",
            "--raw",
            "-z",
            base,
            head,
        ],
    )?;
    let blob = |mode: Option<&str>| !matches!(mode, Some("000000" | "160000"));
    let mut parts = out.split('\0').filter(|p| !p.is_empty());
    let mut list = Vec::new();
    while let Some(meta) = parts.next() {
        let mut modes = meta.trim_start_matches(':').split(' ');
        let (old, new) = (blob(modes.next()), blob(modes.next()));
        let path = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("unexpected git diff --raw output"))?;
        list.push(Change {
            path: path.to_string(),
            old,
            new,
        });
    }
    Ok(list)
}

/// Regions touched between two commits of the repo at `dir`, one `describe` line each. A git
/// failure is an error, never "nothing touched". A changed file with markers whose diff shows no
/// hunks (binary, `-diff`, mode change) counts as wholly touched. So does a changed file with
/// markers only at `tip`, the default branch's head, since its hunks map to neither side.
pub fn touched_between(
    dir: &Path,
    base: &str,
    head: &str,
    tip: Option<&str>,
) -> Result<Vec<String>> {
    let changes = changes(dir, base, head)?;
    let (old, new) = (marked_at(dir, base)?, marked_at(dir, head)?);
    let at_tip = match tip {
        Some(tip) => marked_at(dir, tip)?,
        None => HashSet::new(),
    };
    let mut out = Vec::new();
    for change in changes {
        let path = change.path.as_str();
        let region = if old.contains(path) || new.contains(path) {
            touched_file(dir, base, head, &change)?
        } else if let Some(tip) = tip.filter(|_| at_tip.contains(path)) {
            let text = git::run(
                dir,
                &["--literal-pathspecs", "show", &format!("{tip}:{path}")],
            )?;
            Some(Region {
                start: 1,
                end: text.lines().count(),
                reason: None,
            })
        } else {
            None
        };
        out.extend(region.map(|r| describe(path, &r)));
    }
    Ok(out)
}

/// The region one changed file touches between two commits, if any.
fn touched_file(dir: &Path, base: &str, head: &str, change: &Change) -> Result<Option<Region>> {
    let path = change.path.as_str();
    let show = |rev: &str| {
        git::run(
            dir,
            &["--literal-pathspecs", "show", &format!("{rev}:{path}")],
        )
    };
    let old = if change.old {
        show(base)?
    } else {
        String::new()
    };
    let new = if change.new {
        show(head)?
    } else {
        String::new()
    };
    let diff = git::run(
        dir,
        &[
            "--literal-pathspecs",
            "diff",
            "--no-renames",
            "--no-ext-diff",
            "--text",
            "-U0",
            base,
            head,
            "--",
            path,
        ],
    )?;
    let hunks = hunks(&diff);
    if hunks.is_empty() {
        let end = old.lines().count().max(new.lines().count()).max(1);
        return Ok(Some(Region {
            start: 1,
            end,
            reason: None,
        }));
    }
    Ok(touched(&old, &new, &hunks))
}

#[derive(Debug, Serialize)]
struct FileError {
    file: String,
    line: usize,
    message: String,
}

/// `ns check-markers [path]`: check every tracked text file under `path`.
pub fn cli(path: &Path, human: bool) -> Result<ExitCode> {
    if !path.is_dir() {
        return Err(SfError::usage(
            format!("{} is not a directory", path.display()),
            "ns check-markers .",
        )
        .into());
    }
    let files = git::run(path, &["ls-files", "-z"]).map_err(|_| {
        SfError::usage(
            format!("{} is not inside a git repository", path.display()),
            "ns check-markers path/to/repo",
        )
    })?;
    let mut errors = Vec::new();
    let mut checked = 0;
    for f in files.split('\0').filter(|f| !f.is_empty()) {
        let Ok(text) = std::fs::read_to_string(path.join(f)) else {
            continue;
        };
        checked += 1;
        if !has_markers(&text) {
            continue;
        }
        errors.extend(check(&text).into_iter().map(|e| FileError {
            file: f.to_string(),
            line: e.line,
            message: e.message,
        }));
    }
    if human {
        for e in &errors {
            println!("{}:{}: {}", e.file, e.line, e.message);
        }
        println!("{checked} files checked, {} errors", errors.len());
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"ok": errors.is_empty(), "files": checked, "errors": errors})
            )?
        );
    }
    Ok(if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

#[cfg(test)]
mod tests;
