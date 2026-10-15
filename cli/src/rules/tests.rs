use super::*;

fn file(entries: &[(&str, &str)]) -> String {
    let violations: Vec<Value> = entries
        .iter()
        .map(|(snippet, message)| {
            json!({"tool": "t", "rule": "r", "file": "a.py", "message": message, "snippet": snippet})
        })
        .collect();
    json!({"version": 1, "violations": violations}).to_string()
}

#[test]
fn removal_only_holds_when_every_head_entry_has_its_own_base_entry() {
    let base = file(&[("x", "m"), ("y", "m"), ("y", "m")]);
    assert_eq!(removal_only(&base, &base), Ok(true));
    assert_eq!(
        removal_only(&base, &file(&[("y", "m"), ("x", "m")])),
        Ok(true)
    );
    assert_eq!(removal_only(&base, &file(&[])), Ok(true));
    assert_eq!(
        removal_only(&base, &file(&[("y", "m"), ("y", "m"), ("x", "m")])),
        Ok(true)
    );
    assert_eq!(
        removal_only(&base, &file(&[("x", "m"), ("x", "m")])),
        Ok(false)
    );
    assert_eq!(removal_only(&base, &file(&[("z", "m")])), Ok(false));
    let older = r#"{"version": 1, "violations": [{"tool": "t", "rule": "r", "file": "a.py", "line": 3, "message": "m", "snippet": "x"}]}"#;
    assert_eq!(removal_only(older, &file(&[("x", "m")])), Ok(true));
}

#[test]
fn removal_only_lets_a_message_change_through_but_not_a_new_entry() {
    let base = file(&[("x", "m"), ("x", "n")]);
    assert_eq!(removal_only(&base, &file(&[("x", "m2")])), Ok(true));
    assert_eq!(
        removal_only(&base, &file(&[("x", "m2"), ("x", "m3")])),
        Ok(true)
    );
    assert_eq!(
        removal_only(&base, &file(&[("x", "m"), ("x", "m"), ("x", "m")])),
        Ok(false)
    );
    assert_eq!(
        removal_only(&base, &file(&[("x", "m2"), ("z", "m")])),
        Ok(false)
    );
}

#[test]
fn removal_only_is_an_error_naming_the_side_that_is_not_a_baseline() {
    let good = file(&[("x", "m")]);
    let e = removal_only("{", &good).unwrap_err();
    assert!(
        e.starts_with("the base's baseline is not a rules baseline"),
        "{e}"
    );
    let e = removal_only(&good, "{}").unwrap_err();
    assert!(
        e.starts_with("the PR's baseline is not a rules baseline"),
        "{e}"
    );
}

/// A finding of a rule whose message measures a value.
fn sized(lines: u64, file: &str, snippet: &str) -> Finding {
    Finding {
        tool: "clippy".into(),
        rule: "clippy::too_many_lines".into(),
        file: file.into(),
        line: Some(1),
        message: format!("this function has too many lines ({lines}/100) (quality bar)"),
        snippet: snippet.into(),
    }
}

/// A baseline's text for `findings`, as `ns rules baseline` writes it.
fn text(findings: &[Finding]) -> String {
    let violations: Vec<Entry> = findings.iter().map(Entry::from).collect();
    json!({"version": 1, "violations": violations}).to_string()
}

#[test]
fn measured_reads_the_value_from_the_registered_shapes_and_nothing_else() {
    assert_eq!(measured(&sized(261, "a.rs", "fn a() {")), Some(261));
    let mut d = f("Cargo.toml", 1, "syn");
    d.tool = "cargo-deny".into();
    d.rule = "duplicate".into();
    d.message = "found 2 duplicate entries for crate 'syn' (one road per job)".into();
    assert_eq!(measured(&d), Some(2));
    // Another rule of the same tool, a rule the table does not know, and a message that is not
    // the rule's shape all measure nothing.
    let mut other = sized(261, "a.rs", "fn a() {");
    other.rule = "clippy::needless_pass_by_value".into();
    assert_eq!(measured(&other), None);
    assert_eq!(measured(&f("a.py", 1, "x")), None);
    let mut reworded = sized(261, "a.rs", "fn a() {");
    reworded.message = "this function is too long".into();
    assert_eq!(measured(&reworded), None);
    reworded.message = "this function has too many lines (261 lines)".into();
    assert_eq!(measured(&reworded), None);
    // No digits before the suffix, or a number too large to read, measure nothing.
    reworded.message = "this function has too many lines (/100)".into();
    assert_eq!(measured(&reworded), None);
    reworded.message = "this function has too many lines (99999999999999999999/100)".into();
    assert_eq!(measured(&reworded), None);
    d.message = "found two duplicate entries for crate 'syn'".into();
    assert_eq!(measured(&d), None);
    d.message = "found 2 duplicates for crate 'syn'".into();
    assert_eq!(measured(&d), None);
}

#[test]
fn a_matched_value_that_grew_is_new_and_names_both_values() {
    let base = [sized(221, "a.rs", "fn a() {")];
    let m = compare(&[sized(261, "a.rs", "fn a() {")], &base);
    assert!(m.accepted.is_empty());
    assert!(m.fixed.is_empty(), "the entry it grew past is not fixed");
    assert_eq!(m.new.len(), 1);
    assert_eq!(m.new[0].growth, Some(Growth { was: 221, now: 261 }));
    assert_eq!(
        m.new[0].line(),
        "a.rs:1: clippy/clippy::too_many_lines: this function has too many lines (261/100) \
         (quality bar) (grew from 221 to 261)"
    );
}

#[test]
fn a_value_equal_to_or_smaller_than_the_entry_still_matches() {
    let base = [sized(221, "a.rs", "fn a() {")];
    for lines in [221, 100] {
        let m = compare(&[sized(lines, "a.rs", "fn a() {")], &base);
        assert_eq!(m.accepted.len(), 1, "at {lines}");
        assert!(m.new.is_empty(), "at {lines}");
    }
}

#[test]
fn a_rule_or_message_with_no_value_matches_as_it_always_did() {
    // The rule has no measured value.
    let m = compare(&[f("a", 9, "x")], &[f("a", 1, "x")]);
    assert_eq!(m.accepted.len(), 1);
    // The entry's message carries no value (it predates the table, or the rule was dropped).
    let mut old = sized(221, "a.rs", "fn a() {");
    old.message = "this function is too long".into();
    let m = compare(&[sized(261, "a.rs", "fn a() {")], &[old]);
    assert_eq!(m.accepted.len(), 1);
    // The finding's message no longer matches the shape.
    let mut reworded = sized(261, "a.rs", "fn a() {");
    reworded.message = "this function is too long".into();
    let m = compare(&[reworded], &[sized(221, "a.rs", "fn a() {")]);
    assert_eq!(m.accepted.len(), 1);
}

#[test]
fn a_grown_duplicate_takes_the_entry_that_still_covers_it() {
    let base = [
        sized(100, "a.rs", "fn a() {"),
        sized(300, "a.rs", "fn a() {"),
    ];
    let m = compare(
        &[
            sized(200, "a.rs", "fn a() {"),
            sized(200, "a.rs", "fn a() {"),
        ],
        &base,
    );
    assert_eq!(m.accepted.len(), 1);
    assert_eq!(m.new.len(), 1);
    assert_eq!(m.new[0].growth, Some(Growth { was: 100, now: 200 }));
    assert!(m.fixed.is_empty(), "both entries were paired");
}

#[test]
fn a_finding_that_fits_only_a_large_entry_is_not_starved_by_a_smaller_one() {
    let base = [
        sized(100, "a.rs", "fn a() {"),
        sized(200, "a.rs", "fn a() {"),
        sized(300, "a.rs", "fn a() {"),
    ];
    let current = [
        sized(50, "a.rs", "fn a() {"),
        sized(150, "a.rs", "fn a() {"),
        sized(250, "a.rs", "fn a() {"),
    ];
    let m = compare(&current, &base);
    assert_eq!(m.accepted.len(), 3);
    assert!(m.new.is_empty());
    assert!(m.fixed.is_empty());
}

#[test]
fn a_grown_finding_is_named_after_the_others_take_the_entries_they_fit() {
    let base = [sized(5, "a.rs", "fn a() {"), sized(10, "a.rs", "fn a() {")];
    let m = compare(
        &[sized(7, "a.rs", "fn a() {"), sized(10, "a.rs", "fn a() {")],
        &base,
    );
    assert_eq!(m.accepted.len(), 1);
    assert_eq!(m.new.len(), 1);
    assert_eq!(m.new[0].growth, Some(Growth { was: 5, now: 7 }));
}

#[test]
fn a_finding_no_entry_covers_does_not_take_the_entry_another_finding_fits() {
    let base = [sized(2, "a.rs", "fn a() {")];
    let current = [sized(10, "a.rs", "fn a() {"), sized(1, "a.rs", "fn a() {")];
    let m = compare(&current, &base);
    assert_eq!(m.accepted.len(), 1);
    assert_eq!(m.accepted[0].finding, 1, "the 1 fits the 2");
    assert_eq!(m.new.len(), 1);
    assert_eq!(m.new[0].finding, sized(10, "a.rs", "fn a() {"));
    assert_eq!(m.new[0].growth, None, "no entry was left to grow from");
    assert!(m.fixed.is_empty());
}

#[test]
fn a_grown_finding_grows_from_the_largest_entry_it_outgrew() {
    let base = [
        sized(100, "a.rs", "fn a() {"),
        sized(300, "a.rs", "fn a() {"),
    ];
    let m = compare(&[sized(400, "a.rs", "fn a() {")], &base);
    assert!(m.accepted.is_empty());
    assert_eq!(m.new[0].growth, Some(Growth { was: 300, now: 400 }));
    assert_eq!(m.fixed, [0], "the 100 entry is the one that is gone");
}

#[test]
fn a_finding_takes_the_entry_of_its_own_size_whatever_the_baseline_order() {
    let base = [
        sized(261, "a.rs", "fn a() {"),
        sized(50, "a.rs", "fn a() {"),
    ];
    let m = compare(&[sized(50, "a.rs", "fn a() {")], &base);
    assert_eq!(m.accepted.len(), 1);
    assert_eq!(m.accepted[0].entry, 1, "the 50 entry, not the 261 one");
    assert_eq!(m.fixed, [0]);
}

#[test]
fn a_value_less_finding_takes_the_last_entry_of_its_key_as_main_did() {
    let mut first = f("a.py", 1, "x");
    first.message = "first".into();
    let mut second = f("a.py", 1, "x");
    second.message = "second".into();
    let mut now = f("a.py", 9, "x");
    now.message = "second".into();
    let m = compare(&[now], &[first, second]);
    assert_eq!(m.accepted.len(), 1);
    assert_eq!(m.accepted[0].entry, 1);
    assert_eq!(m.fixed, [0]);
}

#[test]
fn a_valued_finding_pairs_with_the_valued_entry_before_a_value_less_one() {
    let mut plain = sized(221, "a.rs", "fn a() {");
    plain.message = "this function has too many lines (quality bar)".into();
    let m = compare(
        &[sized(100, "a.rs", "fn a() {"), plain.clone()],
        &[plain, sized(221, "a.rs", "fn a() {")],
    );
    assert!(m.new.is_empty());
    let pairs: Vec<(usize, usize)> = m.accepted.iter().map(|a| (a.finding, a.entry)).collect();
    assert_eq!(pairs, [(0, 1), (1, 0)]);
}

#[test]
fn a_value_less_entry_is_taken_after_the_valued_one_under_its_key() {
    let mut plain = sized(221, "a.rs", "fn a() {");
    plain.message = "this function has too many lines (quality bar)".into();
    let m = compare(
        &[sized(221, "a.rs", "fn a() {"), plain.clone()],
        &[sized(221, "a.rs", "fn a() {"), plain],
    );
    assert!(m.new.is_empty());
    let pairs: Vec<(usize, usize)> = m.accepted.iter().map(|a| (a.finding, a.entry)).collect();
    assert_eq!(pairs, [(0, 0), (1, 1)]);
}

#[test]
fn removal_only_pairs_a_head_entry_with_the_entry_that_fits_it_in_any_order() {
    let base = text(&[
        sized(100, "a.rs", "fn a() {"),
        sized(200, "a.rs", "fn a() {"),
        sized(300, "a.rs", "fn a() {"),
    ]);
    let head = text(&[
        sized(50, "a.rs", "fn a() {"),
        sized(150, "a.rs", "fn a() {"),
        sized(250, "a.rs", "fn a() {"),
    ]);
    assert_eq!(removal_only(&base, &head), Ok(true));
    let base = text(&[sized(5, "a.rs", "fn a() {"), sized(10, "a.rs", "fn a() {")]);
    let head = text(&[sized(7, "a.rs", "fn a() {"), sized(10, "a.rs", "fn a() {")]);
    assert_eq!(removal_only(&base, &head), Ok(false));
}

#[test]
fn fixed_lists_the_baseline_entries_in_finding_order() {
    let base = [f("b.py", 1, "x"), f("a.py", 1, "x")];
    let m = compare(&[], &base);
    assert_eq!(m.fixed, [1, 0]);
}

fn duplicates(count: u64) -> Finding {
    Finding {
        tool: "cargo-deny".into(),
        rule: "duplicate".into(),
        file: "Cargo.toml".into(),
        line: None,
        message: format!("found {count} duplicate entries for crate 'syn'"),
        snippet: "syn".into(),
    }
}

#[test]
fn a_duplicate_that_gains_a_version_is_new_and_names_both_counts() {
    let m = compare(&[duplicates(2)], &[duplicates(1)]);
    assert!(m.accepted.is_empty());
    assert_eq!(m.new.len(), 1);
    assert_eq!(m.new[0].growth, Some(Growth { was: 1, now: 2 }));
    assert_eq!(
        m.new[0].line(),
        "Cargo.toml: cargo-deny/duplicate: found 2 duplicate entries for crate 'syn' \
         (grew from 1 to 2)"
    );
}

#[test]
fn kept_drops_fixed_entries_and_lowers_a_shrunk_value() {
    let base = [
        f("a.py", 0, "x"),
        sized(221, "a.rs", "fn a() {"),
        f("b.py", 0, "z"),
    ];
    let current = [sized(100, "a.rs", "fn a() {"), f("b.py", 0, "z")];
    let m = compare(&current, &base);
    let kept = kept(&base, &m, &current);
    let messages: Vec<&str> = kept.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "this function has too many lines (100/100) (quality bar)",
            "m"
        ]
    );
    assert_eq!(kept[0].snippet, "fn a() {");
    assert_eq!(kept[1].snippet, "z");
}

#[test]
fn planned_lowers_a_shrunk_value_and_writes_nothing_when_unchanged() {
    let base = [sized(221, "a.rs", "fn a() {")];
    let current = [sized(100, "a.rs", "fn a() {")];
    let m = compare(&current, &base);
    let prune = plan(&base, &m, &current).unwrap();
    assert_eq!((prune.dropped, prune.lowered), (0, 1));
    assert!(prune.entries[0].message.contains("(100/100)"));

    let unchanged = compare(&[sized(221, "a.rs", "fn a() {")], &base);
    assert!(plan(&base, &unchanged, &[sized(221, "a.rs", "fn a() {")]).is_none());
}

#[test]
fn removal_only_rejects_a_head_entry_whose_measured_value_grew() {
    let base = text(&[sized(221, "a.rs", "fn a() {")]);
    assert_eq!(removal_only(&base, &base), Ok(true));
    assert_eq!(
        removal_only(&base, &text(&[sized(100, "a.rs", "fn a() {")])),
        Ok(true)
    );
    assert_eq!(
        removal_only(&base, &text(&[sized(261, "a.rs", "fn a() {")])),
        Ok(false)
    );
}

#[test]
fn removal_only_gives_a_grown_duplicate_the_entry_that_still_covers_it() {
    let base = text(&[
        sized(100, "a.rs", "fn a() {"),
        sized(300, "a.rs", "fn a() {"),
    ]);
    // One entry removed; the other grew from 100 to 200 but stays under the 300 entry.
    let head = text(&[sized(200, "a.rs", "fn a() {")]);
    assert_eq!(removal_only(&base, &head), Ok(true));
    // Two entries at 200 need two entries that cover 200; only the 100 one does not.
    let head = text(&[
        sized(200, "a.rs", "fn a() {"),
        sized(200, "a.rs", "fn a() {"),
    ]);
    assert_eq!(removal_only(&base, &head), Ok(false));
}

fn f(file: &str, line: u64, snippet: &str) -> Finding {
    Finding {
        tool: "t".into(),
        rule: "r".into(),
        file: file.into(),
        line: Some(line),
        message: "m".into(),
        snippet: snippet.into(),
    }
}

#[test]
fn commands_reads_every_backticked_span_in_the_rules_row() {
    assert_eq!(
        commands("| rules | `ruff check .`, `lint-imports` `cargo deny check` | 5 s |"),
        ["ruff check .", "lint-imports", "cargo deny check"]
    );
    assert_eq!(
        commands("| `rules` | `a | jq -c .` | see `docs` |"),
        ["a | jq -c ."]
    );
    assert_eq!(commands("| rules | `a`, then `b` |"), ["a"]);
    assert_eq!(commands("| rules | none |\n| rules | `x` |"), ["x"]);
    assert!(commands("| rules | `` |").is_empty());
    assert!(commands("| rules | `open").is_empty());
    assert!(commands("| ci-local | `make ci` |").is_empty());
    assert!(commands("rules: `x`").is_empty());
}

#[test]
fn parse_line_reads_json_and_file_line_col_message() {
    let j = parse_line(
        "ast-grep",
        r#"{"tool": "sg", "rule": "no-eval", "file": "a.py", "line": 4, "message": "m", "snippet": "  x =  1 "}"#,
    )
    .unwrap();
    assert_eq!(
        (j.tool.as_str(), j.rule.as_str(), j.line, j.snippet.as_str()),
        ("sg", "no-eval", Some(4), "x = 1")
    );
    let j = parse_line("ast-grep", r#"{"file": "a.py"}"#).unwrap();
    assert_eq!(
        (j.tool.as_str(), j.line, j.message.as_str()),
        ("ast-grep", None, "")
    );
    assert_eq!(parse_line("x", r#"{"rule": "r"}"#), None);
    assert_eq!(parse_line("x", "{not json"), None);

    let p = parse_line("ruff", "a.py:3:8: F401 `os` imported but unused").unwrap();
    assert_eq!(
        (p.file.as_str(), p.line, p.message.as_str()),
        ("a.py", Some(3), "F401 `os` imported but unused")
    );
    let p = parse_line("cc", "src/x.c:12: warning: unused: 7").unwrap();
    assert_eq!(
        (p.line, p.message.as_str()),
        (Some(12), "warning: unused: 7")
    );
    assert_eq!(parse_line("x", "warning: unused variable"), None);
    assert_eq!(parse_line("x", "a.py:x:1: m"), None);
    assert_eq!(parse_line("x", "   Compiling foo v1: 2: x"), None);
    assert_eq!(parse_line("x", ":3: m"), None);
    assert_eq!(parse_line("x", "my file.py:3: m"), None);
    assert_eq!(parse_line("x", "a.py:3"), None);
}

#[test]
fn tool_name_is_the_first_word_without_its_directory() {
    assert_eq!(tool_name("./scripts/lint-x --strict"), "lint-x");
    assert_eq!(tool_name("ruff check ."), "ruff");
}

#[test]
fn settle_makes_paths_relative_and_snippets_from_source() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.py"), "one\n  two   2\n").unwrap();
    let mut src = HashMap::new();
    let mut raw = f(&tmp.path().join("a.py").to_string_lossy(), 2, "");
    raw.message = "msg".into();
    let s = settle(raw.clone(), tmp.path(), &mut src);
    assert_eq!((s.file.as_str(), s.snippet.as_str()), ("a.py", "two 2"));
    raw.file = "./a.py".into();
    raw.line = Some(9);
    let s = settle(raw.clone(), tmp.path(), &mut src);
    assert_eq!((s.file.as_str(), s.snippet.as_str()), ("a.py", "msg"));
    raw.line = Some(0);
    assert_eq!(settle(raw.clone(), tmp.path(), &mut src).snippet, "msg");
    raw.line = None;
    raw.file = "missing.py".into();
    assert_eq!(settle(raw.clone(), tmp.path(), &mut src).snippet, "msg");
    raw.snippet = "given".into();
    assert_eq!(settle(raw, tmp.path(), &mut src).snippet, "given");
}

#[test]
fn refreshed_counts_accepted_findings_no_entry_equals_in_full() {
    let msg = |m: &str| Finding {
        line: None,
        message: m.into(),
        ..f("a", 0, "x")
    };
    let base = [msg("a"), msg("b")];
    let accepted = |current: &[Finding]| compare(current, &base).accepted;
    let current = [msg("a"), msg("b")];
    assert_eq!(refreshed(&current, &base, &accepted(&current)), 0);
    let current = [msg("b"), msg("c")];
    assert_eq!(refreshed(&current, &base, &accepted(&current)), 1);
    let current = [msg("a"), msg("a")];
    assert_eq!(refreshed(&current, &base, &accepted(&current)), 1);
    let current = [msg("c")];
    assert_eq!(refreshed(&current, &base, &accepted(&current)), 1);
}

#[test]
fn compare_matches_on_tool_rule_file_and_snippet_and_counts_duplicates() {
    let base = [f("a", 1, "x"), f("a", 5, "y")];
    let m = compare(&[f("a", 9, "x"), f("a", 10, "x")], &base);
    assert_eq!(m.new.len(), 1);
    assert_eq!(m.new[0].finding, f("a", 10, "x"));
    assert_eq!(m.accepted.len(), 1);
    assert_eq!(m.fixed, [1]);
    let mut other = f("a", 1, "x");
    other.rule = "r2".into();
    let m = compare(std::slice::from_ref(&other), &base);
    assert_eq!(m.new.len(), 1);
    assert_eq!(m.new[0].finding, other);
    let m = compare(&[], &[]);
    assert!(m.new.is_empty() && m.fixed.is_empty() && m.accepted.is_empty());
}

#[test]
fn a_malformed_baseline_is_an_error_naming_the_fix() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("b.json");
    assert!(load_baseline(&p).unwrap().is_none());
    let e = load_baseline(tmp.path()).unwrap_err();
    assert!(format!("{e:#}").contains("cannot read"), "{e:#}");
    fs::write(&p, "{}").unwrap();
    let e = load_baseline(&p).unwrap_err();
    let e = e.downcast_ref::<SfError>().unwrap();
    assert!(e.hint.as_deref().unwrap().contains("git checkout --"));
}

#[test]
fn human_lines_name_the_rule_when_there_is_one() {
    assert_eq!(line(&f("a.py", 3, "")), "a.py:3: t/r: m");
    let mut g = f("a.py", 3, "");
    g.rule.clear();
    g.line = None;
    assert_eq!(line(&g), "a.py: t: m");
}
