use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::{DateTime, Local, NaiveTime};
use clap::Args as ClapArgs;
use serde_json::json;

use super::{run as unit_run, triage, troubleshoot};
use crate::forge::Forge;
use crate::os::Os;
use crate::{auto, forge, harness, lock, os, until, vcs, workspace};

#[derive(ClapArgs)]
pub struct Args {
    /// Print what would be triaged and run, in order, and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// How many ready units to run. Without it, every one that is ready.
    #[arg(long)]
    max_units: Option<usize>,
    /// Start no unit after this time of day, like 06:30. A unit already running finishes. If
    /// the time has passed today, it means tomorrow.
    #[arg(long, value_parser = until::time_parse)]
    until: Option<NaiveTime>,
    #[command(flatten)]
    options: unit_run::Options,
}

/// Triages every unit nobody has triaged, then runs the ready units one at a time, each
/// unattended, until none is left or `--max-units` have been run. A unit that is stuck or goes
/// wrong is left for a human by `ns run`, and the next unit goes on.
pub fn run(args: Args) -> Result<()> {
    let here = std::env::current_dir()?;
    let vcs = vcs::get();
    let root = vcs.root_find(&here)?;
    let forge = forge::get();
    let open = forge.unit_list(&root)?;
    let plan = auto::plan(&open);
    if args.dry_run {
        let list = |units: &[&forge::Unit]| -> Vec<_> {
            units
                .iter()
                .map(|u| json!({ "unit": u.number, "title": u.title }))
                .collect()
        };
        println!(
            "{}",
            json!({
                "queue": list(&plan.queue),
                "triage": list(&plan.triage),
                "repro": list(&plan.repro),
                "skipped": plan.skipped.iter()
                    .map(|(u, why)| json!({ "unit": u.number, "why": why }))
                    .collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }

    // Two at once would run the same units twice.
    let _lock = lock::lock_take(
        &vcs.state_dir_find(&root)?,
        "auto.lock",
        "another ns auto is running in this repository",
    )?;
    let harness = harness::get();
    let waiting: Vec<u64> = plan.triage.iter().map(|u| u.number).collect();
    let mut failed = triage::units_triage(
        forge.as_ref(),
        harness.as_ref(),
        &root,
        &waiting,
        Duration::from_secs(args.options.harness_timeout_seconds),
    );
    // Triage may have sent units to be reproduced.
    let open = forge.unit_list(&root)?;
    let repro: Vec<u64> = auto::plan(&open).repro.iter().map(|u| u.number).collect();
    failed += troubleshoot::units_troubleshoot(
        forge.as_ref(),
        harness.as_ref(),
        vcs.as_ref(),
        &root,
        &repro,
        Duration::from_secs(args.options.harness_timeout_seconds),
    );
    let os = os::get();
    let limits = Limits {
        max_units: args.max_units,
        deadline: args
            .until
            .map(|time| until::deadline_find(os.time_now(), time)),
    };
    failed += units_run(forge.as_ref(), os.as_ref(), &root, &limits, &mut |unit| {
        unit_run::run(unit_run::Args::unattended(unit, args.options.clone()))
    })?;
    if failed > 0 {
        bail!("{failed} units were not triaged or did not finish");
    }
    Ok(())
}

/// When `units_run` stops starting units.
struct Limits {
    /// How many units to start. `None` is every ready one.
    max_units: Option<usize>,
    /// The moment after which no unit is started. A unit already running finishes.
    deadline: Option<DateTime<Local>>,
}

/// Starts the ready units one at a time with `run_unit` until none is left or `limits` say stop,
/// printing how each ended. Returns how many did not end well.
fn units_run(
    forge: &dyn Forge,
    os: &dyn Os,
    root: &Path,
    limits: &Limits,
    run_unit: &mut dyn FnMut(u64) -> Result<()>,
) -> Result<usize> {
    let mut failed = 0;
    // A unit is run once, even when it is still ready afterwards.
    let mut run_already = HashSet::new();
    while limits.max_units.is_none_or(|max| run_already.len() < max) {
        if limits.deadline.is_some_and(|at| os.time_now() >= at) {
            println!("{}", json!({ "stopped": "until" }));
            break;
        }
        // Read again: triage and the units just run change what is ready and what is blocked.
        let open = forge.unit_list(root)?;
        let plan = auto::plan(&open);
        let Some(unit) = plan
            .queue
            .iter()
            .map(|u| u.number)
            .find(|n| !run_already.contains(n))
        else {
            break;
        };
        run_already.insert(unit);
        let ended = run_unit(unit);
        if let Err(e) = &ended {
            eprintln!("ns: unit {unit}: {e:#}");
            failed += 1;
        }
        let status: Vec<String> = forge
            .unit_get(root, unit)
            .map(|u| u.labels)
            .unwrap_or_default()
            .into_iter()
            .filter(|l| l.starts_with(auto::STATE_PREFIX))
            .collect();
        // A unit that never got as far as a change request has none.
        let change_request = workspace::locate(root, &unit.to_string())
            .ok()
            .and_then(|w| forge.change_request_get(root, &w.branch).ok().flatten())
            .map(|cr| cr.url);
        println!(
            "{}",
            json!({
                "unit": unit,
                "ok": ended.is_ok(),
                "status": status,
                "change_request": change_request,
                "error": ended.as_ref().err().map(|e| format!("{e:#}")),
            })
        );
    }
    Ok(failed)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::process::Command;

    use chrono::{Duration as Span, TimeZone};

    use super::*;
    use crate::forge::{ChangeRequest, Checks, Unit};
    use crate::os::Finished;

    /// Ready units 1, 2 and 3, and nothing else.
    struct Ready;

    impl Forge for Ready {
        fn login_get(&self, _: &Path) -> Result<String> {
            unreachable!()
        }
        fn unit_list(&self, _: &Path) -> Result<Vec<Unit>> {
            Ok((1..=3)
                .map(|number| Unit {
                    number,
                    title: String::new(),
                    body: String::new(),
                    labels: vec![auto::READY.into()],
                    from_team: true,
                })
                .collect())
        }
        fn unit_get(&self, _: &Path, number: u64) -> Result<Unit> {
            Ok(Unit {
                number,
                title: String::new(),
                body: String::new(),
                labels: Vec::new(),
                from_team: true,
            })
        }
        fn labels_list(&self, _: &Path) -> Result<Vec<String>> {
            unreachable!()
        }
        fn label_create(&self, _: &Path, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_labels_set(&self, _: &Path, _: u64, _: &[String], _: &[String]) -> Result<()> {
            unreachable!()
        }
        fn unit_body_set(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_close(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn change_request_get(&self, _: &Path, _: &str) -> Result<Option<ChangeRequest>> {
            Ok(None)
        }
        fn change_request_create(
            &self,
            _: &Path,
            _: &str,
            _: u64,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<ChangeRequest> {
            unreachable!()
        }
        fn change_request_checks_get(&self, _: &Path, _: u64) -> Result<Checks> {
            unreachable!()
        }
        fn change_request_merge(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn release_download(&self, _: &str, _: &str, _: &[&str], _: &Path) -> Result<()> {
            unreachable!()
        }
    }

    /// An operating system whose clock only moves when a test moves it.
    struct Clock(Cell<DateTime<Local>>);

    impl Os for Clock {
        fn process_run(&self, _: Command, _: &[u8], _: Duration) -> Result<Finished> {
            unreachable!()
        }
        fn process_interact(&self, _: Command) -> Result<bool> {
            unreachable!()
        }
        fn process_replace(&self, _: Command) -> anyhow::Error {
            unreachable!()
        }
        fn shell_command(&self, _: &str) -> Command {
            unreachable!()
        }
        fn time_now(&self) -> DateTime<Local> {
            self.0.get()
        }
    }

    fn start() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 3, 10, 22, 0, 0)
            .earliest()
            .unwrap()
    }

    /// Runs the ready units with a clock that each unit moves on by `each`, and returns the units
    /// that were started.
    fn started(limits: Limits, each: Span) -> Vec<u64> {
        let clock = Clock(Cell::new(start()));
        let started = RefCell::new(Vec::new());
        units_run(&Ready, &clock, Path::new("."), &limits, &mut |unit| {
            started.borrow_mut().push(unit);
            clock.0.set(clock.0.get() + each);
            Ok(())
        })
        .unwrap();
        started.into_inner()
    }

    #[test]
    fn no_unit_is_started_after_the_deadline_and_the_one_running_finishes() {
        let limits = Limits {
            max_units: None,
            deadline: Some(start() + Span::hours(1)),
        };
        // Unit 2 starts at 22:30:30, before the deadline, and ends at 23:01, after it. Unit 3 never
        // starts.
        let started = started(limits, Span::minutes(30) + Span::seconds(30));
        assert_eq!(started, [1, 2]);
    }

    #[test]
    fn a_deadline_that_has_already_passed_starts_nothing() {
        let limits = Limits {
            max_units: None,
            deadline: Some(start() - Span::minutes(1)),
        };
        assert!(started(limits, Span::minutes(1)).is_empty());
    }

    #[test]
    fn without_a_deadline_every_ready_unit_is_started() {
        let limits = Limits {
            max_units: None,
            deadline: None,
        };
        assert_eq!(started(limits, Span::hours(9)), [1, 2, 3]);
    }

    #[test]
    fn the_deadline_and_the_count_each_stop_the_night() {
        let limits = Limits {
            max_units: Some(1),
            deadline: Some(start() + Span::hours(9)),
        };
        assert_eq!(started(limits, Span::minutes(1)), [1]);
    }
}
