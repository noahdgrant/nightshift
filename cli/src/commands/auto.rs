use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, NaiveTime};
use clap::Args as ClapArgs;
use serde_json::json;

use super::{run as unit_run, triage, troubleshoot};
use crate::auto::Streak;
use crate::forge::Forge;
use crate::harness::Sessions;
use crate::manifest::Place;
use crate::os::Os;
use crate::runlog::{Move, Reason, RunLog};
use crate::state::State;
use crate::transcripts::Transcripts;
use crate::{auto, forge, harness, lock, manifest, os, summary, until, vcs, workspace};

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
    /// Start no more units after this many in a row got stuck because a state kept going wrong
    /// (the harness did not finish, ran out of time, the forge did not answer), not because
    /// checks failed, or this many triage or troubleshoot sessions in a row failed. A unit that
    /// finishes or fails its checks, or a session that ends, ends the streak. 0 turns it off.
    #[arg(long, default_value_t = 2)]
    max_failures_in_a_row: usize,
    #[command(flatten)]
    options: unit_run::Options,
}

/// Triages every unit nobody has triaged, then runs the ready units one at a time, each
/// unattended, until none is left or `--max-units` have been run. A unit that is stuck or goes
/// wrong is marked stuck by `ns run`, and the next unit goes on, unless too many in a row went
/// wrong.
pub fn run(args: Args) -> Result<()> {
    let here = std::env::current_dir()?;
    let vcs = vcs::get();
    let root = vcs.root_find(&here)?;
    if args.dry_run {
        night_run(
            &args,
            vcs.as_ref(),
            &root,
            os::get().as_ref(),
            Mode::DryRun,
            &mut Streak::new(0),
        )?;
        return Ok(());
    }
    let state_dir = vcs.state_dir_find(&root)?;
    // Two at once would run the same units twice. Taken before anything else can fail, so that
    // every night that gets this far leaves a summary; a refused one leaves none, because the
    // run that holds the lock owns the night's file.
    let _lock = night_lock(&state_dir)?;
    let os = os::get();
    let night = os.time_now().date_naive();
    let log = RunLog::open(vcs.as_ref(), &root)?;
    let forge = forge::get();
    let mut streak = Streak::new(args.max_failures_in_a_row);
    let ended = RefCell::new(Vec::new());
    let ran = night_run(
        &args,
        vcs.as_ref(),
        &root,
        os.as_ref(),
        Mode::Live {
            log: &log,
            ended: &ended,
        },
        &mut streak,
    );
    let went_wrong = went_wrong_text(streak.units().len());
    // Written even when the night went wrong, which is when a person most needs it.
    let stopped = stopped_find(
        streak.usage_limit(),
        streak.reached(),
        streak.units().len(),
        &ran,
    );
    night_write(
        forge.as_ref(),
        &root,
        &state_dir,
        night,
        ended.into_inner(),
        stopped.as_deref(),
        &|unit| log.last_move(&unit.to_string()).ok().flatten(),
    )?;
    let failed = ran?;
    if let Some(reason) = streak.usage_limit() {
        // Every unit is as it was, so this is a clean stop and not a failure.
        println!("{}", json!({ "stopped": "usage_limit", "reason": reason }));
        eprintln!("ns: stopped: {reason}");
        return Ok(());
    }
    if streak.reached() {
        println!(
            "{}",
            json!({
                "stopped": "went_wrong",
                "reason": went_wrong,
                "units": streak.units(),
            })
        );
        bail!(
            "stopped: {} units in a row went wrong ({}), which looks like the harness and not the code",
            streak.units().len(),
            streak.units().iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
        );
    }
    if failed > 0 {
        bail!("{failed} units were not triaged or did not finish");
    }
    Ok(())
}

fn went_wrong_text(units: usize) -> String {
    format!("{units} units in a row got stuck because a state kept going wrong")
}

/// Why the night ended before the units ran out, if it did, for the summary. A usage limit comes
/// first, then too many units in a row going wrong, then an error that cut the night short.
fn stopped_find(
    usage_limit: Option<&str>,
    streak_reached: bool,
    streak_units: usize,
    ran: &Result<usize>,
) -> Option<String> {
    if let Some(limit) = usage_limit {
        return Some(limit.to_string());
    }
    if streak_reached {
        return Some(went_wrong_text(streak_units));
    }
    ran.as_ref().err().map(|e| format!("{e:#}"))
}

/// Whether a night is for real or only shows what it would do.
enum Mode<'a> {
    DryRun,
    /// `log` is the run log and `ended` is where each unit run is noted.
    Live {
        log: &'a RunLog,
        ended: &'a RefCell<Vec<summary::Ended>>,
    },
}

/// Writes the night's summary file into `state_dir`: each unit of `ended` with where it ended up on
/// the forge, or as stuck when the forge cannot say. `last_move` gives a unit's last move in the
/// run log.
fn night_write(
    forge: &dyn Forge,
    root: &Path,
    state_dir: &Path,
    night: chrono::NaiveDate,
    ended: Vec<summary::Ended>,
    stopped: Option<&str>,
    last_move: &dyn Fn(u64) -> Option<Move>,
) -> Result<()> {
    let entries: Vec<_> = ended
        .iter()
        .map(|e| {
            let labels = forge
                .unit_get(root, e.unit)
                .map(|u| u.labels)
                .map_err(|err| format!("{err:#}"));
            summary::entry_find(
                e.unit,
                labels.as_deref().map_err(String::as_str),
                last_move(e.unit).as_ref(),
                &e.run,
            )
        })
        .collect();
    let file = state_dir.join(summary::file_name(night));
    std::fs::write(&file, summary::render(night, &entries, stopped))
        .with_context(|| format!("cannot write {}", file.display()))
}

/// Everything of a night up to the summary: triage, troubleshooting and the units. Returns how
/// many did not end well. A dry run only lists the plan.
fn night_run(
    args: &Args,
    vcs: &dyn vcs::Vcs,
    root: &Path,
    os: &dyn Os,
    mode: Mode,
    streak: &mut Streak,
) -> Result<usize> {
    // Worked out once, so every unit of the night gets the same number.
    let mut options = args.options.clone();
    options.max_attempts = Some(match options.max_attempts {
        Some(n) => n,
        None => manifest::max_attempts_read(root)?,
    });
    options.ci_timeout_seconds = Some(match options.ci_timeout_seconds {
        Some(n) => n,
        None => manifest::ci_timeout_read(root)?,
    });
    let keep = manifest::transcript_days_read(root)?;
    // Read here so a bad value stops `ns` before it runs anything.
    manifest::review_sessions_read(root)?;
    manifest::retrospective_share_read(root)?;
    manifest::model_read(root, Place::Build, None)?;
    let forge = forge::get();
    let open = forge.unit_list(root)?;
    let plan = auto::plan(&open, &blockers_read(forge.as_ref(), root, &open));
    let Mode::Live { log, ended } = mode else {
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
        return Ok(0);
    };

    Transcripts::open(vcs, root)?.prune(keep, SystemTime::now());
    let harness = harness::get();
    let triage_model = manifest::model_read(root, Place::Triage, args.options.model.as_deref())?;
    let troubleshoot_model =
        manifest::model_read(root, Place::Troubleshoot, args.options.model.as_deref())?;
    let waiting: Vec<u64> = plan.triage.iter().map(|u| u.number).collect();
    let mut failed = triage::units_triage(
        forge.as_ref(),
        harness.as_ref(),
        root,
        &waiting,
        Duration::from_secs(args.options.harness_timeout_seconds),
        triage_model.as_deref(),
        streak,
    );
    // Triage may have sent units to be reproduced.
    let open = forge.unit_list(root)?;
    let repro: Vec<u64> = auto::plan(&open, &HashMap::new())
        .repro
        .iter()
        .map(|u| u.number)
        .collect();
    failed += troubleshoot::units_troubleshoot(
        forge.as_ref(),
        harness.as_ref(),
        vcs,
        root,
        &repro,
        &Sessions {
            timeout: Duration::from_secs(args.options.harness_timeout_seconds),
            model: troubleshoot_model.as_deref(),
        },
        streak,
    );
    let limits = Limits {
        max_units: args.max_units,
        deadline: args
            .until
            .map(|time| until::deadline_find(os.time_now(), time)),
    };
    // The unit's last move before it ran, to tell a move this run wrote from an older one.
    let before = RefCell::new(None);
    failed += units_run(
        forge.as_ref(),
        os,
        root,
        &limits,
        streak,
        &mut |unit| {
            *before.borrow_mut() = log.last_move(&unit.to_string()).ok().flatten();
            let result = unit_run::run(unit_run::Args::unattended(
                unit,
                options.clone(),
                limits.deadline,
            ));
            let run = match &result {
                Ok(()) => summary::Run::Done,
                Err(e) => summary::Run::Failed(format!("{e:#}")),
            };
            ended.borrow_mut().push(summary::Ended { unit, run });
            result
        },
        &mut |unit| {
            went_wrong_since(
                before.take(),
                log.last_move(&unit.to_string()).ok().flatten(),
            )
        },
    )?;
    Ok(failed)
}

/// The lock that keeps two `ns auto` out of one repository. It fails at once, naming itself, when
/// another holds it; the operating system lets go of it when its holder dies.
fn night_lock(state_dir: &Path) -> Result<lock::Lock> {
    lock::lock_take(
        state_dir,
        "auto.lock",
        "another ns auto is running in this repository",
    )
}

/// What the forge says blocks each ready unit of `open`, by unit. Only a ready unit's blockers
/// matter to the plan, so only those are asked for.
fn blockers_read(forge: &dyn Forge, root: &Path, open: &[forge::Unit]) -> HashMap<u64, Vec<u64>> {
    open.iter()
        .filter(|u| u.from_team && u.labels.iter().any(|l| l == auto::READY))
        .map(|u| (u.number, forge::blockers_get(forge, root, u.number)))
        .collect()
}

/// Whether a unit's run moved it into `stuck` because a state kept going wrong. `before` and
/// `after` are its last move in the run log before and after the run: a move the run did not write
/// says nothing about it.
fn went_wrong_since(before: Option<Move>, after: Option<Move>) -> bool {
    after != before
        && matches!(
            after,
            Some(Move {
                to: State::Stuck,
                reason: Some(Reason::WentWrong),
                ..
            })
        )
}

/// When `units_run` stops starting units.
struct Limits {
    /// How many units to start. `None` is every ready one.
    max_units: Option<usize>,
    /// The moment after which no unit is started. A unit already running finishes.
    deadline: Option<DateTime<Local>>,
}

/// Starts the ready units one at a time with `run_unit` until none is left or `limits` say stop,
/// printing how each ended, and returns how many did not end well. `went_wrong` says, of a unit that
/// did not end well, whether it got stuck because a state kept going wrong; those count toward
/// `streak`, which also stops the night once it is reached.
fn units_run(
    forge: &dyn Forge,
    os: &dyn Os,
    root: &Path,
    limits: &Limits,
    streak: &mut Streak,
    run_unit: &mut dyn FnMut(u64) -> Result<()>,
    went_wrong: &mut dyn FnMut(u64) -> bool,
) -> Result<usize> {
    let mut failed = 0;
    // A unit is run once, even when it is still ready afterwards.
    let mut run_already = HashSet::new();
    while limits.max_units.is_none_or(|max| run_already.len() < max) && !streak.reached() {
        if limits.deadline.is_some_and(|at| os.time_now() >= at) {
            println!("{}", json!({ "stopped": "until" }));
            break;
        }
        // Read again: triage and the units just run change what is ready and what is blocked.
        let open = forge.unit_list(root)?;
        let plan = auto::plan(&open, &blockers_read(forge, root, &open));
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
            streak.error_record(unit, e, went_wrong(unit));
        } else {
            streak.record(unit, false);
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

    /// Ready units 1, 2 and 3, and nothing else. The forge says an open unit 99 blocks each unit
    /// in `blocked`.
    #[derive(Default)]
    struct Ready {
        blocked: &'static [u64],
        /// Whether the forge fails to give a unit back.
        get_fails: bool,
    }

    impl Forge for Ready {
        fn branch_protection_get(&self, _: &Path) -> Result<crate::forge::BranchProtection> {
            unreachable!()
        }
        fn branch_rules_get(&self, _: &Path, _: &str) -> Result<crate::forge::BranchRules> {
            unreachable!()
        }
        fn login_get(&self, _: &Path) -> Result<String> {
            unreachable!()
        }
        fn unit_blocker_add(&self, _: &Path, _: u64, _: u64) -> Result<()> {
            unreachable!()
        }
        fn unit_blockers_list(&self, _: &Path, number: u64) -> Result<Vec<u64>> {
            Ok(if self.blocked.contains(&number) {
                vec![99]
            } else {
                Vec::new()
            })
        }
        fn unit_create(&self, _: &Path, _: &str, _: &str, _: &[String]) -> Result<u64> {
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
                    url: String::new(),
                })
                .collect())
        }
        fn unit_get(&self, _: &Path, number: u64) -> Result<Unit> {
            if self.get_fails {
                anyhow::bail!("rate limited");
            }
            Ok(Unit {
                number,
                title: String::new(),
                body: String::new(),
                labels: Vec::new(),
                from_team: true,
                url: String::new(),
            })
        }
        fn unit_types_list(&self, _: &Path) -> Result<Vec<String>> {
            unreachable!()
        }
        fn unit_type_set(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn labels_list(&self, _: &Path) -> Result<Vec<crate::forge::Label>> {
            unreachable!()
        }
        fn label_create(&self, _: &Path, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        fn label_rename(&self, _: &Path, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        fn label_describe(&self, _: &Path, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_labels_set(&self, _: &Path, _: u64, _: &[String], _: &[String]) -> Result<()> {
            unreachable!()
        }
        fn unit_body_set(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_comment_add(&self, _: &Path, _: u64, _: &str) -> Result<()> {
            unreachable!()
        }
        fn unit_assignee_add(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn unit_close(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn change_request_assignee_add(&self, _: &Path, _: u64) -> Result<()> {
            unreachable!()
        }
        fn change_request_label_add(&self, _: &Path, _: u64, _: &str) -> Result<()> {
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
        fn change_request_merge(&self, _: &Path, _: u64, _: &str) -> Result<()> {
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
        fn sleep(&self, _: Duration) {
            unreachable!()
        }
        fn runner_install(&self, _: &crate::os::Runner) -> Result<crate::os::Installed> {
            unreachable!()
        }
        fn runner_remove(&self, _: &Path) -> Result<bool> {
            unreachable!()
        }
        fn runner_check(&self, _: &crate::os::Runner) -> Result<crate::os::RunnerState> {
            unreachable!()
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
        units_run(
            &Ready::default(),
            &clock,
            Path::new("."),
            &limits,
            &mut Streak::new(0),
            &mut |unit| {
                started.borrow_mut().push(unit);
                clock.0.set(clock.0.get() + each);
                Ok(())
            },
            &mut |_| false,
        )
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
    fn a_unit_the_forge_says_is_blocked_is_not_started() {
        let started = RefCell::new(Vec::new());
        units_run(
            &Ready {
                blocked: &[2],
                ..Ready::default()
            },
            &Clock(Cell::new(start())),
            Path::new("."),
            &Limits {
                max_units: None,
                deadline: None,
            },
            &mut Streak::new(0),
            &mut |unit| {
                started.borrow_mut().push(unit);
                Ok(())
            },
            &mut |_| false,
        )
        .unwrap();
        assert_eq!(started.into_inner(), [1, 3]);
    }

    #[test]
    fn only_ready_units_of_the_team_are_asked_about() {
        let unit = |number, labels: &[&str], from_team| Unit {
            number,
            title: String::new(),
            body: String::new(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            from_team,
            url: String::new(),
        };
        let open = [
            unit(1, &[auto::READY], true),
            unit(2, &[auto::READY], true),
            unit(3, &[], true),
            unit(4, &[auto::READY], false),
        ];
        let read = blockers_read(
            &Ready {
                blocked: &[2, 3, 4],
                ..Ready::default()
            },
            Path::new("."),
            &open,
        );
        assert_eq!(read, HashMap::from([(1, Vec::new()), (2, vec![99])]));
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

    #[test]
    fn a_second_ns_auto_is_refused_by_the_lock_and_one_after_the_first_ends_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let first = night_lock(tmp.path()).unwrap();
        let err = night_lock(tmp.path()).err().unwrap();
        assert_eq!(
            err.to_string(),
            "another ns auto is running in this repository"
        );
        drop(first);
        night_lock(tmp.path()).unwrap();
    }

    #[test]
    fn a_lock_file_left_by_a_process_that_died_does_not_block() {
        let tmp = tempfile::tempdir().unwrap();
        // A dead process holds no lock but may leave its file.
        std::fs::write(tmp.path().join("auto.lock"), "").unwrap();
        night_lock(tmp.path()).unwrap();
    }

    fn night_text(forge: &Ready, ended: Vec<summary::Ended>, stopped: Option<&str>) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        night_write(
            forge,
            Path::new("."),
            tmp.path(),
            date,
            ended,
            stopped,
            &|_| None,
        )
        .unwrap();
        std::fs::read_to_string(tmp.path().join("night-2026-03-10.md")).unwrap()
    }

    #[test]
    fn a_usage_limit_is_named_before_a_streak_or_an_error() {
        let ran: Result<usize> = Err(anyhow::anyhow!("boom"));
        assert_eq!(
            stopped_find(Some("limit hit"), true, 3, &ran).as_deref(),
            Some("limit hit")
        );
    }

    #[test]
    fn a_streak_is_named_before_an_error() {
        let ran: Result<usize> = Err(anyhow::anyhow!("boom"));
        assert_eq!(
            stopped_find(None, true, 3, &ran).as_deref(),
            Some("3 units in a row got stuck because a state kept going wrong")
        );
    }

    #[test]
    fn an_error_alone_is_the_reason_the_night_stopped() {
        let ran: Result<usize> = Err(anyhow::anyhow!("forge down").context("cannot list"));
        assert_eq!(
            stopped_find(None, false, 0, &ran).as_deref(),
            Some("cannot list: forge down")
        );
    }

    #[test]
    fn a_night_that_ran_out_of_units_did_not_stop_early() {
        assert_eq!(stopped_find(None, false, 0, &Ok(2)), None);
    }

    #[test]
    fn a_night_with_no_unit_still_writes_its_summary() {
        assert_eq!(
            night_text(&Ready::default(), Vec::new(), None),
            "# ns auto, night of 2026-03-10\n\nNo unit was ready to run.\n"
        );
    }

    #[test]
    fn a_night_that_stopped_early_says_why_in_its_summary() {
        let text = night_text(
            &Ready::default(),
            Vec::new(),
            Some("cannot read the settings"),
        );
        assert!(
            text.contains("Stopped early: cannot read the settings\n"),
            "{text}"
        );
    }

    #[test]
    fn a_unit_whose_run_failed_is_listed_stuck_with_the_error() {
        let ended = vec![
            summary::Ended {
                unit: 1,
                run: summary::Run::Done,
            },
            summary::Ended {
                unit: 2,
                run: summary::Run::Failed("boom".into()),
            },
        ];
        let text = night_text(&Ready::default(), ended, None);
        assert!(text.contains("## Merged\n\n- #1\n"), "{text}");
        assert!(text.contains("## Stuck\n\n- #2: boom\n"), "{text}");
    }

    #[test]
    fn a_unit_the_forge_cannot_describe_is_not_listed_as_merged() {
        let forge = Ready {
            get_fails: true,
            ..Ready::default()
        };
        let ended = vec![summary::Ended {
            unit: 1,
            run: summary::Run::Done,
        }];
        let text = night_text(&forge, ended, None);
        assert!(text.contains("## Merged\n\nNone\n"), "{text}");
        assert!(
            text.contains("- #1: its labels could not be read from the forge: rate limited\n"),
            "{text}"
        );
    }

    #[derive(Clone, Copy)]
    enum End {
        Finishes,
        ChecksFail,
        WentWrong,
    }

    /// Runs the ready units, each ending as the next of `ends`, and returns the units started.
    fn ended(streak: &mut Streak, ends: [End; 3]) -> Vec<u64> {
        let limits = Limits {
            max_units: None,
            deadline: None,
        };
        let clock = Clock(Cell::new(start()));
        let started = RefCell::new(Vec::new());
        let end_of = |unit: u64| ends[unit as usize - 1];
        units_run(
            &Ready::default(),
            &clock,
            Path::new("."),
            &limits,
            streak,
            &mut |unit| {
                started.borrow_mut().push(unit);
                match end_of(unit) {
                    End::Finishes => Ok(()),
                    End::ChecksFail | End::WentWrong => Err(anyhow::anyhow!("stuck")),
                }
            },
            &mut |unit| matches!(end_of(unit), End::WentWrong),
        )
        .unwrap();
        started.into_inner()
    }

    #[test]
    fn a_usage_limit_stops_the_night_and_is_not_a_unit_that_went_wrong() {
        let mut streak = Streak::new(2);
        let started = RefCell::new(Vec::new());
        units_run(
            &Ready::default(),
            &Clock(Cell::new(start())),
            Path::new("."),
            &Limits {
                max_units: None,
                deadline: None,
            },
            &mut streak,
            &mut |unit| {
                started.borrow_mut().push(unit);
                Err(anyhow::Error::new(crate::harness::UsageLimit {
                    resets: None,
                }))
            },
            &mut |_| true,
        )
        .unwrap();
        assert_eq!(started.into_inner(), [1]);
        assert!(streak.units().is_empty());
        assert!(streak.usage_limit().is_some());
    }

    fn moved(to: State, reason: Option<Reason>, at: u64) -> Option<Move> {
        Some(Move {
            from: Some(State::Build),
            to,
            at: Some(at),
            reason,
        })
    }

    #[test]
    fn only_a_new_move_into_stuck_for_a_wrong_state_counts_as_going_wrong() {
        let wrong = || moved(State::Stuck, Some(Reason::WentWrong), 2);
        assert!(went_wrong_since(None, wrong()));
        assert!(went_wrong_since(moved(State::Verify, None, 1), wrong()));
        assert!(!went_wrong_since(
            None,
            moved(State::Stuck, Some(Reason::Failed), 2)
        ));
        assert!(!went_wrong_since(None, moved(State::Done, None, 2)));
        assert!(!went_wrong_since(None, None));
        // The same move as before the run: an older night's, not this run's.
        assert!(!went_wrong_since(
            moved(State::Stuck, Some(Reason::WentWrong), 2),
            wrong()
        ));
    }

    #[test]
    fn units_that_went_wrong_in_a_row_stop_the_night_and_are_named() {
        let mut streak = Streak::new(2);
        let started = ended(&mut streak, [End::WentWrong, End::WentWrong, End::Finishes]);
        assert_eq!(started, [1, 2]);
        assert!(streak.reached());
        assert_eq!(streak.units(), [1, 2]);
    }

    #[test]
    fn a_unit_stuck_for_failing_checks_does_not_count_and_ends_the_streak() {
        let mut streak = Streak::new(2);
        let started = ended(
            &mut streak,
            [End::WentWrong, End::ChecksFail, End::WentWrong],
        );
        assert_eq!(started, [1, 2, 3]);
        assert!(!streak.reached());
    }

    #[test]
    fn a_unit_that_finishes_ends_the_streak() {
        let mut streak = Streak::new(2);
        let started = ended(&mut streak, [End::WentWrong, End::Finishes, End::WentWrong]);
        assert_eq!(started, [1, 2, 3]);
        assert!(!streak.reached());
    }

    #[test]
    fn without_a_limit_every_unit_runs_however_many_went_wrong() {
        let mut streak = Streak::new(0);
        let started = ended(&mut streak, [End::WentWrong; 3]);
        assert_eq!(started, [1, 2, 3]);
        assert!(!streak.reached());
    }

    #[test]
    fn failures_of_triage_and_of_runs_are_one_streak() {
        // Unit 9's triage session failed before the night's units ran.
        let mut streak = Streak::new(2);
        streak.record(9, true);
        let started = ended(&mut streak, [End::WentWrong, End::Finishes, End::Finishes]);
        assert_eq!(started, [1]);
        assert_eq!(streak.units(), [9, 1]);
    }

    #[test]
    fn a_streak_already_reached_starts_no_unit() {
        let mut streak = Streak::new(1);
        streak.record(9, true);
        assert!(ended(&mut streak, [End::Finishes; 3]).is_empty());
    }
}
