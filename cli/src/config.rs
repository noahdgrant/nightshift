//! `~/.config/nightshift/config.toml`: harness commands and role mapping.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub harness: BTreeMap<String, Harness>,
    #[serde(default)]
    pub roles: BTreeMap<String, Role>,
    #[serde(default)]
    pub eval: EvalConfig,
}

/// `[eval]`: how `ns eval` runs trials. Every key has a default, so the section is optional.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct EvalConfig {
    /// Name of the harness under `[eval.harnesses.<name>]` (or a built-in) to run trials with.
    pub harness: String,
    pub model: Option<String>,
    /// Starting trials per arm.
    pub trials: u32,
    /// Adaptive ceiling per arm.
    pub max_trials: u32,
    /// Stop starting trials once the reported cost reaches this, per invocation.
    pub budget_usd: f64,
    pub max_runs: u32,
    /// Raw transcripts and the baseline cache; never committed.
    pub transcripts: String,
    pub harnesses: BTreeMap<String, EvalHarness>,
    /// `[eval.capability.<name>]`: configured when the table exists.
    pub capability: BTreeMap<String, Capability>,
    /// `subscription` (default): claude runs never see API-key variables or `--bare`, and a
    /// missing subscription login stops the run. `api` turns the guard off.
    pub billing: String,
}

/// A capability: arbitrary string keys (`base`, `sdk`, ...) plus an optional `path_prepend`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Capability {
    /// Prepended to `PATH` for the whole trial; `~` is expanded.
    #[serde(default)]
    pub path_prepend: Vec<String>,
    #[serde(flatten)]
    pub keys: BTreeMap<String, String>,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            harness: "claude".into(),
            model: None,
            trials: 2,
            max_trials: 5,
            budget_usd: 5.0,
            max_runs: 40,
            transcripts: "~/.local/share/nightshift/evals".into(),
            harnesses: BTreeMap::new(),
            capability: BTreeMap::new(),
            billing: "subscription".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EvalHarness {
    /// argv; `{model}` is replaced by `[eval].model`. The prompt goes to stdin.
    pub command: Vec<String>,
    /// Paths relative to `$HOME` symlinked into each trial's throwaway home (credentials).
    #[serde(default)]
    pub carry: Vec<String>,
    /// Transcript format: `claude-stream-json`, or `none` (no metrics, triggers unsupported).
    #[serde(default = "default_output")]
    pub output: String,
}

fn default_output() -> String {
    "none".into()
}

/// Output formats `ns eval` can parse, plus `none`.
pub const EVAL_OUTPUTS: &[&str] = &["claude-stream-json", "none"];

/// Built-in eval harnesses, used when `[eval.harnesses.<name>]` is not configured.
pub fn builtin_eval_harness(name: &str) -> Option<EvalHarness> {
    let owned = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match name {
        "claude" => Some(EvalHarness {
            command: owned(&[
                "claude",
                "-p",
                "--model",
                "{model}",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "bypassPermissions",
            ]),
            carry: owned(&[".claude/.credentials.json", ".claude.json"]),
            output: "claude-stream-json".into(),
        }),
        _ => None,
    }
}

impl EvalConfig {
    /// The harness named `name`: configured first, else built in.
    pub fn harness_named(&self, name: &str) -> Option<EvalHarness> {
        self.harnesses
            .get(name)
            .cloned()
            .or_else(|| builtin_eval_harness(name))
    }
}

/// Expand a leading `~/` (or a bare `~`) to `$HOME`.
pub fn expand_tilde(p: &str) -> PathBuf {
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Harness {
    /// Read-only argv; `{model}` is replaced by the role's model.
    pub command: Vec<String>,
    /// argv for `ns ask --write`: may edit files and run commands in the working directory.
    #[serde(default)]
    pub command_write: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub harness: String,
    #[serde(default)]
    pub model: Option<String>,
}

pub const EXAMPLE: &str = r#"[roles.default]
harness = "claude"

[roles."review.security"]
harness = "codex"
model = "gpt-5-codex"

# Optional: override a built-in harness or add a new one.
[harness.codex]
command = ["codex", "exec", "--sandbox", "read-only", "-m", "{model}"]
command_write = ["codex", "exec", "--sandbox", "workspace-write", "-m", "{model}"]"#;

/// Built-in harness commands, used when the config does not define the harness.
pub fn builtin_harness(name: &str) -> Option<Harness> {
    let (read, write): (&[&str], &[&str]) = match name {
        "claude" => (
            &[
                "claude",
                "-p",
                "--permission-mode",
                "plan",
                "--model",
                "{model}",
            ],
            &[
                "claude",
                "-p",
                "--permission-mode",
                "acceptEdits",
                "--model",
                "{model}",
            ],
        ),
        "codex" => (
            &["codex", "exec", "--sandbox", "read-only", "-m", "{model}"],
            &[
                "codex",
                "exec",
                "--sandbox",
                "workspace-write",
                "-m",
                "{model}",
            ],
        ),
        _ => return None,
    };
    let owned = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Some(Harness {
        command: owned(read),
        command_write: Some(owned(write)),
    })
}

/// Config path: `$NS_CONFIG`, else `$XDG_CONFIG_HOME/nightshift/config.toml`, else `~/.config/nightshift/config.toml`.
pub fn path() -> PathBuf {
    path_from(
        std::env::var_os("NS_CONFIG"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

fn path_from(
    ns: Option<std::ffi::OsString>,
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> PathBuf {
    if let Some(p) = ns.filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(x) = xdg.filter(|s| !s.is_empty()) {
        return PathBuf::from(x).join("nightshift").join("config.toml");
    }
    PathBuf::from(home.unwrap_or_default())
        .join(".config")
        .join("nightshift")
        .join("config.toml")
}

pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(text)
}

pub fn load(path: &Path) -> anyhow::Result<Option<Config>> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("cannot parse {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("cannot read {}: {e}", path.display())),
    }
}

/// Candidate role names, most specific first: `review.security` -> `review` -> `default`.
pub fn role_chain(role: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut cur = role;
    loop {
        chain.push(cur.to_string());
        match cur.rfind('.') {
            Some(i) => cur = &cur[..i],
            None => break,
        }
    }
    if role != "default" {
        chain.push("default".to_string());
    }
    chain
}

#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub requested: String,
    pub matched: String,
    pub harness: String,
    pub model: Option<String>,
    /// `None` if the harness is neither configured nor built in.
    pub command: Option<Vec<String>>,
    /// `None` if the harness has no `command_write`.
    pub command_write: Option<Vec<String>>,
}

impl Config {
    pub fn resolve(&self, role: &str) -> Option<Resolved> {
        let (matched, r) = role_chain(role)
            .into_iter()
            .find_map(|name| self.roles.get(&name).map(|r| (name, r)))?;
        let harness = self
            .harness
            .get(&r.harness)
            .cloned()
            .or_else(|| builtin_harness(&r.harness));
        Some(Resolved {
            requested: role.to_string(),
            matched,
            harness: r.harness.clone(),
            model: r.model.clone(),
            command: harness
                .as_ref()
                .map(|h| expand(&h.command, r.model.as_deref())),
            command_write: harness
                .as_ref()
                .and_then(|h| h.command_write.as_ref())
                .map(|c| expand(c, r.model.as_deref())),
        })
    }
}

/// Substitute `{model}`. With no model, an argument that is exactly `{model}` is dropped,
/// together with the flag before it (`--model {model}` disappears).
pub fn expand(argv: &[String], model: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(argv.len());
    for arg in argv {
        match model {
            Some(m) => out.push(arg.replace("{model}", m)),
            None if arg == "{model}" => {
                if out.len() > 1 && out.last().is_some_and(|p| p.starts_with('-')) {
                    out.pop();
                }
            }
            None if arg.contains("{model}") => {}
            None => out.push(arg.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        parse(
            r#"
[harness.fake]
command = ["fake", "--m", "{model}"]
[roles.default]
harness = "claude"
[roles.review]
harness = "fake"
model = "small"
[roles."review.security"]
harness = "codex"
model = "big"
"#,
        )
        .unwrap()
    }

    #[test]
    fn chain() {
        assert_eq!(
            role_chain("review.security"),
            ["review.security", "review", "default"]
        );
        assert_eq!(role_chain("a.b.c"), ["a.b.c", "a.b", "a", "default"]);
        assert_eq!(role_chain("default"), ["default"]);
    }

    #[test]
    fn resolves_exact_parent_and_default() {
        let c = cfg();
        let r = c.resolve("review.security").unwrap();
        assert_eq!(r.matched, "review.security");
        assert_eq!(
            r.command.unwrap(),
            ["codex", "exec", "--sandbox", "read-only", "-m", "big"]
        );

        let r = c.resolve("review.comments").unwrap();
        assert_eq!(r.matched, "review");
        assert_eq!(r.command.unwrap(), ["fake", "--m", "small"]);
        assert_eq!(r.command_write, None);

        let r = c.resolve("verify").unwrap();
        assert_eq!(r.matched, "default");
        assert_eq!(
            r.command.unwrap(),
            ["claude", "-p", "--permission-mode", "plan"]
        );
        assert_eq!(
            r.command_write.unwrap(),
            ["claude", "-p", "--permission-mode", "acceptEdits"]
        );
    }

    #[test]
    fn write_commands() {
        let c = cfg();
        assert_eq!(
            c.resolve("verify").unwrap().command_write.unwrap(),
            ["claude", "-p", "--permission-mode", "acceptEdits"]
        );
        assert_eq!(
            c.resolve("review.security").unwrap().command_write.unwrap(),
            ["codex", "exec", "--sandbox", "workspace-write", "-m", "big"]
        );
        // Configured harness without command_write.
        assert_eq!(c.resolve("review.spec").unwrap().command_write, None);
        let c = parse(
            "[harness.w]\ncommand = [\"w\"]\ncommand_write = [\"w\", \"--rw\", \"{model}\"]\n[roles.default]\nharness = \"w\"\nmodel = \"m\"\n",
        )
        .unwrap();
        assert_eq!(
            c.resolve("x").unwrap().command_write.unwrap(),
            ["w", "--rw", "m"]
        );
    }

    #[test]
    fn no_match_without_default() {
        let c = parse("[roles.review]\nharness = \"codex\"\n").unwrap();
        assert!(c.resolve("verify").is_none());
        assert!(c.resolve("review.spec").is_some());
    }

    #[test]
    fn unknown_harness_has_no_command() {
        let c = parse("[roles.default]\nharness = \"nope\"\n").unwrap();
        assert_eq!(c.resolve("x").unwrap().command, None);
    }

    #[test]
    fn eval_defaults_and_overrides() {
        let c = parse("").unwrap();
        assert_eq!(c.eval, EvalConfig::default());
        let h = c.eval.harness_named("claude").unwrap();
        assert_eq!(h.output, "claude-stream-json");
        assert!(h.command.contains(&"bypassPermissions".to_string()));
        assert!(c.eval.harness_named("nope").is_none());

        let c = parse(
            r#"
[eval]
harness = "fake"
model = "haiku"
trials = 3
budget_usd = 1.5
[eval.harnesses.fake]
command = ["fake", "{model}"]
[eval.capability.zephyr]
base = "~/zephyr"
path_prepend = ["~/venv/bin"]
[eval.capability.empty]
"#,
        )
        .unwrap();
        assert_eq!(c.eval.trials, 3);
        assert_eq!(c.eval.max_trials, 5);
        let h = c.eval.harness_named("fake").unwrap();
        assert_eq!(h.output, "none");
        assert!(h.carry.is_empty());
        assert_eq!(c.eval.capability["zephyr"].keys["base"], "~/zephyr");
        assert_eq!(c.eval.capability["zephyr"].path_prepend, ["~/venv/bin"]);
        assert!(c.eval.capability.contains_key("empty"));
        assert!(parse("[eval.capability.x]\nbase = 3\n").is_err());
        assert!(parse(
            "[eval]
trails = 3
"
        )
        .is_err());
        assert!(parse(
            "[eval.harnesses.x]
command = []
ouput = \"none\"
"
        )
        .is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse("[roles.default]\nharnes = \"claude\"\n").is_err());
    }

    #[test]
    fn config_path_precedence() {
        let s = |v: &str| Some(std::ffi::OsString::from(v));
        assert_eq!(
            path_from(s("/c.toml"), s("/x"), s("/h")),
            PathBuf::from("/c.toml")
        );
        assert_eq!(
            path_from(None, s("/x"), s("/h")),
            PathBuf::from("/x/nightshift/config.toml")
        );
        assert_eq!(
            path_from(None, None, s("/h")),
            PathBuf::from("/h/.config/nightshift/config.toml")
        );
    }
}
