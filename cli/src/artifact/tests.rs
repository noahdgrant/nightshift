use super::*;
use super::{Cause as C, Next as N, Phase as P, Status as S};

/// The cases of `run::currency`'s `state_table` and base-moved tests, artifact by artifact.
const V1_CASES: &[(Phase, &str, &str, Route)] = &[
    (P::Triage, "pass", "", (S::Pass, C::None, N::Build)),
    (P::Triage, "fail", "x", (S::Fail, C::NeedsHuman, N::Human)),
    (
        P::Triage,
        "split",
        "Split into #8 and #9.",
        (S::Split, C::None, N::None),
    ),
    (
        P::Triage,
        "blocked",
        "Six behaviours",
        (S::Blocked, C::NeedsHuman, N::Human),
    ),
    (P::Build, "pass", "", (S::Pass, C::None, N::Verify)),
    (
        P::Build,
        "fail",
        "fail body",
        (S::Fail, C::GateRed, N::Build),
    ),
    // Build's own fail is a failed attempt, whatever it says.
    (
        P::Build,
        "fail",
        "Base moved: rebase",
        (S::Fail, C::GateRed, N::Build),
    ),
    (
        P::Build,
        "blocked",
        "no toolchain",
        (S::Blocked, C::NeedsHuman, N::Human),
    ),
    (P::Verify, "pass", "", (S::Pass, C::None, N::Review)),
    (
        P::Verify,
        "fail",
        "fail body",
        (S::Fail, C::CriteriaFailed, N::Build),
    ),
    (
        P::Verify,
        "fail",
        "Base moved: rebase onto main\n",
        (S::Fail, C::BaseMoved, N::Build),
    ),
    (
        P::Verify,
        "fail",
        "Tests fail\nbase moved too\n",
        (S::Fail, C::CriteriaFailed, N::Build),
    ),
    (
        P::Verify,
        "fail",
        "Tests fail; not because the base moved\n",
        (S::Fail, C::CriteriaFailed, N::Build),
    ),
    (
        P::Verify,
        "blocked",
        "Needs a board",
        (S::Blocked, C::NeedsHuman, N::Human),
    ),
    (
        P::Verify,
        "blocked",
        "denied by `Bash(x)`: `x`",
        (S::Blocked, C::Denied, N::Human),
    ),
    (P::Review, "pass", "", (S::Pass, C::None, N::Ship)),
    (
        P::Review,
        "fail",
        "fail body",
        (S::Fail, C::FindingsOpen, N::Build),
    ),
    (
        P::Review,
        "fail",
        "Base moved: rebase onto main\n",
        (S::Fail, C::BaseMoved, N::Build),
    ),
    (
        P::Review,
        "blocked",
        "Open after at least 2 fix attempts: I3.",
        (S::Blocked, C::NeedsHuman, N::Human),
    ),
    (P::Ship, "pass", "", (S::Pass, C::None, N::None)),
    (
        P::Ship,
        "fail",
        "evidence is stale; ns-review must rerun",
        (S::Fail, C::CriteriaFailed, N::Review),
    ),
    (
        P::Ship,
        "fail",
        "ns-verify must rerun before review",
        (S::Fail, C::CriteriaFailed, N::Verify),
    ),
    (
        P::Ship,
        "fail",
        "fail body",
        (S::Fail, C::NeedsHuman, N::Human),
    ),
    (
        P::Ship,
        "fail",
        "NO-GO: base moved, the branch conflicts with main\n",
        (S::Fail, C::BaseMoved, N::Build),
    ),
    (
        P::Ship,
        "blocked",
        "needs approval",
        (S::Blocked, C::NeedsHuman, N::Human),
    ),
];

const VALID: &str = "---
schema: ns-artifact/2
unit: 142-uart-timeout
phase: verify
status: fail
cause: criteria_failed
next: build
reason: \"AC2 fails: #12's timeout is still 5s\"
sha: 0e05787
updated: 2026-10-08T21:14:00Z
future_key: ignored
---
# Evidence
";

fn errors(text: &str, phase: Option<&str>) -> Vec<String> {
    check(text, phase).expect_err("invalid")
}

/// `VALID` with the line starting `key:` replaced by `line` (dropped when empty).
fn with(key: &str, line: &str) -> String {
    VALID
        .lines()
        .filter_map(|l| {
            if l.starts_with(&format!("{key}:")) {
                (!line.is_empty()).then(|| line.to_string())
            } else {
                Some(l.to_string())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn a_valid_artifact_reads_every_field_and_ignores_unknown_keys() {
    let t = check(VALID, Some("verify")).unwrap();
    assert_eq!(t.schema, Schema::V2);
    assert_eq!(t.unit.as_deref(), Some("142-uart-timeout"));
    assert_eq!(t.phase, Phase::Verify);
    assert_eq!(t.status, Status::Fail);
    assert_eq!(t.cause, Cause::CriteriaFailed);
    assert_eq!(t.next, Next::Build);
    // YAML reads the quoted reason, so its `#` is not a comment.
    assert_eq!(t.reason, "AC2 fails: #12's timeout is still 5s");
    assert_eq!(t.sha.as_deref(), Some("0e05787"));
    assert_eq!(t.updated.as_deref(), Some("2026-10-08T21:14:00Z"));
    assert!(t.warnings.is_empty());
}

#[test]
fn frontmatter_must_open_the_file() {
    assert_eq!(
        errors(&format!("Some notes\n{VALID}"), None),
        ["frontmatter must be the first thing in the file; found text above it"]
    );
    assert_eq!(
        errors("# Evidence\n", None),
        ["no frontmatter: the file must open with a `---` line, the ns-artifact/2 keys, then `---`"]
    );
    assert_eq!(
        errors("---\nschema: ns-artifact/2\n", None),
        ["frontmatter has no closing `---` line"]
    );
    assert_eq!(errors("---\n---\nbody\n", None), ["frontmatter is empty"]);
    assert_eq!(
        errors("---\n- a\n---\n", None),
        ["frontmatter must be `key: value` lines"]
    );
    let bad = errors("---\nstatus: [pass\n---\n", None);
    assert!(
        bad.len() == 1 && bad[0].starts_with("frontmatter is not valid YAML: "),
        "{bad:?}"
    );
}

#[test]
fn schema_must_be_v2() {
    assert_eq!(
        errors(&with("schema", ""), None),
        ["`schema` is missing: add `schema: ns-artifact/2` (an artifact with no schema reads as ns-artifact/1)"]
    );
    assert_eq!(
        errors(&with("schema", "schema: ns-artifact/3"), None),
        ["`schema: ns-artifact/3` is not ns-artifact/2"]
    );
}

#[test]
fn enum_values_name_the_allowed_set() {
    assert_eq!(
        errors(&with("cause", "cause: foo"), None),
        ["`cause: foo` is not one of none, findings_open, criteria_failed, gate_red, base_moved, denied, needs_human, out_of_scope"]
    );
    assert_eq!(
        errors(&with("status", "status: pass | fail"), None),
        ["`status: pass | fail` is not one of pass, fail, blocked, split"]
    );
    assert_eq!(
        errors(&with("phase", "phase: qa"), None),
        ["`phase: qa` is not one of triage, troubleshoot, build, verify, review, review-fix, ship"]
    );
    assert_eq!(
        errors(&with("next", "next: later"), None),
        ["`next: later` is not one of none, build, verify, review, ship, define, troubleshoot, human, split, info, wontfix"]
    );
}

#[test]
fn required_keys_are_named_when_missing() {
    assert_eq!(
        errors(&with("unit", ""), None),
        ["`unit` is missing: the unit id, such as `142-uart-timeout`"]
    );
    assert_eq!(
        errors(&with("phase", ""), None),
        ["`phase` is missing: one of triage, troubleshoot, build, verify, review, review-fix, ship"]
    );
    assert_eq!(
        errors(&with("status", ""), None),
        ["`status` is missing: one of pass, fail, blocked, split"]
    );
    assert_eq!(
        errors(&with("cause", ""), None),
        ["`cause` is missing: one of none, findings_open, criteria_failed, gate_red, base_moved, denied, needs_human, out_of_scope"]
    );
    assert_eq!(
        errors(&with("updated", ""), None),
        ["`updated` is missing: the UTC time written, such as `2026-10-08T21:14:00Z`"]
    );
    assert_eq!(
        errors(&with("sha", ""), None),
        ["`sha` is missing: `phase: verify` names the commit it describes (`git rev-parse --short HEAD`)"]
    );
}

#[test]
fn the_phase_must_be_the_one_checked() {
    assert_eq!(
        errors(VALID, Some("review")),
        ["`phase: verify` but this artifact is checked as `review`"]
    );
}

#[test]
fn a_pass_has_no_cause() {
    let pass = with("status", "status: pass");
    assert_eq!(
        errors(&pass, None),
        ["`status: pass` needs `cause: none`, found `cause: criteria_failed`"]
    );
    let clean = with("cause", "cause: none").replace("status: fail", "status: pass");
    let t = check(&clean, None).unwrap();
    assert_eq!((t.status, t.cause), (Status::Pass, Cause::None));
    // A pass needs no reason and no next.
    let bare = "---\nschema: ns-artifact/2\nunit: u\nphase: build\nstatus: pass\ncause: none\nsha: abc1234\nupdated: 2026-10-08T21:14:00Z\n---\n";
    let t = check(bare, None).unwrap();
    assert_eq!((t.next, t.reason.as_str()), (Next::None, ""));
}

#[test]
fn a_fail_or_block_needs_a_cause_a_next_and_a_reason() {
    for s in ["fail", "blocked"] {
        let base = with("status", &format!("status: {s}"));
        let none = base.replace("cause: criteria_failed", "cause: none");
        assert_eq!(
            errors(&none, None),
            [format!("`status: {s}` needs a cause other than `none`: one of findings_open, criteria_failed, gate_red, base_moved, denied, needs_human, out_of_scope")]
        );
        let no_next = base.replace("next: build\n", "");
        assert_eq!(
            errors(&no_next, None),
            [format!(
                "`status: {s}` needs `next`: the phase that picks the unit up, or `human`"
            )]
        );
        let next_none = base.replace("next: build", "next: none");
        assert_eq!(errors(&next_none, None).len(), 1);
        let no_reason = base.replace("reason: \"AC2 fails: #12's timeout is still 5s\"\n", "");
        assert_eq!(
            errors(&no_reason, None),
            [format!(
                "`reason` is missing: `status: {s}` needs one line saying why"
            )]
        );
        let empty = base.replace(
            "reason: \"AC2 fails: #12's timeout is still 5s\"",
            "reason: \"\"",
        );
        assert_eq!(errors(&empty, None).len(), 1);
    }
}

#[test]
fn a_reason_is_one_short_line() {
    let block = with("reason", "reason: |\n  line one\n  line two");
    assert_eq!(errors(&block, None), ["`reason` must be one line"]);
    let long = with("reason", &format!("reason: {}", "x".repeat(201)));
    assert_eq!(
        errors(&long, None),
        ["`reason` is longer than 200 characters"]
    );
    let at_cap = with("reason", &format!("reason: {}", "x".repeat(200)));
    assert!(check(&at_cap, None).is_ok());
    // A reason YAML reads as a number is kept as written.
    let n = with("reason", "reason: 404");
    assert_eq!(check(&n, None).unwrap().reason, "404");
}

fn triage(status: &str, next: &str) -> String {
    format!(
        "---\nschema: ns-artifact/2\nunit: 7-x\nphase: triage\nstatus: {status}\ncause: none\n{next}reason: Split into #8 and #9.\nbase: main\ntouches:\n  - cli/src/run.rs\nupdated: 2026-10-08T21:14:00Z\n---\nIssue: u\n"
    )
}

#[test]
fn split_is_for_triage_only_and_triage_names_next() {
    let t = check(&triage("split", ""), Some("triage")).unwrap();
    assert_eq!(t.status, Status::Split);
    assert_eq!(t.touches, ["cli/src/run.rs"]);
    assert_eq!(t.base.as_deref(), Some("main"));
    let pass = check(&triage("pass", "next: define\n"), None).unwrap();
    assert_eq!(pass.next, Next::Define);
    assert_eq!(
        errors(&triage("pass", ""), None),
        ["`next` is missing: a passed triage names the next phase: one of build, define, troubleshoot"]
    );
    assert_eq!(errors(&triage("pass", "next: none\n"), None).len(), 1);
    let other = with("status", "status: split").replace("cause: criteria_failed", "cause: none");
    assert_eq!(
        errors(&other, None),
        ["`status: split` is only for triage's brief.md, not `phase: verify`"]
    );
    let no_reason = triage("split", "").replace("reason: Split into #8 and #9.\n", "");
    assert_eq!(
        errors(&no_reason, None),
        ["`reason` is missing: `status: split` needs one line saying why"]
    );
}

/// A troubleshoot brief.md under `status`, `cause` and `next`.
fn troubleshoot(status: &str, cause: &str, next: &str) -> String {
    format!(
        "---\nschema: ns-artifact/2\nunit: 7-x\nphase: troubleshoot\nstatus: {status}\ncause: {cause}\nnext: {next}\nreason: \"needs redesign: route ns-define\"\nbase: main\nupdated: 2026-10-08T21:14:00Z\n---\nIssue: u\n"
    )
}

#[test]
fn a_troubleshoot_brief_checks_as_triages_brief() {
    for (status, cause, next) in [
        ("pass", "none", "build"),
        ("blocked", "needs_human", "define"),
        ("fail", "needs_human", "human"),
    ] {
        let text = troubleshoot(status, cause, next);
        for want in [None, Some("troubleshoot"), Some("triage")] {
            let t = check(&text, want).unwrap_or_else(|e| panic!("{status} as {want:?}: {e:?}"));
            assert_eq!(t.phase, Phase::Troubleshoot);
        }
        assert_eq!(
            errors(&text, Some("build")),
            ["`phase: troubleshoot` but this artifact is checked as `build`"]
        );
    }
    let t = read(&troubleshoot("pass", "none", "build"), Phase::Triage).unwrap();
    assert_eq!((t.status, t.next), (Status::Pass, Next::Build));
    assert_eq!(
        errors(&troubleshoot("split", "none", "build"), None),
        ["`status: split` is only for triage's brief.md, not `phase: troubleshoot`"]
    );
}

#[test]
fn touches_and_findings_must_be_lists_of_strings() {
    let scalar = triage("split", "").replace("touches:\n  - cli/src/run.rs", "touches: cli/");
    assert_eq!(
        errors(&scalar, None),
        ["`touches` must be a list, one `- item` per line"]
    );
    let mixed = triage("split", "").replace("  - cli/src/run.rs", "  - cli/\n  - {a: 1}");
    assert_eq!(
        errors(&mixed, None),
        ["`touches` must be a list of strings"]
    );
}

fn review(status: &str, cause: &str, findings: &str) -> String {
    format!(
        "---\nschema: ns-artifact/2\nunit: 7-x\nphase: review\nstatus: {status}\ncause: {cause}\nnext: human\nreason: \"Open after at least 2 fix attempts: I3.\"\nsha: abc1234\n{findings}updated: 2026-10-08T21:14:00Z\n---\n"
    )
}

#[test]
fn findings_belong_to_review_and_name_ids() {
    let t = check(
        &review("blocked", "findings_open", "findings: [C1, I3]\n"),
        None,
    )
    .unwrap();
    assert_eq!(t.findings, ["C1", "I3"]);
    assert_eq!(
        errors(&review("blocked", "findings_open", ""), None),
        ["`cause: findings_open` needs `findings`: the open finding ids, such as `[C1, I3]`"]
    );
    assert_eq!(
        errors(
            &review("blocked", "needs_human", "findings: [three]\n"),
            None
        ),
        ["`findings` entry `three` is not a finding id such as `I3`"]
    );
    let on_verify = with("unit", "unit: u\nfindings: [I3]");
    assert_eq!(
        errors(&on_verify, None),
        ["`findings` is for review artifacts, not `phase: verify`"]
    );
}

#[test]
fn shas_and_timestamps_are_checked() {
    assert_eq!(
        errors(&with("sha", "sha: <git rev-parse --short HEAD>"), None),
        ["`sha: <git rev-parse --short HEAD>` is not a commit sha: write `git rev-parse --short HEAD`'s output"]
    );
    assert_eq!(
        errors(&with("unit", "unit: u\ndelta_of: zz"), None),
        ["`delta_of: zz` is not a commit sha: write `git rev-parse --short HEAD`'s output"]
    );
    let delta = check(&with("unit", "unit: u\ndelta_of: 9659e41"), None).unwrap();
    assert_eq!(delta.delta_of.as_deref(), Some("9659e41"));
    assert_eq!(
        errors(&with("updated", "updated: <UTC timestamp>"), None),
        ["`updated: <UTC timestamp>` is not a UTC timestamp such as `2026-10-08T21:14:00Z`"]
    );
    // A sha YAML would read as a number stays verbatim.
    let numeric = check(&with("sha", "sha: 1234567"), None).unwrap();
    assert_eq!(numeric.sha.as_deref(), Some("1234567"));
}

#[test]
fn a_passed_ship_names_its_pr() {
    let ship = |pr: &str| {
        format!("---\nschema: ns-artifact/2\nunit: u\nphase: ship\nstatus: pass\ncause: none\nnext: none\nsha: abc1234\n{pr}updated: 2026-10-08T21:14:00Z\n---\nbody\n")
    };
    assert_eq!(
        errors(&ship(""), None),
        ["`pr` is missing: a passed ship names the PR's URL"]
    );
    let t = check(&ship("pr: https://github.com/o/r/pull/12\n"), Some("ship")).unwrap();
    assert_eq!(t.pr.as_deref(), Some("https://github.com/o/r/pull/12"));
}

#[test]
fn errors_are_all_reported_at_once() {
    let text = "---\nschema: ns-artifact/2\nphase: build\nstatus: fail\ncause: none\n---\n";
    assert_eq!(
        errors(text, Some("build")),
        [
            "`unit` is missing: the unit id, such as `142-uart-timeout`",
            "`updated` is missing: the UTC time written, such as `2026-10-08T21:14:00Z`",
            "`status: fail` needs a cause other than `none`: one of findings_open, criteria_failed, gate_red, base_moved, denied, needs_human, out_of_scope",
            "`status: fail` needs `next`: the phase that picks the unit up, or `human`",
            "`reason` is missing: `status: fail` needs one line saying why",
            "`sha` is missing: `phase: build` names the commit it describes (`git rev-parse --short HEAD`)",
        ]
    );
}

/// A `/1` artifact as `ns run` reads it today: frontmatter with no schema, then a body.
fn v1(status: &str, body: &str) -> String {
    format!("---\nstatus: {status}\nsha: abc1234\n---\n{body}")
}

type Route = (Status, Cause, Next);

fn route(phase: Phase, status: &str, body: &str) -> Route {
    let t = from_v1(&v1(status, body), phase);
    (t.status, t.cause, t.next)
}

#[test]
fn v1_routes_as_decide_does() {
    for (phase, status, body, want) in V1_CASES {
        assert_eq!(
            route(*phase, status, body),
            *want,
            "{} {status} {body:?}",
            phase.as_str()
        );
    }
}

#[test]
fn v1_unknown_statuses_block_on_a_named_reason() {
    for (phase, status) in [
        (Phase::Build, "split"),
        (Phase::Verify, ""),
        (Phase::Ship, "done"),
    ] {
        let t = from_v1(&v1(status, "body"), phase);
        assert_eq!(
            (t.status, t.cause, t.next),
            (Status::Blocked, Cause::NeedsHuman, Next::Human)
        );
        assert_eq!(
            t.reason,
            format!("status {status:?} is not pass, fail or blocked")
        );
    }
}

#[test]
fn v1_keeps_fields_reason_and_warns() {
    let text = "---\nunit: u\nphase: ship\nstatus: fail\nsha: 0e05787\npr: '12'\nbase: develop\ntouches:\n  - a/**\nupdated: 2026-10-08T21:14:00Z\n---\n\n## NO-GO: ns-review must rerun\nlater\n";
    let t = from_v1(text, Phase::Ship);
    assert_eq!(t.schema, Schema::V1);
    assert_eq!(t.reason, "NO-GO: ns-review must rerun");
    assert_eq!(t.sha.as_deref(), Some("0e05787"));
    assert_eq!(t.pr.as_deref(), Some("12"));
    assert_eq!(t.base.as_deref(), Some("develop"));
    assert_eq!(t.touches, ["a/**"]);
    assert_eq!(t.unit.as_deref(), Some("u"));
    assert_eq!(
        t.warnings,
        ["no `schema` key: read as ns-artifact/1; write `schema: ns-artifact/2`"]
    );
    // No frontmatter: the whole text is the body and the status is unknown, as `read_art` has it.
    let none = from_v1("just text\n", Phase::Verify);
    assert_eq!(none.status, Status::Blocked);
    assert_eq!(none.sha, None);
}

#[test]
fn read_picks_the_schema_the_artifact_declares() {
    assert_eq!(
        read(&v1("pass", ""), Phase::Build).unwrap().schema,
        Schema::V1
    );
    assert_eq!(read(VALID, Phase::Verify).unwrap().schema, Schema::V2);
    // A `/2` artifact with text above its frontmatter is invalid, not read as `/1`.
    assert_eq!(
        read(&format!("notes\n{VALID}"), Phase::Verify).unwrap_err(),
        ["frontmatter must be the first thing in the file; found text above it"]
    );
    assert!(read(VALID, Phase::Review).is_err());
}

#[test]
fn first_line_drops_headings_and_caps_length() {
    assert_eq!(
        first_line("\n  ## Needs hardware  \nlater"),
        "Needs hardware"
    );
    assert_eq!(first_line(""), "");
    assert_eq!(first_line(&"x".repeat(300)), "x".repeat(200));
}

#[test]
fn names_round_trip() {
    for p in Phase::ALL {
        assert_eq!(Phase::parse(p.as_str()), Some(*p));
    }
    for s in Status::ALL {
        assert_eq!(Status::parse(s.as_str()), Some(*s));
    }
    for c in Cause::ALL {
        assert_eq!(Cause::parse(c.as_str()), Some(*c));
    }
    for n in Next::ALL {
        assert_eq!(Next::parse(n.as_str()), Some(*n));
    }
    assert_eq!(
        serde_json::to_value(Cause::BaseMoved).unwrap(),
        "base_moved"
    );
    assert_eq!(serde_json::to_value(Schema::V2).unwrap(), "ns-artifact/2");
}

#[test]
fn a_file_line_naming_a_touched_file_needs_a_tag() {
    let touched = [
        "src/inventory/warehouse.py".to_string(),
        "README.md".to_string(),
    ];
    let errors = |body: &str| misfiled(body, &touched);
    let want = |path: &str| {
        vec![format!(
            "a `file:` line names `{path}`, which this unit's diff touches: fix it in the unit, or tag the line `unrelated: <reason>` or `too-big: <lines>`"
        )]
    };
    assert_eq!(
        errors("## Open risks\n- file: stock_levels counts expired reservations: src/inventory/warehouse.py:40, status shows 2\n"),
        want("src/inventory/warehouse.py")
    );
    assert_eq!(errors("file: stale intro: README.md.\n"), want("README.md"));
    assert_eq!(errors("- file: x: `README.md`\n"), want("README.md"));
    // Tagged lines pass.
    assert!(
        errors("- file: x: src/inventory/warehouse.py:40, unrelated: a different feature\n")
            .is_empty()
    );
    assert!(errors("- file: x: src/inventory/warehouse.py:40, too-big: 300\n").is_empty());
    // A tag needs its value.
    assert_eq!(
        errors("- file: x: README.md, unrelated:\n"),
        want("README.md")
    );
    assert_eq!(
        errors("- file: x: README.md, too-big: lots\n"),
        want("README.md")
    );
    // Untouched files, longer names, other kinds of line and prose pass.
    assert!(errors("- file: x: src/inventory/stock.py:9\n").is_empty());
    assert!(errors("- file: x: src/inventory/warehouse.pyc and docs/README.md\n").is_empty());
    assert!(errors("- file: x: src/inventory/warehouse.py.bak\n").is_empty());
    assert!(errors("- note: src/inventory/warehouse.py has an untested path\n").is_empty());
    assert!(errors("- filed: #12 src/inventory/warehouse.py\n").is_empty());
    assert!(errors("A file: src/inventory/warehouse.py in prose\n").is_empty());
    assert!(misfiled("- file: x: README.md\n", &[]).is_empty());
}
