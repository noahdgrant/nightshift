//! `ns eval replay`: replay past units on other models, and show quality against cost.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use clap::Args as ClapArgs;
use serde_json::{json, Value};

use super::{case_name_check, case_names_list};
use crate::grades::{self, Entry};
use crate::harness::{usage_limit_of, Harness, Reply, Task, TimedOut};
use crate::runlog::RunLog;
use crate::states::{build, retrospective};
use crate::vcs::Vcs;
use crate::{harness, rubric, vcs};

#[derive(ClapArgs)]
pub struct ReplayArgs {
    /// The model to replay the cases on. Give it again to compare models.
    #[arg(
        long,
        required = true,
        num_args = 1..,
        value_parser = clap::builder::NonEmptyStringValueParser::new()
    )]
    model: Vec<String>,
    /// How many times to replay each case on each model.
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..))]
    runs: u32,
    /// Replay only this case.
    #[arg(long)]
    case: Option<String>,
    /// How long one session, and the checks, may take before they are killed and the replay
    /// fails.
    #[arg(long, default_value_t = 1800)]
    harness_timeout_seconds: u64,
    /// Start without asking.
    #[arg(long)]
    yes: bool,
    /// Refuse to start when the sessions it would run are more than this.
    #[arg(long)]
    max_runs: Option<u64>,
    /// Print one JSON line for each model instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs)]
pub struct AddReplayArgs {
    /// The unit to keep as a case, read from the forge.
    unit: u64,
    /// The case's name: lowercase letters, digits and hyphens.
    #[arg(long = "as")]
    case: String,
}

/// A past unit: what `build` is given, and the commit it started on.
struct Case {
    name: String,
    title: String,
    body: String,
    base: String,
    /// The unit's number, for the estimate; `None` for a case written by hand without it.
    unit: Option<u64>,
}

/// The cases in `dir`; only `only` when given. A case whose commit is not in the repository at
/// `root` is an error, so nothing runs on a case that cannot.
fn cases_load(vcs: &dyn Vcs, root: &Path, dir: &Path, only: Option<&str>) -> Result<Vec<Case>> {
    let mut cases = Vec::new();
    for name in case_names_list(dir, only)? {
        let read = |file: &str| -> Result<String> {
            let path = dir.join(&name).join(file);
            std::fs::read_to_string(&path)
                .with_context(|| format!("case {name}: cannot read {}", path.display()))
        };
        let unit = read("unit.md")?;
        let (title, body) = unit.split_once('\n').unwrap_or((&unit, ""));
        let base = vcs
            .commit_find(root, read("base")?.trim())
            .with_context(|| format!("case {name}: its base is not a commit in this repository"))?;
        let number = std::fs::read_to_string(dir.join(&name).join("unit"))
            .ok()
            .and_then(|t| t.trim().parse().ok());
        cases.push(Case {
            title: title.trim().to_string(),
            body: body.trim().to_string(),
            base,
            unit: number,
            name,
        });
    }
    Ok(cases)
}

pub fn add(args: AddReplayArgs) -> Result<()> {
    let vcs = vcs::get();
    let top = vcs.workspace_find(&std::env::current_dir()?)?;
    case_name_check(&args.case)?;
    let dir = top.join("evals/replay").join(&args.case);
    if dir.exists() {
        bail!("the case {} already exists in {}", args.case, dir.display());
    }
    let unit = args.unit.to_string();
    let lines = RunLog::sibling_open(&*vcs, &top, retrospective::GRADES_FILE)?.events()?;
    let Some(base) = grades::unit_base(&lines, &unit) else {
        bail!("unit {unit} has no grade line with a base commit in grades.jsonl");
    };
    let base = vcs
        .commit_find(&top, &base)
        .with_context(|| format!("the base {base} of unit {unit} is not a commit here"))?;
    let read = crate::forge::get()
        .unit_get(&top, args.unit)
        .with_context(|| format!("cannot read unit {}", args.unit))?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let files = [
        (
            "unit.md",
            format!("{}\n\n{}\n", read.title, read.body.trim()),
        ),
        ("base", format!("{base}\n")),
        ("unit", format!("{unit}\n")),
    ];
    for (file, text) in files {
        std::fs::write(dir.join(file), text)
            .with_context(|| format!("cannot write {}", dir.join(file).display()))?;
    }
    println!("{}", json!({ "case": args.case, "dir": dir }));
    Ok(())
}

/// What one replay came to: whether it passed, and the tokens its sessions used (input plus
/// output), `None` when no session said.
#[derive(Debug, PartialEq)]
struct Replayed {
    passed: bool,
    tokens: Option<u64>,
}

/// Removes the replay's workspace and its branch when dropped, also after a failure.
struct Cleanup<'a> {
    vcs: &'a dyn Vcs,
    root: &'a Path,
    path: std::path::PathBuf,
    branch: String,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.vcs.workspace_discard(self.root, &self.path) {
            eprintln!("ns: cannot remove {}: {e:#}", self.path.display());
        }
        if let Err(e) = self.vcs.branch_delete(self.root, &self.branch) {
            eprintln!("ns: cannot delete branch {}: {e:#}", self.branch);
        }
    }
}

struct Env<'a> {
    vcs: &'a dyn Vcs,
    harness: &'a dyn Harness,
    root: &'a Path,
    rubrics: &'a [rubric::Rubric],
    timeout: Duration,
}

/// Runs one session. `None` when it failed or timed out; a usage limit is an error, for a pass
/// rate that counts it as a miss would be a lie. Its tokens are added to `tokens` either way.
fn session(env: &Env, task: &Task, tokens: &mut Option<u64>) -> Result<Option<Reply>> {
    let count = |tokens: &mut Option<u64>, raws: &[&Vec<u8>]| {
        for raw in raws {
            let used = env.harness.transcript_usage(&String::from_utf8_lossy(raw));
            if let Some(t) = used.tokens {
                *tokens = Some(tokens.unwrap_or(0) + t.billable());
            }
        }
    };
    match env.harness.task_run(task) {
        Ok(reply) => {
            let all: Vec<_> = reply.earlier.iter().chain([&reply.transcript]).collect();
            count(tokens, &all);
            Ok(reply.success.then_some(reply))
        }
        Err(e) if usage_limit_of(&e).is_some() => Err(e),
        Err(e) => {
            if let Some(t) = e.downcast_ref::<TimedOut>() {
                let all: Vec<_> = t.earlier.iter().chain([&t.transcript]).collect();
                count(tokens, &all);
            }
            eprintln!("ns: a session failed: {e:#}");
            Ok(None)
        }
    }
}

static REPLAYS: AtomicU32 = AtomicU32::new(0);

fn replay_one(env: &Env, case: &Case, model: &str) -> Result<Replayed> {
    let mut tokens = None;
    let fail = |tokens| {
        Ok(Replayed {
            passed: false,
            tokens,
        })
    };
    let tmp = tempfile::tempdir().context("cannot create a temporary directory")?;
    let path = tmp.path().join("workspace");
    let branch = format!(
        "eval-replay/{}-{}",
        std::process::id(),
        REPLAYS.fetch_add(1, Ordering::Relaxed)
    );
    let made = env
        .vcs
        .workspace_create(env.root, &path, &branch, Some(&case.base));
    // Whatever the replay does from here, the workspace and its branch go.
    let _cleanup = Cleanup {
        vcs: env.vcs,
        root: env.root,
        path: path.clone(),
        branch,
    };
    if let Err(e) = made {
        eprintln!("ns: cannot make a workspace for {}: {e:#}", case.name);
        return fail(tokens);
    }
    let prompt = build::replay_prompt(&path, &case.title, &case.body, &case.base)?;
    let built = session(
        env,
        &Task {
            dir: path.clone(),
            prompt,
            write: true,
            model: Some(model.to_string()),
            timeout: env.timeout,
            resume: None,
        },
        &mut tokens,
    )?;
    let Some(built) = built else {
        return fail(tokens);
    };
    let checked = crate::checks::run(
        env.vcs,
        &*crate::os::get(),
        env.root,
        &path,
        false,
        env.timeout,
    );
    match checked {
        Ok(Some(report)) if !report.passed => return fail(tokens),
        Err(e) => {
            eprintln!("ns: the checks of {} could not run: {e:#}", case.name);
            return fail(tokens);
        }
        _ => {}
    }
    let diff = env.vcs.workspace_diff(&path, &case.base)?;
    let prompt = retrospective::prompt(
        env.rubrics,
        &case.title,
        &case.body,
        &built.text,
        &diff,
        &[],
        &[],
    );
    let graded = session(
        env,
        &Task {
            dir: path,
            prompt,
            write: false,
            model: Some(model.to_string()),
            timeout: env.timeout,
            resume: None,
        },
        &mut tokens,
    )?;
    let Some(graded) = graded else {
        return fail(tokens);
    };
    let (grades, missing) = rubric::grades_parse(&graded.text, env.rubrics);
    let passed = missing.is_empty()
        && grades.iter().all(|g| {
            env.rubrics
                .iter()
                .find(|r| r.name == g.rubric)
                .is_some_and(|r| g.score >= r.pass)
        });
    Ok(Replayed { passed, tokens })
}

/// One model's figures over all its replays.
#[derive(Debug, PartialEq)]
struct Summary {
    model: String,
    replays: usize,
    pass_rate: f64,
    lowest: f64,
    highest: f64,
    median_tokens: Option<u64>,
}

/// `runs` is one entry for each run: the replays of every case in it.
fn summarize(model: &str, runs: &[Vec<Replayed>]) -> Summary {
    let rate = |r: &[Replayed]| r.iter().filter(|x| x.passed).count() as f64 / r.len() as f64;
    let rates: Vec<f64> = runs.iter().map(|r| rate(r)).collect();
    let all: Vec<&Replayed> = runs.iter().flatten().collect();
    let mut tokens: Vec<u64> = all.iter().filter_map(|r| r.tokens).collect();
    tokens.sort_unstable();
    let median_tokens = match tokens.len() {
        0 => None,
        n if n % 2 == 1 => Some(tokens[n / 2]),
        n => Some((tokens[n / 2 - 1] + tokens[n / 2]) / 2),
    };
    Summary {
        model: model.to_string(),
        replays: all.len(),
        pass_rate: all.iter().filter(|r| r.passed).count() as f64 / all.len() as f64,
        lowest: rates.iter().copied().fold(f64::INFINITY, f64::min),
        highest: rates.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        median_tokens,
    }
}

impl Summary {
    fn to_json(&self) -> Value {
        json!({
            "model": self.model,
            "replays": self.replays,
            "pass_rate": self.pass_rate,
            "lowest": self.lowest,
            "highest": self.highest,
            "median_tokens": self.median_tokens,
        })
    }

    fn render(&self) -> String {
        let tokens = self
            .median_tokens
            .map_or("unknown".to_string(), |t| t.to_string());
        format!(
            "{}: {} replays, pass rate {:.0}% (lowest {:.0}%, highest {:.0}% over the runs), median tokens {}",
            self.model,
            self.replays,
            self.pass_rate * 100.0,
            self.lowest * 100.0,
            self.highest * 100.0,
            tokens
        )
    }
}

/// What the replays are expected to cost, from the tokens recorded for each case's unit: the
/// tokens of one replay of every case, and how many cases have none recorded.
fn estimate(cases: &[Case], entries: &[Entry]) -> (u64, usize) {
    let mut total = 0;
    let mut unknown = 0;
    for case in cases {
        match case
            .unit
            .and_then(|u| grades::unit_tokens(entries, &u.to_string()))
        {
            Some(t) => total += t,
            None => unknown += 1,
        }
    }
    (total, unknown)
}

/// Whether `answer`, a line the person typed, says yes.
fn yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Whether to start: `read` gives the answer line.
fn confirmed(read: &mut dyn BufRead) -> Result<bool> {
    eprint!("Start? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    read.read_line(&mut line)?;
    Ok(yes(&line))
}

/// Whether to start: always with `yes`; otherwise on a terminal the person's answer on `read`.
/// An error without a terminal, where no one can answer.
fn start_decided(yes: bool, terminal: bool, read: &mut dyn BufRead) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !terminal {
        bail!("there is no terminal to ask on; pass --yes to start");
    }
    confirmed(read)
}

pub fn run(args: ReplayArgs) -> Result<()> {
    let vcs = vcs::get();
    let cwd = std::env::current_dir()?;
    let top = vcs.workspace_find(&cwd)?;
    let root = vcs.root_find(&cwd)?;
    let cases = cases_load(
        &*vcs,
        &root,
        &top.join("evals/replay"),
        args.case.as_deref(),
    )?;
    if cases.is_empty() {
        bail!("there are no cases in evals/replay");
    }
    let (rubrics, invalid) = rubric::in_force(&top)?;
    for line in invalid {
        eprintln!("ns: {line}");
    }
    let replays = cases.len() as u64 * args.model.len() as u64 * u64::from(args.runs);
    let sessions = replays * 2;
    let events = RunLog::sibling_open(&*vcs, &top, retrospective::GRADES_FILE)?.events()?;
    let (tokens, unknown) = estimate(&cases, &grades::parse(&events));
    let estimated = tokens * args.model.len() as u64 * u64::from(args.runs);
    eprintln!(
        "{} cases on {} models, {} runs: {replays} replays, up to {sessions} sessions (a build and a grading session each).",
        cases.len(),
        args.model.len(),
        args.runs
    );
    eprintln!(
        "Estimated tokens: about {estimated} (input plus output, from what each case's unit used); {unknown} of {} cases have no recorded usage and are not counted.",
        cases.len()
    );
    if let Some(max) = args.max_runs {
        if sessions > max {
            bail!("it would run {sessions} sessions, more than --max-runs {max}");
        }
    }
    if !start_decided(
        args.yes,
        std::io::stdin().is_terminal(),
        &mut std::io::stdin().lock(),
    )? {
        eprintln!("ns: not started");
        return Ok(());
    }
    let harness = harness::get();
    let env = Env {
        vcs: &*vcs,
        harness: harness.as_ref(),
        root: &root,
        rubrics: &rubrics,
        timeout: Duration::from_secs(args.harness_timeout_seconds),
    };
    for model in &args.model {
        let mut runs = Vec::new();
        for _ in 0..args.runs {
            let mut run = Vec::new();
            for case in &cases {
                run.push(replay_one(&env, case, model)?);
            }
            runs.push(run);
        }
        let summary = summarize(model, &runs);
        if args.json {
            println!("{}", summary.to_json());
        } else {
            println!("{}", summary.render());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn y_and_yes_start_and_anything_else_does_not() {
        for (answer, started) in [
            ("y\n", true),
            ("YES\n", true),
            (" yes \n", true),
            ("n\n", false),
            ("\n", false),
            ("", false),
            ("yep\n", false),
        ] {
            assert_eq!(
                confirmed(&mut answer.as_bytes()).unwrap(),
                started,
                "{answer:?}"
            );
        }
    }

    #[test]
    fn declining_starts_no_session_and_yes_never_asks() {
        assert!(!start_decided(false, true, &mut "n\n".as_bytes()).unwrap());
        assert!(start_decided(false, true, &mut "y\n".as_bytes()).unwrap());
        assert!(start_decided(true, false, &mut "".as_bytes()).unwrap());
        assert!(start_decided(false, false, &mut "y\n".as_bytes()).is_err());
    }

    #[test]
    fn the_median_of_an_even_number_of_replays_is_the_mean_of_the_middle_two() {
        let replayed = |tokens| Replayed {
            passed: true,
            tokens,
        };
        let runs = [
            vec![replayed(Some(100)), replayed(None)],
            vec![replayed(Some(200))],
        ];
        assert_eq!(summarize("m", &runs).median_tokens, Some(150));
        assert_eq!(summarize("m", &[vec![replayed(None)]]).median_tokens, None);
    }
}
