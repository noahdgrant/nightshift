//! The factory definition: `nightshift.toml`, `runners/<name>.toml` and `agents/<role>/agent.md`
//! (docs/FACTORY.md).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config;
use crate::error::SfError;
use crate::frontmatter;

pub const FILE: &str = "nightshift.toml";

/// The phases `ns run` drives, in pipeline order.
pub const PHASES: &[&str] = &["triage", "build", "verify", "review", "ship"];

/// The artifact each phase writes under `.ns/<unit>/`.
pub fn artifact_of(phase: &str) -> &'static str {
    match phase {
        "triage" => "brief.md",
        "build" => "build.md",
        "verify" => "evidence.md",
        "review" => "review.md",
        "ship" => "pr.md",
        _ => "unknown.md",
    }
}

pub const PLACEHOLDERS: &[&str] = &[
    "skill",
    "unit",
    "issue",
    "issue_url",
    "worktree",
    "gates",
    "phase",
    "attempt",
    "feedback",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Factory {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_gates")]
    pub gates: String,
    #[serde(default)]
    pub worktree: WorktreeDef,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub phases: BTreeMap<String, Phase>,
    #[serde(default)]
    pub queue: Queue,
    #[serde(default)]
    pub limits: Limits,
    /// Absent table: `policy = "human"`.
    #[serde(default)]
    pub merge: Merge,
    /// `runners/<name>.toml`, read by `load`, not part of `nightshift.toml`.
    #[serde(skip)]
    pub runners: BTreeMap<String, Runner>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunnerKind {
    #[default]
    Local,
    /// A lock-bearing kind only; nothing runs remotely.
    Bench,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Runner {
    #[serde(default)]
    pub kind: RunnerKind,
    /// Names of exclusive locks `ns run` holds for the whole phase.
    #[serde(default)]
    pub locks: Vec<String>,
}

/// A lock name becomes a file name, so it may not hold a path separator or start with a dot.
fn valid_lock_name(name: &str) -> bool {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
        .unwrap()
        .is_match(name)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Merge {
    /// `human` (default) or `auto`.
    pub policy: String,
    /// Globs (`**` crosses directories); a PR touching one needs a human merge.
    pub human_review: Vec<String>,
    pub ci_timeout_minutes: u64,
    /// Minutes to wait for the first check to register on the PR head before concluding no CI.
    pub ci_register_timeout: u64,
}

impl Default for Merge {
    fn default() -> Self {
        Self {
            policy: "human".into(),
            human_review: Vec::new(),
            ci_timeout_minutes: 30,
            ci_register_timeout: 3,
        }
    }
}

impl Merge {
    pub fn auto(&self) -> bool {
        self.policy == "auto"
    }

    /// Changed paths matching a human_review glob.
    pub fn human_review_hits(&self, files: &[String]) -> Vec<String> {
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        let pats: Vec<glob::Pattern> = self
            .human_review
            .iter()
            .filter_map(|p| glob::Pattern::new(p).ok())
            .collect();
        files
            .iter()
            .filter(|f| pats.iter().any(|p| p.matches_with(f, opts)))
            .cloned()
            .collect()
    }
}

impl Default for Factory {
    fn default() -> Self {
        parse("").expect("the empty definition parses")
    }
}

fn default_gates() -> String {
    "auto".into()
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeDef {
    /// Shell commands run in a newly created worktree.
    #[serde(default)]
    pub setup: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Defaults {
    pub harness: String,
    pub model: Option<String>,
    pub timeout_minutes: u64,
    pub max_attempts: u32,
    /// `subscription` (default) or `api`.
    pub billing: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            harness: "claude".into(),
            model: None,
            timeout_minutes: 45,
            max_attempts: 2,
            billing: "subscription".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    pub skill: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub timeout_minutes: Option<u64>,
    pub max_attempts: Option<u32>,
    /// A `runners/<name>.toml`; unset runs with no locks.
    pub runner: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Queue {
    pub source: String,
    pub ready_label: String,
    pub in_progress_label: String,
    pub done_label: String,
    pub stuck_label: String,
    /// Sorted on before `order`; an issue with no match goes after the last.
    pub priority: Vec<String>,
    pub order: Vec<String>,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            source: "github".into(),
            ready_label: "status:ready-for-agent".into(),
            in_progress_label: "status:in-progress".into(),
            done_label: "status:in-review".into(),
            stuck_label: "status:ready-for-human".into(),
            priority: ["priority:high", "priority:medium", "priority:low"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            order: [
                "type:fix",
                "type:feat",
                "type:refactor",
                "type:test",
                "type:docs",
                "type:chore",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    /// Unset: no cap; `--until`, the queue and usage limits end the night.
    pub max_units: Option<u32>,
    /// Unset: no cap under `billing = "subscription"`, 25.0 under `api`.
    pub budget_usd: Option<f64>,
}

/// One phase with its defaults applied.
#[derive(Debug, Clone, PartialEq)]
pub struct PhaseSettings {
    pub name: String,
    pub skill: String,
    pub harness: String,
    pub model: Option<String>,
    pub timeout_minutes: u64,
    pub max_attempts: u32,
    pub runner: Option<String>,
}

impl Factory {
    pub fn phase(&self, name: &str) -> PhaseSettings {
        let p = self.phases.get(name).cloned().unwrap_or_default();
        PhaseSettings {
            name: name.to_string(),
            skill: p.skill.unwrap_or_else(|| format!("ns-{name}")),
            harness: p.harness.unwrap_or_else(|| self.defaults.harness.clone()),
            model: p.model.or_else(|| self.defaults.model.clone()),
            timeout_minutes: p.timeout_minutes.unwrap_or(self.defaults.timeout_minutes),
            max_attempts: p.max_attempts.unwrap_or(self.defaults.max_attempts),
            runner: p.runner,
        }
    }

    /// The locks a phase's runner names.
    pub fn locks(&self, p: &PhaseSettings) -> Vec<String> {
        p.runner
            .as_ref()
            .and_then(|r| self.runners.get(r))
            .map(|r| r.locks.clone())
            .unwrap_or_default()
    }

    pub fn subscription(&self) -> bool {
        self.defaults.billing != "api"
    }

    pub fn budget_usd(&self) -> Option<f64> {
        match self.limits.budget_usd {
            Some(b) => Some(b),
            None if self.subscription() => None,
            None => Some(25.0),
        }
    }

    /// Semantic checks beyond parsing; each string is one problem.
    pub fn problems(&self, cfg: Option<&config::Config>) -> Vec<String> {
        let mut out = Vec::new();
        if !["stop", "auto"].contains(&self.gates.as_str()) {
            out.push(format!(
                "gates = {:?}: use \"stop\" or \"auto\"",
                self.gates
            ));
        }
        if !["subscription", "api"].contains(&self.defaults.billing.as_str()) {
            out.push(format!(
                "defaults.billing = {:?}: use \"subscription\" or \"api\"",
                self.defaults.billing
            ));
        }
        if self.queue.source != "github" {
            out.push(format!(
                "queue.source = {:?}: only \"github\" is supported",
                self.queue.source
            ));
        }
        for name in self.phases.keys() {
            if !PHASES.contains(&name.as_str()) {
                out.push(format!(
                    "[phases.{name}]: unknown phase; phases are {}",
                    PHASES.join(", ")
                ));
            }
        }
        for name in PHASES {
            let p = self.phase(name);
            if p.max_attempts == 0 {
                out.push(format!("phase {name}: max_attempts must be at least 1"));
            }
            if p.timeout_minutes == 0 {
                out.push(format!("phase {name}: timeout_minutes must be at least 1"));
            }
            if let Some(r) = &p.runner {
                if !self.runners.contains_key(r) {
                    out.push(format!(
                        "phase {name}: runner {r:?} has no runners/{r}.toml"
                    ));
                }
            }
            let known = cfg.is_some_and(|c| c.harness.contains_key(&p.harness))
                || config::builtin_harness(&p.harness).is_some();
            if !known {
                out.push(format!(
                    "phase {name}: harness {:?} is neither in the user config nor built in (claude, codex)",
                    p.harness
                ));
            }
        }
        let mut by_folded: BTreeMap<String, &str> = BTreeMap::new();
        for (name, r) in &self.runners {
            for l in &r.locks {
                if !valid_lock_name(l) {
                    out.push(format!(
                        "runners/{name}.toml: lock {l:?} must be letters, digits, '.', '_' or '-', not starting with '.'"
                    ));
                    continue;
                }
                let first = by_folded.entry(l.to_lowercase()).or_insert(l);
                if *first != l {
                    out.push(format!(
                        "runners/{name}.toml: lock {l:?} differs from {first:?} only by case, and one file serves both on a case-insensitive filesystem"
                    ));
                }
            }
        }
        if !["auto", "human"].contains(&self.merge.policy.as_str()) {
            out.push(format!(
                "merge.policy = {:?}: use \"auto\" or \"human\"",
                self.merge.policy
            ));
        }
        for p in &self.merge.human_review {
            if glob::Pattern::new(p).is_err() {
                out.push(format!("merge.human_review: {p:?} is not a valid glob"));
            }
        }
        if let Some(b) = self.limits.budget_usd {
            if b < 0.0 {
                out.push("limits.budget_usd must not be negative".into());
            }
        }
        out
    }
}

/// Locate the definition root: `--factory <dir>`, else `<main_root>/.nightshift`.
pub fn root(factory: Option<&Path>, main_root: &Path) -> PathBuf {
    match factory {
        Some(d) => d.to_path_buf(),
        None => main_root.join(".nightshift"),
    }
}

pub fn parse(text: &str) -> Result<Factory, toml::de::Error> {
    toml::from_str(text)
}

/// Load `<root>/nightshift.toml` and `<root>/runners/*.toml`. A missing file gives the defaults.
pub fn load(root: &Path) -> Result<Factory> {
    let p = root.join(FILE);
    let bad = |what: String| -> anyhow::Error {
        SfError::usage(
            what,
            "check it with:\n  ns factory validate\n\nthe schema is in docs/FACTORY.md",
        )
        .into()
    };
    let mut fac = match fs::read_to_string(&p) {
        Ok(t) => parse(&t).map_err(|e| bad(format!("cannot parse {}: {e}", p.display())))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Factory::default(),
        Err(e) => return Err(anyhow::anyhow!("cannot read {}: {e}", p.display())),
    };
    let (runners, problems) = load_runners(root);
    if !problems.is_empty() {
        return Err(bad(problems.join("; ")));
    }
    fac.runners = runners;
    Ok(fac)
}

/// Every `runners/<name>.toml` under the root, and a problem for each file that doesn't parse.
fn load_runners(root: &Path) -> (BTreeMap<String, Runner>, Vec<String>) {
    let mut runners = BTreeMap::new();
    let mut problems = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("runners")) else {
        return (runners, problems);
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("toml") {
            continue;
        }
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| toml::from_str::<Runner>(&t).map_err(|e| e.message().to_string()))
        {
            Ok(r) => {
                runners.insert(name, r);
            }
            Err(e) => problems.push(format!("runners/{name}.toml: {e}")),
        }
    }
    problems.sort();
    (runners, problems)
}

/// The prompt template for a role: `agents/<role>/agent.md` body, else `None` (built-in).
pub fn agent_template(root: &Path, role: &str) -> Result<Option<String>> {
    let p = root.join("agents").join(role).join("agent.md");
    match fs::read_to_string(&p) {
        Ok(text) => Ok(Some(match frontmatter::split(&text) {
            Some((_, body)) => body.trim_start().to_string(),
            None => text,
        })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("cannot read {}: {e}", p.display())),
    }
}

pub const BUILTIN_PROMPT: &str = "Load the `{skill}` skill and run it for unit `{unit}` (issue {issue_url}).
Work in {worktree}. gates: {gates}.
Phase: {phase} (attempt {attempt}). End by writing the phase's artifact under .ns/{unit}/ with its frontmatter status.
Never merge, never approve, and never push to the default branch.
";

pub struct PromptVars<'a> {
    pub skill: &'a str,
    pub unit: &'a str,
    pub issue: &'a str,
    pub issue_url: &'a str,
    pub worktree: &'a str,
    pub gates: &'a str,
    pub phase: &'a str,
    pub attempt: u32,
    pub feedback: &'a str,
}

/// Replace each known `{name}` in one pass, so placeholder-like text in values stays as is.
pub fn render(template: &str, v: &PromptVars<'_>) -> String {
    let re = regex::Regex::new(&format!(r"\{{({})\}}", PLACEHOLDERS.join("|"))).unwrap();
    re.replace_all(template, |c: &regex::Captures<'_>| match &c[1] {
        "skill" => v.skill.to_string(),
        "unit" => v.unit.to_string(),
        "issue" => v.issue.to_string(),
        "issue_url" => v.issue_url.to_string(),
        "worktree" => v.worktree.to_string(),
        "gates" => v.gates.to_string(),
        "phase" => v.phase.to_string(),
        "attempt" => v.attempt.to_string(),
        "feedback" => v.feedback.to_string(),
        _ => String::new(),
    })
    .into_owned()
}

/// The full prompt for a phase: the role's agent.md, else the built-in with feedback appended.
pub fn prompt(root: &Path, v: &PromptVars<'_>) -> Result<String> {
    match agent_template(root, v.phase)? {
        Some(t) => Ok(render(&t, v)),
        None => {
            let mut s = render(BUILTIN_PROMPT, v);
            if !v.feedback.trim().is_empty() {
                s.push_str("\nThe artifact that sent this work back says:\n\n");
                s.push_str(v.feedback.trim());
                s.push('\n');
            }
            Ok(s)
        }
    }
}

/// Problems in `agents/*/agent.md`: unknown roles, a mismatched `role:`, unknown placeholders.
fn agent_problems(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("agents")) else {
        return out;
    };
    let any = regex::Regex::new(r"\{([a-z_]+)\}").unwrap();
    for e in entries.flatten() {
        let dir = e.file_name().to_string_lossy().into_owned();
        let p = e.path().join("agent.md");
        let Ok(text) = fs::read_to_string(&p) else {
            out.push(format!("agents/{dir}: no agent.md"));
            continue;
        };
        if !PHASES.contains(&dir.as_str()) {
            out.push(format!(
                "agents/{dir}: not a phase; roles are {}",
                PHASES.join(", ")
            ));
        }
        match frontmatter::parse(&text) {
            Ok(Some(fm)) => {
                let role = fm.get("role").and_then(|r| r.as_str()).unwrap_or("");
                if role != dir {
                    out.push(format!(
                        "agents/{dir}/agent.md: frontmatter role {role:?} should be {dir:?}"
                    ));
                }
            }
            Ok(None) => out.push(format!(
                "agents/{dir}/agent.md: missing frontmatter (---\\nrole: {dir}\\n---)"
            )),
            Err(e) => out.push(format!("agents/{dir}/agent.md: bad frontmatter: {e}")),
        }
        for c in any.captures_iter(&text) {
            if !PLACEHOLDERS.contains(&&c[1]) {
                out.push(format!(
                    "agents/{dir}/agent.md: unknown placeholder {{{}}}",
                    &c[1]
                ));
            }
        }
    }
    out
}

/// `ns factory validate`.
pub fn validate(factory: Option<&Path>) -> Result<ExitCode> {
    let start = std::env::current_dir()?;
    let main_root = match factory {
        Some(_) => start.clone(),
        None => crate::git::Repo::discover(&start)?.root,
    };
    let root = root(factory, &main_root);
    let file = root.join(FILE);
    let cfg = config::load(&config::path()).ok().flatten();
    let mut problems = Vec::new();
    let mut phases = Vec::new();
    let mut name = None;
    let exists = file.is_file();
    if !exists {
        problems.push(format!(
            "{} does not exist; ns run would use the built-in defaults",
            file.display()
        ));
    } else {
        match fs::read_to_string(&file)
            .map_err(|e| e.to_string())
            .and_then(|t| parse(&t).map_err(|e| e.to_string()))
        {
            Ok(mut f) => {
                name = f.name.clone();
                let (runners, bad) = load_runners(&root);
                f.runners = runners;
                problems.extend(bad);
                problems.extend(f.problems(cfg.as_ref()));
                problems.extend(agent_problems(&root));
                for name in PHASES {
                    let p = f.phase(name);
                    let custom = root.join("agents").join(name).join("agent.md").is_file();
                    phases.push(json!({
                        "phase": p.name,
                        "skill": p.skill,
                        "harness": p.harness,
                        "model": p.model,
                        "timeout_minutes": p.timeout_minutes,
                        "max_attempts": p.max_attempts,
                        "runner": p.runner,
                        "locks": f.locks(&p),
                        "prompt": if custom { "agent.md" } else { "built-in" },
                    }));
                }
            }
            Err(e) => problems.push(e),
        }
    }
    let ok = problems.is_empty();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "ok": ok,
            "path": file.to_string_lossy(),
            "name": name,
            "phases": phases,
            "errors": problems,
        }))?
    );
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
name = "nightshift"
gates = "auto"
[worktree]
setup = ["git submodule update --init"]
[defaults]
harness = "claude"
model = "opus"
timeout_minutes = 45
max_attempts = 2
[phases.triage]
skill = "ns-triage"
[phases.build]
skill = "ns-build"
timeout_minutes = 90
[phases.verify]
skill = "ns-verify"
[phases.review]
skill = "ns-review"
harness = "codex"
[phases.ship]
skill = "ns-ship"
[queue]
source = "github"
ready_label = "status:ready-for-agent"
in_progress_label = "status:in-progress"
done_label = "status:in-review"
stuck_label = "status:ready-for-human"
order = ["type:fix", "type:feat"]
[limits]
max_units = 4
budget_usd = 25.0
[merge]
policy = "auto"
human_review = [".github/**", ".nightshift/**", "cli/src/run.rs"]
ci_timeout_minutes = 30
"#;

    #[test]
    fn parses_the_spec_example() {
        let f = parse(FULL).unwrap();
        assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
        let b = f.phase("build");
        assert_eq!(b.timeout_minutes, 90);
        assert_eq!(b.model.as_deref(), Some("opus"));
        assert_eq!(f.phase("review").harness, "codex");
        assert_eq!(f.worktree.setup.len(), 1);
        assert_eq!(f.budget_usd(), Some(25.0));
        assert!(f.subscription());
        assert!(f.merge.auto());
        let hits = f.merge.human_review_hits(&[
            ".github/workflows/ci.yml".into(),
            "cli/src/run.rs".into(),
            "cli/src/main.rs".into(),
            ".nightshift/agents/build/agent.md".into(),
        ]);
        assert_eq!(hits.len(), 3, "{hits:?}");
        assert!(!parse("").unwrap().merge.auto());
    }

    #[test]
    fn rejects_unknown_keys_everywhere() {
        for bad in [
            "nmae = \"x\"\n",
            "[defaults]\nharnes = \"claude\"\n",
            "[phases.build]\nskil = \"x\"\n",
            "[queue]\nready = \"x\"\n",
            "[limits]\nmax = 1\n",
            "[worktree]\nsetups = []\n",
            "[automations]\n",
            "[merge]\npolcy = \"auto\"\n",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn defaults_and_budget_by_billing() {
        let f = parse("").unwrap();
        assert_eq!(f.gates, "auto");
        assert_eq!(f.phase("verify").skill, "ns-verify");
        assert_eq!(f.phase("verify").max_attempts, 2);
        assert_eq!(f.budget_usd(), None);
        assert_eq!(f.merge.ci_register_timeout, 3);
        let f = parse("[merge]\nci_register_timeout = 7\n").unwrap();
        assert_eq!(f.merge.ci_register_timeout, 7);
        let f = parse("[defaults]\nbilling = \"api\"\n").unwrap();
        assert_eq!(f.budget_usd(), Some(25.0));
    }

    #[test]
    fn queue_priority_defaults_and_overrides() {
        let f = parse("").unwrap();
        assert_eq!(
            f.queue.priority,
            ["priority:high", "priority:medium", "priority:low"]
        );
        let f = parse("[queue]\npriority = [\"p0\", \"p1\"]\n").unwrap();
        assert_eq!(f.queue.priority, ["p0", "p1"]);
        assert_eq!(f.queue.order[0], "type:fix");
    }

    #[test]
    fn semantic_problems() {
        let f = parse("gates = \"maybe\"\n[phases.deploy]\n[defaults]\nharness = \"nope\"\nmax_attempts = 0\n").unwrap();
        let p = f.problems(None).join("\n");
        assert!(p.contains("gates"));
        assert!(p.contains("unknown phase"));
        assert!(p.contains("\"nope\""));
        assert!(p.contains("max_attempts"));
    }

    fn runners(files: &[(&str, &str)]) -> (BTreeMap<String, Runner>, Vec<String>) {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("runners")).unwrap();
        for (name, text) in files {
            fs::write(tmp.path().join("runners").join(name), text).unwrap();
        }
        load_runners(tmp.path())
    }

    #[test]
    fn runner_files_load_with_local_as_the_default_kind() {
        let (r, bad) = runners(&[
            ("zephyr.toml", "locks = [\"zephyr-workspace\"]\n"),
            (
                "bench.toml",
                "kind = \"bench\"\nlocks = [\"bench-1\", \"zephyr-workspace\"]\n",
            ),
            ("README.md", "not a runner"),
        ]);
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(r.len(), 2);
        assert_eq!(r["zephyr"].kind, RunnerKind::Local);
        assert_eq!(r["bench"].kind, RunnerKind::Bench);
    }

    #[test]
    fn a_bad_runner_file_is_a_problem() {
        let (r, bad) = runners(&[
            ("remote.toml", "kind = \"remote\"\n"),
            ("typo.toml", "lock = [\"a\"]\n"),
            ("notalist.toml", "locks = \"a\"\n"),
        ]);
        assert!(r.is_empty());
        assert_eq!(bad.len(), 3, "{bad:?}");
        assert!(bad[1].starts_with("runners/remote.toml"), "{bad:?}");
    }

    #[test]
    fn runner_references_and_lock_names_are_checked() {
        let mut f = parse("[phases.verify]\nrunner = \"bench\"\n").unwrap();
        let p = f.problems(None).join("\n");
        assert!(
            p.contains("phase verify: runner \"bench\" has no runners/bench.toml"),
            "{p}"
        );
        f.runners.insert(
            "bench".into(),
            Runner {
                kind: RunnerKind::Bench,
                locks: vec![
                    "../x".into(),
                    "bench-1".into(),
                    "bench-1".into(),
                    "a/b".into(),
                ],
            },
        );
        let p = f.problems(None);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].contains("\"../x\""), "{p:?}");
        assert!(f.locks(&f.phase("build")).is_empty());
    }

    #[test]
    fn lock_names_that_differ_only_by_case_are_a_problem() {
        let mut f = parse("").unwrap();
        for (runner, lock) in [("a", "Bench"), ("b", "bench"), ("c", "Bench")] {
            f.runners.insert(
                runner.into(),
                Runner {
                    kind: RunnerKind::Local,
                    locks: vec![lock.into()],
                },
            );
        }
        let p = f.problems(None);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].starts_with("runners/b.toml"), "{p:?}");
        assert!(p[0].contains("only by case"), "{p:?}");
    }

    #[test]
    fn render_is_single_pass() {
        let v = PromptVars {
            skill: "ns-build",
            unit: "7-x",
            issue: "7",
            issue_url: "https://x/7",
            worktree: "/w",
            gates: "auto",
            phase: "build",
            attempt: 2,
            feedback: "keep {unit} literal",
        };
        let s = render("{skill} {unit} {attempt} {nope} {feedback}", &v);
        assert_eq!(s, "ns-build 7-x 2 {nope} keep {unit} literal");
    }
}
