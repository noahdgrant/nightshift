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

/// Regions in a text, read leniently so a bad file still guards its code: a nested start
/// deepens the open region, which closes when its own end is reached, a stray end is ignored,
/// and an unclosed start runs to the last line.
pub fn regions(text: &str) -> Vec<Region> {
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
            (Some(Marker::Start(_)), Some((_, depth))) => *depth += 1,
            (Some(Marker::End), Some((r, depth))) => {
                *depth -= 1;
                if *depth == 0 {
                    r.end = last;
                    out.extend(open.take().map(|(r, _)| r));
                }
            }
            _ => {}
        }
    }
    if let Some((mut r, _)) = open {
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

/// One changed file between two commits.
enum Change {
    Added,
    Deleted,
    Modified,
}

fn changes(dir: &Path, base: &str, head: &str) -> Result<Vec<(String, Change)>> {
    let out = git::run(
        dir,
        &[
            "--literal-pathspecs",
            "diff",
            "--no-renames",
            "--name-status",
            "-z",
            base,
            head,
        ],
    )?;
    let mut parts = out.split('\0').filter(|p| !p.is_empty());
    let mut list = Vec::new();
    while let Some(status) = parts.next() {
        let path = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("unexpected git diff --name-status output"))?;
        let change = match status.chars().next() {
            Some('A') => Change::Added,
            Some('D') => Change::Deleted,
            _ => Change::Modified,
        };
        list.push((path.to_string(), change));
    }
    Ok(list)
}

/// Regions touched between two commits of the repo at `dir`, one `describe` line each. A git
/// failure is an error, never "nothing touched". A changed file with markers whose diff shows no
/// hunks (binary, `-diff`, mode change) counts as wholly touched.
pub fn touched_between(dir: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for (path, change) in changes(dir, base, head)? {
        let show = |rev: &str| {
            git::run(
                dir,
                &["--literal-pathspecs", "show", &format!("{rev}:{path}")],
            )
        };
        let old = match change {
            Change::Added => String::new(),
            _ => show(base)?,
        };
        let new = match change {
            Change::Deleted => String::new(),
            _ => show(head)?,
        };
        if !has_markers(&old) && !has_markers(&new) {
            continue;
        }
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
                &path,
            ],
        )?;
        let hunks = hunks(&diff);
        let region = if hunks.is_empty() {
            let end = old.lines().count().max(new.lines().count()).max(1);
            Some(Region {
                start: 1,
                end,
                reason: None,
            })
        } else {
            touched(&old, &new, &hunks)
        };
        if let Some(r) = region {
            out.push(describe(&path, &r));
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
    fn a_nested_start_does_not_close_the_outer_region_early() {
        let text = m("@start\n@start\na\n@end\nSAFETY=5\n@end\nz\n");
        assert_eq!(regions(&text), [region(1, 6, None)]);
        let stray = m("@end\n@start\na\n@end\n@end\n");
        assert_eq!(regions(&stray), [region(2, 4, None)]);
    }

    #[test]
    fn regions_and_check_read_one_nested_file_differently() {
        let text = m("@end\n@start: r\n@start\na\n@end\nb\n@end\n");
        assert_eq!(regions(&text), [region(2, 7, Some("r"))]);
        let errs: Vec<_> = check(&text)
            .into_iter()
            .map(|e| (e.line, e.message))
            .collect();
        assert_eq!(
            errs,
            [
                (1, format!("{TAG} end with no start")),
                (
                    3,
                    format!("{TAG} start nested inside the region opened on line 2")
                ),
                (7, format!("{TAG} end with no start")),
            ]
        );
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

    #[test]
    fn closers_glued_to_the_keyword_are_markers() {
        assert_eq!(marker(&m("/* @start*/")), Some(Marker::Start(None)));
        assert_eq!(marker(&m("<!-- @end-->")), Some(Marker::End));
        assert_eq!(
            marker(&m("<!-- @start: why-->")),
            Some(Marker::Start(Some("why".into())))
        );
        assert_eq!(marker(&m("keep \"@start\" lines")), None);
        assert_eq!(marker(&m("// @start-ish")), None);
        assert_eq!(marker(&m("// @end_of")), None);
    }

    #[test]
    fn multi_hunk_diff_around_an_untouched_region_is_not_touched() {
        let base = m("a\nb\n// @start\nx\n// @end\nc\nd\n");
        let hunks = [
            Hunk {
                old_start: 2,
                old_len: 1,
                new_start: 2,
                new_len: 1,
            },
            Hunk {
                old_start: 6,
                old_len: 1,
                new_start: 6,
                new_len: 2,
            },
        ];
        assert_eq!(touched(&base, &base, &hunks), None);
        let inside = [
            hunks[0],
            Hunk {
                old_start: 4,
                old_len: 1,
                new_start: 4,
                new_len: 1,
            },
        ];
        assert_eq!(touched(&base, &base, &inside), Some(region(3, 5, None)));
    }

    #[test]
    fn tracked_docs_hold_no_live_markers() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let files = git::run(&root, &["ls-files", "-z", "docs"]).unwrap();
        for f in files.split('\0').filter(|f| f.ends_with(".md")) {
            let text = std::fs::read_to_string(root.join(f)).unwrap();
            assert_eq!(regions(&text), [], "{f}");
        }
    }

    mod repo {
        use super::*;
        use std::fs;

        fn git_in(dir: &Path, args: &[&str]) {
            git::run(dir, args).unwrap();
        }

        fn init() -> tempfile::TempDir {
            let t = tempfile::tempdir().unwrap();
            for args in [
                &["init", "-q"][..],
                &["config", "user.email", "t@example.com"],
                &["config", "user.name", "t"],
                &["config", "commit.gpgsign", "false"],
            ] {
                git_in(t.path(), args);
            }
            t
        }

        fn commit(dir: &Path, msg: &str) -> String {
            git_in(dir, &["add", "-A"]);
            git_in(dir, &["commit", "-q", "-m", msg]);
            git::run(dir, &["rev-parse", "HEAD"]).unwrap()
        }

        fn write(dir: &Path, name: &str, text: &str) {
            fs::write(dir.join(name), m(text)).unwrap();
        }

        const FILE: &str = "a\n// @start: limits\nx = 1\n// @end\nb\n";

        #[test]
        fn binary_marked_files_cannot_hide_an_edit() {
            let t = init();
            write(t.path(), ".gitattributes", "*.c -diff\n");
            write(t.path(), "f.c", FILE);
            let base = commit(t.path(), "base");
            write(t.path(), "f.c", &FILE.replace("x = 1", "x = 2"));
            let head = commit(t.path(), "head");
            let got = touched_between(t.path(), &base, &head).unwrap();
            assert_eq!(got.len(), 1, "{got:?}");
            assert!(got[0].starts_with("f.c:"), "{got:?}");
        }

        #[test]
        fn nul_bytes_cannot_hide_an_edit() {
            let t = init();
            write(t.path(), "g.c", &format!("\0{FILE}"));
            let base = commit(t.path(), "base");
            write(
                t.path(),
                "g.c",
                &format!("\0{}", FILE.replace("x = 1", "x = 2")),
            );
            let head = commit(t.path(), "head");
            assert_eq!(touched_between(t.path(), &base, &head).unwrap().len(), 1);
        }

        #[test]
        fn glob_characters_in_a_filename_are_literal() {
            let t = init();
            write(t.path(), "a[1]*.c", FILE);
            let base = commit(t.path(), "base");
            write(t.path(), "a[1]*.c", &FILE.replace("x = 1", "x = 2"));
            let head = commit(t.path(), "head");
            let got = touched_between(t.path(), &base, &head).unwrap();
            assert_eq!(got, ["a[1]*.c:2-4: limits"]);
        }

        #[test]
        fn added_and_deleted_files_with_markers_are_touched() {
            let t = init();
            write(t.path(), "keep.txt", "k\n");
            write(t.path(), "gone.c", FILE);
            let base = commit(t.path(), "base");
            fs::remove_file(t.path().join("gone.c")).unwrap();
            write(t.path(), "new.c", FILE);
            let head = commit(t.path(), "head");
            let mut got = touched_between(t.path(), &base, &head).unwrap();
            got.sort();
            assert_eq!(got, ["gone.c:2-4: limits", "new.c:2-4: limits"]);
        }

        #[test]
        fn edits_outside_a_region_pass_and_git_errors_propagate() {
            let t = init();
            write(t.path(), "f.c", FILE);
            let base = commit(t.path(), "base");
            write(
                t.path(),
                "f.c",
                &FILE.replace("a\n", "a2\n").replace("b\n", "b2\n"),
            );
            let head = commit(t.path(), "head");
            assert!(touched_between(t.path(), &base, &head).unwrap().is_empty());
            assert!(touched_between(t.path(), &base, "no-such-rev").is_err());
            assert!(touched_between(&t.path().join("missing"), &base, &head).is_err());
        }

        #[test]
        fn a_mode_only_change_touches_the_whole_file() {
            let t = init();
            write(t.path(), "f.c", FILE);
            let base = commit(t.path(), "base");
            git_in(t.path(), &["update-index", "--chmod=+x", "f.c"]);
            git_in(t.path(), &["commit", "-q", "-m", "mode"]);
            let head = git::run(t.path(), &["rev-parse", "HEAD"]).unwrap();
            let got = touched_between(t.path(), &base, &head).unwrap();
            let whole = Region {
                start: 1,
                end: FILE.lines().count(),
                reason: None,
            };
            assert_eq!(got, [describe("f.c", &whole)]);
        }

        #[test]
        fn deleting_only_the_end_marker_is_touched() {
            let t = init();
            write(t.path(), "f.c", FILE);
            let base = commit(t.path(), "base");
            write(t.path(), "f.c", &FILE.replace("// @end\n", ""));
            let head = commit(t.path(), "head");
            let got = touched_between(t.path(), &base, &head).unwrap();
            assert_eq!(got.len(), 1, "{got:?}");
            assert!(got[0].starts_with("f.c:"), "{got:?}");
        }

        #[test]
        fn an_unreadable_side_is_an_error_not_an_absent_file() {
            let t = init();
            write(t.path(), "f.c", FILE);
            let base = commit(t.path(), "base");
            write(t.path(), "f.c", &FILE.replace("x = 1", "x = 2"));
            commit(t.path(), "head");
            let blob = git::run(t.path(), &["rev-parse", "HEAD:f.c"]).unwrap();
            let loose = t
                .path()
                .join(".git/objects")
                .join(&blob[..2])
                .join(&blob[2..]);
            fs::remove_file(loose).unwrap();
            assert!(touched_between(t.path(), &base, "HEAD").is_err());
        }
    }
}
