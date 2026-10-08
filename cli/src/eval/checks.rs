//! Deterministic graders and the LLM judge, run in the scratch repo after the harness exits.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::spec::Check;
use super::trial::{self, Scratch};

const CHECK_TIMEOUT: Duration = Duration::from_secs(600);
const JUDGE_DIFF_LIMIT: usize = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckResult {
    #[serde(rename = "type")]
    pub kind: String,
    pub required: bool,
    pub passed: bool,
    pub detail: String,
}

/// What the trial changed relative to its start commit, committed or not.
pub struct Changes {
    /// Every changed path, deletions included.
    pub all: Vec<String>,
    /// Added or modified test files that still exist.
    pub tests: Vec<String>,
}

pub fn changes(scratch: &Scratch) -> Result<Changes> {
    let start = scratch
        .start
        .as_deref()
        .context("trial has no start commit")?;
    trial::git(&scratch.repo, &["add", "-A"])?;
    let lines = |s: String| -> Vec<String> {
        s.lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect()
    };
    let all = lines(trial::git(
        &scratch.repo,
        &["diff", "--cached", "--name-only", "--no-renames", start],
    )?);
    let tests = lines(trial::git(
        &scratch.repo,
        &[
            "diff",
            "--cached",
            "--name-only",
            "--no-renames",
            "--diff-filter=AM",
            start,
        ],
    )?)
    .into_iter()
    .filter(|p| is_test_path(p))
    .collect();
    Ok(Changes { all, tests })
}

/// Test files by common conventions: a `test`/`tests`/`spec` directory, or a
/// `test_*`, `*_test.*`, `*.test.*`, `*_spec.*`, `*.spec.*`, `*Test.*` file name.
pub fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let (file, dirs) = parts.split_last().unwrap_or((&"", &[]));
    if dirs
        .iter()
        .any(|d| matches!(*d, "test" | "tests" | "__tests__" | "spec" | "specs"))
    {
        return true;
    }
    let stem = file.split('.').next().unwrap_or("");
    stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
        || stem.ends_with("Test")
        || stem.ends_with("Tests")
        || file.contains(".test.")
        || file.contains(".spec.")
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn expand(run: &str, changes: &Changes) -> String {
    let tests: Vec<String> = changes.tests.iter().map(|t| shell_quote(t)).collect();
    run.replace("{changed_tests}", &tests.join(" "))
}

fn glob_match(pattern: &glob::Pattern, path: &str) -> bool {
    pattern.matches_with(
        path,
        glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        },
    )
}

pub struct Ctx<'a> {
    pub scratch: &'a Scratch,
    pub prompt: &'a str,
    /// The `ns` binary, for the judge (`ns ask --role eval.judge`).
    pub ns_exe: &'a Path,
}

pub fn run_all(checks: &[Check], ctx: &Ctx<'_>) -> Result<Vec<CheckResult>> {
    let changes = changes(ctx.scratch)?;
    let mut out = Vec::new();
    for (i, c) in checks.iter().enumerate() {
        let (passed, detail) = run_one(c, i, &changes, ctx)?;
        out.push(CheckResult {
            kind: c.kind().to_string(),
            required: c.required(),
            passed,
            detail,
        });
    }
    Ok(out)
}

fn read_in_repo(repo: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(repo.join(rel)).ok()
}

fn run_one(c: &Check, i: usize, changes: &Changes, ctx: &Ctx<'_>) -> Result<(bool, String)> {
    let s = ctx.scratch;
    let log = s.root().join("logs").join(format!("check-{i}.log"));
    Ok(match c {
        Check::Command {
            run, expect_exit, ..
        } => {
            let cmd = expand(run, changes);
            let r = trial::run_sh(&cmd, s, &s.repo, CHECK_TIMEOUT, &log)?;
            let ok = !r.timed_out && r.exit == Some(*expect_exit);
            let detail = format!("`{cmd}` {} (expected {expect_exit})", r.describe());
            (
                ok,
                if ok {
                    detail
                } else {
                    format!("{detail}: {}", r.tail)
                },
            )
        }
        Check::FailsOnBase { run, .. } => fails_on_base(run, i, changes, s, &log)?,
        Check::FileExists { path, .. } => {
            let ok = s.repo.join(path).exists();
            (
                ok,
                format!("{path} {}", if ok { "exists" } else { "is missing" }),
            )
        }
        Check::Frontmatter {
            path, key, equals, ..
        } => match read_in_repo(&s.repo, path) {
            None => (false, format!("{path} is missing")),
            Some(text) => match crate::frontmatter::parse(&text) {
                Ok(Some(fm)) => {
                    let got = fm.get(key.as_str()).and_then(crate::worktree::yaml_scalar);
                    let ok = got.as_deref() == Some(equals.as_str());
                    (ok, format!("{path}: {key} = {got:?}, want {equals:?}"))
                }
                Ok(None) => (false, format!("{path} has no frontmatter")),
                Err(e) => (false, format!("{path}: frontmatter is not valid YAML: {e}")),
            },
        },
        Check::DiffScope { allow, .. } => {
            let mut patterns = Vec::new();
            for a in allow {
                match glob::Pattern::new(a) {
                    Ok(p) => patterns.push(p),
                    Err(e) => return Ok((false, format!("bad glob {a:?}: {e}"))),
                }
            }
            let outside: Vec<&String> = changes
                .all
                .iter()
                .filter(|p| !patterns.iter().any(|g| glob_match(g, p)))
                .collect();
            if outside.is_empty() {
                (
                    true,
                    format!("{} changed paths, all in scope", changes.all.len()),
                )
            } else {
                (
                    false,
                    format!(
                        "out of scope: {}",
                        outside
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            }
        }
        Check::Regex {
            path,
            pattern,
            present,
            ..
        } => {
            let re = match regex::Regex::new(pattern) {
                Ok(r) => r,
                Err(e) => return Ok((false, format!("bad pattern {pattern:?}: {e}"))),
            };
            let found = read_in_repo(&s.repo, path).is_some_and(|t| re.is_match(&t));
            let ok = found == *present;
            (
                ok,
                format!(
                    "/{pattern}/ {} in {path} (want {})",
                    if found { "found" } else { "not found" },
                    if *present { "present" } else { "absent" }
                ),
            )
        }
        Check::Judge { rubric, .. } => judge(rubric, ctx)?,
    })
}

fn fails_on_base(
    run: &str,
    i: usize,
    changes: &Changes,
    s: &Scratch,
    log: &Path,
) -> Result<(bool, String)> {
    if changes.tests.is_empty() {
        return Ok((false, "the trial added or changed no test files".into()));
    }
    let start = s.start.as_deref().context("trial has no start commit")?;
    let base = s.root().join(format!("base-{i}"));
    let base_str = base.to_string_lossy().into_owned();
    trial::git(
        &s.repo,
        &["worktree", "add", "-q", "--detach", &base_str, start],
    )?;
    for t in &changes.tests {
        let to = base.join(t);
        if let Some(p) = to.parent() {
            fs::create_dir_all(p)?;
        }
        fs::copy(s.repo.join(t), &to)?;
    }
    let cmd = expand(run, changes);
    let r = trial::run_sh(&cmd, s, &base, CHECK_TIMEOUT, log)?;
    let _ = trial::git(&s.repo, &["worktree", "remove", "--force", &base_str]);
    let ok = !r.timed_out && r.exit.is_some_and(|c| c != 0);
    let detail = format!("`{cmd}` on the base commit {}", r.describe());
    Ok((
        ok,
        if ok {
            format!("{detail}: the new tests go red on the bug")
        } else {
            format!("{detail}: the new tests do not fail on the base commit")
        },
    ))
}

/// `pass`/`fail` from the judge's first non-empty line, ignoring case and markdown.
pub fn parse_verdict(text: &str) -> Option<bool> {
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    let word: String = first
        .trim()
        .trim_start_matches(|c: char| !c.is_ascii_alphabetic())
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase();
    match word.as_str() {
        "pass" => Some(true),
        "fail" => Some(false),
        _ => None,
    }
}

fn judge(rubric: &str, ctx: &Ctx<'_>) -> Result<(bool, String)> {
    let s = ctx.scratch;
    let start = s.start.as_deref().context("trial has no start commit")?;
    let mut diff = trial::git(&s.repo, &["diff", "--cached", start])?;
    if diff.len() > JUDGE_DIFF_LIMIT {
        let mut cut = JUDGE_DIFF_LIMIT;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff.truncate(cut);
        diff.push_str("\n[diff truncated]\n");
    }
    let prompt = format!(
        "You are grading an AI agent's change against a rubric.\n\n\
         Answer `pass` or `fail` alone on the first line, then your reasoning.\n\n\
         ## Rubric\n\n{rubric}\n\n## Task the agent was given\n\n{}\n\n## The agent's diff\n\n```diff\n{diff}\n```\n",
        ctx.prompt
    );
    let out_path = s.root().join("logs").join("judge.out");
    let out = fs::File::create(&out_path)?;
    let mut cmd = Command::new(ctx.ns_exe);
    cmd.args(["ask", "--role", "eval.judge", "--cwd"])
        .arg(&s.repo)
        .stdout(out)
        .stderr(Stdio::null());
    for (k, v) in &s.env {
        cmd.env(k, v);
    }
    let (status, timed_out, _) = trial::run_process(cmd, Some(prompt.into_bytes()), CHECK_TIMEOUT)
        .context("cannot run ns ask for the judge")?;
    let text = fs::read_to_string(&out_path).unwrap_or_default();
    if timed_out || !status.is_some_and(|st| st.success()) {
        return Ok((
            false,
            "judge did not answer: `ns ask --role eval.judge` failed; check with `ns ask --role eval.judge --dry-run`".into(),
        ));
    }
    let reasoning: String = text.trim().chars().take(1000).collect();
    Ok(match parse_verdict(&text) {
        Some(v) => (v, reasoning),
        None => (
            false,
            format!("judge verdict unreadable (want pass or fail on the first line): {reasoning}"),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths() {
        for p in [
            "tests/test_core.py",
            "src/foo_test.go",
            "a/b.test.ts",
            "spec/x_spec.rb",
            "test_a.py",
            "src/FooTest.java",
            "zephyr/tests/unit/main.c",
        ] {
            assert!(is_test_path(p), "{p}");
        }
        for p in ["src/core.py", "README.md", "src/contest.py", "latest.txt"] {
            assert!(!is_test_path(p), "{p}");
        }
    }

    #[test]
    fn verdicts() {
        assert_eq!(parse_verdict("pass\nbecause"), Some(true));
        assert_eq!(parse_verdict("\n**FAIL**: no"), Some(false));
        assert_eq!(parse_verdict("Pass."), Some(true));
        assert_eq!(parse_verdict("passable"), None);
        assert_eq!(parse_verdict("I think it passes"), None);
        assert_eq!(parse_verdict(""), None);
    }

    #[test]
    fn globs_and_quoting() {
        let g = glob::Pattern::new("src/inventory/**").unwrap();
        assert!(glob_match(&g, "src/inventory/core.py"));
        assert!(glob_match(&g, "src/inventory/a/b.py"));
        assert!(!glob_match(&g, "src/other.py"));
        let g = glob::Pattern::new("*.md").unwrap();
        assert!(glob_match(&g, "README.md"));
        assert!(!glob_match(&g, "docs/a.md"));
        assert_eq!(shell_quote("tests/a.py"), "tests/a.py");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
