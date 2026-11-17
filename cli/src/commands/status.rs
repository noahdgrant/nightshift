use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use serde_json::{json, Value};

use crate::forge::{ChangeState, Checks, Forge, Unit};
use crate::runlog::RunLog;
use crate::state::State;
use crate::states::{self, findings, ship};
use crate::transcripts::Transcripts;
use crate::vcs::Vcs;
use crate::{artifact, auto, forge, harness, lock, manifest, status, token_totals, vcs, workspace};

/// Whether `unit_describe` asks the forge for the change request's checks, which the list skips.
#[derive(Clone, Copy, PartialEq)]
enum ChecksRead {
    Skip,
    Read,
}

/// Whether `findings` carries the text of the findings as well as their counts.
#[derive(Clone, Copy, PartialEq)]
enum FindingsText {
    CountsOnly,
    WithText,
}

/// Whether the unit's change request is held for a person to review.
#[derive(Clone, Copy, PartialEq)]
enum ReviewHold {
    Held,
    NotHeld,
}

#[derive(ClapArgs)]
pub struct Args {
    /// The unit to describe in full. Without one, every open unit is listed.
    pub unit: Option<u64>,
    /// With a unit, also give the text of its findings.
    #[arg(long, requires = "unit")]
    pub findings: bool,
}

/// Prints every open unit, in the order `ns auto` takes them, with its status, where the run log
/// says it stands, and its change request, and for an open one whether it is `current` with the
/// base branch or `behind` it, which only the unit's workspace can say. Given a unit, prints that
/// one in full instead: see `unit_detail`. Changes nothing.
pub fn run(args: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let forge = forge::get();
    let log = RunLog::open(vcs.as_ref(), &root)?;
    let queue = manifest::merge_queue_read(&root)?;
    if let Some(number) = args.unit {
        let unit = forge
            .unit_get(&root, number)
            .with_context(|| format!("unit {number} does not exist or cannot be read"))?;
        let detail = unit_detail(
            forge.as_ref(),
            vcs.as_ref(),
            &log,
            &root,
            queue,
            &unit,
            if args.findings {
                FindingsText::WithText
            } else {
                FindingsText::CountsOnly
            },
        )?;
        println!("{detail}");
        return Ok(());
    }
    let mut open = forge.unit_list(&root)?;
    open.sort_by_key(auto::order);
    let units = open
        .iter()
        .map(|unit| {
            unit_describe(
                forge.as_ref(),
                vcs.as_ref(),
                &log,
                &root,
                queue,
                unit,
                ChecksRead::Skip,
            )
        })
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
    checks: ChecksRead,
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
            if checks == ChecksRead::Read && cr.state == ChangeState::Open {
                described["checks"] = match forge.change_request_checks_get(root, cr.number)? {
                    Checks::Passed => json!({ "state": "passing" }),
                    Checks::Failed(names) => json!({ "state": "failing", "failing": names }),
                    // No check reported yet on an open request: none has started, so it is not
                    // passing.
                    Checks::Pending | Checks::NoneReported => json!({ "state": "pending" }),
                };
            }
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
    let running = lock::lock_holder(&vcs.state_dir_find(root)?, &format!("unit-{id}.lock"));
    let label = unit
        .labels
        .iter()
        .find(|l| l.starts_with(auto::STATE_PREFIX));
    let mut described = json!({
        "unit": unit.number,
        "title": unit.title,
        "status": label,
        "state": last.as_ref().map(|m| m.to.to_string()),
        "moved_at": last.and_then(|m| m.at),
        "change_request": change_request,
        "running": running.as_ref().map(|h| json!({ "pid": h.pid, "since": h.since })),
    });
    // In progress with no process running it: a run that died without a trace.
    if running.is_none() && label.is_some_and(|s| s == status::IN_PROGRESS) {
        described["stale"] = true.into();
    }
    Ok(described)
}

/// One unit in full: what `unit_describe` gives, with the attempt it is in out of the most a state
/// may have, why it stopped when it is stuck, what a person must do when the unit waits for one,
/// the tokens of its kept sessions, the paths of its artifacts, and how many findings the checks
/// made (their text with `text`). What needs the unit's workspace is left out when the workspace
/// is gone.
fn unit_detail(
    forge: &dyn Forge,
    vcs: &dyn Vcs,
    log: &RunLog,
    root: &std::path::Path,
    queue: bool,
    unit: &Unit,
    text: FindingsText,
) -> Result<Value> {
    let id = unit.number.to_string();
    let mut described = unit_describe(forge, vcs, log, root, queue, unit, ChecksRead::Read)?;
    described["attempt"] = json!(log.attempt(&id)?);
    described["max_attempts"] = manifest::max_attempts_read(root)?.into();
    // Only the sessions kept so far: the one running is kept when it ends.
    let kept = Transcripts::open(vcs, root)?.sessions(&id)?;
    described["tokens"] = token_totals::read(&*harness::get(), &kept).json();
    let last = log.last_move(&id)?;
    let stuck = last.filter(|m| m.to == State::Stuck);
    if let Some(m) = &stuck {
        described["stopped"] = json!({
            "reason": m.reason.map(|r| r.name()),
            "state": m.from.map(|s| s.to_string()),
        });
    }
    let wanted = workspace::locate(root, &id)?;
    if !wanted.path.exists() {
        return Ok(described);
    }
    let dir = wanted.path;
    let open = described["change_request"]["state"] == "open";
    let waiting = described["status"] == status::WAITING_FOR_HUMAN;
    let held = if open && waiting {
        ReviewHold::Held
    } else {
        ReviewHold::NotHeld
    };
    if let Some(needs) = needs_find(vcs, root, &dir, &id, stuck, held)? {
        described["needs"] = needs;
    }
    described["artifacts"] = artifacts_list(&dir, &id);
    if let Some(found) = findings_read(&dir, &id, text) {
        described["findings"] = found;
    }
    Ok(described)
}

/// What a person must do, when `ns` knows: for a change request that waits for a person, the files
/// and regions that make them merge it; for a unit stuck as inconclusive, the requirements still
/// unproven. A unit that waits for another reason (hardware, credentials, a decision) has none, for
/// `ns` does not know what the person must do then.
fn needs_find(
    vcs: &dyn Vcs,
    root: &std::path::Path,
    dir: &std::path::Path,
    id: &str,
    stuck: Option<crate::runlog::Move>,
    held: ReviewHold,
) -> Result<Option<Value>> {
    if stuck.is_some_and(|m| m.reason == Some(crate::runlog::Reason::Inconclusive)) {
        return Ok(states::unproven_list(dir, id)
            .ok()
            .filter(|l| !l.is_empty())
            .map(|unproven| json!({ "unproven": unproven })));
    }
    if held == ReviewHold::Held {
        let holds = ship::holds_list(vcs, root, dir)?;
        if !holds.is_empty() {
            return Ok(Some(json!({ "review": holds })));
        }
    }
    Ok(None)
}

/// The paths, in the workspace, of the build summary, the latest verification and the latest
/// review, for each that exists.
fn artifacts_list(dir: &std::path::Path, id: &str) -> Value {
    let path = |name: String| dir.join(artifact::DIR).join(id).join(name);
    let latest = |kind: &str| {
        let n = states::rounds_so_far(dir, id, kind);
        (n > 0).then(|| path(format!("{kind}-{n}.md")))
    };
    let mut listed = serde_json::Map::new();
    let build = path("build.md".into());
    if build.exists() {
        listed.insert("build".into(), json!(build));
    }
    for kind in ["verify", "review"] {
        if let Some(p) = latest(kind) {
            listed.insert(kind.into(), json!(p));
        }
    }
    Value::Object(listed)
}

/// How many findings the latest verification and each session of the latest review made, with
/// their text when asked. `None` when no check has run.
fn findings_read(dir: &std::path::Path, id: &str, text: FindingsText) -> Option<Value> {
    let read = |kind: &str| {
        let n = states::rounds_so_far(dir, id, kind);
        (n > 0)
            .then(|| artifact::read(dir, id, &format!("{kind}-{n}.md")).ok())
            .flatten()
    };
    let verification = read("verify").map(|t| findings::verification(&t));
    let review = read("review").map(|t| findings::review(&t));
    if verification.is_none() && review.is_none() {
        return None;
    }
    let mut counts = serde_json::Map::new();
    let mut texts = serde_json::Map::new();
    if let Some(found) = verification {
        counts.insert("verification".into(), found.len().into());
        texts.insert("verification".into(), json!(found));
    }
    if let Some(sessions) = review {
        let count = |s: &findings::Session| (s.name.clone(), json!(s.findings.len()));
        let text = |s: &findings::Session| (s.name.clone(), json!(s.findings));
        counts.insert(
            "review".into(),
            Value::Object(sessions.iter().map(count).collect()),
        );
        texts.insert(
            "review".into(),
            Value::Object(sessions.iter().map(text).collect()),
        );
    }
    if text == FindingsText::WithText {
        counts.insert("text".into(), Value::Object(texts));
    }
    Some(Value::Object(counts))
}
