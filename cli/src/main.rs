//! `ns`: the Nightshift CLI.

mod args;
mod artifact;
mod ask;
mod billing;
#[cfg(test)]
#[path = "../build_stamp.rs"]
mod build_stamp;
mod clean;
mod clock;
mod config;
mod cpus;
mod decisions;
mod doctor;
mod error;
mod eval;
mod exe;
mod factory;
mod forge;
mod frontmatter;
mod gate;
mod git;
mod hook_guard;
mod install;
mod lint;
mod log;
mod markers;
mod memcap;
mod phase_output;
mod pin;
mod proc;
mod quality;
mod ratchet;
mod review_md;
mod rules;
mod run;
mod score;
mod scorer;
mod shell;
mod skills_sync;
mod stack;
mod stats;
mod status;
mod stop;
#[cfg(test)]
mod testutil;
mod touches;
mod transcripts;
mod upstream;
mod watch;
mod which;
mod worktree;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use args::{
    Artifact, ArtifactCmd, Ask, CheckMarkers, Clean, Cli, Cmd, Eval, Factory, FactoryCmd, Install,
    Lint, Log, Quality, QualityCmd, Ratchet, RatchetCmd, Rules, RulesCmd, Run, Score, Status,
    Transcripts, TranscriptsCmd, Upstream, UpstreamCmd, Watch, Worktree, WorktreeCmd,
};
use error::SfError;

fn format(human: bool) -> ratchet::Format {
    if human {
        ratchet::Format::Human
    } else {
        ratchet::Format::Json
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Cmd::Worktree(Worktree { command }) => match command {
            WorktreeCmd::New {
                unit_id,
                base,
                repo,
                no_setup,
            } => return worktree::new(&unit_id, base.as_deref(), repo.as_deref(), no_setup),
            WorktreeCmd::Setup { unit_id, repo } => {
                return worktree::setup(&unit_id, repo.as_deref())
            }
            WorktreeCmd::List { repo } => worktree::list(repo.as_deref())?,
            WorktreeCmd::Remove {
                unit_id,
                dry_run,
                force,
                repo,
            } => worktree::remove(&unit_id, dry_run, force, repo.as_deref())?,
        },
        Cmd::Ask(Ask {
            role,
            prompt_file,
            write,
            cwd,
            dry_run,
        }) => ask::run(ask::AskArgs {
            role: &role,
            prompt_file: prompt_file.as_deref(),
            write,
            cwd: cwd.as_deref(),
            dry_run,
        })?,
        Cmd::Lint(Lint { path, human }) => {
            let report = lint::lint_dir(&path)?;
            if human {
                for e in &report.errors {
                    println!("{}: {}: {}", e.file, e.skill, e.message);
                }
                println!(
                    "{} skills checked, {} errors",
                    report.skills,
                    report.errors.len()
                );
            } else {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            if !report.ok {
                return Ok(ExitCode::from(1));
            }
        }
        Cmd::CheckMarkers(CheckMarkers { path, human }) => return markers::cli(&path, human),
        Cmd::Artifact(Artifact { command }) => match command {
            ArtifactCmd::Check { file, phase } => return artifact::cli(&file, phase.as_deref()),
        },
        Cmd::Install(Install {
            source,
            harness,
            target,
            dry_run,
        }) => {
            let targets = install::targets(&harness, target);
            let source = source.unwrap_or_else(install::default_source);
            let report = install::run(&source, &targets, dry_run)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.conflicts > 0 {
                eprintln!(
                    "error: {} conflict(s): real files or unrelated symlinks are in the way; move them, then re-run:\n  ns install --dry-run",
                    report.conflicts
                );
                return Ok(ExitCode::from(1));
            }
        }
        Cmd::Eval(Eval {
            skills,
            cases,
            arms,
            compare,
            trials,
            changed_since,
            budget_usd,
            max_runs,
            triggers_only,
            cases_only,
            dry_run,
            no_cache,
            out,
            skills_dir,
            no_write_results,
            human,
            harness,
            model,
            trigger_timeout,
        }) => {
            return eval::run(eval::Args {
                skills,
                cases,
                arms,
                compare,
                trials,
                changed_since,
                budget_usd,
                max_runs,
                triggers_only,
                cases_only,
                dry_run,
                no_cache,
                out,
                skills_dir,
                no_write_results,
                human,
                harness,
                model,
                trigger_timeout_secs: trigger_timeout,
            })
        }
        Cmd::Factory(Factory { command }) => match command {
            FactoryCmd::Validate { factory } => return factory::validate(factory.as_deref()),
        },
        Cmd::Score(Score {
            unit_id,
            scorer,
            factory,
            dry_run,
        }) => return score::cli(&unit_id, scorer.as_deref(), factory.as_deref(), dry_run),
        Cmd::Run(Run {
            unit_id,
            issue,
            from,
            gates,
            base,
            dry_run,
            factory,
            night,
            cpus,
            fresh,
        }) => {
            return run::cli(run::RunArgs {
                unit: unit_id,
                issue,
                from,
                gates,
                dry_run,
                factory,
                base,
                triage_only: false,
                night,
                cpus,
                fresh,
            })
        }
        Cmd::Watch(Watch {
            once,
            until,
            max_units,
            parallel,
            dry_run,
            factory,
            no_self_update,
            resume_night,
        }) => {
            return watch::run(watch::WatchArgs {
                once,
                until,
                max_units,
                parallel,
                dry_run,
                factory,
                no_self_update,
                resume_night,
            })
        }
        Cmd::Quality(Quality {
            command: Some(QualityCmd::Import { dir, dry_run }),
            ..
        }) => return quality::import(&dir, dry_run),
        Cmd::Quality(Quality {
            command: None,
            since,
            until,
            json,
            bugs: true,
            weeks,
            ..
        }) => return quality::bugs::cli(since, until, weeks, json),
        Cmd::Quality(Quality {
            command: None,
            since,
            until,
            human,
            reviewers,
            json,
            ..
        }) => {
            let view = match (reviewers, human, json) {
                (true, _, false) => quality::View::ReviewerTable,
                (true, _, true) => quality::View::ReviewerJson,
                (false, true, _) => quality::View::Text,
                (false, false, _) => quality::View::Report,
            };
            return quality::cli(since, until, view);
        }
        Cmd::Clean(Clean { dry_run }) => return clean::cli(dry_run),
        Cmd::Log(Log {
            unit,
            since: _,
            follow,
            json,
            phase_output: true,
        }) => return phase_output::cli(unit.as_deref(), follow, json),
        Cmd::Log(Log {
            unit,
            since,
            follow,
            json,
            phase_output: false,
        }) => return log::cli(unit, since, follow, json),
        Cmd::Status(Status { since, json }) => return status::cli(since, json),
        Cmd::Ratchet(Ratchet { command }) => match command {
            RatchetCmd::Check { factory, human, .. } => {
                return ratchet::check(factory.as_deref(), format(human))
            }
            RatchetCmd::Update {
                factory,
                allow_loosen,
                dry_run,
                human,
                ..
            } => return ratchet::update(factory.as_deref(), allow_loosen, dry_run, format(human)),
        },
        Cmd::Rules(Rules { command }) => match command {
            RulesCmd::Check { human, only } => return rules::check(only.as_deref(), format(human)),
            RulesCmd::Baseline {
                allow_growth,
                dry_run,
                human,
            } => return rules::baseline(allow_growth, dry_run, format(human)),
        },
        Cmd::Transcripts(Transcripts { command }) => match command {
            TranscriptsCmd::List {
                since,
                project,
                new,
                mark,
            } => {
                return transcripts::list(&transcripts::ListArgs {
                    since,
                    project,
                    new,
                    mark,
                })
            }
            TranscriptsCmd::Read { id, compact } => return transcripts::read(&id, compact),
        },
        Cmd::Upstream(Upstream { command }) => match command {
            UpstreamCmd::Status { skills_dir, human } => {
                return upstream::status_cli(&skills_dir, human)
            }
            UpstreamCmd::Diff {
                skill,
                skills_dir,
                human,
            } => return upstream::diff_cli(&skills_dir, &skill, human),
        },
        Cmd::Doctor(_) => return doctor::run(),
        Cmd::GuardHooks(_) => hook_guard::run()?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Remove every git location variable from ns's own environment. Call before any thread starts,
/// so `remove_var` is sound.
fn clear_git_location_vars() {
    for v in git::LOCATION_VARS {
        std::env::remove_var(v);
    }
}

fn main() -> ExitCode {
    clear_git_location_vars();
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            if let Some(ns) = e.downcast_ref::<SfError>() {
                eprintln!("error: {}", ns.message);
                if let Some(h) = &ns.hint {
                    for line in h.lines() {
                        if line.is_empty() {
                            eprintln!();
                        } else {
                            eprintln!("  {line}");
                        }
                    }
                }
                ExitCode::from(ns.code as u8)
            } else {
                eprintln!("error: {e:#}");
                ExitCode::from(1)
            }
        }
    }
}

#[cfg(test)]
mod clear_git_location_tests {
    const MARK: &str = "NS_TEST_CLEAR_GIT_LOCATION_PROBE";

    #[test]
    fn clear_probe() {
        if std::env::var_os(MARK).is_none() {
            return;
        }
        let vars = crate::git::LOCATION_VARS;
        std::env::set_var(vars[0], "/decoy");
        std::env::set_var(vars[1], "");
        super::clear_git_location_vars();
        let left: Vec<_> = vars
            .iter()
            .filter(|v| std::env::var_os(v).is_some())
            .collect();
        assert!(left.is_empty(), "still set: {left:?}");
    }

    #[test]
    fn clear_git_location_vars_removes_set_and_empty_variables() {
        let out = crate::proc::Proc::tool(std::env::current_exe().unwrap())
            .args(["--exact", "clear_git_location_tests::clear_probe"])
            .args(["--test-threads=1", "--nocapture"])
            .env(MARK, "1")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{text}");
        assert!(text.contains("1 passed"), "{text}");
    }
}
