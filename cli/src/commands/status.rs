use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::{json, Value};

use crate::forge::{ChangeState, Forge, Unit};
use crate::runlog::RunLog;
use crate::states::ship;
use crate::vcs::Vcs;
use crate::{auto, forge, manifest, vcs, workspace};

#[derive(ClapArgs)]
pub struct Args {}

/// Prints every open unit, in the order `ns auto` takes them, with its status, where the run log
/// says it stands, and its change request, and for an open one whether it is `current` with the
/// base branch or `behind` it, which only the unit's workspace can say. Changes nothing.
pub fn run(_: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let forge = forge::get();
    let log = RunLog::open(vcs.as_ref(), &root)?;
    let queue = manifest::merge_queue_read(&root)?;
    let mut open = forge.unit_list(&root)?;
    open.sort_by_key(auto::order);
    let units = open
        .iter()
        .map(|unit| unit_describe(forge.as_ref(), vcs.as_ref(), &log, &root, queue, unit))
        .collect::<Result<Vec<_>>>()?;
    println!("{}", json!({ "units": units }));
    Ok(())
}

fn unit_describe(
    forge: &dyn Forge,
    vcs: &dyn Vcs,
    log: &RunLog,
    root: &std::path::Path,
    queue: bool,
    unit: &Unit,
) -> Result<Value> {
    let id = unit.number.to_string();
    let last = log.last_move(&id)?;
    let wanted = workspace::locate(root, &id)?;
    let change_request = match forge.change_request_get(root, &wanted.branch)? {
        None => None,
        Some(cr) => {
            let state = match cr.state {
                ChangeState::Open => "open",
                ChangeState::Merged => "merged",
                ChangeState::Closed => "closed",
            };
            let mut described = json!({ "url": cr.url, "state": state });
            if queue && cr.state == ChangeState::Open && wanted.path.exists() {
                // An unreachable origin must not fail the whole list; the base is just unknown.
                described["base"] = match ship::base_behind(vcs, log, root, &wanted.path, &id) {
                    Ok(Some(_)) => "behind",
                    Ok(None) => "current",
                    Err(_) => "unknown",
                }
                .into();
            }
            Some(described)
        }
    };
    Ok(json!({
        "unit": unit.number,
        "title": unit.title,
        "status": unit.labels.iter().find(|l| l.starts_with(auto::STATE_PREFIX)),
        "state": last.as_ref().map(|m| m.to.to_string()),
        "moved_at": last.and_then(|m| m.at),
        "change_request": change_request,
    }))
}
