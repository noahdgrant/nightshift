//! `ns improve`: turns the grades that did not pass into units that fix their cause. It files
//! units and changes nothing else, so the normal states check every fix.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::auto::NEEDS_TRIAGE;
use crate::forge::{self, Forge, Unit, DISCLOSURE};
use crate::harness::{self, Harness, Sessions, Task};
use crate::labels;

/// How many different units must fail one rubric for the same cause before a unit is filed.
pub const MIN_UNITS: usize = 3;

/// The most units one run files.
pub const MAX_FILED: usize = 3;

/// Opens the marker a filed unit carries to name its cause.
const MARKER_OPEN: &str = "<!-- ns:improve ";

/// The prefixes of the labels a session may ask for. A state is for `ns` to set.
const CHOSEN_PREFIXES: [&str; 3] = ["type:", "priority:", "area:"];

/// The reply that says no cause reached the threshold.
const NOTHING: &str = "NOTHING";

/// A grade that did not pass.
#[derive(Debug, PartialEq)]
pub struct Failure {
    pub unit: String,
    pub rubric: String,
    pub score: f64,
    pub reason: String,
    /// Seconds since the epoch.
    pub at: i64,
}

/// The grades among `events` that did not pass, made at `since` or later. An event that is not a
/// whole grade is left out.
pub fn failures_parse(events: &[Value], since: i64) -> Vec<Failure> {
    events
        .iter()
        .filter(|v| v["pass"].as_bool() == Some(false))
        .filter_map(|v| {
            Some(Failure {
                unit: v["unit"].as_str()?.to_string(),
                rubric: v["rubric"].as_str()?.to_string(),
                score: v["score"].as_f64()?,
                reason: v["reason"].as_str().unwrap_or_default().to_string(),
                at: v["at"].as_i64()?,
            })
        })
        .filter(|f| f.at >= since)
        .collect()
}

/// Whether some rubric has failures from at least [`MIN_UNITS`] different units.
fn threshold_reached(failures: &[Failure]) -> bool {
    let mut units: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for f in failures {
        units.entry(&f.rubric).or_default().insert(&f.unit);
    }
    units.values().any(|u| u.len() >= MIN_UNITS)
}

/// The layer a fix goes in. The highest one that removes the whole category wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Code,
    Check,
    Prose,
}

impl Layer {
    fn parse(text: &str) -> Option<Layer> {
        match text {
            "code" => Some(Layer::Code),
            "check" => Some(Layer::Check),
            "prose" => Some(Layer::Prose),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Layer::Code => "code",
            Layer::Check => "check",
            Layer::Prose => "prose",
        }
    }
}

/// One unit the session proposes.
#[derive(Debug, PartialEq)]
pub struct Proposal {
    pub title: String,
    pub rubric: String,
    pub cause: String,
    pub layer: Layer,
    /// The `type:`, `priority:` and `area:` labels it asked for.
    pub labels: Vec<String>,
    pub brief: String,
}

impl Proposal {
    /// The marker that names the proposal's cause.
    fn marker(&self) -> String {
        marker(&self.rubric, &self.cause)
    }

    /// The unit's description: the disclosure, the marker, the layer and its rule, then the brief.
    fn body(&self) -> String {
        format!(
            "{DISCLOSURE}\n\n{}\n\nLayer to fix: {}. Choose the highest layer that removes the whole category (code, then check, then prose), and prose only when neither fits.\n\n{}",
            self.marker(),
            self.layer.name(),
            self.brief
        )
    }
}

fn marker(rubric: &str, cause: &str) -> String {
    format!("{MARKER_OPEN}{rubric} {cause} -->")
}

/// Whether `text` is one word of letters, digits, `-` and `_`, as a marker needs.
fn is_word(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `title` is a Conventional Commit title: `type(scope): subject`, the scope and a `!`
/// being optional.
fn is_conventional(title: &str) -> bool {
    let Some((head, subject)) = title.split_once(": ") else {
        return false;
    };
    let head = head.strip_suffix('!').unwrap_or(head);
    let kind = match head.split_once('(') {
        Some((kind, scope)) => {
            if !scope.ends_with(')') || scope.len() < 2 {
                return false;
            }
            kind
        }
        None => head,
    };
    !subject.trim().is_empty() && !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase())
}

/// The cause markers the open units carry.
fn markers_open(units: &[Unit]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for unit in units {
        let mut rest = unit.body.as_str();
        while let Some(at) = rest.find(MARKER_OPEN) {
            rest = &rest[at..];
            let Some(end) = rest.find("-->") else { break };
            found.insert(rest[..end + 3].to_string());
            rest = &rest[end + 3..];
        }
    }
    found
}

/// Reads the reply: `NOTHING`, or one or more `PROPOSAL` blocks. A block has `TITLE:`, `RUBRIC:`,
/// `CAUSE:`, `LAYER:` and `LABELS:` lines, then `BRIEF:` and the brief up to the next `PROPOSAL`.
pub fn parse(reply: &str) -> Result<Vec<Proposal>> {
    let lines: Vec<&str> = reply.lines().collect();
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].trim() == "PROPOSAL")
        .collect();
    if starts.is_empty() {
        if lines.iter().any(|l| l.trim() == NOTHING) {
            return Ok(Vec::new());
        }
        bail!("the answer has neither a {NOTHING} line nor a PROPOSAL");
    }
    let mut ends: Vec<usize> = starts[1..].to_vec();
    ends.push(lines.len());
    starts
        .iter()
        .zip(ends)
        .map(|(&start, end)| proposal_parse(&lines[start + 1..end]))
        .collect()
}

fn proposal_parse(lines: &[&str]) -> Result<Proposal> {
    let brief_at = lines
        .iter()
        .position(|l| l.trim() == "BRIEF:")
        .context("a proposal has no BRIEF: line")?;
    let field = |name: &str| -> Result<String> {
        lines[..brief_at]
            .iter()
            .find_map(|l| l.trim().strip_prefix(name))
            .map(|v| v.trim().to_string())
            .with_context(|| format!("a proposal has no {name} line"))
    };
    let title = field("TITLE:")?;
    if !is_conventional(&title) {
        bail!("the proposal's title {title:?} is not a Conventional Commit title");
    }
    let (rubric, cause) = (field("RUBRIC:")?, field("CAUSE:")?);
    for (what, word) in [("rubric", &rubric), ("cause", &cause)] {
        if !is_word(word) {
            bail!("the proposal's {what} {word:?} is not one word of letters, digits, - and _");
        }
    }
    let layer = field("LAYER:")?;
    let layer = Layer::parse(&layer)
        .with_context(|| format!("the proposal's layer {layer:?} is not code, check or prose"))?;
    let labels = field("LABELS:")?
        .split(',')
        .map(|l| l.trim().to_string())
        .filter(|l| CHOSEN_PREFIXES.iter().any(|p| l.starts_with(p)))
        .collect();
    let brief = lines[brief_at + 1..].join("\n").trim().to_string();
    if brief.is_empty() {
        bail!("the proposal {title:?} has an empty brief");
    }
    Ok(Proposal {
        title,
        rubric,
        cause,
        layer,
        labels,
        brief,
    })
}

/// What the improve session is told before it is shown the grades.
const INSTRUCTIONS: &str = "You look for causes behind grades that did not pass, so the factory \
can improve from its own record. You only read; change nothing in the repository.\n\nGroup the \
grades below by cause: the same underlying reason a rubric was failed. Propose a unit only for a \
cause that at least 3 different units failed on the same rubric. Never propose one for a cause \
that an open improvement unit below already covers.\n\nA proposal's brief is in the repository's \
unit format: Goal, Requirements, Out of scope, Open questions. It cites the units and the grades \
behind it. It names the layer to fix: `code`, `check` or `prose`. Choose the highest layer that \
removes the whole category: a change to code, then a check (`.nightshift/check`, a lint rule or a \
test), and prose (docs, standards) only when neither fits.\n\nReply with `NOTHING` alone when no \
cause reaches 3 units. Otherwise reply with up to 3 proposals, each as:\n\nPROPOSAL\nTITLE: \
<a Conventional Commits title>\nRUBRIC: <the rubric's name>\nCAUSE: <a short slug of letters, \
digits and dashes that names the cause>\nLAYER: <code|check|prose>\nLABELS: <type:..., \
priority:..., area:... labels the repository has>\nBRIEF:\n<the brief, in markdown, to the \
end of the proposal>\n\nDo not write the disclosure or the cause marker in the brief; `ns` adds \
them.";

/// What the session may propose to change, by whether the repository owns the skills.
const ORDINARY_SCOPE: &str = "This is an ordinary repository. A proposal may change only what \
the repository owns: `docs/agents/*`, `AGENTS.md`, the coding standards in force, \
`.nightshift/check`, lint rules, tests and code. A skill's text lives in the `ns` binary and the \
repository cannot patch it. When a cause lies in a skill, say so in a section of the brief \
titled \"For the skill's owner\" and propose no change to the skill.";

const OWN_SKILLS_SCOPE: &str = "This repository has its own `skills/ns-*`, so a proposal may \
change them besides `docs/agents/*`, `AGENTS.md`, the coding standards in force, \
`.nightshift/check`, lint rules, tests and code.";

/// Whether the repository at `root` has skills of its own, as the nightshift repository does.
fn has_own_skills(root: &Path) -> bool {
    std::fs::read_dir(root.join("skills"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("ns-"))
}

/// What the improve session is given.
fn prompt(root: &Path, failures: &[Failure], open: &[Unit]) -> String {
    let scope = if has_own_skills(root) {
        OWN_SKILLS_SCOPE
    } else {
        ORDINARY_SCOPE
    };
    let mut text = format!("{INSTRUCTIONS}\n\n{scope}\n\n# The grades that did not pass\n\n");
    for f in failures {
        text.push_str(&format!(
            "- unit {}, rubric {}, score {}: {}\n",
            f.unit,
            f.rubric,
            f.score,
            f.reason.replace('\n', " ")
        ));
    }
    text.push_str("\n# The open improvement units\n\n");
    let improvements: Vec<&Unit> = open
        .iter()
        .filter(|u| u.body.contains(MARKER_OPEN))
        .collect();
    if improvements.is_empty() {
        text.push_str("(none)\n");
    }
    for unit in improvements {
        text.push_str(&format!("- {}\n", unit.title));
    }
    text.push_str(&format!(
        "\n# The repository\n\nIt is at {}; read it as you need.\n",
        root.display()
    ));
    text
}

/// Whether `run` files the units or only says which it would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    File,
    DryRun,
}

/// What `run` is asked: which grades to learn from and how.
pub struct Request<'a> {
    pub failures: &'a [Failure],
    pub mode: Mode,
    pub sessions: &'a Sessions<'a>,
}

/// Asks the harness for causes behind `request.failures` and files a unit for each, printing one
/// JSON line as soon as each is filed (or, as a dry run, would be). Files no more than
/// [`MAX_FILED`], and none whose marker an open unit already carries.
pub fn run(forge: &dyn Forge, harness: &dyn Harness, root: &Path, request: &Request) -> Result<()> {
    if !threshold_reached(request.failures) {
        println!("{}", json!({"improve": "nothing to propose"}));
        return Ok(());
    }
    let open = forge.unit_list(root)?;
    let reply = harness
        .task_run(&Task {
            dir: root.to_path_buf(),
            prompt: prompt(root, request.failures, &open),
            write: false,
            model: request.sessions.model.map(str::to_string),
            timeout: request.sessions.timeout,
            resume: None,
        })
        .map_err(harness::session_failed)?;
    if !reply.success {
        return Err(harness::session_failed(anyhow!(
            "the harness did not finish the improve session"
        )));
    }
    let mut taken = markers_open(&open);
    let proposals: Vec<Proposal> = parse(&reply.text)?
        .into_iter()
        .filter(|p| taken.insert(p.marker()))
        .take(MAX_FILED)
        .collect();
    if proposals.is_empty() {
        println!("{}", json!({"improve": "nothing to propose"}));
        return Ok(());
    }
    let have: BTreeSet<String> = match request.mode {
        Mode::File => forge
            .labels_list(root)?
            .into_iter()
            .map(|l| l.name)
            .collect(),
        Mode::DryRun => BTreeSet::new(),
    };
    let native = match request.mode {
        Mode::File => forge::unit_types_get(forge, root),
        Mode::DryRun => Vec::new(),
    };
    for proposal in &proposals {
        let line = match request.mode {
            Mode::File => {
                let number = file(forge, root, proposal, &have, &native)?;
                line(proposal, Some(number))
            }
            Mode::DryRun => line(proposal, None),
        };
        println!("{line}");
    }
    Ok(())
}

/// Files `proposal` as a unit and returns its number. It carries the labels it asked for that the
/// repository has, and `status:needs-triage`, never `status:ready-for-agent`.
fn file(
    forge: &dyn Forge,
    root: &Path,
    proposal: &Proposal,
    have: &BTreeSet<String>,
    native: &[String],
) -> Result<u64> {
    let split = labels::types_split(native, &proposal.labels);
    let mut labels: Vec<String> = split
        .labels
        .into_iter()
        .filter(|l| have.contains(l))
        .collect();
    labels.push(NEEDS_TRIAGE.to_string());
    let number = forge.unit_create(root, &proposal.title, &proposal.body(), &labels)?;
    if let Some(name) = split.native {
        forge
            .unit_type_set(root, number, &name)
            .with_context(|| format!("unit {number} was filed but its type was not set"))?;
    }
    Ok(number)
}

/// The JSON line for one unit: filed as `number`, or only proposed.
fn line(proposal: &Proposal, number: Option<u64>) -> Value {
    let mut line = json!({
        "title": proposal.title,
        "rubric": proposal.rubric,
        "cause": proposal.cause,
        "layer": proposal.layer.name(),
        "labels": proposal.labels,
    });
    match number {
        Some(number) => line["unit"] = number.into(),
        None => line["dry_run"] = true.into(),
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(unit: &str, rubric: &str, pass: bool, at: i64) -> Value {
        json!({"unit": unit, "rubric": rubric, "score": 0.25, "pass": pass,
               "reason": "too long", "at": at})
    }

    #[test]
    fn only_grades_that_did_not_pass_in_the_window_are_kept() {
        let events = [
            event("1", "verbosity", false, 100),
            event("2", "verbosity", true, 100),
            event("3", "verbosity", false, 10),
            json!({"unit": "4"}),
        ];
        let kept = failures_parse(&events, 50);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].unit, "1");
        assert_eq!(kept[0].reason, "too long");
    }

    #[test]
    fn three_different_units_on_one_rubric_reach_the_threshold() {
        let f = |unit: &str, rubric: &str| Failure {
            unit: unit.into(),
            rubric: rubric.into(),
            score: 0.0,
            reason: String::new(),
            at: 0,
        };
        // Two units, one failing twice, and a third on another rubric.
        let few = [f("1", "a"), f("1", "a"), f("2", "a"), f("3", "b")];
        assert!(!threshold_reached(&few));
        let enough = [f("1", "a"), f("2", "a"), f("3", "a")];
        assert!(threshold_reached(&enough));
    }

    #[test]
    fn a_title_is_conventional_or_not() {
        assert!(is_conventional("fix(cli): stop the loop"));
        assert!(is_conventional("feat!: drop it"));
        assert!(!is_conventional("Stop the loop"));
        assert!(!is_conventional("fix(cli:) x"));
        assert!(!is_conventional("fix: "));
    }

    #[test]
    fn a_reply_of_nothing_has_no_proposals() {
        assert_eq!(parse("Nothing fits.\nNOTHING").unwrap(), Vec::new());
    }

    #[test]
    fn a_reply_with_neither_is_an_error() {
        assert!(parse("hmm").is_err());
    }

    #[test]
    fn a_proposal_is_read_with_its_brief() {
        let reply = "PROPOSAL\nTITLE: fix(cli): x\nRUBRIC: verbosity\nCAUSE: long-summaries\n\
                     LAYER: check\nLABELS: type:fix, status:ready-for-agent, area:cli\n\
                     BRIEF:\n## Goal\n\nShorter.\nPROPOSAL\nTITLE: fix: y\nRUBRIC: r\n\
                     CAUSE: c\nLAYER: prose\nLABELS:\nBRIEF:\nb";
        let got = parse(reply).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].layer, Layer::Check);
        assert_eq!(got[0].labels, ["type:fix", "area:cli"]);
        assert_eq!(got[0].brief, "## Goal\n\nShorter.");
        assert_eq!(
            got[0].marker(),
            "<!-- ns:improve verbosity long-summaries -->"
        );
        assert_eq!(got[1].brief, "b");
    }

    #[test]
    fn a_proposal_with_a_bad_layer_title_or_cause_is_refused() {
        let ok = "PROPOSAL\nTITLE: fix: x\nRUBRIC: r\nCAUSE: c\nLAYER: code\nLABELS:\nBRIEF:\nb";
        assert!(parse(ok).is_ok());
        assert!(parse(&ok.replace("LAYER: code", "LAYER: skill")).is_err());
        assert!(parse(&ok.replace("fix: x", "x")).is_err());
        assert!(parse(&ok.replace("CAUSE: c", "CAUSE: a b")).is_err());
        assert!(parse(&ok.replace("BRIEF:\nb", "BRIEF:\n")).is_err());
        assert!(parse(&ok.replace("BRIEF:\nb", "")).is_err());
        assert!(parse(&ok.replace("RUBRIC: r\n", "")).is_err());
    }

    #[test]
    fn the_markers_of_open_units_are_found() {
        let unit = |body: &str| Unit {
            number: 1,
            title: String::new(),
            body: body.into(),
            labels: Vec::new(),
            from_team: true,
            url: String::new(),
        };
        let found = markers_open(&[
            unit("x\n<!-- ns:improve a b -->\ny <!-- ns:improve c d -->"),
            unit("<!-- ns:improve broken"),
        ]);
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            ["<!-- ns:improve a b -->", "<!-- ns:improve c d -->"]
        );
    }
}
