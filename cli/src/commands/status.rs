use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::{json, Value};

use crate::forge::{ChangeState, Forge, Unit};
use crate::runlog::RunLog;
use crate::{auto, forge, vcs, workspace};

#[derive(ClapArgs)]
pub struct Args {}

/// Prints every open unit, in the order `ns auto` takes them, with its status, where the run log
/// says it stands, and its change request. Changes nothing.
pub fn run(_: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let forge = forge::get();
    let log = RunLog::open(vcs.as_ref(), &root)?;
    let mut open = forge.unit_list(&root)?;
    open.sort_by_key(auto::order);
    let units = open
        .iter()
        .map(|unit| unit_describe(forge.as_ref(), &log, &root, unit))
        .collect::<Result<Vec<_>>>()?;
    println!("{}", json!({ "units": units }));
    Ok(())
}

fn unit_describe(
    forge: &dyn Forge,
    log: &RunLog,
    root: &std::path::Path,
    unit: &Unit,
) -> Result<Value> {
    let id = unit.number.to_string();
    let last = log.last_move(&id)?;
    let change_request = forge
        .change_request_get(root, &workspace::locate(root, &id)?.branch)?
        .map(|cr| {
            let state = match cr.state {
                ChangeState::Open => "open",
                ChangeState::Merged => "merged",
                ChangeState::Closed => "closed",
            };
            json!({ "url": cr.url, "state": state })
        });
    Ok(json!({
        "unit": unit.number,
        "title": unit.title,
        "status": unit.labels.iter().find(|l| l.starts_with(auto::STATE_PREFIX)),
        "state": last.as_ref().map(|m| m.to.to_string()),
        "moved_at": last.and_then(|m| m.at),
        "change_request": change_request,
    }))
}
