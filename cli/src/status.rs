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
/// The unit needs an answer from its author before anything else.
pub const NEEDS_INFO: &str = "status:needs-info";
/// The unit will not be done.
pub const WONTFIX: &str = "status:wontfix";
/// `ns run` has the unit.
pub const IN_PROGRESS: &str = "status:in-progress";
/// The unit ended `stuck`: a state failed every attempt, or verification could not prove it twice.
pub const STUCK: &str = "status:stuck";
/// The unit waits for a person and is not stuck: a merge, hardware, credentials or a decision.
pub const WAITING_FOR_HUMAN: &str = "status:waiting-for-human";
/// The name `WAITING_FOR_HUMAN` had before it was told apart from `STUCK`. `ns` never sets it.
pub const LEGACY_READY_FOR_HUMAN: &str = "status:ready-for-human";

/// Every status label `ns` sets, with what it tells a reader of the forge.
pub fn all() -> [(&'static str, &'static str); 9] {
    [
        (crate::auto::NEEDS_TRIAGE, "Nobody has looked at this yet"),
        (NEEDS_DEFINE, "Too vague to build; waits for a spec"),
        (NEEDS_REPRO, "A bug nobody has reproduced yet"),
        (NEEDS_INFO, "Waits for an answer from the author"),
        (crate::auto::READY, "Ready for the agent to build"),
        (IN_PROGRESS, "ns is working on this"),
        (
            STUCK,
            "A state failed every attempt or could not be proven; a person looks first",
        ),
        (
            WAITING_FOR_HUMAN,
            "Waiting for a person: a merge, hardware, credentials or a decision",
        ),
        (WONTFIX, "Will not be done"),
    ]
}

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
