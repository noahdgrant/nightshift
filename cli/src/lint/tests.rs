use super::*;

const GOOD: &str = "---\nname: ns-tdd\ndescription: Test first.\nmetadata:\n  upstream: mattpocock/skills@b0618bc436ad:skills/engineering/tdd\n---\nbody\n";

#[test]
fn good_frontmatter() {
    assert!(check_frontmatter("ns-tdd", GOOD, SkillRoot::Shipped).is_empty());
}

#[test]
fn name_mismatch_and_prefix() {
    let e = check_frontmatter("tdd", GOOD, SkillRoot::Shipped);
    assert_eq!(e.len(), 1, "{e:?}");
    let e = check_frontmatter(
        "tdd",
        "---\nname: tdd\ndescription: x\n---\n",
        SkillRoot::Shipped,
    );
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("ns-"));
}

#[test]
fn description_rules() {
    let e = check_frontmatter(
        "ns-a",
        "---\nname: ns-a\ndescription: \"  \"\n---\n",
        SkillRoot::Shipped,
    );
    assert_eq!(e, ["description is empty"]);
    let long = "x".repeat(1025);
    let e = check_frontmatter(
        "ns-a",
        &format!("---\nname: ns-a\ndescription: {long}\n---\n"),
        SkillRoot::Shipped,
    );
    assert!(e[0].contains("1025"));
    let e = check_frontmatter("ns-a", "---\nname: ns-a\n---\n", SkillRoot::Shipped);
    assert_eq!(e, ["missing description"]);
}

#[test]
fn user_invoked_skills_disable_model_invocation() {
    let without = "---\nname: ns-auto\ndescription: x\n---\n";
    let e = check_frontmatter("ns-auto", without, SkillRoot::Shipped);
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("disable-model-invocation"));
    let with = "---\nname: ns-auto\ndescription: x\ndisable-model-invocation: true\n---\n";
    assert!(check_frontmatter("ns-auto", with, SkillRoot::Shipped).is_empty());
    for bad in ["false", "\"true\""] {
        let src =
            format!("---\nname: ns-auto\ndescription: x\ndisable-model-invocation: {bad}\n---\n");
        assert!(
            !check_frontmatter("ns-auto", &src, SkillRoot::Shipped).is_empty(),
            "{bad} must error"
        );
    }
    assert!(check_frontmatter(
        "ns-a",
        "---\nname: ns-a\ndescription: x\n---\n",
        SkillRoot::Shipped
    )
    .is_empty());
}

#[test]
fn bad_yaml_and_missing() {
    assert!(
        check_frontmatter("ns-a", "---\nname: [unclosed\n---\n", SkillRoot::Shipped)[0]
            .contains("YAML")
    );
    assert!(
        check_frontmatter("ns-a", "# no fm\n", SkillRoot::Shipped)[0]
            .contains("missing frontmatter")
    );
}

#[test]
fn upstream_formats() {
    assert!(valid_upstream(
        "cursor/plugins@ccb5507cec15:pstack/skills/x"
    ));
    assert!(!valid_upstream("cursor/plugins@main:x"));
    assert!(!valid_upstream("cursor/plugins@ccb5507cec15"));
    assert!(!valid_upstream("plugins@ccb5507cec15:x"));
    assert!(!valid_upstream("cursor/plugins@ABCDEF0:x"));
    for bad in [
        "../..@abc1234:x",
        "./x@abc1234:x",
        "a/..@abc1234:x",
        "a/.git@abc1234:x",
        ".a/b@abc1234:x",
    ] {
        assert!(!valid_upstream(bad), "{bad}");
    }
    assert_eq!(
        parse_upstream("a/b@1234567:p/q.md"),
        Some(Upstream {
            repo: "a/b",
            sha: "1234567",
            path: "p/q.md"
        })
    );
    let list = "---\nname: ns-a\ndescription: x\nmetadata:\n  upstream:\n    - a/b@1234567:p\n    - a/b@nothex:p\n---\n";
    let e = check_frontmatter("ns-a", list, SkillRoot::Shipped);
    assert_eq!(e.len(), 1);
    assert!(e[0].contains("nothex"));
}

#[test]
fn finds_relative_links_only() {
    let text = "See [a](./a.md), [b](../ns-b/SKILL.md#top), [c](https://x.y/z), [d](#anchor), [e](./e.md \"t\").\n```\n[f](./ignored.md)\n```\n[g](refs/no-dot.md#x), [h](mailto:a@b.c), [i](/abs.md), [j](<refs/angle.md>), [k](refs/a:b.md), [l](:c.md), [m](git+ssh://h/r), [n](x-y.z:q)\n";
    assert_eq!(
        relative_links(text),
        [
            "./a.md",
            "../ns-b/SKILL.md",
            "./e.md",
            "refs/no-dot.md",
            "refs/angle.md",
            "refs/a:b.md",
            ":c.md"
        ]
    );
}

#[test]
fn links_inside_code_spans_are_skipped() {
    let text = "Run `printf '[a](./gone.md)'` or ``x [b](./gone2.md) ` y`` then [c](./c.md), and `[d](./d.md)\n";
    assert_eq!(relative_links(text), ["./c.md", "./d.md"]);
    let text = "`a `` [x](./x.md) b` [c](./c.md) and [e](./e`.md)\n";
    assert_eq!(relative_links(text), ["./c.md", "./e`.md"]);
}

fn skill(body: &str, refs: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("ns-a");
    fs::create_dir_all(dir.join("references")).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: ns-a\ndescription: A.\n---\n{body}"),
    )
    .unwrap();
    for (name, text) in refs {
        fs::write(dir.join("references").join(name), text).unwrap();
    }
    tmp
}

fn messages(tmp: &tempfile::TempDir) -> Vec<String> {
    let report = lint_dir(tmp.path()).unwrap();
    report.errors.into_iter().map(|e| e.message).collect()
}

#[test]
fn a_fixed_tmp_path_fails_in_a_skill_and_its_references() {
    let tmp = skill(
        "Save it:\n```bash\ncp out /tmp/ns/out.txt\n```\n[r](references/r.md)\n",
        &[(
            "r.md",
            "Run `mktemp -d /tmp/x.XXXXXX` first.\nOpen file:///tmp/report.html.\n/tmp/run.log holds it.\n",
        )],
    );
    assert_eq!(
        messages(&tmp),
        [
            "line 7: fixed path /tmp/ns/out.txt; use mktemp or a name under $TMPDIR",
            "line 1: fixed path /tmp/x.XXXXXX; use mktemp or a name under $TMPDIR",
            "line 2: fixed path /tmp/report.html; use mktemp or a name under $TMPDIR",
            "line 3: fixed path /tmp/run.log; use mktemp or a name under $TMPDIR",
        ]
    );
}

#[test]
fn tmpdir_paths_and_mentions_of_tmp_pass() {
    let tmp = skill(
        "Never write a fixed `/tmp/<name>` path. Use `$TMPDIR/out`, \
         `${TMPDIR:-/tmp}/out`, `${dir}/tmp/x`, ~/tmp/x, ./tmp/x, `mktemp`, \
         target/tmp/x, /tmp or /tmp/.\n",
        &[],
    );
    assert!(messages(&tmp).is_empty(), "{:?}", messages(&tmp));
}

#[test]
fn eval_files_may_name_fixed_tmp_paths() {
    let tmp = skill("x\n", &[]);
    let case = tmp.path().join("ns-a/evals/cases/c");
    fs::create_dir_all(&case).unwrap();
    fs::write(case.join("brief.md"), "Log to /tmp/run.log.\n").unwrap();
    assert!(messages(&tmp).is_empty(), "{:?}", messages(&tmp));
}

#[test]
fn unprefixed_broken_link_fails() {
    let tmp = skill("[m](references/missing.md)\n", &[]);
    assert_eq!(messages(&tmp), ["broken link: references/missing.md"]);
}

#[test]
fn unlinked_reference_fails_until_linked() {
    let tmp = skill("no links\n", &[("orphan.md", "x\n")]);
    assert_eq!(messages(&tmp), ["unlinked reference: references/orphan.md"]);
    let tmp = skill("[o](references/orphan.md)\n", &[("orphan.md", "x\n")]);
    assert!(messages(&tmp).is_empty(), "{:?}", messages(&tmp));
}

#[test]
fn directory_link_covers_its_readme() {
    let tmp = skill("[ex](references/ex/)\n", &[]);
    let ex = tmp.path().join("ns-a/references/ex");
    fs::create_dir_all(&ex).unwrap();
    fs::write(ex.join("README.md"), "[f](f.md)\n").unwrap();
    fs::write(ex.join("f.md"), "x\n").unwrap();
    assert!(messages(&tmp).is_empty(), "{:?}", messages(&tmp));
    fs::write(
        tmp.path().join("ns-a/SKILL.md"),
        "---\nname: ns-a\ndescription: A.\n---\n",
    )
    .unwrap();
    assert_eq!(
        messages(&tmp),
        ["unlinked reference: references/ex/README.md"]
    );
}

fn write_skill_at(root: &Path, name: &str, extra: &str) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: A.\n---\n{extra}"),
    )
    .unwrap();
}

fn lint_messages(root: &Path) -> (usize, Vec<String>) {
    let report = lint_dir(root).unwrap();
    let msgs = report.errors.into_iter().map(|e| e.message).collect();
    (report.skills, msgs)
}

#[test]
fn rules_index_is_checked_when_the_parent_has_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    write_skill_at(&skills, "ns-a", "");
    let report = lint_dir(&skills).unwrap();
    assert!(report.ok, "{:?}", report.errors);
    assert!(report.errors.iter().all(|e| e.skill != "rules"));

    fs::create_dir_all(tmp.path().join("rules/c")).unwrap();
    let report = lint_dir(&skills).unwrap();
    assert!(!report.ok);
    assert!(
        report.errors.iter().any(|e| e.skill == "rules"),
        "{:?}",
        report.errors
    );
}

#[test]
fn decisions_are_checked_when_the_parent_has_docs_decisions() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    write_skill_at(&skills, "ns-a", "Cites D1.\n");
    assert_eq!(lint_messages(&skills), (1, vec![]));

    fs::create_dir_all(tmp.path().join("docs/decisions")).unwrap();
    let report = lint_dir(&skills).unwrap();
    assert!(!report.ok);
    assert_eq!(report.skills, 1);
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(report.errors[0].skill, "decisions");
    assert_eq!(
        report.errors[0].message,
        "line 5: cites D1, which has no file in docs/decisions/"
    );

    fs::write(
        tmp.path().join("docs/decisions/D1-one.md"),
        "---\nid: D1\n---\n",
    )
    .unwrap();
    assert_eq!(lint_messages(&skills), (1, vec![]));
}

#[test]
fn repo_local_skill_is_exempt_from_the_prefix_rule() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join(".agents/skills");
    write_skill_at(&local, "verify-x", "");
    assert_eq!(lint_messages(&local), (1, vec![]));

    fs::write(
        local.join("verify-x/SKILL.md"),
        "---\nname: verify-y\ndescription: A.\n---\n",
    )
    .unwrap();
    assert_eq!(
        lint_messages(&local),
        (
            1,
            vec!["name \"verify-y\" does not match directory \"verify-x\"".to_string()]
        )
    );

    let shipped = tmp.path().join("skills");
    write_skill_at(&shipped, "verify-x", "");
    assert_eq!(
        lint_messages(&shipped),
        (
            1,
            vec!["name \"verify-x\" must start with \"ns-\"".to_string()]
        )
    );
}

fn feature(root: &Path, file: &str, body: &str) {
    let dir = root.join("verify-x/features");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(file), body).unwrap();
}

const FEATURE: &str = "# F\n\nWhat it does.\n\n## Sub-features\n\n- `f-a` a.\n\n## How to get to it (user POV)\n\n- `x`\n\n## Driving it with control-x\n\nPreconditions:\n\n```\n## not a heading\n```\n\n## Gotchas\n\n- none\n";

#[test]
fn ns_dir_without_skill_md_fails_and_other_dirs_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("skills");
    fs::create_dir_all(root.join("ns-empty")).unwrap();
    fs::create_dir_all(root.join("not-a-skill")).unwrap();
    assert_eq!(
        lint_messages(&root),
        (0, vec!["missing SKILL.md".to_string()])
    );
}

#[test]
fn repo_local_dir_without_skill_md_fails_and_hidden_dirs_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join(".agents/skills");
    fs::create_dir_all(local.join("verify-y")).unwrap();
    fs::create_dir_all(local.join("verify-z")).unwrap();
    fs::write(local.join("verify-z/skill.md"), "x\n").unwrap();
    fs::create_dir_all(local.join(".cache")).unwrap();
    write_skill_at(&local, "verify-x", "");
    assert_eq!(
        lint_messages(&local),
        (1, vec!["missing SKILL.md".to_string(); 2])
    );
}

#[test]
fn regular_files_in_a_root_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join(".agents/skills");
    write_skill_at(&local, "verify-x", "");
    fs::write(local.join("README.md"), "x\n").unwrap();
    assert_eq!(lint_messages(&local), (1, vec![]));

    let shipped = tmp.path().join("skills");
    write_skill_at(&shipped, "ns-a", "");
    fs::write(shipped.join("README.md"), "x\n").unwrap();
    fs::write(shipped.join("ns-notes.md"), "x\n").unwrap();
    assert_eq!(lint_messages(&shipped), (1, vec![]));
}

#[cfg(unix)]
#[test]
fn exemption_follows_symlinks_to_the_real_parent() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join(".agents/real");
    write_skill_at(&real, "verify-x", "");
    let link = tmp.path().join("skills");
    symlink(&real, &link).unwrap();
    assert_eq!(lint_messages(&link), (1, vec![]));

    let plain = tmp.path().join("plain");
    write_skill_at(&plain, "verify-x", "");
    let agents = tmp.path().join("proj/.agents");
    fs::create_dir_all(&agents).unwrap();
    symlink(&plain, agents.join("skills")).unwrap();
    assert_eq!(
        lint_messages(&agents.join("skills")),
        (
            1,
            vec!["name \"verify-x\" must start with \"ns-\"".to_string()]
        )
    );
}

#[test]
fn prefix_exemption_needs_agents_as_the_direct_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let nested = tmp.path().join(".agents/other/skills");
    write_skill_at(&nested, "verify-x", "");
    assert_eq!(
        lint_messages(&nested),
        (
            1,
            vec!["name \"verify-x\" must start with \"ns-\"".to_string()]
        )
    );
}

#[test]
fn near_miss_feature_headings_fail_and_trailing_space_passes() {
    let bare = FEATURE.replace("## Driving it with control-x", "## Driving it with");
    assert!(check_feature(&bare).unwrap().contains("Driving it with,"));
    let no_with = FEATURE.replace("## Driving it with control-x", "## Driving it");
    assert!(check_feature(&no_with).is_some());
    let padded = FEATURE.replace("## Gotchas", "## Gotchas  ");
    assert_eq!(check_feature(&padded), None);
}

#[test]
fn feature_files_need_the_four_h2s_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join(".agents/skills");
    write_skill_at(&local, "verify-x", "");
    feature(&local, "README.md", "# Index\n\n## Features\n");
    feature(&local, "good.md", FEATURE);
    feature(&local, "notes.txt", "## Anything\n");
    assert_eq!(lint_messages(&local), (1, vec![]));

    feature(
        &local,
        "renamed.md",
        &FEATURE.replace("## Driving it with control-x", "## Running it"),
    );
    feature(
        &local,
        "missing.md",
        &FEATURE.replace("## Gotchas", "Gotchas"),
    );
    feature(
        &local,
        "swapped.md",
        &FEATURE
            .replace("## Sub-features", "## TMP")
            .replace("## How to get to it (user POV)", "## Sub-features")
            .replace("## TMP", "## How to get to it (user POV)"),
    );
    let report = lint_dir(&local).unwrap();
    let errs: Vec<_> = report
        .errors
        .iter()
        .map(|e| (e.file.rsplit('/').next().unwrap(), e.message.as_str()))
        .collect();
    let want = "H2 headings must be, in order: Sub-features, How to get to it (user POV), Driving it with <tool>, Gotchas";
    assert_eq!(
        errs,
        [
            (
                "missing.md",
                format!("features/missing.md: {want}; found: Sub-features, How to get to it (user POV), Driving it with control-x").as_str()
            ),
            (
                "renamed.md",
                format!("features/renamed.md: {want}; found: Sub-features, How to get to it (user POV), Running it, Gotchas").as_str()
            ),
            (
                "swapped.md",
                format!("features/swapped.md: {want}; found: How to get to it (user POV), Sub-features, Driving it with control-x, Gotchas").as_str()
            ),
        ]
    );
}

#[test]
fn unreadable_feature_file_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join(".agents/skills");
    write_skill_at(&local, "verify-x", "");
    let dir = local.join("verify-x/features");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("x.md"), [0xff, 0xfe, 0xfd]).unwrap();
    let (skills, errs) = lint_messages(&local);
    assert_eq!(skills, 1);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].starts_with("features/x.md: unreadable: "),
        "{errs:?}"
    );
}

#[test]
fn feature_files_are_checked_under_a_shipped_root() {
    let tmp = tempfile::tempdir().unwrap();
    let shipped = tmp.path().join("skills");
    write_skill_at(&shipped, "ns-a", "");
    let dir = shipped.join("ns-a/features");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("bad.md"), "# F\n\n## Nope\n").unwrap();
    let (_, errs) = lint_messages(&shipped);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].starts_with("features/bad.md: H2 headings must be"),
        "{errs:?}"
    );
}

#[test]
fn extra_or_missing_h2s_fail() {
    let extra = format!("{FEATURE}\n## Appendix\n\n- more\n");
    assert!(check_feature(&extra)
        .unwrap()
        .ends_with("Gotchas, Appendix"));
    assert!(check_feature("# F\n\nNo sections.\n")
        .unwrap()
        .ends_with("found: "));
    assert_eq!(check_feature(FEATURE), None);
}

#[test]
fn only_column_zero_h2_lines_count_as_headings() {
    let body = format!("{FEATURE}\n### Detail\n\n    ## indented\n");
    assert_eq!(check_feature(&body), None);
}

#[test]
fn a_fence_closes_only_on_its_own_marker() {
    let tilde_inside = "```\n~~~\n## Fake\n[x](./gone.md)\n```\n";
    let backtick_inside = "~~~\n```\n## Fake\n[x](./gone.md)\n~~~\n";
    for text in [tilde_inside, backtick_inside] {
        assert_eq!(prose_lines(text), Vec::<&str>::new(), "{text}");
        assert!(relative_links(text).is_empty(), "{text}");
        let body = format!("{FEATURE}\n{text}");
        assert_eq!(check_feature(&body), None, "{text}");
    }
}

#[test]
fn setup_verify_feature_map_example_passes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../skills/ns-setup-verify/references/feature-map-example");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") && !path.ends_with("README.md") {
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(check_feature(&body), None, "{}", path.display());
            checked += 1;
        }
    }
    assert!(checked > 0);
}

#[test]
fn reference_linked_from_another_reference_passes() {
    let tmp = skill(
        "[a](./references/a.md#top)\n",
        &[("a.md", "[b](b.md)\n"), ("b.md", "x\n")],
    );
    assert!(messages(&tmp).is_empty(), "{:?}", messages(&tmp));
}
