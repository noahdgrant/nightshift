//! Eval inputs: `triggers.toml`, `cases/<id>/case.toml` and `evals/fixtures/<name>/fixture.toml`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::error::SfError;
use crate::frontmatter;

pub const DEFAULT_CASE_TIMEOUT_MINUTES: u64 = 20;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggersFile {
    #[serde(default)]
    pub trigger: Vec<Trigger>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    pub prompt: String,
    pub expect: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseFile {
    #[serde(default)]
    pub description: String,
    pub fixture: String,
    /// Installed in the `with` arm. Default: the owning skill.
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    /// Installed in every arm.
    #[serde(default)]
    pub extra_skills: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub timeout_minutes: Option<u64>,
    pub prompt: String,
    #[serde(default)]
    pub setup: Setup,
    #[serde(default, rename = "check")]
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Check {
    Command {
        run: String,
        #[serde(default)]
        expect_exit: i32,
        required: Option<bool>,
    },
    FailsOnBase {
        run: String,
        required: Option<bool>,
    },
    FileExists {
        path: String,
        required: Option<bool>,
    },
    Frontmatter {
        path: String,
        key: String,
        equals: String,
        required: Option<bool>,
    },
    DiffScope {
        allow: Vec<String>,
        required: Option<bool>,
    },
    Regex {
        path: String,
        pattern: String,
        #[serde(default = "yes")]
        present: bool,
        required: Option<bool>,
    },
    Judge {
        rubric: String,
        required: Option<bool>,
    },
}

fn yes() -> bool {
    true
}

impl Check {
    pub fn kind(&self) -> &'static str {
        match self {
            Check::Command { .. } => "command",
            Check::FailsOnBase { .. } => "fails_on_base",
            Check::FileExists { .. } => "file_exists",
            Check::Frontmatter { .. } => "frontmatter",
            Check::DiffScope { .. } => "diff_scope",
            Check::Regex { .. } => "regex",
            Check::Judge { .. } => "judge",
        }
    }

    /// Checks are required by default, except `judge`.
    pub fn required(&self) -> bool {
        match self {
            Check::Command { required, .. }
            | Check::FailsOnBase { required, .. }
            | Check::FileExists { required, .. }
            | Check::Frontmatter { required, .. }
            | Check::DiffScope { required, .. }
            | Check::Regex { required, .. } => required.unwrap_or(true),
            Check::Judge { required, .. } => required.unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureFile {
    /// Documentation only.
    #[serde(default)]
    #[allow(dead_code)]
    pub description: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Case {
    pub id: String,
    pub dir: PathBuf,
    pub file: CaseFile,
}

impl Case {
    pub fn skills(&self, owner: &str) -> Vec<String> {
        self.file
            .skills
            .clone()
            .unwrap_or_else(|| vec![owner.to_string()])
    }

    pub fn timeout_secs(&self) -> u64 {
        self.file
            .timeout_minutes
            .unwrap_or(DEFAULT_CASE_TIMEOUT_MINUTES)
            * 60
    }
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub dir: PathBuf,
    /// `disable-model-invocation: true` in SKILL.md: trigger evals don't apply.
    pub user_invoked: bool,
    pub triggers: Vec<Trigger>,
    pub cases: Vec<Case>,
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    toml::from_str(&text).map_err(|e| {
        SfError::general(format!("cannot parse {}: {e}", path.display()))
            .hint("the format is in docs/EVALS.md; check with:\n  ns eval --dry-run")
            .into()
    })
}

/// Skill directories under `skills_dir` that have an `evals/` directory, sorted by name.
pub fn skills_with_evals(skills_dir: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(skills_dir)
        .with_context(|| format!("cannot read {}", skills_dir.display()))?
        .flatten()
        .filter(|e| e.path().join("SKILL.md").is_file() && e.path().join("evals").is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

/// Load one skill's evals. A skill without `evals/` loads with no triggers and no cases.
pub fn load_skill(skills_dir: &Path, name: &str) -> Result<Skill> {
    let dir = skills_dir.join(name);
    let skill_md = fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
    let user_invoked = frontmatter::parse(&skill_md)
        .ok()
        .flatten()
        .and_then(|fm| fm.get("disable-model-invocation").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    let evals = dir.join("evals");
    let triggers_path = evals.join("triggers.toml");
    let triggers = if triggers_path.is_file() {
        read_toml::<TriggersFile>(&triggers_path)?.trigger
    } else {
        Vec::new()
    };
    let mut cases = Vec::new();
    let cases_dir = evals.join("cases");
    if cases_dir.is_dir() {
        let mut dirs: Vec<PathBuf> = fs::read_dir(&cases_dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("case.toml").is_file())
            .collect();
        dirs.sort();
        for d in dirs {
            let file: CaseFile = read_toml(&d.join("case.toml"))?;
            cases.push(Case {
                id: d.file_name().unwrap().to_string_lossy().into_owned(),
                dir: d,
                file,
            });
        }
    }
    Ok(Skill {
        name: name.to_string(),
        dir,
        user_invoked,
        triggers,
        cases,
    })
}

/// Resolve `evals/fixtures/<name>`, refusing anything that lands outside the fixtures dir.
pub fn fixture_dir(fixtures_root: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() {
        return Err("fixture name is empty".into());
    }
    let root = fixtures_root
        .canonicalize()
        .map_err(|_| format!("fixtures dir {} does not exist", fixtures_root.display()))?;
    let dir = root
        .join(name)
        .canonicalize()
        .map_err(|_| format!("fixture {name:?} not found under {}", root.display()))?;
    if dir == root || !dir.starts_with(&root) {
        return Err(format!(
            "fixture {name:?} resolves to {}, outside {}",
            dir.display(),
            root.display()
        ));
    }
    if !dir.is_dir() {
        return Err(format!("fixture {name:?} is not a directory"));
    }
    Ok(dir)
}

pub fn load_fixture(dir: &Path) -> Result<FixtureFile> {
    let p = dir.join("fixture.toml");
    if p.is_file() {
        read_toml(&p)
    } else {
        Ok(FixtureFile::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_case_with_every_check() {
        let c: CaseFile = toml::from_str(
            r#"
fixture = "py"
prompt = "go"
[setup]
commands = ["true"]
[[check]]
type = "command"
run = "pytest"
[[check]]
type = "fails_on_base"
run = "pytest {changed_tests}"
[[check]]
type = "file_exists"
path = "a"
[[check]]
type = "frontmatter"
path = "a"
key = "status"
equals = "pass"
[[check]]
type = "diff_scope"
allow = ["src/**"]
[[check]]
type = "regex"
path = "a"
pattern = "x"
present = false
[[check]]
type = "judge"
rubric = "ok?"
"#,
        )
        .unwrap();
        assert_eq!(c.checks.len(), 7);
        assert!(c.checks[0].required());
        assert!(!c.checks[6].required());
        assert_eq!(
            c.checks[0],
            Check::Command {
                run: "pytest".into(),
                expect_exit: 0,
                required: None
            }
        );
    }

    #[test]
    fn rejects_unknown_keys_and_types() {
        let bad = [
            "fixture = \"x\"\nprompt = \"p\"\nfixtures = 1\n",
            "fixture = \"x\"\nprompt = \"p\"\n[[check]]\ntype = \"command\"\nrun = \"x\"\nexpect = 0\n",
            "fixture = \"x\"\nprompt = \"p\"\n[[check]]\ntype = \"nope\"\n",
        ];
        for b in bad {
            assert!(toml::from_str::<CaseFile>(b).is_err(), "{b}");
        }
    }

    #[test]
    fn fixture_paths_stay_inside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("evals/fixtures");
        fs::create_dir_all(root.join("ok")).unwrap();
        fs::create_dir_all(tmp.path().join("secret")).unwrap();
        assert!(fixture_dir(&root, "ok").is_ok());
        assert!(fixture_dir(&root, "../../secret").is_err());
        assert!(fixture_dir(&root, "/etc").is_err());
        assert!(fixture_dir(&root, ".").is_err());
        assert!(fixture_dir(&root, "missing").is_err());
    }
}
