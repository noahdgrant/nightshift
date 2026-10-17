//! Triage: reads one unit, decides its route, writes the brief or the reason on the forge, and
//! sets the unit's labels. The harness chooses; `ns` is the only one that writes a state label.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::forge::{Forge, Unit, DISCLOSURE};
use crate::harness::{Harness, Task};
use crate::skill;
use crate::watch::STATE_PREFIX;

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

    fn parse(text: &str) -> Option<Route> {
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
            Route::Build => crate::watch::READY,
            Route::Define => "status:needs-define",
            Route::Repro => "status:needs-repro",
            Route::Info => "status:needs-info",
            Route::Human => "status:ready-for-human",
            Route::Wontfix => "status:wontfix",
        }
    }
}

/// What a triage session answered.
#[derive(Debug, PartialEq, Eq)]
pub struct Triaged {
    pub route: Route,
    /// The `type:`, `priority:` and `area:` labels it chose.
    pub labels: Vec<String>,
    /// The brief, or the reason or questions, without the two closing lines.
    pub text: String,
}

/// Reads the answer: its text, then a `LABELS:` line and a `ROUTE:` line.
pub fn parse(reply: &str) -> Result<Triaged> {
    let mut route = None;
    let mut labels = Vec::new();
    let mut text = Vec::new();
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
            text.push(line);
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
    let text = text.join("\n").trim().to_string();
    if text.is_empty() {
        bail!("the answer has no brief or reason");
    }
    Ok(Triaged {
        route,
        labels,
        text,
    })
}

/// Triages unit `number` of the repository at `root`: asks the harness, posts its text as a
/// comment, then sets the labels. The comment goes first, so a unit is never ready without its
/// brief.
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
    let reply = harness.task_run(&Task {
        dir: root.to_path_buf(),
        prompt: prompt(&skill::load(root, skill::TRIAGE)?, &unit),
        write: false,
        model: None,
        timeout,
    })?;
    if !reply.success {
        bail!("the harness did not finish triaging unit {number}");
    }
    let triaged = parse(&reply.text)?;
    forge.unit_comment_create(root, number, &format!("{DISCLOSURE}\n\n{}", triaged.text))?;
    let remove: Vec<String> = unit
        .labels
        .iter()
        .filter(|l| l.starts_with(STATE_PREFIX))
        .cloned()
        .collect();
    let mut add = triaged.labels.clone();
    add.push(triaged.route.label().to_string());
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
        let t = parse("The brief.\n\nLABELS: type:feat, priority:high\nROUTE: build\n").unwrap();
        assert_eq!(t.route, Route::Build);
        assert_eq!(t.labels, ["type:feat", "priority:high"]);
        assert_eq!(t.text, "The brief.");
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
    fn an_answer_with_no_text_is_refused() {
        assert!(parse("ROUTE: build").is_err());
    }
}
