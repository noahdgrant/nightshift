use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use clap::{Args as ClapArgs, Subcommand};
use serde_json::{json, Value};

use crate::harness::{Harness, Sessions, Task};
use crate::manifest::Place;
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
    /// Add a case to the cases a skill is scored on.
    Add(AddArgs),
}

#[derive(ClapArgs)]
struct AddArgs {
    #[command(subcommand)]
    skill: AddSkill,
}

#[derive(Subcommand)]
enum AddSkill {
    /// Write the change a commit made, and the unit it was for, as a case in evals/review. Use
    /// it after a defect got past review, so the change that carried it is a case.
    Review(AddReviewArgs),
}

#[derive(ClapArgs)]
struct AddReviewArgs {
    /// The commit that carried the defect.
    commit: String,
    /// The unit the commit was made for, read from the forge.
    #[arg(long)]
    unit: u64,
    /// The case's name: lowercase letters, digits and hyphens.
    #[arg(long = "as")]
    case: String,
    /// The verdict the review should reach.
    #[arg(long, value_parser = expect_parse, default_value = "FAIL")]
    expect: Verdict,
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
    /// The model to run the cases with, instead of the one `[model]` in
    /// `.nightshift/nightshift.toml` names for review. Run again with another to compare models.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    model: Option<String>,
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
    let args = match args.skill {
        Skill::Review(args) => args,
        Skill::Add(add) => {
            let AddSkill::Review(add) = add.skill;
            return case_add(add);
        }
    };
    let root = vcs::get().workspace_find(&std::env::current_dir()?)?;
    let cases = cases_load(&root.join("evals/review"), args.case.as_deref())?;
    let skill = review::instructions(&skill::load(skill::REVIEW)?, &review::standards_read(&root));
    let focuses = review::focuses(manifest::review_sessions_read(&root)?);
    let model = manifest::model_read(&root, Place::Review, args.model.as_deref())?;
    let harness = harness::get();
    let report = review_measure(
        harness.as_ref(),
        &root,
        &skill,
        focuses,
        &cases,
        args.runs,
        &Sessions {
            timeout: Duration::from_secs(args.harness_timeout_seconds),
            model: model.as_deref(),
        },
    );
    println!("{}", report);
    if report["ok"] != true {
        std::process::exit(1);
    }
    Ok(())
}

fn case_add(args: AddReviewArgs) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.workspace_find(&std::env::current_dir()?)?;
    let case = &args.case;
    if case.is_empty()
        || !case
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        bail!("the case name {case:?} must be lowercase letters, digits and hyphens");
    }
    let dir = root.join("evals/review").join(case);
    if dir.exists() {
        bail!("the case {case} already exists in {}", dir.display());
    }
    let commit = vcs.commit_find(&root, &args.commit)?;
    let change = vcs.commit_show(&root, &commit)?;
    let unit = crate::forge::get()
        .unit_get(&root, args.unit)
        .with_context(|| format!("cannot read unit {}", args.unit))?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let files = [
        (
            "unit.md",
            format!("{}\n\n{}\n", unit.title, unit.body.trim()),
        ),
        ("change.diff", format!("{}\n", change.diff)),
        ("build.md", format!("{}\n", change.body)),
        ("expect", format!("{}\n", name(&args.expect))),
    ];
    for (file, text) in files {
        std::fs::write(dir.join(file), text)
            .with_context(|| format!("cannot write {}", dir.join(file).display()))?;
    }
    println!("{}", json!({ "case": case, "dir": dir }));
    Ok(())
}

/// The verdicts a case can expect: the two a review ends with.
fn expect_parse(text: &str) -> Result<Verdict, String> {
    match text {
        "PASS" => Ok(Verdict::Pass),
        "FAIL" => Ok(Verdict::Fail),
        other => Err(format!("expect must be PASS or FAIL, not {other:?}")),
    }
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
    limits: &Sessions,
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
                    review::Earlier::default(),
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
                        model: limits.model.map(str::to_string),
                        timeout: limits.timeout,
                        resume: None,
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
    let expect = expect_parse(read("expect")?.trim()).map_err(|e| anyhow!("case {name}: {e}"))?;
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
                ("no-idempotency-key", "FAIL"),
                ("no-symptom-for-cache", "FAIL"),
                ("points-at-generated-file", "FAIL"),
                ("repeats-a-mistake", "FAIL"),
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

    #[test]
    fn an_expect_file_as_add_writes_it_loads_back() {
        for verdict in [Verdict::Pass, Verdict::Fail] {
            let dir = tempfile::tempdir().unwrap();
            let case = dir.path().join("c");
            std::fs::create_dir(&case).unwrap();
            for file in ["unit.md", "build.md", "change.diff"] {
                std::fs::write(case.join(file), "x\n").unwrap();
            }
            std::fs::write(case.join("expect"), format!("{}\n", name(&verdict))).unwrap();
            assert_eq!(case_load(dir.path(), "c").unwrap().expect, verdict);
        }
    }
}
