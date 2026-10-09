//! The factory definition: `nightshift.toml` and `agents/<role>/agent.md` (docs/FACTORY.md).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use serde::Deserialize;
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

#[derive(Debug, Clone, Default, Deserialize)]
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Merge {
    /// `human` (default) or `auto`.
    pub policy: String,
    /// Globs (`**` crosses directories); a PR touching one needs a human merge.
    pub protected: Vec<String>,
    pub ci_timeout_minutes: u64,
}

impl Default for Merge {
    fn default() -> Self {
        Self {
            policy: "human".into(),
            protected: Vec::new(),
            ci_timeout_minutes: 30,
        }
    }
}

impl Merge {
    pub fn auto(&self) -> bool {
        self.policy == "auto"
    }

    /// Changed paths matching a protected glob.
    pub fn protected_hits(&self, files: &[String]) -> Vec<String> {
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        let pats: Vec<glob::Pattern> = self
            .protected
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Queue {
    pub source: String,
    pub ready_label: String,
    pub in_progress_label: String,
    pub done_label: String,
    pub stuck_label: String,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    pub max_units: u32,
    /// Unset: no cap under `billing = "subscription"`, 25.0 under `api`.
    pub budget_usd: Option<f64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_units: 4,
            budget_usd: None,
        }
    }
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
        }
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
            let known = cfg.is_some_and(|c| c.harness.contains_key(&p.harness))
                || config::builtin_harness(&p.harness).is_some();
            if !known {
                out.push(format!(
                    "phase {name}: harness {:?} is neither in the user config nor built in (claude, codex)",
                    p.harness
                ));
            }
        }
        if !["auto", "human"].contains(&self.merge.policy.as_str()) {
            out.push(format!(
                "merge.policy = {:?}: use \"auto\" or \"human\"",
                self.merge.policy
            ));
        }
        for p in &self.merge.protected {
            if glob::Pattern::new(p).is_err() {
                out.push(format!("merge.protected: {p:?} is not a valid glob"));
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

/// Load `<root>/nightshift.toml`. A missing file gives the defaults.
pub fn load(root: &Path) -> Result<Factory> {
    let p = root.join(FILE);
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Factory::default()),
        Err(e) => return Err(anyhow::anyhow!("cannot read {}: {e}", p.display())),
    };
    parse(&text).map_err(|e| {
        SfError::usage(
            format!("cannot parse {}: {e}", p.display()),
            "check it with:\n  ns factory validate\n\nthe schema is in docs/FACTORY.md",
        )
        .into()
    })
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
            Ok(f) => {
                name = f.name.clone();
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
protected = [".github/**", ".nightshift/**", "cli/src/run.rs"]
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
        let hits = f.merge.protected_hits(&[
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
        let f = parse("[defaults]\nbilling = \"api\"\n").unwrap();
        assert_eq!(f.budget_usd(), Some(25.0));
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
