//! The `ns-artifact/2` schema: the typed frontmatter every phase artifact opens with, its
//! checker (`ns artifact check`), and the reader that maps an `ns-artifact/1` artifact (no
//! `schema` key) onto the same fields by today's routing rules.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use regex::Regex;
use serde::Serialize;
use serde_yaml::Value;

use crate::error::SfError;
use crate::frontmatter;

pub const SCHEMA_V2: &str = "ns-artifact/2";
pub const SCHEMA_V1: &str = "ns-artifact/1";

/// The longest `reason` kept, in characters: what `ns run` quotes in a stuck reason.
const REASON_MAX: usize = 200;

macro_rules! names {
    ($(#[$m:meta])* $ty:ident { $($v:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        pub enum $ty { $(#[serde(rename = $s)] $v),+ }

        // Generated for every enum, so some go unused on one of them outside the tests.
        #[allow(dead_code)]
        impl $ty {
            pub const ALL: &'static [$ty] = &[$($ty::$v),+];
            pub const NAMES: &'static [&'static str] = &[$($s),+];

            pub fn as_str(self) -> &'static str {
                match self { $($ty::$v => $s),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some($ty::$v),)+ _ => None }
            }
        }
    };
}

names!(
    /// The phase that wrote the artifact.
    Phase {
        Triage = "triage",
        Build = "build",
        Verify = "verify",
        Review = "review",
        ReviewFix = "review-fix",
        Ship = "ship",
    }
);

names!(
    /// The gate result.
    Status {
        Pass = "pass",
        Fail = "fail",
        Blocked = "blocked",
        Split = "split",
    }
);

names!(
    /// Why the gate did not pass; `none` on a pass.
    Cause {
        None = "none",
        FindingsOpen = "findings_open",
        CriteriaFailed = "criteria_failed",
        GateRed = "gate_red",
        BaseMoved = "base_moved",
        Denied = "denied",
        NeedsHuman = "needs_human",
        OutOfScope = "out_of_scope",
    }
);

names!(
    /// Who picks the unit up next.
    Next {
        None = "none",
        Build = "build",
        Verify = "verify",
        Review = "review",
        Ship = "ship",
        Define = "define",
        Troubleshoot = "troubleshoot",
        Human = "human",
    }
);

/// The schema an artifact was read under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Schema {
    #[serde(rename = "ns-artifact/1")]
    V1,
    #[serde(rename = "ns-artifact/2")]
    V2,
}

/// A checked artifact. Routing reads `status`, `cause` and `next`; `reason` is for humans.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Typed {
    pub schema: Schema,
    pub unit: Option<String>,
    pub phase: Phase,
    pub status: Status,
    pub cause: Cause,
    pub next: Next,
    pub reason: String,
    pub sha: Option<String>,
    pub delta_of: Option<String>,
    pub findings: Vec<String>,
    pub pr: Option<String>,
    pub base: Option<String>,
    pub touches: Vec<String>,
    pub updated: Option<String>,
    /// Read as `/1`, or anything else worth telling the operator; never a reason to reject.
    pub warnings: Vec<String>,
}

fn one_of(key: &str, value: &str, names: &[&str]) -> String {
    format!("`{key}: {value}` is not one of {}", names.join(", "))
}

/// The frontmatter's YAML text and mapping, or the errors that stop any further check.
fn frontmatter_of(text: &str) -> Result<(&str, serde_yaml::Mapping), Vec<String>> {
    let Some((yaml, _)) = frontmatter::split(text) else {
        let fence = text.lines().position(|l| l.trim_end() == "---");
        return Err(vec![match fence {
            Some(0) => "frontmatter has no closing `---` line".to_string(),
            Some(_) => "frontmatter must be the first thing in the file; found text above it"
                .to_string(),
            None => "no frontmatter: the file must open with a `---` line, the ns-artifact/2 keys, then `---`"
                .to_string(),
        }]);
    };
    match serde_yaml::from_str::<Value>(yaml) {
        Ok(Value::Mapping(m)) => Ok((yaml, m)),
        Ok(Value::Null) => Err(vec!["frontmatter is empty".into()]),
        Ok(_) => Err(vec!["frontmatter must be `key: value` lines".into()]),
        Err(e) => Err(vec![format!(
            "frontmatter is not valid YAML: {e}; quote a value that holds `: ` or ` #`, such as `reason: \"...\"`"
        )]),
    }
}

/// A list of strings under `key`: absent is empty, anything but a list of strings is an error.
fn string_list(map: &serde_yaml::Mapping, key: &str, errors: &mut Vec<String>) -> Vec<String> {
    match map.get(key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Sequence(items)) => {
            let strings: Vec<String> = items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if strings.len() != items.len() {
                errors.push(format!("`{key}` must be a list of strings"));
            }
            strings
        }
        Some(_) => {
            errors.push(format!("`{key}` must be a list, one `- item` per line"));
            Vec::new()
        }
    }
}

/// `reason` as YAML reads it, so a `#` inside a quoted reason stays part of it.
fn reason_of(map: &serde_yaml::Mapping, yaml: &str, errors: &mut Vec<String>) -> String {
    let reason = match map.get("reason") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(_) => frontmatter::raw_field(yaml, "reason").unwrap_or_default(),
    };
    if reason.contains('\n') {
        errors.push("`reason` must be one line".into());
    }
    if reason.chars().count() > REASON_MAX {
        errors.push(format!("`reason` is longer than {REASON_MAX} characters"));
    }
    reason
}

/// The value of an enum key, as written. A missing or unknown value is an error unless `default`.
fn enum_field<T: Copy>(
    yaml: &str,
    key: &str,
    names: &[&str],
    parse: fn(&str) -> Option<T>,
    default: Option<T>,
    errors: &mut Vec<String>,
) -> Option<T> {
    match frontmatter::raw_field(yaml, key) {
        None if default.is_some() => default,
        None => {
            errors.push(format!("`{key}` is missing: one of {}", names.join(", ")));
            None
        }
        Some(v) => {
            let parsed = parse(&v);
            if parsed.is_none() {
                errors.push(one_of(key, &v, names));
            }
            parsed
        }
    }
}

fn is_sha(s: &str) -> bool {
    s.len() >= 4 && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_utc_timestamp(s: &str) -> bool {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?Z$")
        .expect("valid regex")
        .is_match(s)
}

/// The phases whose artifact names the commit it describes.
fn needs_sha(p: Phase) -> bool {
    match p {
        Phase::Triage => false,
        Phase::Build | Phase::Verify | Phase::Review | Phase::ReviewFix | Phase::Ship => true,
    }
}

fn is_review(p: Phase) -> bool {
    matches!(p, Phase::Review | Phase::ReviewFix)
}

/// Validate an `ns-artifact/2` artifact. `expected_phase` is the phase the caller ran; the
/// artifact must name it. Every error is one line saying what to change.
pub fn check(text: &str, expected_phase: Option<&str>) -> Result<Typed, Vec<String>> {
    let (yaml, map) = frontmatter_of(text)?;
    let mut errors = Vec::new();
    match frontmatter::raw_field(yaml, "schema") {
        None => errors.push(format!(
            "`schema` is missing: add `schema: {SCHEMA_V2}` (an artifact with no schema reads as {SCHEMA_V1})"
        )),
        Some(s) if s != SCHEMA_V2 => {
            errors.push(format!("`schema: {s}` is not {SCHEMA_V2}"));
        }
        Some(_) => {}
    }
    let raw = |k: &str| frontmatter::raw_field(yaml, k);
    let unit = raw("unit");
    if unit.is_none() {
        errors.push("`unit` is missing: the unit id, such as `142-uart-timeout`".into());
    }
    let phase = enum_field(yaml, "phase", Phase::NAMES, Phase::parse, None, &mut errors);
    if let (Some(p), Some(want)) = (phase, expected_phase) {
        if p.as_str() != want {
            errors.push(format!(
                "`phase: {}` but this artifact is checked as `{want}`",
                p.as_str()
            ));
        }
    }
    let status = enum_field(
        yaml,
        "status",
        Status::NAMES,
        Status::parse,
        None,
        &mut errors,
    );
    let cause = enum_field(yaml, "cause", Cause::NAMES, Cause::parse, None, &mut errors);
    let next = enum_field(
        yaml,
        "next",
        Next::NAMES,
        Next::parse,
        Some(Next::None),
        &mut errors,
    );
    let reason = reason_of(&map, yaml, &mut errors);
    let findings = string_list(&map, "findings", &mut errors);
    let touches = string_list(&map, "touches", &mut errors);
    let typed = Typed {
        schema: Schema::V2,
        unit,
        phase: phase.unwrap_or(Phase::Triage),
        status: status.unwrap_or(Status::Blocked),
        cause: cause.unwrap_or(Cause::NeedsHuman),
        next: next.unwrap_or(Next::None),
        reason,
        sha: raw("sha"),
        delta_of: raw("delta_of"),
        findings,
        pr: raw("pr"),
        base: raw("base"),
        touches,
        updated: raw("updated"),
        warnings: Vec::new(),
    };
    check_values(&typed, &mut errors);
    if let (Some(phase), Some(status), Some(cause), Some(_)) = (phase, status, cause, next) {
        check_consistency(&typed, phase, status, cause, &mut errors);
    }
    if errors.is_empty() {
        Ok(typed)
    } else {
        Err(errors)
    }
}

/// Values whose shape is checkable on their own: shas, timestamps, finding ids.
fn check_values(t: &Typed, errors: &mut Vec<String>) {
    match &t.updated {
        None => errors.push(
            "`updated` is missing: the UTC time written, such as `2026-10-08T21:14:00Z`".into(),
        ),
        Some(u) if !is_utc_timestamp(u) => errors.push(format!(
            "`updated: {u}` is not a UTC timestamp such as `2026-10-08T21:14:00Z`"
        )),
        Some(_) => {}
    }
    for (key, v) in [("sha", &t.sha), ("delta_of", &t.delta_of)] {
        if let Some(v) = v.as_deref().filter(|v| !is_sha(v)) {
            errors.push(format!(
                "`{key}: {v}` is not a commit sha: write `git rev-parse --short HEAD`'s output"
            ));
        }
    }
    let id = Regex::new(r"^[A-Z]+\d+$").expect("valid regex");
    for f in t.findings.iter().filter(|f| !id.is_match(f)) {
        errors.push(format!(
            "`findings` entry `{f}` is not a finding id such as `I3`"
        ));
    }
}

/// The rules that tie status, cause, next and the phase together.
fn check_consistency(
    t: &Typed,
    phase: Phase,
    status: Status,
    cause: Cause,
    errors: &mut Vec<String>,
) {
    let s = status.as_str();
    match status {
        Status::Pass if cause != Cause::None => errors.push(format!(
            "`status: pass` needs `cause: none`, found `cause: {}`",
            cause.as_str()
        )),
        Status::Fail | Status::Blocked => {
            if cause == Cause::None {
                errors.push(format!(
                    "`status: {s}` needs a cause other than `none`: one of {}",
                    Cause::NAMES[1..].join(", ")
                ));
            }
            if t.next == Next::None {
                errors.push(format!(
                    "`status: {s}` needs `next`: the phase that picks the unit up, or `human`"
                ));
            }
        }
        Status::Split if phase != Phase::Triage => errors.push(format!(
            "`status: split` is only for triage's brief.md, not `phase: {}`",
            phase.as_str()
        )),
        Status::Pass | Status::Split => {}
    }
    if status != Status::Pass && t.reason.is_empty() {
        errors.push(format!(
            "`reason` is missing: `status: {s}` needs one line saying why"
        ));
    }
    if phase == Phase::Triage && status == Status::Pass && t.next == Next::None {
        errors.push("`next` is missing: a passed triage names the next phase: one of build, define, troubleshoot".into());
    }
    if needs_sha(phase) && t.sha.is_none() {
        errors.push(format!(
            "`sha` is missing: `phase: {}` names the commit it describes (`git rev-parse --short HEAD`)",
            phase.as_str()
        ));
    }
    if phase == Phase::Ship && status == Status::Pass && t.pr.is_none() {
        errors.push("`pr` is missing: a passed ship names the PR's URL".into());
    }
    if !t.findings.is_empty() && !is_review(phase) {
        errors.push(format!(
            "`findings` is for review artifacts, not `phase: {}`",
            phase.as_str()
        ));
    }
    if cause == Cause::FindingsOpen && t.findings.is_empty() {
        errors.push(
            "`cause: findings_open` needs `findings`: the open finding ids, such as `[C1, I3]`"
                .into(),
        );
    }
}

/// The body's first non-empty line: `/1`'s one-sentence reason. A heading marker is dropped and
/// its text kept.
pub fn first_line(body: &str) -> String {
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim_start())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(REASON_MAX)
        .collect()
}

/// A `/1` first line starting `base moved`, after an optional `NO-GO:`.
fn base_moved(body: &str) -> bool {
    let line = first_line(body).to_lowercase();
    let rest = line.strip_prefix("no-go:").unwrap_or(&line);
    rest.trim_start().starts_with("base moved")
}

/// The phase a `/1` failed ship names first: `verify` or `review`.
fn named_phase(body: &str) -> Option<Phase> {
    let lower = body.to_lowercase();
    match (lower.find("verify"), lower.find("review")) {
        (Some(a), Some(b)) => Some(if a < b { Phase::Verify } else { Phase::Review }),
        (Some(_), None) => Some(Phase::Verify),
        (None, Some(_)) => Some(Phase::Review),
        (None, None) => None,
    }
}

/// The phase after `p` once it passes.
fn after(p: Phase) -> Next {
    match p {
        Phase::Triage => Next::Build,
        Phase::Build => Next::Verify,
        Phase::Verify => Next::Review,
        Phase::Review | Phase::ReviewFix => Next::Ship,
        Phase::Ship => Next::None,
    }
}

/// A `/1` fail's cause and next, by `decide`'s rules in `run/currency.rs`.
fn v1_fail(phase: Phase, body: &str) -> (Cause, Next) {
    match phase {
        Phase::Triage => (Cause::NeedsHuman, Next::Human),
        Phase::Build => (Cause::GateRed, Next::Build),
        Phase::Verify | Phase::Review | Phase::ReviewFix | Phase::Ship if base_moved(body) => {
            (Cause::BaseMoved, Next::Build)
        }
        Phase::Verify => (Cause::CriteriaFailed, Next::Build),
        Phase::Review | Phase::ReviewFix => (Cause::FindingsOpen, Next::Build),
        Phase::Ship => match named_phase(body) {
            Some(Phase::Verify) => (Cause::CriteriaFailed, Next::Verify),
            Some(_) => (Cause::CriteriaFailed, Next::Review),
            None => (Cause::NeedsHuman, Next::Human),
        },
    }
}

/// Read an `ns-artifact/1` artifact (no `schema` key) as `/2`, routing it as `decide` does
/// today. `phase` is the phase whose file it is: `/1` artifacts don't always name it.
// `ns run` reads artifacts through `read` once `decide` routes on cause and next.
#[allow(dead_code)]
pub fn from_v1(text: &str, phase: Phase) -> Typed {
    let (yaml, body) = match (frontmatter::parse(text), frontmatter::split(text)) {
        (Ok(Some(_)), Some((yaml, body))) => (Some(yaml), body),
        _ => (None, text),
    };
    let raw = |k: &str| yaml.and_then(|y| frontmatter::raw_field(y, k));
    let written = raw("status").unwrap_or_default();
    let mut reason = first_line(body);
    let mut warnings = vec![format!(
        "no `schema` key: read as {SCHEMA_V1}; write `schema: {SCHEMA_V2}`"
    )];
    let (status, cause, next) = match (Status::parse(&written), phase) {
        (Some(Status::Pass), _) => (Status::Pass, Cause::None, after(phase)),
        (Some(Status::Split), Phase::Triage) => (Status::Split, Cause::None, Next::None),
        (Some(Status::Blocked), Phase::Verify)
            if reason.to_lowercase().starts_with("denied by") =>
        {
            (Status::Blocked, Cause::Denied, Next::Human)
        }
        (Some(Status::Blocked), _) => (Status::Blocked, Cause::NeedsHuman, Next::Human),
        (Some(Status::Fail), _) => {
            let (cause, next) = v1_fail(phase, body);
            (Status::Fail, cause, next)
        }
        (Some(Status::Split), _) | (None, _) => {
            reason = format!("status {written:?} is not pass, fail or blocked");
            warnings.push(reason.clone());
            (Status::Blocked, Cause::NeedsHuman, Next::Human)
        }
    };
    let list = |k: &str| -> Vec<String> {
        frontmatter::parse(text)
            .ok()
            .flatten()
            .and_then(|v| v.get(k).and_then(Value::as_sequence).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    };
    Typed {
        schema: Schema::V1,
        unit: raw("unit"),
        phase,
        status,
        cause,
        next,
        reason,
        sha: raw("sha"),
        delta_of: None,
        findings: Vec::new(),
        pr: raw("pr"),
        base: raw("base"),
        touches: list("touches"),
        updated: raw("updated"),
        warnings,
    }
}

/// Read `phase`'s artifact under whichever schema it declares: `check` for one that names a
/// `schema`, wherever the key sits, and `from_v1` for one that names none.
#[allow(dead_code)]
pub fn read(text: &str, phase: Phase) -> Result<Typed, Vec<String>> {
    if text.lines().any(|l| l.trim_start().starts_with("schema:")) {
        check(text, Some(phase.as_str()))
    } else {
        Ok(from_v1(text, phase))
    }
}

/// `ns artifact check`: print the result as JSON; exit 0 valid, 1 invalid, 2 unreadable file.
pub fn cli(file: &Path, phase: Option<&str>) -> Result<ExitCode> {
    let text = fs::read_to_string(file).map_err(|e| {
        SfError::usage(
            format!("cannot read {}: {e}", file.display()),
            "ns artifact check .ns/$NS_UNIT/evidence.md --phase verify",
        )
    })?;
    let result = check(&text, phase);
    let as_written = |k: &str| frontmatter::field(&text, k);
    let out = match &result {
        Ok(t) => serde_json::json!({
            "ok": true,
            "file": file,
            "schema": t.schema,
            "phase": t.phase,
            "status": t.status,
            "cause": t.cause,
            "next": t.next,
            "errors": [],
        }),
        Err(errors) => serde_json::json!({
            "ok": false,
            "file": file,
            "schema": as_written("schema"),
            "phase": as_written("phase"),
            "status": as_written("status"),
            "cause": as_written("cause"),
            "next": as_written("next"),
            "errors": errors,
        }),
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ["`phase: qa` is not one of triage, build, verify, review, review-fix, ship"]
        );
        assert_eq!(
            errors(&with("next", "next: later"), None),
            ["`next: later` is not one of none, build, verify, review, ship, define, troubleshoot, human"]
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
            ["`phase` is missing: one of triage, build, verify, review, review-fix, ship"]
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
        let other =
            with("status", "status: split").replace("cause: criteria_failed", "cause: none");
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

    /// The cases of `run::currency`'s `state_table` and base-moved tests, artifact by artifact.
    #[test]
    fn v1_routes_as_decide_does() {
        use Cause as C;
        use Next as N;
        use Phase as P;
        use Status as S;
        let cases: &[(Phase, &str, &str, Route)] = &[
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
        for (phase, status, body, want) in cases {
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
}
