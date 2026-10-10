//! `ns eval`: measure whether skills help. The spec is docs/EVALS.md.

mod checks;
pub(crate) mod parser;
mod spec;
pub(crate) mod trial;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::{self, EvalConfig, EvalHarness};
use crate::error::{SfError, EXIT_HARNESS_MISSING, EXIT_NOT_CONFIGURED};
use crate::stats::{median, round};
use crate::which::which;

use checks::CheckResult;
use parser::OutputParser;
use spec::{Case, Skill};

pub const DEFAULT_TRIGGER_TIMEOUT_SECS: u64 = 180;

pub struct Args {
    pub skills: Vec<String>,
    pub cases: Vec<String>,
    pub arms: Option<String>,
    pub compare: Option<String>,
    pub trials: Option<u32>,
    pub changed_since: Option<String>,
    pub budget_usd: Option<f64>,
    pub max_runs: Option<u32>,
    pub triggers_only: bool,
    pub cases_only: bool,
    pub dry_run: bool,
    pub no_cache: bool,
    pub out: Option<PathBuf>,
    pub skills_dir: PathBuf,
    pub no_write_results: bool,
    pub human: bool,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub trigger_timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Arm {
    With,
    Without,
    Old,
}

impl Arm {
    fn label(self) -> &'static str {
        match self {
            Arm::With => "with",
            Arm::Without => "without",
            Arm::Old => "old",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrialRecord {
    arm: String,
    n: u32,
    passed: bool,
    /// `pass`, `fail` or `timeout`.
    status: String,
    exit: Option<i32>,
    cost_usd: Option<f64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    wall_s: f64,
    num_turns: Option<u64>,
    checks: Vec<CheckResult>,
    transcript: Option<String>,
    #[serde(default)]
    cached: bool,
}

struct CasePlan {
    case: Case,
    fixture: Option<PathBuf>,
    env: Vec<(String, String)>,
    skip: Option<String>,
    cache_key: Option<String>,
    cached: Vec<TrialRecord>,
}

struct SkillPlan {
    skill: Skill,
    /// `None` when trigger evals run; otherwise why not.
    triggers_skip: Option<String>,
    cases: Vec<CasePlan>,
}

struct Budget {
    cost: f64,
    runs: u32,
    budget_usd: f64,
    max_runs: u32,
}

impl Budget {
    fn exhausted(&self) -> bool {
        self.cost >= self.budget_usd || self.runs >= self.max_runs
    }
}

/// Everything resolved before the first trial.
struct Env {
    cfg: EvalConfig,
    harness_name: String,
    harness: EvalHarness,
    argv: Vec<String>,
    parser: Option<Box<dyn OutputParser>>,
    skills_dir: PathBuf,
    root: PathBuf,
    fixtures_root: PathBuf,
    arms: Vec<Arm>,
    compare: Option<String>,
    /// Skills dir extracted at the `--compare` ref.
    old_skills: Option<(tempfile::TempDir, PathBuf)>,
    transcripts: PathBuf,
}

fn usage(msg: impl Into<String>, hint: &str) -> anyhow::Error {
    SfError::usage(msg, hint).into()
}

pub fn run(args: Args) -> Result<ExitCode> {
    let env = resolve(&args)?;
    let plans = plan(&args, &env)?;
    if args.dry_run {
        let out = dry_run_json(&args, &env, &plans);
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(ExitCode::SUCCESS);
    }
    execute(&args, &env, plans)
}

fn resolve(args: &Args) -> Result<Env> {
    if args.compare.is_some() && args.arms.is_some() {
        return Err(usage(
            "--arms and --compare cannot be combined",
            "ns eval ns-tdd --compare main\n  ns eval ns-tdd --arms with,without",
        ));
    }
    if args.triggers_only && args.cases_only {
        return Err(usage(
            "--triggers-only and --cases-only cannot be combined",
            "ns eval ns-tdd --triggers-only",
        ));
    }
    let path = config::path();
    let mut cfg = match config::load(&path) {
        Ok(c) => c.unwrap_or_default().eval,
        Err(e) => {
            return Err(SfError::new(EXIT_NOT_CONFIGURED, format!("{e:#}"))
                .hint("fix the file; check with:\n  ns doctor")
                .into())
        }
    };
    if let Some(h) = &args.harness {
        cfg.harness = h.clone();
    }
    if let Some(m) = &args.model {
        cfg.model = Some(m.clone());
    }
    if let Some(t) = args.trials {
        if t == 0 {
            return Err(usage(
                "--trials must be at least 1",
                "ns eval ns-tdd --trials 3",
            ));
        }
        cfg.trials = t;
        cfg.max_trials = cfg.max_trials.max(t);
    }
    if let Some(b) = args.budget_usd {
        cfg.budget_usd = b;
    }
    if let Some(m) = args.max_runs {
        cfg.max_runs = m;
    }
    let harness_name = cfg.harness.clone();
    let Some(harness) = cfg.harness_named(&harness_name) else {
        return Err(SfError::new(
            EXIT_NOT_CONFIGURED,
            format!("eval harness {harness_name:?} is not configured; the built-in is claude"),
        )
        .hint(format!(
            "add it to {}:\n\n[eval.harnesses.{harness_name}]\ncommand = [\"{harness_name}\", \"--model\", \"{{model}}\"]\noutput = \"none\"",
            path.display()
        ))
        .into());
    };
    if !config::EVAL_OUTPUTS.contains(&harness.output.as_str()) {
        return Err(SfError::new(
            EXIT_NOT_CONFIGURED,
            format!(
                "eval harness {harness_name:?} has output {:?}; supported: {}",
                harness.output,
                config::EVAL_OUTPUTS.join(", ")
            ),
        )
        .hint(format!(
            "fix [eval.harnesses.{harness_name}] in {}",
            path.display()
        ))
        .into());
    }
    let mut argv = config::expand(&harness.command, cfg.model.as_deref());
    if !["subscription", "api"].contains(&cfg.billing.as_str()) {
        return Err(SfError::new(
            EXIT_NOT_CONFIGURED,
            format!(
                "[eval] billing = {:?}: use \"subscription\" or \"api\"",
                cfg.billing
            ),
        )
        .into());
    }
    if cfg.billing == "subscription" && crate::billing::is_claude(&argv) {
        argv = crate::billing::strip_bare(argv);
        if !args.dry_run {
            crate::billing::check_login()?;
        }
    }
    if argv.is_empty() {
        return Err(SfError::new(
            EXIT_NOT_CONFIGURED,
            format!("eval harness {harness_name:?} has an empty command"),
        )
        .into());
    }
    let parser = parser::for_output(&harness.output);

    if !args.skills_dir.is_dir() {
        return Err(usage(
            format!("skills dir {} does not exist", args.skills_dir.display()),
            "run from the nightshift checkout, or pass --skills-dir:\n  ns eval --skills-dir ~/src/nightshift/skills --dry-run",
        ));
    }
    let skills_dir = args.skills_dir.canonicalize()?;
    let root = skills_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| skills_dir.clone());
    let fixtures_root = root.join("evals").join("fixtures");

    let arms = match (&args.compare, &args.arms) {
        (Some(_), _) => vec![Arm::With, Arm::Old],
        (None, None) => vec![Arm::With, Arm::Without],
        (None, Some(list)) => {
            let mut arms = Vec::new();
            for a in list.split(',').map(str::trim).filter(|a| !a.is_empty()) {
                let arm = match a {
                    "with" => Arm::With,
                    "without" => Arm::Without,
                    _ => {
                        return Err(usage(
                            format!("unknown arm {a:?}: use with, without, or --compare <ref>"),
                            "ns eval ns-tdd --arms with,without",
                        ))
                    }
                };
                if !arms.contains(&arm) {
                    arms.push(arm);
                }
            }
            if arms.is_empty() {
                return Err(usage("--arms is empty", "ns eval ns-tdd --arms with"));
            }
            arms
        }
    };

    let old_skills = match &args.compare {
        Some(r) => Some(extract_at_ref(&skills_dir, r)?),
        None => None,
    };

    Ok(Env {
        transcripts: config::expand_tilde(&cfg.transcripts),
        cfg,
        harness_name,
        harness,
        argv,
        parser,
        skills_dir,
        root,
        fixtures_root,
        arms,
        compare: args.compare.clone(),
        old_skills,
    })
}

/// Extract the skills dir as it was at `git_ref` into a temp dir; returns (guard, skills path).
fn extract_at_ref(skills_dir: &Path, git_ref: &str) -> Result<(tempfile::TempDir, PathBuf)> {
    let hint = "ns eval ns-tdd --compare main\n  ns eval ns-tdd --compare HEAD~3";
    if !crate::git::ok(
        skills_dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{git_ref}^{{commit}}"),
        ],
    ) {
        return Err(usage(
            format!("--compare {git_ref:?} does not name a commit"),
            hint,
        ));
    }
    let top = PathBuf::from(crate::git::run(
        skills_dir,
        &["rev-parse", "--show-toplevel"],
    )?);
    let top = top.canonicalize().unwrap_or(top);
    let rel = skills_dir
        .strip_prefix(&top)
        .map_err(|_| SfError::general("the skills dir is not inside its git checkout"))?
        .to_string_lossy()
        .into_owned();
    let tmp = tempfile::Builder::new().prefix("ns-eval-ref-").tempdir()?;
    let status = crate::git::scrub(&mut Command::new("sh"))
        .arg("-c")
        .arg("git -C \"$1\" archive --format=tar \"$2\" -- \"$3\" | tar -x -C \"$4\"")
        .arg("sh")
        .arg(&top)
        .arg(git_ref)
        .arg(if rel.is_empty() {
            ".".to_string()
        } else {
            rel.clone()
        })
        .arg(tmp.path())
        .status()
        .context("cannot run git archive")?;
    if !status.success() {
        return Err(SfError::general(format!(
            "cannot extract {rel} at {git_ref} with git archive"
        ))
        .into());
    }
    let path = tmp.path().join(&rel);
    Ok((tmp, path))
}

fn resolve_skill_name(skills_dir: &Path, name: &str) -> Option<String> {
    let name = name.trim_end_matches('/');
    let name = name.rsplit('/').next().unwrap_or(name);
    [name.to_string(), format!("ns-{name}")]
        .into_iter()
        .find(|n| skills_dir.join(n).join("SKILL.md").is_file())
}

fn changed_skills(skills_dir: &Path, git_ref: &str) -> Result<BTreeSet<String>> {
    let hint = "ns eval --changed-since origin/main --dry-run";
    let diff = crate::git::run(
        skills_dir,
        &["diff", "--name-only", "--relative", git_ref, "--", "."],
    )
    .map_err(|e| usage(format!("--changed-since {git_ref:?}: {e:#}"), hint))?;
    let untracked = crate::git::run(
        skills_dir,
        &["ls-files", "--others", "--exclude-standard", "--", "."],
    )?;
    Ok(diff
        .lines()
        .chain(untracked.lines())
        .filter_map(|p| p.split('/').next())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
}

/// Substitute `{capability.<name>.<key>}` and a leading `~/`.
fn expand_env_value(
    v: &str,
    caps: &BTreeMap<String, config::Capability>,
) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = v;
    while let Some(i) = rest.find("{capability.") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unclosed placeholder in {v:?}"))?;
        let key = &after[..end];
        let mut parts = key.splitn(3, '.');
        let (_, cap, field) = (parts.next(), parts.next(), parts.next());
        let val = cap
            .zip(field)
            .and_then(|(c, f)| caps.get(c).and_then(|m| m.keys.get(f)))
            .ok_or_else(|| {
                format!("{{{key}}} is not configured (add it under [eval.capability])")
            })?;
        out.push_str(&config::expand_tilde(val).to_string_lossy());
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(config::expand_tilde(&out).to_string_lossy().into_owned())
}

fn hash_dir(h: &mut Sha256, dir: &Path) -> Result<()> {
    fn walk(h: &mut Sha256, base: &Path, dir: &Path) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let rel = p
                .strip_prefix(base)
                .unwrap_or(&p)
                .to_string_lossy()
                .into_owned();
            let ft = e.file_type()?;
            if ft.is_symlink() {
                h.update(format!("L{rel}\0{}\0", fs::read_link(&p)?.display()).as_bytes());
            } else if ft.is_dir() {
                walk(h, base, &p)?;
            } else {
                let bytes = fs::read(&p)?;
                h.update(format!("F{rel}\0{}\0", bytes.len()).as_bytes());
                h.update(&bytes);
            }
        }
        Ok(())
    }
    walk(h, dir, dir)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Baseline cache key: (case content, fixture, harness, model, extra skills).
fn cache_key(env: &Env, case: &Case, fixture: &Path) -> Result<String> {
    let part = |f: &dyn Fn(&mut Sha256) -> Result<()>| -> Result<String> {
        let mut h = Sha256::new();
        f(&mut h)?;
        Ok(hex(&h.finalize()))
    };
    let case_hash = part(&|h| hash_dir(h, &case.dir))?;
    let fixture_hash = part(&|h| hash_dir(h, fixture))?;
    let mut extras = case.file.extra_skills.clone();
    extras.sort();
    let extras_hash = part(&|h| {
        for s in &extras {
            h.update(format!("S{s}\0").as_bytes());
            hash_dir(h, &env.skills_dir.join(s))?;
        }
        Ok(())
    })?;
    let mut h = Sha256::new();
    for p in [
        case_hash.as_str(),
        fixture_hash.as_str(),
        env.harness_name.as_str(),
        env.cfg.model.as_deref().unwrap_or(""),
        extras_hash.as_str(),
    ] {
        h.update(p.as_bytes());
        h.update(b"\0");
    }
    Ok(hex(&h.finalize())[..32].to_string())
}

fn cache_path(env: &Env, key: &str) -> PathBuf {
    env.transcripts.join("cache").join(format!("{key}.json"))
}

fn load_cache(env: &Env, key: &str) -> Vec<TrialRecord> {
    let Ok(text) = fs::read_to_string(cache_path(env, key)) else {
        return Vec::new();
    };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| serde_json::from_value::<Vec<TrialRecord>>(v["trials"].clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|mut t| {
            t.cached = true;
            t
        })
        .collect()
}

fn save_cache(env: &Env, key: &str, skill: &str, case: &str, trials: &[TrialRecord]) -> Result<()> {
    let p = cache_path(env, key);
    fs::create_dir_all(p.parent().unwrap())?;
    let stored: Vec<TrialRecord> = trials
        .iter()
        .cloned()
        .map(|mut t| {
            t.cached = false;
            t
        })
        .collect();
    let v = json!({"key": key, "skill": skill, "case": case, "harness": env.harness_name, "model": env.cfg.model, "trials": stored});
    fs::write(&p, serde_json::to_string_pretty(&v)?)
        .with_context(|| format!("cannot write {}", p.display()))
}

fn plan(args: &Args, env: &Env) -> Result<Vec<SkillPlan>> {
    let mut names: Vec<String> =
        if args.skills.is_empty() {
            spec::skills_with_evals(&env.skills_dir)?
        } else {
            let mut v = Vec::new();
            for s in &args.skills {
                match resolve_skill_name(&env.skills_dir, s) {
                    Some(n) => v.push(n),
                    None => return Err(usage(
                        format!("no skill {s:?} under {}", env.skills_dir.display()),
                        "ns eval ns-tdd --dry-run\n  ns eval --dry-run   (every skill with evals/)",
                    )),
                }
            }
            v
        };
    if let Some(r) = &args.changed_since {
        let changed = changed_skills(&env.skills_dir, r)?;
        names.retain(|n| changed.contains(n));
    }

    let mut plans = Vec::new();
    let mut matched_case = false;
    for name in names {
        let skill = spec::load_skill(&env.skills_dir, &name)?;
        let triggers_skip = if args.cases_only {
            Some("--cases-only".to_string())
        } else if !args.cases.is_empty() {
            Some("--case given".to_string())
        } else if skill.triggers.is_empty() {
            Some("no triggers.toml entries".to_string())
        } else if skill.user_invoked {
            Some("user-invoked skill (disable-model-invocation: true)".to_string())
        } else if env.parser.is_none() {
            Some(format!(
                "unsupported: harness output {:?} has no parser",
                env.harness.output
            ))
        } else {
            None
        };
        let mut cases = Vec::new();
        if !args.triggers_only {
            for case in skill.cases.clone() {
                if !args.cases.is_empty() && !args.cases.contains(&case.id) {
                    continue;
                }
                matched_case = true;
                cases.push(plan_case(args, env, &skill, case)?);
            }
        }
        plans.push(SkillPlan {
            skill,
            triggers_skip,
            cases,
        });
    }
    if !args.cases.is_empty() && !matched_case {
        return Err(usage(
            format!(
                "no case named {} in the selected skills",
                args.cases.join(", ")
            ),
            "ns eval ns-tdd --case reserve-off-by-one --dry-run",
        ));
    }
    Ok(plans)
}

fn plan_case(args: &Args, env: &Env, skill: &Skill, case: Case) -> Result<CasePlan> {
    let mut p = CasePlan {
        case,
        fixture: None,
        env: Vec::new(),
        skip: None,
        cache_key: None,
        cached: Vec::new(),
    };
    let fixture = match spec::fixture_dir(&env.fixtures_root, &p.case.file.fixture) {
        Ok(f) => f,
        Err(e) if e.contains("outside") => {
            return Err(SfError::general(format!(
                "refusing case {}/{}: {e}",
                skill.name, p.case.id
            ))
            .hint("a case's fixture must name a directory under evals/fixtures/, e.g. fixture = \"py-inventory\"")
            .into())
        }
        Err(e) => {
            p.skip = Some(e);
            return Ok(p);
        }
    };
    let fixture_file = spec::load_fixture(&fixture)?;
    let mut requires: Vec<String> = p.case.file.requires.clone();
    for r in &fixture_file.requires {
        if !requires.contains(r) {
            requires.push(r.clone());
        }
    }
    let missing: Vec<&String> = requires
        .iter()
        .filter(|r| !env.cfg.capability.contains_key(*r))
        .collect();
    if !missing.is_empty() {
        p.skip = Some(format!(
            "missing capability: {} (configure [eval.capability.<name>])",
            missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        return Ok(p);
    }
    for (k, v) in &fixture_file.env {
        match expand_env_value(v, &env.cfg.capability) {
            Ok(val) => p.env.push((k.clone(), val)),
            Err(e) => {
                p.skip = Some(format!("fixture env {k}: {e}"));
                return Ok(p);
            }
        }
    }
    let mut prepend: Vec<String> = Vec::new();
    for r in &requires {
        for d in &env.cfg.capability[r].path_prepend {
            let d = config::expand_tilde(d).to_string_lossy().into_owned();
            if !prepend.contains(&d) {
                prepend.push(d);
            }
        }
    }
    if !prepend.is_empty() {
        let base = p
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var("PATH").ok())
            .unwrap_or_default();
        p.env.retain(|(k, _)| k != "PATH");
        prepend.push(base);
        p.env.push(("PATH".into(), prepend.join(":")));
    }
    let mut needed = p.case.skills(&skill.name);
    needed.extend(p.case.file.extra_skills.iter().cloned());
    if let Some(m) = needed
        .iter()
        .find(|s| !env.skills_dir.join(s).join("SKILL.md").is_file())
    {
        p.skip = Some(format!(
            "skill {m:?} not found under {}",
            env.skills_dir.display()
        ));
        return Ok(p);
    }
    if env.arms.contains(&Arm::Without) {
        let key = cache_key(env, &p.case, &fixture)?;
        if !args.no_cache {
            p.cached = load_cache(env, &key);
        }
        p.cache_key = Some(key);
    }
    p.fixture = Some(fixture);
    Ok(p)
}

fn dry_run_json(args: &Args, env: &Env, plans: &[SkillPlan]) -> Value {
    let (trials, max_trials) = (env.cfg.trials, env.cfg.max_trials);
    let mut min_runs = 0u32;
    let mut max_runs = 0u32;
    let mut skipped = Vec::new();
    let mut skills = Vec::new();
    for sp in plans {
        let triggers_planned = if sp.triggers_skip.is_none() {
            sp.skill.triggers.len() as u32
        } else {
            0
        };
        min_runs += triggers_planned;
        max_runs += triggers_planned;
        if let Some(r) = &sp.triggers_skip {
            if !sp.skill.triggers.is_empty() && !args.cases_only && args.cases.is_empty() {
                skipped.push(json!({"skill": sp.skill.name, "triggers": sp.skill.triggers.len(), "reason": r}));
            }
        }
        let mut cases = Vec::new();
        for cp in &sp.cases {
            if let Some(r) = &cp.skip {
                skipped.push(json!({"skill": sp.skill.name, "case": cp.case.id, "reason": r}));
                cases.push(
                    json!({"case": cp.case.id, "fixture": cp.case.file.fixture, "skipped": r}),
                );
                continue;
            }
            let arms: Vec<Value> = env
                .arms
                .iter()
                .map(|a| {
                    let cached = if *a == Arm::Without {
                        cp.cached.len() as u32
                    } else {
                        0
                    };
                    min_runs += trials.saturating_sub(cached);
                    max_runs += max_trials.saturating_sub(cached);
                    json!({
                        "arm": a.label(),
                        "skills": arm_skill_names(env, &sp.skill, &cp.case, *a),
                        "trials_planned": trials,
                        "max_trials": max_trials,
                        "cached": cached,
                    })
                })
                .collect();
            cases.push(json!({
                "case": cp.case.id,
                "description": cp.case.file.description,
                "fixture": cp.case.file.fixture,
                "timeout_minutes": cp.case.timeout_secs() / 60,
                "checks": cp.case.file.checks.iter().map(|c| c.kind()).collect::<Vec<_>>(),
                "env": cp.env.iter().cloned().collect::<BTreeMap<_, _>>(),
                "cache_key": cp.cache_key,
                "arms": arms,
                "skipped": Value::Null,
            }));
        }
        skills.push(json!({
            "skill": sp.skill.name,
            "triggers": {"count": sp.skill.triggers.len(), "planned": triggers_planned, "skipped": sp.triggers_skip},
            "cases": cases,
        }));
    }
    json!({
        "dry_run": true,
        "harness": {
            "name": env.harness_name,
            "command": env.argv,
            "output": env.harness.output,
            "on_path": which(&env.argv[0]).is_some(),
        },
        "model": env.cfg.model,
        "arms": env.arms.iter().map(|a| a.label()).collect::<Vec<_>>(),
        "compare": env.compare,
        "trials": trials,
        "max_trials": max_trials,
        "budget_usd": env.cfg.budget_usd,
        "max_runs": env.cfg.max_runs,
        "skills_dir": env.skills_dir.to_string_lossy(),
        "fixtures_dir": env.fixtures_root.to_string_lossy(),
        "transcripts": env.transcripts.to_string_lossy(),
        "skills": skills,
        "skipped": skipped,
        "estimated_runs": {
            "min": min_runs.min(env.cfg.max_runs),
            "max": max_runs.min(env.cfg.max_runs),
            "uncapped_max": max_runs,
        },
    })
}

/// Skill directory names installed for `arm`.
fn arm_skill_names(env: &Env, skill: &Skill, case: &Case, arm: Arm) -> Vec<String> {
    let mut v = match arm {
        Arm::With => case.skills(&skill.name),
        Arm::Without => Vec::new(),
        Arm::Old => case
            .skills(&skill.name)
            .into_iter()
            .filter(|s| {
                env.old_skills
                    .as_ref()
                    .is_some_and(|(_, d)| d.join(s).join("SKILL.md").is_file())
            })
            .map(|s| format!("{s}@{}", env.compare.as_deref().unwrap_or("")))
            .collect(),
    };
    v.extend(case.file.extra_skills.iter().cloned());
    v
}

fn arm_skill_dirs(env: &Env, skill: &Skill, case: &Case, arm: Arm) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match arm {
        Arm::With => case
            .skills(&skill.name)
            .iter()
            .map(|s| env.skills_dir.join(s))
            .collect(),
        Arm::Without => Vec::new(),
        Arm::Old => match &env.old_skills {
            Some((_, d)) => case
                .skills(&skill.name)
                .iter()
                .map(|s| d.join(s))
                .filter(|p| p.join("SKILL.md").is_file())
                .collect(),
            None => Vec::new(),
        },
    };
    v.extend(
        case.file
            .extra_skills
            .iter()
            .map(|s| env.skills_dir.join(s)),
    );
    v
}

struct CaseOutcome {
    id: String,
    fixture: String,
    skip: Option<String>,
    error: Option<String>,
    trials: BTreeMap<Arm, Vec<TrialRecord>>,
    skipped_budget: u32,
}

struct TriggerOutcome {
    prompt: String,
    expect: bool,
    /// `None` when not run (budget).
    loaded: Option<bool>,
    transcript: Option<String>,
}

struct SkillOutcome {
    name: String,
    dir: PathBuf,
    cases: Vec<CaseOutcome>,
    triggers: Vec<TriggerOutcome>,
    triggers_skip: Option<String>,
}

fn execute(args: &Args, env: &Env, plans: Vec<SkillPlan>) -> Result<ExitCode> {
    let any_runs = plans
        .iter()
        .any(|p| p.triggers_skip.is_none() || p.cases.iter().any(|c| c.skip.is_none()));
    if any_runs && which(&env.argv[0]).is_none() {
        return Err(SfError::new(
            EXIT_HARNESS_MISSING,
            format!(
                "eval harness {:?} needs `{}`, which is not on PATH",
                env.harness_name, env.argv[0]
            ),
        )
        .hint("install it, or set [eval] harness; preview the plan with:\n  ns eval --dry-run")
        .into());
    }
    let run_id = time::run_id();
    let run_dir = env.transcripts.join("runs").join(&run_id);
    fs::create_dir_all(&run_dir).with_context(|| format!("cannot create {}", run_dir.display()))?;
    let ns_exe = std::env::current_exe().context("cannot find the ns binary")?;
    let mut budget = Budget {
        cost: 0.0,
        runs: 0,
        budget_usd: env.cfg.budget_usd,
        max_runs: env.cfg.max_runs,
    };

    let mut outcomes = Vec::new();
    for sp in plans {
        let mut so = SkillOutcome {
            name: sp.skill.name.clone(),
            dir: sp.skill.dir.clone(),
            cases: Vec::new(),
            triggers: Vec::new(),
            triggers_skip: sp.triggers_skip.clone(),
        };
        if sp.triggers_skip.is_none() {
            for (i, t) in sp.skill.triggers.iter().enumerate() {
                let mut to = TriggerOutcome {
                    prompt: t.prompt.clone(),
                    expect: t.expect,
                    loaded: None,
                    transcript: None,
                };
                if !budget.exhausted() {
                    eprintln!(
                        "ns eval: {} trigger {}/{}",
                        sp.skill.name,
                        i + 1,
                        sp.skill.triggers.len()
                    );
                    let dir = run_dir.join(&sp.skill.name).join("triggers");
                    fs::create_dir_all(&dir)?;
                    let out = dir.join(format!("{i}.jsonl"));
                    let scratch = trial::prepare_empty(
                        std::slice::from_ref(&sp.skill.dir),
                        &env.harness.carry,
                    )?;
                    trial::run_harness(
                        &env.argv,
                        &t.prompt,
                        &scratch,
                        Duration::from_secs(args.trigger_timeout_secs),
                        &out,
                        &dir.join(format!("{i}.stderr")),
                        env.cfg.billing == "subscription",
                    )?;
                    let parsed = env
                        .parser
                        .as_ref()
                        .expect("triggers run only with a parser")
                        .parse(&fs::read_to_string(&out).unwrap_or_default());
                    budget.runs += 1;
                    budget.cost += parsed.cost_usd.unwrap_or(0.0);
                    to.loaded = Some(parsed.loaded(&sp.skill.name));
                    to.transcript = Some(out.to_string_lossy().into_owned());
                }
                so.triggers.push(to);
            }
        }
        for cp in sp.cases {
            so.cases.push(run_case(
                env,
                &sp.skill,
                cp,
                &run_dir,
                &ns_exe,
                &mut budget,
            )?);
        }
        outcomes.push(so);
    }

    let commit = short_sha(&env.root);
    let date = time::date();
    let mut results_files = Vec::new();
    let mut skills_json = Vec::new();
    let mut skipped = Vec::new();
    for so in &outcomes {
        let summary = skill_summary(env, so, false);
        let ran = so.triggers.iter().any(|t| t.loaded.is_some())
            || so
                .cases
                .iter()
                .any(|c| c.trials.values().any(|v| !v.is_empty()));
        if ran && !args.no_write_results {
            let dir = so.dir.join("evals").join("results");
            fs::create_dir_all(&dir)?;
            let p = dir.join(format!("{date}-{commit}.json"));
            let mut doc = json!({
                "skill": so.name,
                "date": date,
                "commit": commit,
                "dirty": dirty(&env.root, &so.dir),
                "config": config_json(env),
            });
            merge(&mut doc, &summary);
            fs::write(&p, serde_json::to_string_pretty(&doc)? + "\n")
                .with_context(|| format!("cannot write {}", p.display()))?;
            results_files.push(p.to_string_lossy().into_owned());
        }
        if let Some(r) = &so.triggers_skip {
            if !args.cases_only && args.cases.is_empty() && r != "no triggers.toml entries" {
                skipped.push(json!({"skill": so.name, "triggers": true, "reason": r}));
            }
        }
        let budget_triggers = so.triggers.iter().filter(|t| t.loaded.is_none()).count();
        if budget_triggers > 0 {
            skipped
                .push(json!({"skill": so.name, "triggers": budget_triggers, "reason": "budget"}));
        }
        for c in &so.cases {
            if let Some(r) = c.skip.as_ref().or(c.error.as_ref()) {
                skipped.push(json!({"skill": so.name, "case": c.id, "reason": r}));
            } else if c.skipped_budget > 0 {
                skipped.push(json!({"skill": so.name, "case": c.id, "trials": c.skipped_budget, "reason": "budget"}));
            }
        }
        skills_json.push(skill_summary(env, so, true));
    }

    let out = json!({
        "ok": true,
        "run_id": run_id,
        "harness": env.harness_name,
        "model": env.cfg.model,
        "arms": env.arms.iter().map(|a| a.label()).collect::<Vec<_>>(),
        "compare": env.compare,
        "runs": budget.runs,
        "cost_usd": round(budget.cost, 4),
        "budget": {"budget_usd": budget.budget_usd, "max_runs": budget.max_runs, "exhausted": budget.exhausted()},
        "transcripts": run_dir.to_string_lossy(),
        "skills": skills_json,
        "skipped": skipped,
        "results_files": results_files,
    });
    let text = serde_json::to_string_pretty(&out)?;
    if let Some(p) = &args.out {
        fs::write(p, format!("{text}\n"))
            .with_context(|| format!("cannot write {}", p.display()))?;
    }
    println!("{text}");
    if args.human {
        print_table(&outcomes, env);
    }
    Ok(ExitCode::SUCCESS)
}

fn merge(doc: &mut Value, extra: &Value) {
    if let (Some(d), Some(e)) = (doc.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            if k != "skill" {
                d.insert(k.clone(), v.clone());
            }
        }
    }
}

fn config_json(env: &Env) -> Value {
    json!({
        "harness": env.harness_name,
        "model": env.cfg.model,
        "arms": env.arms.iter().map(|a| a.label()).collect::<Vec<_>>(),
        "compare": env.compare,
        "trials": env.cfg.trials,
        "max_trials": env.cfg.max_trials,
        "budget_usd": env.cfg.budget_usd,
        "max_runs": env.cfg.max_runs,
    })
}

/// More trials help only while some arm's outcomes are mixed.
fn needs_more(trials: &BTreeMap<Arm, Vec<TrialRecord>>) -> bool {
    trials
        .values()
        .any(|v| v.iter().any(|t| t.passed) && v.iter().any(|t| !t.passed))
}

fn run_case(
    env: &Env,
    skill: &Skill,
    cp: CasePlan,
    run_dir: &Path,
    ns_exe: &Path,
    budget: &mut Budget,
) -> Result<CaseOutcome> {
    let mut out = CaseOutcome {
        id: cp.case.id.clone(),
        fixture: cp.case.file.fixture.clone(),
        skip: cp.skip.clone(),
        error: None,
        trials: env.arms.iter().map(|a| (*a, Vec::new())).collect(),
        skipped_budget: 0,
    };
    let Some(fixture) = cp.fixture.clone() else {
        return Ok(out);
    };
    if env.arms.contains(&Arm::Without) {
        out.trials.insert(Arm::Without, cp.cached.clone());
    }
    let dir = run_dir.join(&skill.name).join(&cp.case.id);
    fs::create_dir_all(&dir)?;
    let mut target = env.cfg.trials;
    'outer: loop {
        for n in 0..target {
            for arm in env.arms.clone() {
                if out.trials[&arm].len() as u32 > n {
                    continue;
                }
                if budget.exhausted() {
                    out.skipped_budget = env
                        .arms
                        .iter()
                        .map(|a| target.saturating_sub(out.trials[a].len() as u32))
                        .sum();
                    break 'outer;
                }
                eprintln!(
                    "ns eval: {}/{} {} trial {}",
                    skill.name,
                    cp.case.id,
                    arm.label(),
                    n + 1
                );
                let rec = match run_trial(env, skill, &cp, &fixture, arm, n + 1, &dir, ns_exe) {
                    Ok(r) => r,
                    Err(e) if e.downcast_ref::<SfError>().is_some() => return Err(e),
                    Err(e) => {
                        out.error = Some(format!("{e:#}"));
                        break 'outer;
                    }
                };
                budget.runs += 1;
                budget.cost += rec.cost_usd.unwrap_or(0.0);
                out.trials.get_mut(&arm).unwrap().push(rec);
                if arm == Arm::Without {
                    if let Some(k) = &cp.cache_key {
                        save_cache(env, k, &skill.name, &cp.case.id, &out.trials[&Arm::Without])?;
                    }
                }
            }
        }
        if target >= env.cfg.max_trials || !needs_more(&out.trials) {
            break;
        }
        target += 1;
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn run_trial(
    env: &Env,
    skill: &Skill,
    cp: &CasePlan,
    fixture: &Path,
    arm: Arm,
    n: u32,
    dir: &Path,
    ns_exe: &Path,
) -> Result<TrialRecord> {
    let skills = arm_skill_dirs(env, skill, &cp.case, arm);
    let scratch = trial::prepare_case(
        fixture,
        &cp.case,
        &skills,
        &env.harness.carry,
        cp.env.clone(),
    )?;
    let stdout_path = dir.join(format!("{}-{n}.jsonl", arm.label()));
    let r = trial::run_harness(
        &env.argv,
        &cp.case.file.prompt,
        &scratch,
        Duration::from_secs(cp.case.timeout_secs()),
        &stdout_path,
        &dir.join(format!("{}-{n}.stderr", arm.label())),
        env.cfg.billing == "subscription",
    )?;
    let parsed = env
        .parser
        .as_ref()
        .map(|p| p.parse(&fs::read_to_string(&stdout_path).unwrap_or_default()))
        .unwrap_or_default();
    let checks = checks::run_all(
        &cp.case.file.checks,
        &checks::Ctx {
            scratch: &scratch,
            prompt: &cp.case.file.prompt,
            ns_exe,
        },
    )?;
    let passed = !r.timed_out && checks.iter().filter(|c| c.required).all(|c| c.passed);
    Ok(TrialRecord {
        arm: arm.label().into(),
        n,
        passed,
        status: if r.timed_out {
            "timeout"
        } else if passed {
            "pass"
        } else {
            "fail"
        }
        .into(),
        exit: r.exit,
        cost_usd: parsed.cost_usd,
        input_tokens: parsed.input_tokens,
        output_tokens: parsed.output_tokens,
        wall_s: round(r.wall_s, 2),
        num_turns: parsed.num_turns,
        checks,
        transcript: Some(stdout_path.to_string_lossy().into_owned()),
        cached: false,
    })
}

fn arm_metrics(trials: &[TrialRecord]) -> Value {
    let pick =
        |f: &dyn Fn(&TrialRecord) -> Option<f64>| median(trials.iter().filter_map(f).collect());
    let passed = trials.iter().filter(|t| t.passed).count();
    json!({
        "trials": trials.len(),
        "passed": passed,
        "pass_rate": if trials.is_empty() { None } else { Some(round(passed as f64 / trials.len() as f64, 3)) },
        "cached": trials.iter().filter(|t| t.cached).count(),
        "median_input_tokens": pick(&|t| t.input_tokens.map(|x| x as f64)),
        "median_output_tokens": pick(&|t| t.output_tokens.map(|x| x as f64)),
        "median_cost_usd": pick(&|t| t.cost_usd).map(|c| round(c, 4)),
        "median_wall_s": pick(&|t| Some(t.wall_s)).map(|w| round(w, 2)),
        "median_turns": pick(&|t| t.num_turns.map(|x| x as f64)),
        "outcomes": trials.iter().map(|t| t.passed).collect::<Vec<_>>(),
    })
}

fn pass_rate(trials: &[TrialRecord]) -> Option<f64> {
    (!trials.is_empty())
        .then(|| trials.iter().filter(|t| t.passed).count() as f64 / trials.len() as f64)
}

fn skill_summary(env: &Env, so: &SkillOutcome, with_trials: bool) -> Value {
    let baseline = if env.compare.is_some() {
        Arm::Old
    } else {
        Arm::Without
    };
    let mut uplifts = Vec::new();
    let mut pooled: BTreeMap<Arm, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    let mut cases = Vec::new();
    for c in &so.cases {
        let status = if c.skip.is_some() {
            "skipped"
        } else if c.error.is_some() {
            "error"
        } else if c.trials.values().all(|v| v.is_empty()) {
            "skipped"
        } else {
            "ran"
        };
        let mut arms = serde_json::Map::new();
        for (arm, trials) in &c.trials {
            let mut m = arm_metrics(trials);
            if with_trials {
                m["trial_results"] = json!(trials);
            }
            arms.insert(arm.label().into(), m);
            let e = pooled.entry(*arm).or_default();
            for t in trials {
                if let (Some(i), Some(o)) = (t.input_tokens, t.output_tokens) {
                    e.0.push((i + o) as f64);
                }
                e.1.push(t.wall_s);
            }
        }
        if let (Some(w), Some(b)) = (
            c.trials.get(&Arm::With).and_then(|t| pass_rate(t)),
            c.trials.get(&baseline).and_then(|t| pass_rate(t)),
        ) {
            uplifts.push(w - b);
        }
        cases.push(json!({
            "case": c.id,
            "fixture": c.fixture,
            "status": status,
            "reason": c.skip.clone().or(c.error.clone()).or((c.skipped_budget > 0).then(|| "budget".to_string())),
            "skipped_trials": c.skipped_budget,
            "arms": arms,
        }));
    }
    let uplift =
        (!uplifts.is_empty()).then(|| round(uplifts.iter().sum::<f64>() / uplifts.len() as f64, 3));
    let med = |arm: Arm, tokens: bool| {
        pooled
            .get(&arm)
            .and_then(|(t, w)| median(if tokens { t.clone() } else { w.clone() }))
    };
    let ratio = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) if b > 0.0 => Some(round(a / b, 3)),
        _ => None,
    };
    let efficiency = json!({
        "with": {"median_tokens": med(Arm::With, true), "median_wall_s": med(Arm::With, false)},
        baseline.label(): {"median_tokens": med(baseline, true), "median_wall_s": med(baseline, false)},
        "tokens_ratio": ratio(med(Arm::With, true), med(baseline, true)),
        "time_ratio": ratio(med(Arm::With, false), med(baseline, false)),
    });
    json!({
        "skill": so.name,
        "cases": cases,
        "metrics": {
            "uplift": uplift,
            "baseline": baseline.label(),
            "efficiency": efficiency,
            "triggers": trigger_metrics(so),
        },
        "triggers": so.triggers.iter().map(|t| {
            let mut v = json!({"prompt": t.prompt, "expect": t.expect, "loaded": t.loaded, "status": match t.loaded { None => "skipped: budget", Some(l) if l == t.expect => "ok", Some(_) => "wrong" }});
            if with_trials {
                v["transcript"] = json!(t.transcript);
            }
            v
        }).collect::<Vec<_>>(),
    })
}

fn trigger_metrics(so: &SkillOutcome) -> Value {
    if let Some(r) = &so.triggers_skip {
        let status = if r.starts_with("unsupported") {
            "unsupported"
        } else {
            "skipped"
        };
        return json!({"status": status, "reason": r});
    }
    let ran: Vec<&TriggerOutcome> = so.triggers.iter().filter(|t| t.loaded.is_some()).collect();
    let count = |e: bool, l: bool| {
        ran.iter()
            .filter(|t| t.expect == e && t.loaded == Some(l))
            .count()
    };
    let (tp, fp, tn, fneg) = (
        count(true, true),
        count(false, true),
        count(false, false),
        count(true, false),
    );
    let div = |a: usize, b: usize| (b > 0).then(|| round(a as f64 / b as f64, 3));
    json!({
        "status": "ok",
        "total": so.triggers.len(),
        "run": ran.len(),
        "tp": tp, "fp": fp, "tn": tn, "fn": fneg,
        "precision": div(tp, tp + fp),
        "recall": div(tp, tp + fneg),
    })
}

fn print_table(outcomes: &[SkillOutcome], env: &Env) {
    let baseline = if env.compare.is_some() {
        Arm::Old
    } else {
        Arm::Without
    };
    eprintln!(
        "{:<28} {:<24} {:<8} {:>6} {:>6} {:>10} {:>9} {:>8}",
        "skill", "case", "arm", "trials", "pass", "tokens", "cost", "wall_s"
    );
    for so in outcomes {
        for c in &so.cases {
            if let Some(r) = c.skip.as_ref().or(c.error.as_ref()) {
                eprintln!("{:<28} {:<24} skipped: {r}", so.name, c.id);
                continue;
            }
            for (arm, trials) in &c.trials {
                let m = arm_metrics(trials);
                let tokens = match (
                    m["median_input_tokens"].as_f64(),
                    m["median_output_tokens"].as_f64(),
                ) {
                    (Some(i), Some(o)) => format!("{:.0}", i + o),
                    _ => "-".into(),
                };
                eprintln!(
                    "{:<28} {:<24} {:<8} {:>6} {:>6} {:>10} {:>9} {:>8}",
                    so.name,
                    c.id,
                    arm.label(),
                    trials.len(),
                    m["pass_rate"]
                        .as_f64()
                        .map(|p| format!("{p:.2}"))
                        .unwrap_or("-".into()),
                    tokens,
                    m["median_cost_usd"]
                        .as_f64()
                        .map(|p| format!("{p:.4}"))
                        .unwrap_or("-".into()),
                    m["median_wall_s"]
                        .as_f64()
                        .map(|p| format!("{p:.1}"))
                        .unwrap_or("-".into()),
                );
            }
        }
        let s = skill_summary(env, so, false);
        let t = &s["metrics"]["triggers"];
        let fmt = |v: &Value| v.as_f64().map(|x| format!("{x:+.2}")).unwrap_or("-".into());
        let triggers = if t["status"] == "ok" {
            format!(
                "precision {} recall {}",
                t["precision"]
                    .as_f64()
                    .map(|x| format!("{x:.2}"))
                    .unwrap_or("-".into()),
                t["recall"]
                    .as_f64()
                    .map(|x| format!("{x:.2}"))
                    .unwrap_or("-".into())
            )
        } else {
            t["status"].as_str().unwrap_or("-").to_string()
        };
        eprintln!(
            "{}: uplift {} vs {}, tokens x{}, time x{}, triggers {}",
            so.name,
            fmt(&s["metrics"]["uplift"]),
            baseline.label(),
            s["metrics"]["efficiency"]["tokens_ratio"]
                .as_f64()
                .map(|x| format!("{x:.2}"))
                .unwrap_or("-".into()),
            s["metrics"]["efficiency"]["time_ratio"]
                .as_f64()
                .map(|x| format!("{x:.2}"))
                .unwrap_or("-".into()),
            triggers
        );
    }
}

fn short_sha(root: &Path) -> String {
    crate::git::run(root, &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|_| "nogit".into())
}

fn dirty(root: &Path, dir: &Path) -> bool {
    crate::git::run(
        root,
        &["status", "--porcelain", "--", &dir.to_string_lossy()],
    )
    .map(|s| s.lines().any(|l| !l.contains("/evals/results/")))
    .unwrap_or(false)
}

mod time {
    fn now() -> (i64, u32, u32, u64) {
        let secs = crate::clock::Clock::from_env().now().max(0) as u64;
        let days = (secs / 86_400) as i64;
        let (y, m, d) = civil(days);
        (y, m, d, secs % 86_400)
    }

    /// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
    fn civil(z: i64) -> (i64, u32, u32) {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (yoe + era * 400 + i64::from(m <= 2), m, d)
    }

    pub fn date() -> String {
        let (y, m, d, _) = now();
        format!("{y:04}-{m:02}-{d:02}")
    }

    pub fn run_id() -> String {
        let (y, m, d, s) = now();
        format!(
            "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z-{}",
            s / 3600,
            s / 60 % 60,
            s % 60,
            std::process::id()
        )
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn civil_dates() {
            assert_eq!(super::civil(0), (1970, 1, 1));
            assert_eq!(super::civil(20_734), (2026, 10, 8));
            assert_eq!(super::civil(11_016), (2000, 2, 29));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(passed: bool) -> TrialRecord {
        TrialRecord {
            arm: "with".into(),
            n: 1,
            passed,
            status: String::new(),
            exit: Some(0),
            cost_usd: Some(0.1),
            input_tokens: Some(10),
            output_tokens: Some(5),
            wall_s: 1.0,
            num_turns: Some(2),
            checks: Vec::new(),
            transcript: None,
            cached: false,
        }
    }

    #[test]
    fn adaptive_stops_when_every_arm_is_unanimous() {
        let mut t = BTreeMap::new();
        t.insert(Arm::With, vec![rec(true), rec(true)]);
        t.insert(Arm::Without, vec![rec(false), rec(false)]);
        assert!(!needs_more(&t));
        t.insert(Arm::Without, vec![rec(true), rec(true)]);
        assert!(!needs_more(&t));
        t.insert(Arm::Without, vec![rec(true), rec(false)]);
        assert!(needs_more(&t));
    }

    #[test]
    fn capability_placeholders() {
        let mut caps = BTreeMap::new();
        caps.insert(
            "zephyr".to_string(),
            config::Capability {
                path_prepend: Vec::new(),
                keys: BTreeMap::from([("base".to_string(), "/opt/z".to_string())]),
            },
        );
        assert_eq!(
            expand_env_value("{capability.zephyr.base}/x", &caps).unwrap(),
            "/opt/z/x"
        );
        assert_eq!(expand_env_value("plain", &caps).unwrap(), "plain");
        assert!(expand_env_value("{capability.zephyr.sdk}", &caps).is_err());
        assert!(expand_env_value("{capability.nope.base}", &caps).is_err());
    }
}
