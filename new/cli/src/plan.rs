//! Plan: cuts one unit into slices and records them in the unit's description. The harness
//! reads and answers; `ns` is the only one that changes the unit.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::forge::{Forge, Unit};
use crate::harness::{Harness, Task};
use crate::{auto, brief, skill, status};

/// Plans unit `number` of the repository at `root`: asks the harness for the slices, adds them to
/// the unit's description and makes the unit ready. Returns the slices.
pub fn run(
    forge: &dyn Forge,
    harness: &dyn Harness,
    root: &Path,
    number: u64,
    timeout: Duration,
) -> Result<String> {
    unit_plan(
        forge,
        harness,
        root,
        &forge.unit_get(root, number)?,
        timeout,
    )
}

/// Like [`run`], for a `unit` that has just been read or written.
pub fn unit_plan(
    forge: &dyn Forge,
    harness: &dyn Harness,
    root: &Path,
    unit: &Unit,
    timeout: Duration,
) -> Result<String> {
    let number = unit.number;
    if !unit.from_team {
        bail!("the author of unit {number} is not on the team");
    }
    if brief::is_planned(&unit.body) {
        bail!("unit {number} already has slices");
    }
    let reply = harness.task_run(&Task {
        dir: root.to_path_buf(),
        prompt: prompt(&skill::load(root, skill::DESIGN_DOC)?, unit),
        write: false,
        model: None,
        timeout,
    })?;
    if !reply.success {
        bail!("the harness did not finish planning unit {number}");
    }
    let slices = brief::slices_parse(&reply.text)?;
    forge.unit_body_set(root, number, &brief::body_planned(&unit.body, &slices)?)?;
    status::unit_status_set(forge, root, number, auto::READY)?;
    Ok(slices)
}

fn prompt(skill: &str, unit: &Unit) -> String {
    format!(
        "{skill}\n\n# The unit\n\n## {}\n\n{}\n",
        unit.title, unit.body
    )
}
