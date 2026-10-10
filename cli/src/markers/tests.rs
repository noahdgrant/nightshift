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

    /// `touched_between`, and how many git processes it started.
    fn touched_counting(dir: &Path, base: &str, head: &str) -> (Vec<String>, usize) {
        git::reset_spawns();
        let got = touched_between(dir, base, head, None).unwrap();
        (got, git::spawns())
    }

    #[test]
    fn unmarked_files_cost_no_git_call_each() {
        let t = init();
        let names = ["a.c", "b.c", "c d.c", "e.c", "f.c", "g.c"];
        for n in names {
            write(t.path(), n, "a\n");
        }
        let base = commit(t.path(), "base");
        for n in names {
            write(t.path(), n, "b\n");
        }
        let head = commit(t.path(), "head");
        assert_eq!(touched_counting(t.path(), &base, &head), (vec![], 3));
    }

    #[test]
    fn markers_on_one_side_only_are_touched() {
        let t = init();
        let plain = "a\nx = 1\nb\n";
        write(t.path(), "was.c", FILE);
        write(t.path(), "now.c", plain);
        let base = commit(t.path(), "base");
        write(t.path(), "was.c", plain);
        write(t.path(), "now.c", FILE);
        let head = commit(t.path(), "head");
        let mut got = touched_between(t.path(), &base, &head, None).unwrap();
        got.sort();
        assert_eq!(got, ["now.c:2-4: limits", "was.c:2-4: limits"]);
    }

    /// A fence that lands on the tip after the PR branched is on neither the base nor the head.
    #[test]
    fn a_fence_added_at_the_tip_touches_the_whole_file() {
        let t = init();
        let plain = "a\nx = 1\nb\n";
        write(t.path(), "f.c", plain);
        write(t.path(), "g.c", plain);
        let base = commit(t.path(), "base");
        write(t.path(), "f.c", FILE);
        let tip = commit(t.path(), "fence on the default branch");
        git_in(t.path(), &["checkout", "-q", "-b", "pr", &base]);
        write(t.path(), "f.c", &plain.replace("x = 1", "x = 2"));
        let head = commit(t.path(), "pr edits f.c");
        assert_eq!(
            touched_between(t.path(), &base, &head, Some(&tip)).unwrap(),
            ["f.c:1-5"]
        );
        assert!(touched_between(t.path(), &base, &head, None)
            .unwrap()
            .is_empty());
        git_in(t.path(), &["checkout", "-q", "-b", "other", &base]);
        write(t.path(), "g.c", &plain.replace("x = 1", "x = 2"));
        let head = commit(t.path(), "pr edits g.c");
        assert!(touched_between(t.path(), &base, &head, Some(&tip))
            .unwrap()
            .is_empty());
        assert!(touched_between(t.path(), &base, &head, Some("no-such-rev")).is_err());
    }

    #[test]
    fn a_subdirectory_sees_marked_files_across_the_repo() {
        let t = init();
        fs::create_dir(t.path().join("d")).unwrap();
        for n in ["top.c", "d/x y[1]*.c"] {
            write(t.path(), n, FILE);
        }
        let base = commit(t.path(), "base");
        for n in ["top.c", "d/x y[1]*.c"] {
            write(t.path(), n, &FILE.replace("x = 1", "x = 2"));
        }
        let head = commit(t.path(), "head");
        let mut got = touched_between(&t.path().join("d"), &base, &head, None).unwrap();
        got.sort();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].starts_with("d/x y[1]*.c:"), "{got:?}");
        assert!(got[1].starts_with("top.c:"), "{got:?}");
    }

    #[test]
    fn binary_marked_files_cannot_hide_an_edit() {
        let t = init();
        write(t.path(), ".gitattributes", "*.c -diff\n");
        write(t.path(), "f.c", FILE);
        let base = commit(t.path(), "base");
        write(t.path(), "f.c", &FILE.replace("x = 1", "x = 2"));
        let head = commit(t.path(), "head");
        let got = touched_between(t.path(), &base, &head, None).unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].starts_with("f.c:"), "{got:?}");
    }

    #[test]
    fn a_file_without_markers_is_never_touched() {
        let t = init();
        write(t.path(), "f.c", "a\nb\n");
        let base = commit(t.path(), "base");
        git_in(t.path(), &["update-index", "--chmod=+x", "f.c"]);
        git_in(t.path(), &["commit", "-q", "-m", "mode"]);
        let head = git::run(t.path(), &["rev-parse", "HEAD"]).unwrap();
        assert!(touched_between(t.path(), &base, &head, None)
            .unwrap()
            .is_empty());
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
        assert_eq!(
            touched_between(t.path(), &base, &head, None).unwrap().len(),
            1
        );
    }

    #[test]
    fn glob_characters_in_a_filename_are_literal() {
        let t = init();
        write(t.path(), "a[1]*.c", FILE);
        let base = commit(t.path(), "base");
        write(t.path(), "a[1]*.c", &FILE.replace("x = 1", "x = 2"));
        let head = commit(t.path(), "head");
        let got = touched_between(t.path(), &base, &head, None).unwrap();
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
        let mut got = touched_between(t.path(), &base, &head, None).unwrap();
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
        assert!(touched_between(t.path(), &base, &head, None)
            .unwrap()
            .is_empty());
        assert!(touched_between(t.path(), &base, "no-such-rev", None).is_err());
        assert!(touched_between(&t.path().join("missing"), &base, &head, None).is_err());
    }

    #[test]
    fn a_mode_only_change_touches_the_whole_file() {
        let t = init();
        write(t.path(), "f.c", FILE);
        let base = commit(t.path(), "base");
        git_in(t.path(), &["update-index", "--chmod=+x", "f.c"]);
        git_in(t.path(), &["commit", "-q", "-m", "mode"]);
        let head = git::run(t.path(), &["rev-parse", "HEAD"]).unwrap();
        let got = touched_between(t.path(), &base, &head, None).unwrap();
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
        let got = touched_between(t.path(), &base, &head, None).unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].starts_with("f.c:"), "{got:?}");
    }

    /// A repo holding one commit, to be added as a submodule.
    fn sub_repo() -> (tempfile::TempDir, String) {
        let s = init();
        write(s.path(), "s.txt", "1\n");
        let sha = commit(s.path(), "s1");
        (s, sha)
    }

    fn add_submodule(dir: &Path, sub: &Path) {
        let url = sub.to_str().unwrap();
        git_in(
            dir,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                url,
                "sub",
            ],
        );
    }

    fn bump_submodule(dir: &Path, sub: &Path) {
        write(sub, "s.txt", "2\n");
        let sha = commit(sub, "s2");
        let checkout = dir.join("sub");
        git_in(
            &checkout,
            &["-c", "protocol.file.allow=always", "fetch", "-q", "origin"],
        );
        git_in(&checkout, &["checkout", "-q", &sha]);
    }

    #[test]
    fn a_submodule_bump_touches_nothing() {
        let (s, _) = sub_repo();
        let t = init();
        write(t.path(), "f.c", FILE);
        add_submodule(t.path(), s.path());
        let base = commit(t.path(), "base");
        bump_submodule(t.path(), s.path());
        let head = commit(t.path(), "bump");
        assert!(touched_between(t.path(), &base, &head, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_submodule_bump_beside_a_touched_region_reports_the_region() {
        let (s, _) = sub_repo();
        let t = init();
        write(t.path(), "f.c", FILE);
        add_submodule(t.path(), s.path());
        let base = commit(t.path(), "base");
        bump_submodule(t.path(), s.path());
        write(t.path(), "f.c", &FILE.replace("x = 1", "x = 2"));
        let head = commit(t.path(), "head");
        let got = touched_between(t.path(), &base, &head, None).unwrap();
        assert_eq!(got, ["f.c:2-4: limits"]);
    }

    #[test]
    fn adding_or_removing_a_submodule_touches_nothing() {
        let (s, _) = sub_repo();
        let t = init();
        write(t.path(), "k.txt", "k\n");
        let base = commit(t.path(), "base");
        add_submodule(t.path(), s.path());
        let head = commit(t.path(), "add");
        assert!(touched_between(t.path(), &base, &head, None)
            .unwrap()
            .is_empty());
        assert!(touched_between(t.path(), &head, &base, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn replacing_a_marked_file_with_a_submodule_is_touched() {
        let (s, _) = sub_repo();
        let t = init();
        write(t.path(), "sub", FILE);
        let base = commit(t.path(), "base");
        git_in(t.path(), &["rm", "-q", "sub"]);
        add_submodule(t.path(), s.path());
        let head = commit(t.path(), "swap");
        assert_eq!(
            touched_between(t.path(), &base, &head, None).unwrap(),
            ["sub:2-4: limits"]
        );
        assert_eq!(
            touched_between(t.path(), &head, &base, None).unwrap(),
            ["sub:2-4: limits"]
        );
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
        assert!(touched_between(t.path(), &base, "HEAD", None).is_err());
    }

    /// git grep reports an unreadable blob on stderr but exits 1 when nothing else matches,
    /// and 0 when another file does.
    #[test]
    fn a_blob_git_grep_cannot_read_is_an_error() {
        for other_match in [false, true] {
            let t = init();
            write(t.path(), "f.c", "a\n");
            let base = commit(t.path(), "base");
            write(t.path(), "f.c", FILE);
            if other_match {
                write(t.path(), "g.c", FILE);
            }
            commit(t.path(), "head");
            let blob = git::run(t.path(), &["rev-parse", "HEAD:f.c"]).unwrap();
            let loose = t
                .path()
                .join(".git/objects")
                .join(&blob[..2])
                .join(&blob[2..]);
            fs::remove_file(loose).unwrap();
            let got = touched_between(t.path(), &base, "HEAD", None);
            assert!(got.is_err(), "other_match={other_match}: {got:?}");
        }
    }
}

mod grep_paths {
    use super::*;

    #[test]
    fn strips_the_rev_prefix_from_each_path() {
        let got = parse_grep_paths("HEAD", b"HEAD:a.c\0HEAD:d/b c.c\0").unwrap();
        let want: HashSet<String> = ["a.c", "d/b c.c"].map(String::from).into();
        assert_eq!(got, want);
    }

    #[test]
    fn empty_output_is_no_paths() {
        assert!(parse_grep_paths("HEAD", b"").unwrap().is_empty());
    }

    #[test]
    fn a_path_without_the_rev_prefix_is_an_error() {
        let err = parse_grep_paths("HEAD", b"HEAD:a.c\0b.c\0").unwrap_err();
        assert!(err.to_string().contains("unexpected git grep output: b.c"));
    }
}
