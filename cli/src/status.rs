//! A unit's status on the forge: the one `status:` label that says where the unit is. Only `ns`
//! changes it.

use std::path::Path;

use anyhow::Result;

use crate::auto::STATE_PREFIX;
use crate::forge::Forge;

/// The unit is a bug nobody has reproduced yet.
pub const NEEDS_REPRO: &str = "status:needs-repro";
/// The unit is too vague to build and waits for a spec.
pub const NEEDS_DEFINE: &str = "status:needs-define";
/// `ns run` has the unit.
pub const IN_PROGRESS: &str = "status:in-progress";
/// The unit waits for a human: it is stuck, went wrong, or its change request is ready to merge.
pub const READY_FOR_HUMAN: &str = "status:ready-for-human";

/// Makes `status` the only status label on `unit`.
pub fn unit_status_set(forge: &dyn Forge, repo: &Path, unit: u64, status: &str) -> Result<()> {
    let labels = forge.unit_get(repo, unit)?.labels;
    let add: Vec<String> = if labels.iter().any(|l| l == status) {
        Vec::new()
    } else {
        vec![status.to_string()]
    };
    let remove: Vec<String> = labels
        .into_iter()
        .filter(|l| l.starts_with(STATE_PREFIX) && l != status)
        .collect();
    if add.is_empty() && remove.is_empty() {
        return Ok(());
    }
    forge.unit_labels_set(repo, unit, &add, &remove)
}

/// Takes the status label off `unit`, for a unit whose change request has merged.
pub fn unit_status_clear(forge: &dyn Forge, repo: &Path, unit: u64) -> Result<()> {
    let labels = forge.unit_get(repo, unit)?.labels;
    let remove: Vec<String> = labels
        .into_iter()
        .filter(|l| l.starts_with(STATE_PREFIX))
        .collect();
    if remove.is_empty() {
        return Ok(());
    }
    forge.unit_labels_set(repo, unit, &[], &remove)
}
