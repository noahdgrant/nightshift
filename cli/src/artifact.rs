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

        #[allow(
            dead_code,
            reason = "generated for every enum, so some go unused on one of them outside the tests"
        )]
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
mod tests;
