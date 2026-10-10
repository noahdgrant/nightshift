//! Parse the findings out of a `review.md` (format in skills/ns-review/references/review-md.md).
//!
//! Lenient by design: most artifacts on disk predate the field list, so field names match
//! case-insensitively, `**Field:**` bold is allowed, backticks are stripped, and the older
//! one-line `- S1. text (axis). Open.` form inside a severity section is read too.

use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde::Serialize;

pub const AXES: [&str; 8] = [
    "correctness",
    "readability",
    "architecture",
    "security",
    "performance",
    "tests",
    "spec",
    "comments",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    Important,
    Suggestion,
}

impl Severity {
    fn from_letter(c: &str) -> Severity {
        match c {
            "C" => Severity::Critical,
            "I" => Severity::Important,
            _ => Severity::Suggestion,
        }
    }

    fn from_section(heading: &str) -> Option<Severity> {
        let h = heading.to_lowercase();
        [
            ("critical", Severity::Critical),
            ("important", Severity::Important),
            ("suggestion", Severity::Suggestion),
        ]
        .into_iter()
        .find(|(name, _)| h.starts_with(name))
        .map(|(_, s)| s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Open,
    Fixed,
    Dismissed,
    Deferred,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Changed,
    PreExisting,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub location: Option<String>,
    pub axes: Vec<String>,
    /// No `Axis:` field: `axes` came from the `Raised by:` reviewer names.
    pub axes_from_raised_by: bool,
    pub scope: Scope,
    /// The review pass that first raised it (`Cycle:`), 0 for the first review.
    pub cycle: Option<u32>,
    pub status: Status,
    /// The fix cycle named in `fixed (cycle <n>, ...)`.
    pub fixed_cycle: Option<u32>,
}

impl Finding {
    /// Critical or Important, and not dismissed.
    pub fn blocking(&self) -> bool {
        self.severity != Severity::Suggestion && self.status != Status::Dismissed
    }

    /// Blocking and not `pre-existing`: what holds the unit back. A pre-existing finding is an
    /// escape and never blocks (docs/DESIGN.md D29).
    pub fn against_unit(&self) -> bool {
        self.blocking() && self.scope != Scope::PreExisting
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// `I3.`, `**I3**:`, or a range `I1-I7` that one heading covers.
const ID: &str = r"\**([CIS])(\d+)(?:\s*[-–]\s*([CIS])(\d+))?\**[.:)]?";

/// Largest range a heading may cover; anything wider is a typo.
const MAX_RANGE: u32 = 50;

/// The ids a matched `ID` names: one, or each in a same-letter ascending range.
fn ids(c: &Captures) -> Vec<String> {
    let letter = &c[1];
    let from: u32 = c[2].parse().unwrap_or(0);
    let to = c
        .get(4)
        .filter(|_| c.get(3).is_some_and(|l| l.as_str() == letter))
        .and_then(|n| n.as_str().parse::<u32>().ok())
        .filter(|to| *to > from && to - from <= MAX_RANGE)
        .unwrap_or(from);
    (from..=to).map(|n| format!("{letter}{n}")).collect()
}

fn heading_id() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&format!(r"^#{{2,6}}\s+{ID}\s+(.*)$")).unwrap())
}

fn bullet_id() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&format!(r"^[-*]\s+{ID}\s+(.*)$")).unwrap())
}

/// Field names a finding's lines use; old artifacts pack several on one line.
fn field_key() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)\b(status|location|raised by|axis|axes|scope|cycle|finding|evidence|fix):\**\s*",
    )
}

/// A field value without markup, cut where the next packed field starts.
fn clean(v: &str) -> String {
    let v = v.replace(['`', '*'], "");
    let end = field_key().find(&v).map_or(v.len(), |m| m.start());
    v[..end]
        .trim()
        .trim_end_matches(['.', ',', ';'])
        .trim_end()
        .to_string()
}

/// `- Name: value` (also `- **Name:** value`): the lowercased name and the cleaned value.
fn field(line: &str) -> Option<(String, String)> {
    let l = line.trim_start().strip_prefix(['-', '*'])?.replace('*', "");
    let (k, v) = l.split_once(':')?;
    Some((k.trim().to_lowercase(), clean(v)))
}

/// `Name: value` anywhere in a line, for old artifacts that pack several fields on one line.
fn embedded(lines: &[&str], name: &str) -> Option<String> {
    lines.iter().find_map(|l| {
        field_key()
            .captures_iter(l)
            .find(|c| c[1].eq_ignore_ascii_case(name))
            .map(|c| clean(&l[c.get(0).unwrap().end()..]))
    })
}

fn first_number(v: &str) -> Option<u32> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\d+").find(v)?.as_str().parse().ok()
}

fn fixed_cycle(v: &str) -> Option<u32> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(?i)cycles?\s+(\d+)").captures(v)?[1].parse().ok()
}

fn status_of(v: &str) -> Status {
    let l = v.trim().to_lowercase();
    [
        ("open", Status::Open),
        ("fixed", Status::Fixed),
        ("dismissed", Status::Dismissed),
        ("deferred", Status::Deferred),
    ]
    .into_iter()
    .find(|(word, _)| l.starts_with(word))
    .map_or(Status::Unknown, |(_, s)| s)
}

/// A one-line finding with no `Status:`: the first status word it mentions, fixed first.
fn status_in_prose(v: &str) -> Status {
    let l = v.to_lowercase();
    [
        ("fixed", Status::Fixed),
        ("dismissed", Status::Dismissed),
        ("deferred", Status::Deferred),
        ("open", Status::Open),
    ]
    .into_iter()
    .find(|(word, _)| l.contains(word))
    .map_or(Status::Unknown, |(_, s)| s)
}

fn scope_of(v: &str) -> Scope {
    let l = v.to_lowercase().replace([' ', '_'], "-");
    if l.starts_with("changed") {
        Scope::Changed
    } else if l.contains("pre-existing") || l.contains("preexisting") {
        Scope::PreExisting
    } else {
        Scope::Unknown
    }
}

/// The review axes named in `text`, in order of first mention.
pub fn axes_in(text: &str) -> Vec<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = re(
        &R,
        r"(?i)\b(correctness|readability|architecture|arch|security|performance|perf|tests?|spec|comments?)\b",
    );
    let mut axes: Vec<String> = Vec::new();
    for m in r.find_iter(text) {
        let a = match m.as_str().to_lowercase().as_str() {
            "test" => "tests".to_string(),
            "comment" => "comments".to_string(),
            "perf" => "performance".to_string(),
            "arch" => "architecture".to_string(),
            a => a.to_string(),
        };
        if !axes.contains(&a) {
            axes.push(a);
        }
    }
    axes
}

/// The text inside every `( ... )` of a line, joined.
fn parenthesised(line: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\(([^()]*)\)")
        .captures_iter(line)
        .map(|c| c[1].to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

struct Block<'a> {
    ids: Vec<String>,
    severity: Severity,
    title: String,
    lines: Vec<&'a str>,
    one_line: bool,
}

impl<'a> Block<'a> {
    fn start(c: &Captures, lines: Vec<&'a str>, one_line: bool) -> Self {
        Block {
            ids: ids(c),
            severity: Severity::from_letter(&c[1]),
            title: c[5].to_string(),
            lines,
            one_line,
        }
    }

    /// One finding per id; a range heading's ids share its fields.
    fn finish(self) -> Vec<Finding> {
        let fields: Vec<(String, String)> = self.lines.iter().filter_map(|l| field(l)).collect();
        let get = |names: &[&str]| {
            fields
                .iter()
                .find(|(k, _)| names.contains(&k.as_str()))
                .map(|(_, v)| v.clone())
                .or_else(|| names.iter().find_map(|n| embedded(&self.lines, n)))
        };
        let status_text = get(&["status"]);
        let status = match (&status_text, self.one_line) {
            (Some(s), _) => status_of(s),
            (None, true) => status_in_prose(&self.title),
            (None, false) => Status::Unknown,
        };
        let axis_field = get(&["axis", "axes"])
            .map(|v| axes_in(&v))
            .filter(|a| !a.is_empty());
        let axes_from_raised_by = axis_field.is_none();
        let axes = axis_field.unwrap_or_else(|| match get(&["raised by"]) {
            Some(r) => axes_in(&r),
            None if self.one_line => axes_in(&parenthesised(&self.title)),
            None => Vec::new(),
        });
        let finding = Finding {
            id: String::new(),
            severity: self.severity,
            title: self.title.replace(['`', '*'], "").trim().to_string(),
            location: get(&["location"]).filter(|l| !l.is_empty()),
            axes,
            axes_from_raised_by,
            scope: get(&["scope"]).map_or(Scope::Unknown, |s| scope_of(&s)),
            cycle: get(&["cycle"]).and_then(|c| first_number(&c)),
            fixed_cycle: status_text
                .as_deref()
                .or(self.one_line.then_some(self.title.as_str()))
                .filter(|_| status == Status::Fixed)
                .and_then(fixed_cycle),
            status,
        };
        self.ids
            .into_iter()
            .map(|id| Finding {
                id,
                ..finding.clone()
            })
            .collect()
    }
}

/// Every finding in a review artifact, in file order. Code fences are skipped.
pub fn parse(text: &str) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut block: Option<Block> = None;
    let mut section: Option<Severity> = None;
    let mut fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if line.starts_with('#') {
            found.extend(block.take().into_iter().flat_map(Block::finish));
            if let Some(c) = heading_id().captures(line) {
                block = Some(Block::start(&c, Vec::new(), false));
            } else if line.starts_with("## ") {
                section = Severity::from_section(line.trim_start_matches('#').trim());
            }
            continue;
        }
        let bullet = section.and_then(|_| bullet_id().captures(line));
        if let Some(c) = bullet.filter(|_| block.is_none()) {
            found.extend(Block::start(&c, vec![line], true).finish());
        } else if let Some(b) = block.as_mut() {
            b.lines.push(line);
        }
    }
    found.extend(block.into_iter().flat_map(Block::finish));
    found
}

/// One Critical or Important finding a free-form `cycle-<n>.md` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleEntry {
    pub id: String,
    /// Axes named before the line's first `:` (`- I1 perf: ...`, `I2 Important (spec): ...`).
    pub axes: Vec<String>,
}

/// The Critical and Important ids named at the start of a line in a `cycle-<n>.md`
/// (`## I1. ...`, `- I1 perf: ...`, `I1 Important (...)`), first mention of each.
pub fn cycle_entries(text: &str) -> Vec<CycleEntry> {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = re(&R, r"^\s*(?:#{1,6}\s*|[-*]\s+)?\**([CI]\d+)\b");
    let mut entries: Vec<CycleEntry> = Vec::new();
    for line in text.lines() {
        let Some(c) = r.captures(line) else {
            continue;
        };
        if entries.iter().all(|e| e.id != c[1]) {
            let head = line.split(':').next().unwrap_or(line);
            entries.push(CycleEntry {
                id: c[1].to_string(),
                axes: axes_in(head),
            });
        }
    }
    entries
}

/// Headings that are not findings yet carry a `Status:` field, such as `### C3-1. ...` or
/// `### D1. ...`: findings in a shape the parser can't number.
pub fn stray_statuses(text: &str) -> usize {
    let mut count = 0;
    let mut stray = false;
    let mut fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        } else if fence {
        } else if line.starts_with('#') {
            stray = line.starts_with("##") && !heading_id().is_match(line);
        } else if stray && field(line).is_some_and(|(k, _)| k == "status") {
            count += 1;
            stray = false;
        }
    }
    count
}

/// Changed lines from the Summary's `Change size:` line: `<n> insertions, <m> deletions`
/// sums both; otherwise the first `<n> lines`. Commas in numbers are allowed.
pub fn change_size(text: &str) -> Option<u64> {
    static R: OnceLock<Regex> = OnceLock::new();
    let line = text
        .lines()
        .find(|l| l.to_lowercase().replace('*', "").contains("change size"))?;
    let r = re(
        &R,
        r"(?i)(\d[\d,]*)\s+(insertions?|deletions?|(?:changed\s+|diff\s+)?lines)\b",
    );
    let mut ins = None;
    let mut del = None;
    let mut lines = None;
    for c in r.captures_iter(line) {
        let n: u64 = c[1].replace(',', "").parse().ok()?;
        let kind = c[2].to_lowercase();
        if kind.starts_with("insertion") {
            ins.get_or_insert(n);
        } else if kind.starts_with("deletion") {
            del.get_or_insert(n);
        } else {
            lines.get_or_insert(n);
        }
    }
    match (ins, del) {
        (None, None) => lines,
        (i, d) => Some(i.unwrap_or(0) + d.unwrap_or(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Finding {
        let mut f = parse(text);
        assert_eq!(f.len(), 1, "{f:?}");
        f.remove(0)
    }

    #[test]
    fn reads_every_field_of_the_agreed_format() {
        let f = one("\
## Important
### I3. Drops the `SKU` on retry
- Location: `cli/src/x.rs:42`
- Axis: correctness, tests
- Scope: pre-existing
- Cycle: 2
- Raised by: security (provider-a)
- Finding: text
- Status: fixed (cycle 3, abc1234)
");
        assert_eq!(
            f,
            Finding {
                id: "I3".into(),
                severity: Severity::Important,
                title: "Drops the SKU on retry".into(),
                location: Some("cli/src/x.rs:42".into()),
                axes: vec!["correctness".into(), "tests".into()],
                axes_from_raised_by: false,
                scope: Scope::PreExisting,
                cycle: Some(2),
                status: Status::Fixed,
                fixed_cycle: Some(3),
            }
        );
        assert!(f.blocking());
    }

    #[test]
    fn field_names_are_lenient() {
        let f = one("\
### C1: Bold fields
- **LOCATION:** `a.py:1`
- **axes:** Security
- **Scope:** changed
- **cycle:** 0
- **Status:** Open
");
        assert_eq!(f.severity, Severity::Critical);
        assert_eq!(f.location.as_deref(), Some("a.py:1"));
        assert_eq!(f.axes, ["security"]);
        assert_eq!(f.scope, Scope::Changed);
        assert_eq!(f.cycle, Some(0));
        assert_eq!(f.status, Status::Open);
        assert_eq!(f.fixed_cycle, None);
    }

    #[test]
    fn without_axis_the_axes_come_from_raised_by() {
        let f = one("### I1. t\n- Raised by: lead (correctness), cycle 2 test, comment, Correctness again. Cross-provider.\n- Status: open\n");
        assert_eq!(f.axes, ["correctness", "tests", "comments"]);
        assert!(f.axes_from_raised_by);
        let f = one("### I1. t\n- Axis: none of them\n- Raised by: spec\n");
        assert_eq!(f.axes, ["spec"]);
        assert!(f.axes_from_raised_by);
        let f = one("### I1. t\n- Status: open\n");
        assert!(f.axes.is_empty());
        assert_eq!(f.scope, Scope::Unknown);
        assert_eq!(f.cycle, None);
        assert_eq!(f.location, None);
    }

    #[test]
    fn fields_packed_on_one_line_are_found() {
        let f = one("### I2. Path used as a pathspec\n- Location: `a.rs:3`. Raised by: security, tests. Status: fixed (cycle 1, 6620ac2)\n");
        assert_eq!(f.status, Status::Fixed);
        assert_eq!(f.fixed_cycle, Some(1));
        assert_eq!(f.axes, ["security", "tests"]);
        assert_eq!(f.location.as_deref(), Some("a.rs:3"));
        let f = one("### I1. t\n- Location: `a.rs:5`; Fix: rename. Evidence: read it\n- Status: open. Fix: later\n");
        assert_eq!(f.location.as_deref(), Some("a.rs:5"));
        assert_eq!(f.status, Status::Open);
        let f = one("### I1. Fix: keep a title whole.\n");
        assert_eq!(f.title, "Fix: keep a title whole.");
    }

    #[test]
    fn scopes_locations_and_fixed_cycles() {
        let sc = |s: &str| one(&format!("### I1. t\n- Scope: {s}\n")).scope;
        assert_eq!(sc("Pre existing"), Scope::PreExisting);
        assert_eq!(sc("preexisting code"), Scope::PreExisting);
        assert_eq!(sc("pre_existing"), Scope::PreExisting);
        assert_eq!(sc("Changed lines"), Scope::Changed);
        assert_eq!(sc("n/a"), Scope::Unknown);
        assert_eq!(sc("unchanged"), Scope::Unknown);
        assert_eq!(one("### I1. t\n- Location: ``\n").location, None);
        let f = one("### I1. t\n- Status: fixed (cycles 1 and 3, abc)\n");
        assert_eq!(f.fixed_cycle, Some(1));
        let f = one("### I1. t\n- Status: open since cycle 2\n");
        assert_eq!((f.status, f.fixed_cycle), (Status::Open, None));
        let f = one("### I1. t\n- Cycle: pass 2 (second)\n");
        assert_eq!(f.cycle, Some(2));
    }

    #[test]
    fn statuses() {
        let st = |s: &str| one(&format!("### I1. t\n- Status: {s}\n")).status;
        assert_eq!(st("open (Suggestion). Fix: x"), Status::Open);
        assert_eq!(st("Fixed (cycle 2 `779a955`)"), Status::Fixed);
        assert_eq!(st("dismissed: noted: premature"), Status::Dismissed);
        assert_eq!(st("deferred: #99"), Status::Deferred);
        assert_eq!(st("accepted"), Status::Unknown);
        assert!(!one("### I1. t\n- Status: dismissed: wrong\n").blocking());
        assert!(!one("### S1. t\n- Status: open\n").blocking());
        assert!(one("### I1. t\n- Status: deferred: #9\n").blocking());
    }

    #[test]
    fn a_heading_ends_a_finding_and_sets_the_section() {
        let f = parse(
            "\
## Critical
### C1. One
- Status: open
### Cycle 3 notes
- Status: fixed
## Important
### I1. Two
## Suggestion
- Status: open
### S1. Three
## Mutation check
- S9. not a finding outside a severity section
",
        );
        let ids: Vec<_> = f.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["C1", "I1", "S1"]);
        assert_eq!(f[0].status, Status::Open);
        assert_eq!(f[1].status, Status::Unknown);
    }

    #[test]
    fn one_line_findings_in_a_severity_section() {
        let f = parse(
            "\
## Suggestion
- S1. Mutation reruns cost time (performance). Left open.
- S2 `timeout` leaves grandchildren (security, correctness). Fixed (cycle 1).
- **S3**: Narration (comments). Status: dismissed: preference
- S4. No status word here
- S5. Was open, now fixed (cycle 2).
- not a finding
### Grouped notes
- S6. Still in the Suggestion section
",
        );
        assert_eq!(f.len(), 6);
        assert_eq!((f[4].status, f[4].fixed_cycle), (Status::Fixed, Some(2)));
        assert_eq!(f[5].id, "S6");
        assert_eq!((f[0].id.as_str(), f[0].status), ("S1", Status::Open));
        assert_eq!(f[0].axes, ["performance"]);
        assert!(f[0].axes_from_raised_by);
        assert_eq!(
            f[0].title,
            "Mutation reruns cost time (performance). Left open."
        );
        assert_eq!((f[1].status, f[1].fixed_cycle), (Status::Fixed, Some(1)));
        assert_eq!(f[1].axes, ["security", "correctness"]);
        assert_eq!(f[2].status, Status::Dismissed);
        assert_eq!(f[2].axes, ["comments"]);
        assert_eq!(f[3].status, Status::Unknown);
        assert_eq!(f[3].severity, Severity::Suggestion);
    }

    #[test]
    fn a_bullet_id_inside_a_heading_finding_is_part_of_it() {
        let f = parse("## Important\n### I1. t\n- I2. mentioned in passing\n- Status: open\n");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].status, Status::Open);
    }

    #[test]
    fn code_fences_are_skipped() {
        let f = parse("```markdown\n### C1. example\n- Status: open\n```\n### I1. real\n");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "I1");
    }

    #[test]
    fn a_range_heading_is_one_finding_per_id() {
        let f = parse(
            "\
## Important
### I1-I3 (cycle 1). Grouped
- Raised by: tests
- Status: fixed (cycle 1, abc)
### I4–I5. En dash
- I6-I7. Bullet range inside a heading finding stays part of it
## Suggestion
- S1-S2. A bullet range
- C3-1. Not a range: no letter on the end
- S5-S3. Descending is just the first
- S9-S99. Too wide is just the first
- S10-I12. Mixed letters is just the first
### C3-1. Cycle 3, item 1: not a finding
",
        );
        let ids: Vec<_> = f.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            ["I1", "I2", "I3", "I4", "I5", "S1", "S2", "S5", "S9", "S10"]
        );
        assert!(f[..3].iter().all(|x| x.title == "(cycle 1). Grouped"
            && x.axes == ["tests"]
            && x.fixed_cycle == Some(1)));
        let wide = parse("## Suggestion\n- S1-S52. too wide\n- S1-S51. widest allowed\n");
        assert_eq!(wide.len(), 1 + 51);
    }

    #[test]
    fn a_plural_section_heading_counts() {
        let f = parse("## Suggestions\n- S1. plural (spec). Open.\n## Importantly\n- I1. also\n");
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].severity, Severity::Important);
    }

    #[test]
    fn the_id_prefix_sets_severity_whatever_the_section() {
        let f = one("## Suggestion\n### C2. misfiled\n");
        assert_eq!(f.severity, Severity::Critical);
    }

    #[test]
    fn cycle_entries_read_free_form_cycle_files() {
        let text = "# Cycle 1\n\n## I1. a (spec + architecture)\nI2 Important (spec): b, not tests\n- I3 perf: c\n- **C1** d\n  - I4 nested\n- S1 not counted\nI1 again (tests)\nIn I5 prose\n";
        let e = cycle_entries(text);
        let ids: Vec<_> = e.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["I1", "I2", "I3", "C1", "I4"]);
        assert_eq!(e[0].axes, ["spec", "architecture"]);
        assert_eq!(e[1].axes, ["spec"]);
        assert_eq!(e[2].axes, ["performance"]);
        assert!(e[3].axes.is_empty());
        assert!(cycle_entries("nothing").is_empty());
    }

    #[test]
    fn stray_statuses_counts_headings_the_parser_skipped() {
        let text = "## Important\n### I1. real\n- Status: open\n### C3-1. cycle note\n- Status: open\n### D1. dismissed\n- **Status:** dismissed\n- Status: again\n### Notes\n- no status\n# Top\n- Status: open\n```\n### X1\n- Status: open\n```\n";
        assert_eq!(stray_statuses(text), 2);
    }

    #[test]
    fn change_size_variants() {
        let cs = |l: &str| change_size(&format!("## Summary\n{l}\n"));
        assert_eq!(
            cs("Change size: 816 insertions, 11 deletions in 7 files"),
            Some(827)
        );
        assert_eq!(
            cs("Change size at cycle 0: 576 insertions, 19 deletions in 13 files"),
            Some(595)
        );
        assert_eq!(cs("Change size: 250 insertions in 3 files"), Some(250));
        assert_eq!(cs("Change size: 3 deletions in 1 file"), Some(3));
        assert_eq!(cs("Change size: about 1,100 lines in 15 files at cycle 1 (about 1,300 with the fixes)"), Some(1100));
        assert_eq!(
            cs("Change size: ~275 diff lines in 6 files (122 added, 27 removed)"),
            Some(275)
        );
        assert_eq!(cs("**Change size:** 170 changed lines"), Some(170));
        assert_eq!(cs("Change size: unknown"), None);
        assert_eq!(
            cs("Change size: 10 insertions, 2 deletions (30 insertions, 5 deletions at HEAD)"),
            Some(12)
        );
        assert_eq!(change_size("Open: 1 Important.\n12 lines"), None);
    }
}
