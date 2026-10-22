//! Triage: reads one unit, decides its route, and records the decision as the unit's labels.
//! It posts nothing. The harness chooses; `ns` is the only one that writes a state label.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use crate::auto::STATE_PREFIX;
use crate::forge::{self, Forge, Unit};
use crate::harness::{self, Harness, Task};
use crate::{labels, skill};

/// The labels a harness may ask for. A state is for `ns` to set, from the route.
const CHOSEN_PREFIXES: [&str; 3] = ["type:", "priority:", "area:"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Build,
    Define,
    Repro,
    Info,
    Human,
    Wontfix,
}

impl Route {
    #[cfg(test)]
    pub const ALL: [Route; 6] = [
        Route::Build,
        Route::Define,
        Route::Repro,
        Route::Info,
        Route::Human,
        Route::Wontfix,
    ];

    pub fn parse(text: &str) -> Option<Route> {
        Some(match text {
            "build" => Route::Build,
            "define" => Route::Define,
            "repro" => Route::Repro,
            "info" => Route::Info,
            "human" => Route::Human,
            "wontfix" => Route::Wontfix,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Route::Build => "build",
            Route::Define => "define",
            Route::Repro => "repro",
            Route::Info => "info",
            Route::Human => "human",
            Route::Wontfix => "wontfix",
        }
    }

    /// The state label a unit takes when triage sends it this way.
    pub fn label(self) -> &'static str {
        match self {
            Route::Build => crate::auto::READY,
            Route::Define => crate::status::NEEDS_DEFINE,
            Route::Repro => crate::status::NEEDS_REPRO,
            Route::Info => crate::status::NEEDS_INFO,
            Route::Human => crate::status::READY_FOR_HUMAN,
            Route::Wontfix => crate::status::WONTFIX,
        }
    }
}

/// What a triage session answered.
#[derive(Debug, PartialEq, Eq)]
pub struct Triaged {
    pub route: Route,
    /// The `type:`, `priority:` and `area:` labels it chose.
    pub labels: Vec<String>,
    /// Why, in a few sentences.
    pub why: String,
}

/// Reads the answer: why, then a `LABELS:` line and a `ROUTE:` line.
pub fn parse(reply: &str) -> Result<Triaged> {
    let mut route = None;
    let mut labels = Vec::new();
    let mut why = Vec::new();
    for line in reply.lines() {
        if let Some(r) = line.trim().strip_prefix("ROUTE:") {
            route = Some(r.trim().to_string());
        } else if let Some(l) = line.trim().strip_prefix("LABELS:") {
            labels = l
                .split(',')
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
        } else {
            why.push(line);
        }
    }
    let route = route.context("the answer has no ROUTE line")?;
    let route = Route::parse(&route).with_context(|| format!("unknown route {route:?}"))?;
    if let Some(bad) = labels
        .iter()
        .find(|l| !CHOSEN_PREFIXES.iter().any(|p| l.starts_with(p)))
    {
        bail!(
            "triage asked for the label {bad:?}, which only type, priority and area labels may be"
        );
    }
    let why = why.join("\n").trim().to_string();
    if why.is_empty() {
        bail!("the answer gives no reason");
    }
    Ok(Triaged { route, labels, why })
}

/// Triages unit `number` of the repository at `root`: asks the harness, then sets the labels.
pub fn run(
    forge: &dyn Forge,
    harness: &dyn Harness,
    root: &Path,
    number: u64,
    timeout: Duration,
) -> Result<Triaged> {
    let unit = forge.unit_get(root, number)?;
    if !unit.from_team {
        bail!("the author of unit {number} is not on the team");
    }
    let reply = harness
        .task_run(&Task {
            dir: root.to_path_buf(),
            prompt: prompt(&skill::load(skill::TRIAGE)?, &unit),
            write: false,
            model: None,
            timeout,
        })
        .map_err(harness::session_failed)?;
    if !reply.success {
        return Err(harness::session_failed(anyhow!(
            "the harness did not finish triaging unit {number}"
        )));
    }
    let triaged = parse(&reply.text)?;
    let remove: Vec<String> = unit
        .labels
        .iter()
        .filter(|l| l.starts_with(STATE_PREFIX))
        .cloned()
        .collect();
    let split = labels::types_split(&forge::unit_types_get(forge, root), &triaged.labels);
    let mut add = split.labels;
    add.push(triaged.route.label().to_string());
    // The type goes first: if it fails the unit is still as it was and triage can run again.
    if let Some(name) = split.native {
        forge.unit_type_set(root, number, &name)?;
    }
    forge.unit_labels_set(root, number, &add, &remove)?;
    if triaged.route == Route::Wontfix {
        forge.unit_close(root, number)?;
    }
    Ok(triaged)
}

fn prompt(skill: &str, unit: &Unit) -> String {
    format!(
        "{skill}\n\n# The unit\n\n## {}\n\n{}\n\nIts labels: {}\n",
        unit.title,
        unit.body,
        unit.labels.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_comes_before_the_two_closing_lines() {
        let t =
            parse("Clear and small.\n\nLABELS: type:feat, priority:high\nROUTE: build\n").unwrap();
        assert_eq!(t.route, Route::Build);
        assert_eq!(t.labels, ["type:feat", "priority:high"]);
        assert_eq!(t.why, "Clear and small.");
    }

    #[test]
    fn an_answer_without_a_route_or_with_an_unknown_one_is_refused() {
        assert!(parse("Text.\nLABELS: type:fix").is_err());
        assert!(parse("Text.\nROUTE: ship").is_err());
    }

    #[test]
    fn a_harness_may_not_choose_a_state() {
        let err = parse("Text.\nLABELS: status:ready-for-agent\nROUTE: build").unwrap_err();
        assert!(err.to_string().contains("status:ready-for-agent"));
    }

    #[test]
    fn an_answer_with_no_reason_is_refused() {
        assert!(parse("ROUTE: build").is_err());
    }
}
