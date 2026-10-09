//! Human-review regions: lines fenced by a start marker (`ns:human-review start`, optionally
//! followed by `: <reason>`) and an end marker (`ns:human-review end`), in any comment syntax. A merge that changes a line inside one needs a
//! human, and `ns check-markers` rejects unbalanced or nested markers.

use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;
use serde_json::json;

use crate::error::SfError;
use crate::git;

const TAG: &str = "ns:human-review";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Marker {
    Start(Option<String>),
    End,
}

/// The marker on a line. The keyword must be followed by the end of the line, whitespace or `:`,
/// so prose that quotes a marker in backticks is not one.
fn marker(line: &str) -> Option<Marker> {
    let mut rest = line;
    while let Some(i) = rest.find(TAG) {
        rest = &rest[i + TAG.len()..];
        let word = rest.trim_start_matches([' ', '\t']);
        for (kw, start) in [("start", true), ("end", false)] {
            let Some(after) = word.strip_prefix(kw) else {
                continue;
            };
            if !(after.is_empty() || after.starts_with([' ', '\t', ':', '\r'])) {
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

/// Regions in a text, read leniently so a bad file still guards its code: a nested start
/// extends the open region, a stray end is ignored, and an unclosed start runs to the last line.
pub fn regions(text: &str) -> Vec<Region> {
    let mut out = Vec::new();
    let mut open: Option<Region> = None;
    let mut last = 0;
    for (i, line) in text.lines().enumerate() {
        last = i + 1;
        match (marker(line), open.as_mut()) {
            (Some(Marker::Start(reason)), None) => {
                open = Some(Region {
                    start: last,
                    end: last,
                    reason,
                })
            }
            (Some(Marker::End), Some(r)) => {
                r.end = last;
                out.extend(open.take());
            }
            _ => {}
        }
    }
    if let Some(mut r) = open {
        r.end = last;
        out.push(r);
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarkerError {
    pub line: usize,
    pub message: String,
}

/// Unbalanced or nested markers.
pub fn check(text: &str) -> Vec<MarkerError> {
    let mut errs = Vec::new();
    let mut open: Option<usize> = None;
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        match (marker(line), open) {
            (Some(Marker::Start(_)), Some(o)) => errs.push(MarkerError {
                line: n,
                message: format!("{TAG} start nested inside the region opened on line {o}"),
            }),
            (Some(Marker::Start(_)), None) => open = Some(n),
            (Some(Marker::End), None) => errs.push(MarkerError {
                line: n,
                message: format!("{TAG} end with no start"),
            }),
            (Some(Marker::End), Some(_)) => open = None,
            (None, _) => {}
        }
    }
    if let Some(o) = open {
        errs.push(MarkerError {
            line: o,
            message: format!("{TAG} start with no end"),
        });
    }
    errs
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

/// Regions touched between two commits of the repo at `dir`, one `describe` line each.
pub fn touched_between(dir: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    let names = git::run(
        dir,
        &["diff", "--no-renames", "--name-only", "-z", base, head],
    )?;
    let mut out = Vec::new();
    for path in names.split('\0').filter(|p| !p.is_empty()) {
        let show =
            |rev: &str| git::run(dir, &["show", &format!("{rev}:{path}")]).unwrap_or_default();
        let (old, new) = (show(base), show(head));
        if !has_markers(&old) && !has_markers(&new) {
            continue;
        }
        let diff = git::run(
            dir,
            &[
                "diff",
                "--no-renames",
                "--no-ext-diff",
                "-U0",
                base,
                head,
                "--",
                path,
            ],
        )?;
        if let Some(r) = touched(&old, &new, &hunks(&diff)) {
            out.push(describe(path, &r));
        }
    }
    Ok(out)
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
mod tests {
    use super::*;

    /// `@start` and `@end` spelled out, so this file holds no real markers.
    fn m(s: &str) -> String {
        s.replace("@start", &format!("{TAG} start"))
            .replace("@end", &format!("{TAG} end"))
            .replace("@tag", TAG)
    }

    /// One hunk spanning everything between the common prefix and the common suffix.
    fn diff(base: &str, head: &str) -> Vec<Hunk> {
        let (a, b): (Vec<&str>, Vec<&str>) = (base.lines().collect(), head.lines().collect());
        let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
        let max = a.len().min(b.len()) - pre;
        let suf = a
            .iter()
            .rev()
            .zip(b.iter().rev())
            .take(max)
            .take_while(|(x, y)| x == y)
            .count();
        if pre + suf == a.len() && a.len() == b.len() {
            return Vec::new();
        }
        vec![Hunk {
            old_start: pre + 1,
            old_len: a.len() - pre - suf,
            new_start: pre + 1,
            new_len: b.len() - pre - suf,
        }]
    }

    fn hit(base: &str, head: &str) -> Option<Region> {
        let (base, head) = (m(base), m(head));
        touched(&base, &head, &diff(&base, &head))
    }

    const BASE: &str = "\
fn a() {}
// @start: brake torque limits
const MAX: u32 = 5;
// @end
fn b() {}
";

    fn region(start: usize, end: usize, reason: Option<&str>) -> Region {
        Region {
            start,
            end,
            reason: reason.map(String::from),
        }
    }

    #[test]
    fn markers_in_every_comment_syntax() {
        for (start, end) in [
            ("// @start", "// @end"),
            ("# @start", "# @end"),
            ("-- @start", "-- @end"),
            ("/* @start */", "/* @end */"),
            ("<!-- @start -->", "<!-- @end -->"),
        ] {
            let text = m(&format!("x\n{start}\ny\n{end}\nz\n"));
            assert_eq!(regions(&text), [region(2, 4, None)], "{start}");
            assert!(check(&text).is_empty(), "{start}");
        }
    }

    #[test]
    fn reasons_are_read_without_the_comment_closer() {
        let reason = |line: &str| match marker(&m(line)) {
            Some(Marker::Start(r)) => r,
            other => panic!("{line}: {other:?}"),
        };
        for (line, want) in [
            ("// @start: brake torque", "brake torque"),
            ("# @start: watchdog", "watchdog"),
            ("-- @start: billing ledger", "billing ledger"),
            ("/* @start: irq table */", "irq table"),
            ("<!-- @start: legal text -->", "legal text"),
        ] {
            assert_eq!(reason(line).as_deref(), Some(want), "{line}");
        }
        assert_eq!(reason("  // @start"), None);
        assert_eq!(reason("/* @start */"), None);
    }

    #[test]
    fn quoted_or_longer_words_are_not_markers() {
        assert_eq!(marker(&m("keep `@start` lines")), None);
        assert_eq!(marker(&m("// @endpoint")), None);
        assert_eq!(marker(&m("// @tag")), None);
    }

    #[test]
    fn edit_inside_a_region_is_touched() {
        let head = BASE.replace("= 5", "= 9");
        assert_eq!(
            hit(BASE, &head),
            Some(region(2, 4, Some("brake torque limits")))
        );
    }

    #[test]
    fn edit_outside_a_region_is_not_touched() {
        assert_eq!(hit(BASE, &BASE.replace("fn b() {}", "fn b() { 1; }")), None);
        assert_eq!(hit(BASE, &BASE.replace("fn a() {}", "fn a() { 1; }")), None);
        assert_eq!(hit(BASE, &format!("{BASE}fn c() {{}}\n")), None);
        assert_eq!(hit(BASE, BASE), None);
    }

    #[test]
    fn removing_a_marker_is_touched() {
        assert!(hit(BASE, &BASE.replace("// @end\n", "")).is_some());
        let head = BASE
            .replace("// @start: brake torque limits\n", "")
            .replace("// @end\n", "");
        assert_eq!(
            hit(BASE, &head).unwrap().reason.as_deref(),
            Some("brake torque limits")
        );
    }

    #[test]
    fn adding_a_marker_is_touched() {
        let head = BASE.replace("fn b() {}", "// @start: new\nfn b() {}\n// @end");
        assert_eq!(hit(BASE, &head).unwrap().reason.as_deref(), Some("new"));
        assert_eq!(
            hit("fn a() {}\n", "fn a() {}\n// @end\n"),
            Some(region(2, 2, None))
        );
    }

    #[test]
    fn moving_a_region_is_touched() {
        let head = "\
// @start: brake torque limits
fn a() {}
const MAX: u32 = 5;
// @end
fn b() {}
";
        assert!(hit(BASE, head).is_some());
    }

    #[test]
    fn edit_inside_a_region_only_in_the_base_is_touched() {
        let head = "fn a() {}\nconst MAX: u32 = 9;\nfn b() {}\n";
        // Only base lines are listed as changed, so the base side alone must catch it.
        let h = [Hunk {
            old_start: 3,
            old_len: 1,
            new_start: 2,
            new_len: 0,
        }];
        assert_eq!(
            touched(&m(BASE), head, &h),
            Some(region(2, 4, Some("brake torque limits")))
        );
    }

    #[test]
    fn deleted_and_added_files_with_markers_are_touched() {
        assert!(hit(BASE, "").is_some());
        assert!(hit("", BASE).is_some());
    }

    #[test]
    fn unbalanced_and_nested_markers_are_reported() {
        let lines = |text: &str| check(&m(text)).iter().map(|e| e.line).collect::<Vec<_>>();
        assert_eq!(lines(BASE), Vec::<usize>::new());
        assert_eq!(lines("# @start\nx\n"), [1]);
        assert_eq!(lines("x\n# @end\n"), [2]);
        assert_eq!(lines("# @start\n# @start\n# @end\n"), [2]);
        let e = &check(&m("# @start\n# @start\n# @end\n"))[0];
        assert!(e.message.contains("nested"), "{e:?}");
    }

    #[test]
    fn a_bad_file_still_guards_its_code() {
        let text = m("a\n# @start: r\nb\n# @start\nc\n");
        assert_eq!(regions(&text), [region(2, 5, Some("r"))]);
    }

    #[test]
    fn hunk_headers_parse() {
        let d = "--- a/x\n+++ b/x\n@@ -3 +3 @@ fn a\n-x\n+y\n@@ -10,0 +11,2 @@\n+p\n+q\n";
        let h = |old_start, old_len, new_start, new_len| Hunk {
            old_start,
            old_len,
            new_start,
            new_len,
        };
        assert_eq!(hunks(d), [h(3, 1, 3, 1), h(10, 0, 11, 2)]);
    }

    #[test]
    fn describe_names_file_lines_and_reason() {
        let r = regions(&m(BASE)).remove(0);
        assert_eq!(
            describe("src/brake.rs", &r),
            "src/brake.rs:2-4: brake torque limits"
        );
    }
}
