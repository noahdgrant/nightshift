//! The phase driver: the loop that feeds `core::step` what the shell observed and performs the
//! action it returns, and the ledger's I/O. Running a phase is `phase_run`'s; the guards and the
//! gate after it are `post_phase`'s.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{json, Value};

use super::core::route::{Settings, State};
use super::core::step::{self, Action, Ending, Event, Last};
use super::core::{Ledger, MergeBack};
use super::currency::{archive_as, gather_facts, read_state};
use super::merge::{merge_step, MergeStep};
use super::phase_run::{run_phase, Ran, Undo};
use super::post_phase::{guards, run_gate};
use super::{finish, remote_default, Ctx, Finish, Outcome, RunArgs, Shared};
use crate::factory::{artifact_of, Factory, PHASES};
use crate::gate;

/// What the driver's actions run with: the unit, the run's flags and shared state, and what the
/// run has recorded so far.
pub(super) struct Io<'r, 'c> {
    pub ctx: &'r Ctx<'c>,
    pub args: &'r RunArgs,
    pub shared: &'r mut Shared,
    pub commands: &'r BTreeMap<&'static str, Vec<String>>,
    pub phases: &'r mut Vec<Value>,
    pub last_artifact: &'r mut Option<String>,
    pub gate_state: gate::Gate,
    /// The default branch and its sha as the guards last saw it.
    pub baseline: Option<(String, String)>,
}

/// Drive the unit to an outcome. A run that finishes removes its ledger, so the next `ns run`
/// of the unit starts fresh; one that stops without finishing (a stop signal, a crash, a reboot)
/// leaves it for the next run to resume.
pub(super) fn drive(
    ctx: &Ctx<'_>,
    args: &RunArgs,
    shared: &mut Shared,
    commands: &BTreeMap<&'static str, Vec<String>>,
    phases: &mut Vec<Value>,
    last_artifact: &mut Option<String>,
) -> Result<Finish> {
    let mut io = Io {
        ctx,
        args,
        shared,
        commands,
        phases,
        last_artifact,
        gate_state: gate::Gate::default(),
        baseline: None,
    };
    let r = drive_loop(&mut io);
    // A paused, out-of-budget or until-bound run is resumed later: its ledger stays.
    if r.as_ref().is_ok_and(|f| {
        matches!(
            f.outcome,
            Outcome::Done | Outcome::Merged | Outcome::Stuck | Outcome::Split
        )
    }) {
        let _ = fs::remove_file(ledger_path(&ctx.artifacts));
    }
    r
}

/// The settings `core::step` decides with, from the factory and the run's flags.
pub(super) fn settings(fac: &Factory, args: &RunArgs) -> Settings {
    Settings {
        max_attempts: PHASES
            .iter()
            .map(|p| (*p, fac.phase(p).max_attempts))
            .collect(),
        merge_auto: fac.merge.auto(),
        triage_only: args.triage_only,
        local_gate: fac.gate.local,
    }
}

/// The unit's state at `head`, with every git fact `core::step` may ask about.
pub(super) fn unit_state(
    ctx: &Ctx<'_>,
    ledger: &Ledger,
    head: String,
    diffs: bool,
    settings: Settings,
) -> State {
    let diff = diffs.then_some((ctx.worktree.as_path(), ctx.base.as_str()));
    let mut s = read_state(&ctx.artifacts, head, ctx.issue.is_some(), diff, settings);
    if diffs {
        gather_facts(&mut s, ledger, &ctx.worktree, &ctx.base);
    }
    s
}

/// The phase run the next guard or gate follows.
struct Current {
    adm: super::core::Admitted,
    undo: Option<Undo>,
}

fn drive_loop(io: &mut Io<'_, '_>) -> Result<Finish> {
    let ctx = io.ctx;
    let (mut ledger, events) = start_ledger(io.args, &ctx.artifacts, &ctx.head());
    for ev in events {
        ctx.log(ev);
    }
    io.baseline = remote_default(&ctx.worktree);
    ctx.log(json!({
        "event": "start",
        "default_branch": io.baseline.as_ref().map(|b| &b.0),
        "default_sha": io.baseline.as_ref().map(|b| &b.1),
        "memory_cap": ctx.env.cap.map(|c| json!({
            "mb": c.mb,
            "via": c.via.label(),
            "fallback": c.fallback,
        })),
        "cpus": io.shared.cpus,
    }));
    let settings = settings(ctx.fac, io.args);
    let mut state = State::default();
    let mut current: Option<Current> = None;
    let mut action = Action::Decide;
    loop {
        let ev = match action {
            Action::Decide => {
                let head = ctx.head();
                ledger.saved_at(&head);
                save_ledger(ctx, &ledger);
                crate::stop::check()?;
                state = unit_state(ctx, &ledger, head, true, settings.clone());
                Event::Start
            }
            Action::ReadRemote => Event::Remote(remote_default(&ctx.worktree)),
            Action::RunPhase(adm) => {
                let ran = run_phase(io, &state, &adm)?;
                let mut cur = Current { adm, undo: None };
                let ev = match ran {
                    Ran::Phase(r, undo) => {
                        cur.undo = Some(undo);
                        Event::PhaseRan(r)
                    }
                    Ran::LockGaveUp(reason) => Event::LockGaveUp(reason),
                    Ran::Finish(f) => return Ok(f),
                };
                current = Some(cur);
                ev
            }
            Action::CheckGuards => {
                let phase = current.as_ref().map_or("", |c| c.adm.phase);
                Event::GuardFacts(guards(ctx, &mut io.baseline, phase)?)
            }
            Action::SetAside { phase, errors } => {
                let file = artifact_of(phase);
                let stem = format!("{}-invalid", file.trim_end_matches(".md"));
                let archived = archive_as(&ctx.artifacts, file, &stem)?;
                ctx.log(json!({
                    "event": "artifact_invalid",
                    "phase": phase,
                    "attempt": current.as_ref().map(|c| c.adm.attempt),
                    "errors": errors,
                    "archived": archived.map(|p| p.to_string_lossy().into_owned()),
                }));
                Event::Archived
            }
            Action::RunGate(call) => {
                let Some(Current {
                    adm,
                    undo: Some(undo),
                }) = &current
                else {
                    anyhow::bail!("ns run: a gate with no phase run before it");
                };
                let at = (adm.phase, adm.attempt);
                Event::GateRan(run_gate(io, &ledger, &call, at, undo)?)
            }
            Action::Merge { run } => {
                let back = match merge_step(ctx, &state, io.shared, &mut io.gate_state, run)? {
                    MergeStep::Finish(f) => return Ok(f),
                    MergeStep::Rebuild(red) => MergeBack::Rebuild {
                        stuck_reason: red.stuck_reason(),
                        feedback: red.feedback,
                        reason: red.reason,
                    },
                    MergeStep::Reship(feedback) => MergeBack::Reship(feedback),
                    MergeStep::Rebase(feedback, why) => MergeBack::Rebase { feedback, why },
                };
                Event::MergeEnded(back)
            }
            Action::Finish(e) => return ended(ctx, io.last_artifact, e),
        };
        action = step::step(&mut ledger, &state, ev);
    }
}

/// Carry out how `core::step` ended the run: its log line and the artifact a human reads next.
fn ended(ctx: &Ctx<'_>, last_artifact: &mut Option<String>, e: Ending) -> Result<Finish> {
    if let Some(ev) = e.log {
        ctx.log(ev);
    }
    match e.artifact {
        Some(Last::File(f)) if last_artifact.is_none() => {
            *last_artifact = Some(ctx.artifacts.join(f).to_string_lossy().into_owned());
        }
        Some(Last::Archived(f)) => {
            let kept = archive_as(&ctx.artifacts, f, f.trim_end_matches(".md"))?;
            *last_artifact = kept.map(|p| p.to_string_lossy().into_owned());
        }
        _ => {}
    }
    Ok(finish(e.outcome, e.reason, e.phase))
}

/// Where the unit's ledger is kept between `ns run` invocations.
fn ledger_path(artifacts: &std::path::Path) -> PathBuf {
    artifacts.join("state.json")
}

/// The ledger a run starts from, with the events to log about it: a new one under `--from`,
/// `--triage-only` or `--fresh`, else the saved one at `head`. A saved ledger that can't be read
/// or doesn't parse (a torn write, an older format) is dropped, and the run re-derives its state
/// from the artifacts.
pub(super) fn start_ledger(
    args: &RunArgs,
    artifacts: &std::path::Path,
    head: &str,
) -> (Ledger, Vec<Value>) {
    let from = args
        .triage_only
        .then_some("triage")
        .or(args.from.as_deref());
    if from.is_some() || args.fresh {
        return (Ledger::new(from), vec![]);
    }
    let path = ledger_path(artifacts);
    match load_ledger(artifacts) {
        Ok(None) => (Ledger::new(None), vec![]),
        Ok(Some(mut l)) => {
            let dropped = l.resumed_at(head);
            let ev = json!({"event": "ledger_resumed", "path": path.to_string_lossy(), "forced_dropped": dropped});
            (l, vec![ev])
        }
        Err(e) => {
            let ev = json!({"event": "state_rederived", "path": path.to_string_lossy(), "error": format!("{e:#}")});
            (Ledger::new(None), vec![ev])
        }
    }
}

/// The saved ledger, `None` when there is none.
fn load_ledger(artifacts: &std::path::Path) -> Result<Option<Ledger>> {
    let text = match fs::read_to_string(ledger_path(artifacts)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some(serde_json::from_str(&text)?))
}

/// Write the ledger through a temp file and a rename, so a crash leaves the old one or the new
/// one. A failed write is logged, not fatal: the run only loses what a restart would resume.
fn save_ledger(ctx: &Ctx<'_>, ledger: &Ledger) {
    let path = ledger_path(&ctx.artifacts);
    let tmp = path.with_extension("json.tmp");
    let write = serde_json::to_vec_pretty(ledger)
        .map_err(anyhow::Error::from)
        .and_then(|b| fs::write(&tmp, b).map_err(anyhow::Error::from))
        .and_then(|()| fs::rename(&tmp, &path).map_err(anyhow::Error::from));
    if let Err(e) = write {
        ctx.log(json!({"event": "ledger_not_saved", "path": path.to_string_lossy(), "error": format!("{e:#}")}));
    }
}

#[cfg(test)]
mod tests;
