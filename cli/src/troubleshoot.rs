//! Troubleshoot: diagnoses one unit that has no reproduction, in a scratch workspace, and records
//! the diagnosis in the unit's description. The harness works and answers; `ns` is the only one
//! that changes the unit.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use crate::forge::{Forge, Unit};
use crate::harness::{self, Harness, Task};
use crate::triage::Route;
use crate::vcs::Vcs;
use crate::{brief, skill, status, workspace};

/// What a troubleshooting session answered.
#[derive(Debug, PartialEq, Eq)]
pub struct Diagnosis {
    /// Where the unit goes next: `build`, `define`, `info` or `human`.
    pub route: Route,
    pub brief: String,
}

/// Reads the answer: the brief, then a `ROUTE:` line.
pub fn parse(reply: &str) -> Result<Diagnosis> {
    let mut route = None;
    let mut brief = Vec::new();
    for line in reply.lines() {
        match line.trim().strip_prefix("ROUTE:") {
            Some(r) => route = Some(r.trim().to_string()),
            None => brief.push(line),
        }
    }
    let route = route.context("the answer has no ROUTE line")?;
    let route = match Route::parse(&route) {
        Some(r @ (Route::Build | Route::Define | Route::Info | Route::Human)) => r,
        Some(Route::Repro | Route::Wontfix) | None => {
            bail!("{route:?} is not a route a diagnosis can take")
        }
    };
    let brief = brief.join("\n").trim().to_string();
    if brief.is_empty() {
        bail!("the answer gives no brief");
    }
    Ok(Diagnosis { route, brief })
}

/// Troubleshoots unit `number` of the repository at `root`: a session in a scratch workspace
/// writes the diagnosis, which `ns` adds to the unit's description before it sends the unit on.
pub fn run(
    forge: &dyn Forge,
    harness: &dyn Harness,
    vcs: &dyn Vcs,
    root: &Path,
    number: u64,
    timeout: Duration,
    model: Option<&str>,
) -> Result<Diagnosis> {
    let unit = forge.unit_get(root, number)?;
    if !unit.from_team {
        bail!("the author of unit {number} is not on the team");
    }
    if brief::is_diagnosed(&unit.body) {
        bail!("unit {number} already has a diagnosis");
    }
    // Its own name, so it never meets the workspace of the unit's build.
    let scratch = format!("troubleshoot-{number}");
    workspace::discard(vcs, root, &scratch)?;
    let made = workspace::ensure(vcs, root, &scratch, None)?;
    let reply = harness.task_run(&Task {
        dir: made.path,
        prompt: prompt(&skill::load(skill::TROUBLESHOOT)?, &unit),
        write: true,
        model: model.map(str::to_string),
        timeout,
    });
    // Nothing in it is kept, whether or not the session got anywhere.
    workspace::discard(vcs, root, &scratch)?;
    let reply = reply.map_err(harness::session_failed)?;
    if !reply.success {
        return Err(harness::session_failed(anyhow!(
            "the harness did not finish troubleshooting unit {number}"
        )));
    }
    let diagnosis = parse(&reply.text)?;
    forge.unit_body_set(
        root,
        number,
        &brief::body_diagnosed(&unit.body, &diagnosis.brief)?,
    )?;
    status::unit_status_set(forge, root, number, diagnosis.route.label())?;
    Ok(diagnosis)
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
    fn the_brief_comes_before_the_route() {
        let d = parse("### Repro\nrun it\n\nROUTE: build\n").unwrap();
        assert_eq!(d.route, Route::Build);
        assert_eq!(d.brief, "### Repro\nrun it");
    }

    #[test]
    fn an_answer_that_cannot_be_used_is_refused() {
        assert!(parse("text").is_err());
        assert!(parse("ROUTE: build").is_err());
        assert!(parse("text\nROUTE: repro").is_err());
        assert!(parse("text\nROUTE: wontfix").is_err());
        assert!(parse("text\nROUTE: ship").is_err());
    }
}
