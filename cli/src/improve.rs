//! Improve: reads the grades that did not pass and files units that fix their cause. A session
//! proposes, `ns` checks each proposal and files it for a person to triage. Nothing is edited.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::json;

use crate::forge::{self, Forge, DISCLOSURE};
use crate::grades::Failure;
use crate::harness::{self, Harness, Task};
use crate::{auto, labels, skill};

/// The fewest different units that must fail one rubric for the same cause.
const MIN_UNITS: usize = 3;

/// The most units one run files.
const MAX_FILED: usize = 3;

/// Starts the marker that names the cause a unit was filed for.
const MARKER_START: &str = "<!-- ns:improve ";

/// The layer of the repository a proposal fixes, from the highest to the lowest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Code,
    Check,
    Prose,
}

impl Layer {
    fn parse(name: &str) -> Option<Layer> {
        match name {
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

/// An open unit that carries an improvement marker.
struct OpenUnit {
    title: String,
    marker: String,
}

/// One unit the session proposes.
#[derive(Debug, PartialEq, Eq)]
pub struct Proposal {
    pub title: String,
    pub rubric: String,
    pub cause: String,
    pub layer: Layer,
    pub labels: Vec<String>,
    pub brief: String,
}

impl Proposal {
    /// The marker that says which cause the unit is for.
    pub fn marker(&self) -> String {
        format!("{MARKER_START}{} {} -->", self.rubric, self.cause)
    }

    fn body(&self) -> String {
        format!(
            "{DISCLOSURE}\n\n{}\n\n{}\n\nLayer to fix: `{}`. Choose the highest layer that removes the whole category: `code`, then `check`, and `prose` only when neither fits.\n",
            self.marker(),
            self.brief.trim(),
            self.layer.name()
        )
    }
}

/// The rubrics that at least [`MIN_UNITS`] different units failed.
fn rubrics_repeated(failures: &[Failure]) -> BTreeSet<&str> {
    let rubrics: BTreeSet<&str> = failures.iter().map(|f| f.rubric.as_str()).collect();
    rubrics
        .into_iter()
        .filter(|r| {
            let units: BTreeSet<&str> = failures
                .iter()
                .filter(|f| f.rubric == *r)
                .map(|f| f.unit.as_str())
                .collect();
            units.len() >= MIN_UNITS
        })
        .collect()
}

/// Reads the session's reply: `NOTHING TO PROPOSE`, or blocks that each start with a `PROPOSAL`
/// line. A block that is not valid is left out, and the reason is the second value.
pub fn parse(reply: &str) -> (Vec<Proposal>, Vec<String>) {
    let mut proposals = Vec::new();
    let mut rejected = Vec::new();
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in reply.lines() {
        if line.trim() == "PROPOSAL" {
            blocks.push(Vec::new());
        } else if let Some(block) = blocks.last_mut() {
            block.push(line);
        }
    }
    for block in blocks {
        match block_parse(&block) {
            Ok(p) => proposals.push(p),
            Err(e) => rejected.push(format!("{e:#}")),
        }
    }
    (proposals, rejected)
}

fn block_parse(block: &[&str]) -> Result<Proposal> {
    let at = block
        .iter()
        .position(|l| l.trim() == "BRIEF:")
        .context("a proposal has no BRIEF: line")?;
    let field = |name: &str| {
        block[..at]
            .iter()
            .find_map(|l| l.trim().strip_prefix(name))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .with_context(|| format!("a proposal has no {name} line"))
    };
    let title = field("TITLE:")?;
    if !conventional(&title) {
        return Err(anyhow!("the title {title:?} is not a Conventional Commit"));
    }
    let rubric = field("RUBRIC:")?;
    let cause = field("CAUSE:")?;
    if !cause
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(anyhow!("the cause {cause:?} is not a lowercase slug"));
    }
    let layer = field("LAYER:")?;
    let layer = Layer::parse(&layer)
        .ok_or_else(|| anyhow!("the layer {layer:?} is not code, check or prose"))?;
    let labels = field("LABELS:")
        .unwrap_or_default()
        .split(',')
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let brief = block[at + 1..].join("\n").trim().to_string();
    if brief.is_empty() {
        return Err(anyhow!("the proposal {title:?} has an empty brief"));
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

/// Whether `title` reads `type(scope): subject`, with the scope optional.
fn conventional(title: &str) -> bool {
    let Some((head, subject)) = title.split_once(": ") else {
        return false;
    };
    let head = head.strip_suffix('!').unwrap_or(head);
    let (kind, scope) = match head.split_once('(') {
        Some((kind, scope)) => (kind, scope.strip_suffix(')')),
        None => (head, Some("x")),
    };
    !kind.is_empty()
        && kind.chars().all(|c| c.is_ascii_lowercase())
        && scope.is_some_and(|s| !s.is_empty())
        && !subject.trim().is_empty()
}

/// The `Fix level:` lines of the review findings kept under `.ns/*/` in the repository.
fn fix_levels(root: &Path) -> Vec<String> {
    let Ok(dirs) = std::fs::read_dir(root.join(crate::artifact::DIR)) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = dirs.flatten().map(|d| d.path()).collect();
    dirs.sort();
    dirs.iter()
        .filter_map(|d| std::fs::read_to_string(d.join("evidence.md")).ok())
        .flat_map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| l.contains("Fix level:"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Whether the repository has skills of its own, which a proposal may then change.
fn has_own_skills(root: &Path) -> bool {
    std::fs::read_dir(root.join("skills")).is_ok_and(|dirs| {
        dirs.flatten()
            .any(|d| d.file_name().to_string_lossy().starts_with("ns-"))
    })
}

fn prompt(
    failures: &[Failure],
    open: &[OpenUnit],
    own_skills: bool,
    levels: &[String],
) -> Result<String> {
    let mut text = format!(
        "{}\n\n# The grades that did not pass\n\n",
        skill::load(skill::IMPROVE)?
    );
    for f in failures {
        text.push_str(&format!(
            "- unit {}, rubric {}, score {}: {}\n",
            f.unit, f.rubric, f.score, f.reason
        ));
    }
    text.push_str("\n# The open improvement units\n\n");
    if open.is_empty() {
        text.push_str("(none)\n");
    }
    for unit in open {
        text.push_str(&format!("- {} {}\n", unit.title, unit.marker));
    }
    text.push_str("\n# The repository's skills\n\n");
    text.push_str(if own_skills {
        "The repository has its own `skills/ns-*`, so a proposal may change them.\n"
    } else {
        "The repository has no skills of its own: the text of a skill lives in the `ns` binary and a proposal cannot change it.\n"
    });
    if !levels.is_empty() {
        text.push_str("\n# Recent review findings\n\n");
        for l in levels {
            text.push_str(&format!("- {l}\n"));
        }
    }
    Ok(text)
}

/// Asks the harness for proposals given `failures` and files up to [`MAX_FILED`] of them, or only
/// lists them when `dry_run`. Returns the JSON line for each unit filed or that would be.
pub fn run(
    forge: &dyn Forge,
    harness: &dyn Harness,
    root: &Path,
    failures: &[Failure],
    dry_run: bool,
    timeout: Duration,
    model: Option<&str>,
) -> Result<Vec<String>> {
    let nothing = || vec![json!({ "improve": "nothing to propose" }).to_string()];
    let repeated = rubrics_repeated(failures);
    if repeated.is_empty() {
        return Ok(nothing());
    }
    let open: Vec<OpenUnit> = forge
        .unit_list(root)?
        .into_iter()
        .filter_map(|u| {
            let at = u.body.find(MARKER_START)?;
            let marker = u.body[at..].split("-->").next()?;
            Some(OpenUnit {
                title: u.title,
                marker: format!("{marker}-->"),
            })
        })
        .collect();
    let reply = harness
        .task_run(&Task {
            dir: root.to_path_buf(),
            prompt: prompt(failures, &open, has_own_skills(root), &fix_levels(root))?,
            write: false,
            model: model.map(str::to_string),
            timeout,
            resume: None,
        })
        .map_err(harness::session_failed)?;
    if !reply.success {
        return Err(harness::session_failed(anyhow!(
            "the harness did not finish the improvement session"
        )));
    }
    let (proposals, rejected) = parse(&reply.text);
    for why in rejected {
        eprintln!("ns: a proposal was left out: {why}");
    }
    let mut seen: BTreeSet<String> = open.into_iter().map(|u| u.marker).collect();
    let mut chosen = Vec::new();
    for p in proposals {
        if !repeated.contains(p.rubric.as_str()) {
            eprintln!(
                "ns: {:?} was left out: fewer than {MIN_UNITS} units failed {}",
                p.title, p.rubric
            );
        } else if seen.insert(p.marker()) {
            chosen.push(p);
        }
        if chosen.len() == MAX_FILED {
            break;
        }
    }
    if chosen.is_empty() {
        return Ok(nothing());
    }
    let have = forge.labels_list(root)?;
    let native = forge::unit_types_get(forge, root);
    let mut lines = Vec::new();
    for p in chosen {
        let split = labels::types_split(&native, &p.labels);
        let mut add: Vec<String> = split
            .labels
            .into_iter()
            .filter(|l| KEPT_PREFIXES.iter().any(|k| l.starts_with(k)))
            .filter(|l| have.iter().any(|h| &h.name == l))
            .collect();
        add.push(auto::NEEDS_TRIAGE.to_string());
        let mut line = json!({
            "title": p.title,
            "rubric": p.rubric,
            "cause": p.cause,
            "layer": p.layer.name(),
            "labels": add,
        });
        if !dry_run {
            let number = forge.unit_create(root, &p.title, &p.body(), &add)?;
            if let Some(name) = split.native {
                forge.unit_type_set(root, number, &name)?;
            }
            line["unit"] = number.into();
        }
        lines.push(line.to_string());
    }
    Ok(lines)
}

/// The only proposed labels a filed unit keeps; a status is `ns`'s to set, never a proposal's.
const KEPT_PREFIXES: [&str; 3] = ["type:", "priority:", "area:"];

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(unit: &str, rubric: &str) -> Failure {
        Failure {
            unit: unit.into(),
            rubric: rubric.into(),
            score: 0.0,
            reason: "r".into(),
        }
    }

    #[test]
    fn a_rubric_needs_three_different_units() {
        let failures = [
            failure("1", "a"),
            failure("1", "a"),
            failure("2", "a"),
            failure("1", "b"),
            failure("2", "b"),
            failure("3", "b"),
        ];
        assert_eq!(rubrics_repeated(&failures), BTreeSet::from(["b"]));
    }

    #[test]
    fn a_proposal_is_read_from_its_block() {
        let reply = "Thinking.\nPROPOSAL\nTITLE: fix(cli): add a lint\nRUBRIC: tests\nCAUSE: untested-branch\nLAYER: check\nLABELS: type:fix, priority:high\nBRIEF:\n## Goal\n\nStop it.\n";
        let (got, rejected) = parse(reply);
        assert!(rejected.is_empty());
        assert_eq!(
            got,
            [Proposal {
                title: "fix(cli): add a lint".into(),
                rubric: "tests".into(),
                cause: "untested-branch".into(),
                layer: Layer::Check,
                labels: vec!["type:fix".into(), "priority:high".into()],
                brief: "## Goal\n\nStop it.".into(),
            }]
        );
        assert_eq!(got[0].marker(), "<!-- ns:improve tests untested-branch -->");
    }

    #[test]
    fn a_block_that_is_not_valid_is_left_out_with_a_reason() {
        for (block, why) in [
            (
                "TITLE: add a lint\nRUBRIC: a\nCAUSE: c\nLAYER: code\nBRIEF:\nx",
                "Conventional",
            ),
            (
                "TITLE: fix: x\nRUBRIC: a\nCAUSE: C c\nLAYER: code\nBRIEF:\nx",
                "slug",
            ),
            (
                "TITLE: fix: x\nRUBRIC: a\nCAUSE: c\nLAYER: skill\nBRIEF:\nx",
                "layer",
            ),
            (
                "TITLE: fix: x\nRUBRIC: a\nCAUSE: c\nLAYER: code\nBRIEF:\n",
                "empty",
            ),
            (
                "TITLE: fix: x\nRUBRIC: a\nCAUSE: c\nLAYER: code\nx",
                "BRIEF",
            ),
        ] {
            let (got, rejected) = parse(&format!("PROPOSAL\n{block}"));
            assert!(got.is_empty(), "{block}");
            assert!(rejected[0].contains(why), "{block}: {rejected:?}");
        }
        assert_eq!(parse("NOTHING TO PROPOSE"), (vec![], vec![]));
    }
}
