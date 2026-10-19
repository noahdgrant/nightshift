//! Define: turns one vague unit into a spec in a session with the user, records it as the unit's
//! description, and plans it. The user decides what the spec says; `ns` is the only one that
//! changes the unit.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::forge::{Forge, Unit};
use crate::harness::Harness;
use crate::vcs::Vcs;
use crate::{brief, plan, skill, status};

/// Defines unit `number` of the repository at `root` and plans it, and returns its slices. A
/// session that writes no spec leaves the unit as it was. When only the plan fails, the unit keeps
/// its spec and `ns plan` can be run on it.
pub fn run(
    forge: &dyn Forge,
    harness: &dyn Harness,
    vcs: &dyn Vcs,
    root: &Path,
    number: u64,
    plan_timeout: Duration,
) -> Result<String> {
    let unit = forge.unit_get(root, number)?;
    if !unit.from_team {
        bail!("the author of unit {number} is not on the team");
    }
    let file = vcs
        .state_dir_find(root)?
        .join("define")
        .join(format!("{number}.md"));
    std::fs::create_dir_all(file.parent().context("the spec has no directory")?)?;
    // A spec left by an earlier session is not this one's.
    let _ = std::fs::remove_file(&file);
    let ended = harness.session_interact(root, &prompt(&unit, &file)?)?;
    let spec = std::fs::read_to_string(&file).unwrap_or_default();
    if spec.trim().is_empty() {
        let how = if ended { "ended" } else { "ended badly" };
        bail!("the session {how} without writing a spec, so unit {number} is unchanged");
    }
    let defined = Unit {
        body: brief::body_defined(&spec, &unit.body),
        labels: unit.labels.clone(),
        title: unit.title.clone(),
        ..unit
    };
    forge.unit_body_set(root, number, &defined.body)?;
    status::unit_status_set(forge, root, number, status::NEEDS_DEFINE)?;
    let _ = std::fs::remove_file(&file);
    plan::unit_plan(forge, harness, root, &defined, plan_timeout).with_context(|| {
        format!("unit {number} has its spec but is not planned; run `ns plan {number}`")
    })
}

fn prompt(unit: &Unit, file: &Path) -> Result<String> {
    Ok(format!(
        "{}\n\n{}\n\n# The unit\n\n## {}\n\n{}\n\nWrite the spec to: {}\n",
        skill::load(skill::GRILLING)?,
        skill::load(skill::REQUIREMENTS)?,
        unit.title,
        unit.body,
        file.display()
    ))
}
