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
    #[serde(default)]
    pub forge: Forges,
    #[serde(default)]
    pub runners: RunnersConfig,
}

/// `[runners]`: where `ns run` keeps runner locks.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunnersConfig {
    /// A directory shared between repos; `~` is expanded. Unset: `<git-common-dir>/ns/locks`.
    pub lock_dir: Option<String>,
}

/// `[forge.github]` and `[forge.gitlab]`: where `ns run` and `ns watch` get forge tokens.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Forges {
    pub github: Option<Forge>,
    pub gitlab: Option<Forge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeKind {
    Github,
    Gitlab,
}

impl ForgeKind {
    pub const ALL: [ForgeKind; 2] = [ForgeKind::Github, ForgeKind::Gitlab];

    pub fn parse(name: &str) -> Option<ForgeKind> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            ForgeKind::Github => "github",
            ForgeKind::Gitlab => "gitlab",
        }
    }

    pub fn token_var(self) -> &'static str {
        match self {
            ForgeKind::Github => "GH_TOKEN",
            ForgeKind::Gitlab => "GITLAB_TOKEN",
        }
    }

    /// The CLI that talks to this forge.
    pub fn cli(self) -> &'static str {
        match self {
            ForgeKind::Github => "gh",
            ForgeKind::Gitlab => "glab",
        }
    }

    pub fn host_var(self) -> &'static str {
        match self {
            ForgeKind::Github => "GH_HOST",
            ForgeKind::Gitlab => "GITLAB_HOST",
        }
    }
}

impl Forges {
    pub fn get(&self, kind: ForgeKind) -> Option<&Forge> {
        match kind {
            ForgeKind::Github => self.github.as_ref(),
            ForgeKind::Gitlab => self.gitlab.as_ref(),
        }
    }

    pub fn each(&self) -> [(ForgeKind, Option<&Forge>); 2] {
        [
            (ForgeKind::Github, self.github.as_ref()),
            (ForgeKind::Gitlab, self.gitlab.as_ref()),
        ]
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(try_from = "RawForge", into = "RawForge")]
pub struct Forge {
    pub token: TokenSource,
    pub host: Option<String>,
    /// `owners."<owner>"`: the token for every repo under an owner, or a GitLab group's full path.
    pub owners: BTreeMap<String, TokenSource>,
    /// `repos."<owner>/<repo>"`: the token for one repo.
    pub repos: BTreeMap<String, TokenSource>,
}

/// The table a forge token came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Table {
    Default,
    Owner(String),
    Repo(String),
}

impl Table {
    pub fn label(&self, kind: ForgeKind) -> String {
        let name = kind.name();
        match self {
            Table::Default => format!("[forge.{name}]"),
            Table::Owner(k) => format!("[forge.{name}.owners.{k:?}]"),
            Table::Repo(k) => format!("[forge.{name}.repos.{k:?}]"),
        }
    }
}

impl Forge {
    /// `repo`, unless `host` (the forge's effective host) is set and `origin_url` is on another one: the owner and
    /// repo tables belong to that host, so a repo elsewhere gets the default table.
    pub fn scope<'a>(
        host: Option<&str>,
        repo: Option<&'a str>,
        origin_url: Option<&str>,
    ) -> Option<&'a str> {
        let on_other_host = match (host, origin_url.and_then(crate::watch::forge::host)) {
            (Some(want), Some(got)) => !crate::watch::forge::host(want)
                .unwrap_or(want)
                .eq_ignore_ascii_case(got),
            _ => false,
        };
        repo.filter(|_| !on_other_host)
    }

    /// The token for `repo` (`owner/name`, or `group/sub/name` on GitLab) and the table it came
    /// from: the repo's table, else its owner's, else the default. Keys match without case.
    pub fn choose(&self, repo: Option<&str>) -> (&TokenSource, Table) {
        let owner = repo.and_then(|r| r.rsplit_once('/')).map(|(o, _)| o);
        if let Some((t, k)) = repo.and_then(|r| find(&self.repos, r)) {
            (t, Table::Repo(k))
        } else if let Some((t, k)) = owner.and_then(|o| find(&self.owners, o)) {
            (t, Table::Owner(k))
        } else {
            (&self.token, Table::Default)
        }
    }
}

fn find<'a>(
    tables: &'a BTreeMap<String, TokenSource>,
    key: &str,
) -> Option<(&'a TokenSource, String)> {
    tables
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(k, t)| (t, k.clone()))
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(try_from = "RawToken", into = "RawToken")]
pub enum TokenSource {
    /// A shell command whose trimmed stdout is the token.
    Command(String),
    /// The name of a variable holding the token.
    Env(String),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawToken {
    token_command: Option<String>,
    token_env: Option<String>,
}

impl TryFrom<RawToken> for TokenSource {
    type Error = String;

    fn try_from(r: RawToken) -> Result<Self, String> {
        match (r.token_command, r.token_env) {
            (Some(c), None) => Ok(TokenSource::Command(c)),
            (None, Some(e)) => Ok(TokenSource::Env(e)),
            _ => Err("set exactly one of token_command and token_env".into()),
        }
    }
}

impl From<TokenSource> for RawToken {
    fn from(t: TokenSource) -> Self {
        match t {
            TokenSource::Command(c) => RawToken {
                token_command: Some(c),
                token_env: None,
            },
            TokenSource::Env(e) => RawToken {
                token_command: None,
                token_env: Some(e),
            },
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawForge {
    token_command: Option<String>,
    token_env: Option<String>,
    host: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    owners: BTreeMap<String, TokenSource>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    repos: BTreeMap<String, TokenSource>,
}

impl TryFrom<RawForge> for Forge {
    type Error = String;

    fn try_from(r: RawForge) -> Result<Self, String> {
        let token = RawToken {
            token_command: r.token_command,
            token_env: r.token_env,
        }
        .try_into()?;
        Ok(Forge {
            token,
            host: r.host,
            owners: r.owners,
            repos: r.repos,
        })
    }
}

impl From<Forge> for RawForge {
    fn from(f: Forge) -> Self {
        let RawToken {
            token_command,
            token_env,
        } = f.token.into();
        RawForge {
            token_command,
            token_env,
            host: f.host,
            owners: f.owners,
            repos: f.repos,
        }
    }
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
    /// Transcript format: `claude-stream-json`, `codex-jsonl`, or `none` (no metrics, triggers
    /// unsupported).
    #[serde(default = "default_output")]
    pub output: String,
}

fn default_output() -> String {
    "none".into()
}

/// Output formats `ns eval` can parse, plus `none`.
pub const EVAL_OUTPUTS: &[&str] = &["claude-stream-json", "codex-jsonl", "none"];

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
        "codex" => Some(EvalHarness {
            command: owned(&[
                "codex",
                "exec",
                "--json",
                "-m",
                "{model}",
                "--sandbox",
                "danger-full-access",
                "--skip-git-repo-check",
            ]),
            carry: owned(&[".codex/auth.json"]),
            output: "codex-jsonl".into(),
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

        let h = c.eval.harness_named("codex").unwrap();
        assert_eq!(h.output, "codex-jsonl");
        assert!(EVAL_OUTPUTS.contains(&"codex-jsonl"));
        assert_eq!(
            h.command,
            [
                "codex",
                "exec",
                "--json",
                "-m",
                "{model}",
                "--sandbox",
                "danger-full-access",
                "--skip-git-repo-check"
            ]
        );
        assert_eq!(h.carry, [".codex/auth.json"]);

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
    fn forge_takes_one_token_source() {
        let c = parse(
            "[forge.github]\ntoken_command = \"pass gh\"\n[forge.gitlab]\ntoken_env = \"MY_GL\"\nhost = \"gitlab.example.com\"\n",
        )
        .unwrap();
        assert_eq!(
            c.forge.github.unwrap().token,
            TokenSource::Command("pass gh".into())
        );
        let gl = c.forge.gitlab.unwrap();
        assert_eq!(gl.token, TokenSource::Env("MY_GL".into()));
        assert_eq!(gl.host.as_deref(), Some("gitlab.example.com"));
        assert_eq!(parse("").unwrap().forge, Forges::default());

        let both = parse("[forge.github]\ntoken_command = \"x\"\ntoken_env = \"Y\"\n");
        assert!(both.unwrap_err().to_string().contains("exactly one"));
        assert!(parse("[forge.github]\n").is_err());
        assert!(parse("[forge.github]\ntoken_cmd = \"x\"\n").is_err());
        assert!(parse("[forge.bitbucket]\ntoken_env = \"X\"\n").is_err());
    }

    const OVERRIDES: &str = "[forge.github]
token_command = \"gh auth token --user personal\"
[forge.github.owners.\"example-org\"]
token_command = \"gh auth token --user work\"
[forge.github.repos.\"example-org/backend\"]
token_env = \"OTHER_TOKEN\"
";

    fn github(text: &str) -> Forge {
        parse(text).unwrap().forge.github.unwrap()
    }

    #[test]
    fn a_forge_with_no_overrides_always_picks_the_default_table() {
        let f = github("[forge.github]\ntoken_env = \"T\"\n");
        for repo in [None, Some("example-org/backend")] {
            let (token, table) = f.choose(repo);
            assert_eq!(token, &TokenSource::Env("T".into()));
            assert_eq!(table, Table::Default);
            assert_eq!(table.label(ForgeKind::Github), "[forge.github]");
        }
    }

    #[test]
    fn the_most_specific_table_wins() {
        let f = github(OVERRIDES);
        let (token, table) = f.choose(Some("example-org/frontend"));
        assert_eq!(
            token,
            &TokenSource::Command("gh auth token --user work".into())
        );
        assert_eq!(
            table.label(ForgeKind::Github),
            "[forge.github.owners.\"example-org\"]"
        );
        let (token, table) = f.choose(Some("Example-Org/Backend"));
        assert_eq!(token, &TokenSource::Env("OTHER_TOKEN".into()));
        assert_eq!(
            table.label(ForgeKind::Github),
            "[forge.github.repos.\"example-org/backend\"]"
        );
        let (_, table) = f.choose(Some("someone/backend"));
        assert_eq!(table, Table::Default);
        assert_eq!(f.choose(None).1, Table::Default);
    }

    #[test]
    fn a_gitlab_owner_is_the_full_nested_group_path() {
        let gl = parse(
            "[forge.gitlab]
token_env = \"DEFAULT\"
host = \"gitlab.example.com\"
[forge.gitlab.owners.\"team/sub\"]
token_env = \"SUB\"
[forge.gitlab.owners.\"team\"]
token_env = \"TEAM\"
",
        )
        .unwrap()
        .forge
        .gitlab
        .unwrap();
        let env = |repo| match gl.choose(Some(repo)).0 {
            TokenSource::Env(v) => v.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(env("team/sub/project"), "SUB");
        assert_eq!(env("team/project"), "TEAM");
        assert_eq!(env("team/other/project"), "DEFAULT");
        assert_eq!(
            gl.choose(Some("team/sub/project"))
                .1
                .label(ForgeKind::Gitlab),
            "[forge.gitlab.owners.\"team/sub\"]"
        );
    }

    #[test]
    fn a_forge_host_scopes_the_override_tables_to_origins_on_that_host() {
        let gl = parse(
            "[forge.gitlab]
token_env = \"DEFAULT\"
host = \"gitlab.example.com\"
[forge.gitlab.owners.\"team\"]
token_env = \"TEAM\"
",
        )
        .unwrap()
        .forge
        .gitlab
        .unwrap();
        let repo = Some("team/project");
        let host = gl.host.as_deref();
        let pick = |url| gl.choose(Forge::scope(host, repo, Some(url))).1;
        assert_eq!(
            pick("git@gitlab.example.com:team/project.git"),
            Table::Owner("team".into())
        );
        assert_eq!(
            pick("https://GitLab.Example.com/team/project"),
            Table::Owner("team".into())
        );
        assert_eq!(pick("https://gitlab.com/team/project"), Table::Default);
        assert_eq!(pick("/srv/git/project"), Table::Owner("team".into()));
        assert_eq!(
            Forge::scope(
                None,
                Some("example-org/backend"),
                Some("https://github.com/x/y")
            ),
            Some("example-org/backend")
        );
    }

    #[test]
    fn a_url_or_ported_forge_host_scopes_like_the_bare_hostname() {
        let repo = Some("team/project");
        for host in [
            "https://gitlab.example.com",
            "https://gitlab.example.com/",
            "gitlab.example.com:8443",
            "GitLab.Example.com",
        ] {
            let url = "git@gitlab.example.com:team/project.git";
            assert_eq!(Forge::scope(Some(host), repo, Some(url)), repo, "{host}");
            assert_eq!(
                Forge::scope(Some(host), repo, Some("https://gitlab.com/team/project")),
                None,
                "{host}"
            );
        }
    }

    #[test]
    fn an_override_table_takes_one_token_source_and_no_other_key() {
        for bad in [
            "[forge.github.owners.o]\ntoken_env = \"A\"\ntoken_command = \"b\"\n",
            "[forge.github.owners.o]\n",
            "[forge.github.repos.\"o/r\"]\ntoken_env = \"A\"\nhost = \"h\"\n",
            "[forge.github.repos.\"o/r\"]\ntoken_env = \"A\"\nbogus = 1\n",
        ] {
            let text = format!("[forge.github]\ntoken_env = \"T\"\n{bad}");
            assert!(parse(&text).is_err(), "{bad}");
        }
        let e = parse("[forge.github]\ntoken_env = \"T\"\n[forge.github.owners.o]\ntoken_env = \"A\"\nbogus = 1\n")
            .unwrap_err();
        assert!(e.to_string().contains("unknown field"), "{e}");
        assert_eq!(
            parse(OVERRIDES).unwrap(),
            parse(&toml::to_string(&parse(OVERRIDES).unwrap()).unwrap()).unwrap()
        );
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
