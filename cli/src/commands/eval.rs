use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use clap::{Args as ClapArgs, Subcommand};
use serde_json::{json, Value};

use crate::harness::{Harness, Task};
use crate::skill;
use crate::states::review::{self, Focus};
use crate::states::verdict::{self, Verdict};
use crate::{harness, manifest, vcs};

/// Each run is a real harness call, so it uses subscription time. The tests use a fake `claude`.
#[derive(ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    skill: Skill,
}

#[derive(Subcommand)]
enum Skill {
    /// Run the review skill on the cases in evals/review and report whether it gives the right
    /// verdict each time. Every run is a real harness call and costs subscription time.
    Review(ReviewArgs),
}

#[derive(ClapArgs)]
struct ReviewArgs {
    /// How many times to run each case.
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..))]
    runs: u32,
    /// Run only this case.
    #[arg(long)]
    case: Option<String>,
    /// How long one run may take before it is killed and counts as a miss.
    #[arg(long, default_value_t = 1800)]
    harness_timeout_seconds: u64,
}

/// A fixture: what the review is given, and the verdict it should reach.
struct Case {
    name: String,
    title: String,
    body: String,
    build: String,
    diff: String,
    expect: Verdict,
}

pub fn run(args: Args) -> Result<()> {
    let Skill::Review(args) = args.skill;
    let root = vcs::get().workspace_find(&std::env::current_dir()?)?;
    let cases = cases_load(&root.join("evals/review"), args.case.as_deref())?;
    let skill = skill::load(skill::REVIEW)?;
    let focuses = review::focuses(manifest::review_sessions_read(&root)?);
    let harness = harness::get();
    let report = review_measure(
        harness.as_ref(),
        &root,
        &skill,
        focuses,
        &cases,
        args.runs,
        Duration::from_secs(args.harness_timeout_seconds),
    );
    println!("{}", report);
    if report["ok"] != true {
        std::process::exit(1);
    }
    Ok(())
}

fn name(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
        Verdict::Inconclusive => "INCONCLUSIVE",
        Verdict::Error => "ERROR",
    }
}

fn review_measure(
    harness: &dyn Harness,
    dir: &Path,
    skill: &str,
    focuses: &[Focus],
    cases: &[Case],
    runs: u32,
    timeout: Duration,
) -> Value {
    let mut ok = true;
    let mut reports = Vec::new();
    for case in cases {
        let prompts: Vec<_> = focuses
            .iter()
            .map(|focus| {
                review::prompt(
                    skill,
                    focus,
                    &case.title,
                    &case.body,
                    &case.build,
                    &case.diff,
                    None,
                )
            })
            .collect();
        let mut verdicts = Vec::new();
        for _ in 0..runs {
            // A run is a whole review: every session, and the verdict they come to together.
            let sessions: Vec<_> = prompts
                .iter()
                .map(|prompt| {
                    let reply = harness.task_run(&Task {
                        dir: dir.to_path_buf(),
                        prompt: prompt.clone(),
                        write: false,
                        model: None,
                        timeout,
                    });
                    match reply {
                        Ok(reply) if reply.success => verdict::parse(&reply.text),
                        Ok(_) => None,
                        Err(e) => {
                            eprintln!("ns: a run of {} failed: {e:#}", case.name);
                            None
                        }
                    }
                })
                .collect();
            verdicts.push(review::verdicts_merge(&sessions));
        }
        let matches = verdicts
            .iter()
            .filter(|v| v.as_ref() == Some(&case.expect))
            .count();
        ok &= matches == verdicts.len();
        reports.push(json!({
            "case": case.name,
            "expected": name(&case.expect),
            "verdicts": verdicts.iter().map(|v| v.as_ref().map(name)).collect::<Vec<_>>(),
            "matches": matches,
        }));
    }
    json!({ "skill": "review", "runs": runs, "ok": ok, "cases": reports })
}

/// The cases in `dir`, by name; only `only` when it is given.
fn cases_load(dir: &Path, only: Option<&str>) -> Result<Vec<Case>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        if entry.path().is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    if let Some(only) = only {
        if !names.iter().any(|n| n == only) {
            bail!("there is no case named {only} in {}", dir.display());
        }
        names.retain(|n| n == only);
    }
    names.iter().map(|n| case_load(dir, n)).collect()
}

fn case_load(dir: &Path, name: &str) -> Result<Case> {
    let read = |file: &str| -> Result<String> {
        let path = dir.join(name).join(file);
        std::fs::read_to_string(&path)
            .with_context(|| format!("case {name}: cannot read {}", path.display()))
    };
    let unit = read("unit.md")?;
    let (title, body) = unit.split_once('\n').unwrap_or((&unit, ""));
    let expect = match read("expect")?.trim() {
        "PASS" => Verdict::Pass,
        "FAIL" => Verdict::Fail,
        other => bail!("case {name}: expect must be PASS or FAIL, not {other:?}"),
    };
    Ok(Case {
        name: name.to_string(),
        title: title.trim().to_string(),
        body: body.trim().to_string(),
        build: read("build.md")?,
        diff: read("change.diff")?,
        expect,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_cases_are_well_formed() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../evals/review");
        let cases = cases_load(&dir, None).unwrap();
        let got: Vec<_> = cases
            .iter()
            .map(|c| (c.name.as_str(), name(&c.expect)))
            .collect();
        assert_eq!(
            got,
            [
                ("clean-change", "PASS"),
                ("misses-requirement", "FAIL"),
                ("needs-a-hand-edit", "PASS"),
                ("stale-comment", "FAIL"),
            ]
        );
        for case in &cases {
            assert!(!case.title.is_empty(), "{}: no title", case.name);
            assert!(!case.body.is_empty(), "{}: no description", case.name);
            assert!(!case.build.trim().is_empty(), "{}: no build", case.name);
            assert!(case.diff.contains("diff --git"), "{}: no diff", case.name);
        }
    }
}
