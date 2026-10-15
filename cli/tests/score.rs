//! End-to-end tests for `ns score` with a fake judge harness and a fake `gh`.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use common::Ns;
use serde_json::Value;
use tempfile::TempDir;

const UNIT: &str = "7-fix-the-thing";

const SCORER: &str = "---
role: review.scorer
passing_score: 1
sampling_rate: 0.25
labels:
  - value: pass
    description: every criterion has evidence
    score: 1
  - value: partial
    score: 0.5
  - value: fail
    score: 0
---
Grade evidence.md against brief.md.
";

fn git(dir: &Path, args: &[&str]) {
    let out = StdCommand::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn script(path: &Path, body: &str) {
    common::write_exe(path, format!("#!/bin/sh\n{body}\n"));
}

struct Env {
    _tmp: TempDir,
    base: PathBuf,
    root: PathBuf,
    bin: PathBuf,
}

impl Env {
    /// A repo with one scorer, unit 7's artifacts (PR #12), a judge that answers `label`, and a
    /// `gh` that reports PR #12 merged with two reviews.
    fn new(label: &str) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("myrepo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        fs::write(root.join("README"), "hi\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        fs::create_dir_all(root.join(".nightshift/scorers")).unwrap();
        fs::write(root.join(".nightshift/scorers/evidence.md"), SCORER).unwrap();
        let arts = root.join(".ns").join(UNIT);
        fs::create_dir_all(&arts).unwrap();
        fs::write(arts.join("brief.md"), "the brief: AC1 works\n").unwrap();
        fs::write(arts.join("evidence.md"), "AC1: proved by run 1\n").unwrap();
        fs::write(
            arts.join("pr.md"),
            "---\nstatus: pass\npr: 12\n---\nopened\n",
        )
        .unwrap();
        let bin = base.join("bin");
        fs::create_dir(&bin).unwrap();
        script(
            &bin.join("judge"),
            &format!(
                "cat > \"$FAKE_DIR/prompt\"\nprintf 'Evidence covers AC1.\\nlabel: {label}\\n'"
            ),
        );
        script(
            &bin.join("gh"),
            "echo \"$@\" >> \"$FAKE_DIR/gh-calls\"\ncat \"$FAKE_DIR/pr.json\"",
        );
        fs::write(
            base.join("pr.json"),
            r#"{"state":"MERGED","mergeCommit":{"oid":"abc1234"},"reviews":[{},{}]}"#,
        )
        .unwrap();
        fs::write(
            base.join("config.toml"),
            "[harness.judge]\ncommand = [\"judge\"]\n\n[roles.\"review.scorer\"]\nharness = \"judge\"\n",
        )
        .unwrap();
        Env {
            _tmp: tmp,
            base,
            root,
            bin,
        }
    }

    fn ns(&self) -> Ns {
        let mut c = common::ns();
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        c.current_dir(&self.root)
            .env("PATH", path)
            .env("NS_CONFIG", self.base.join("config.toml"))
            .env("FAKE_DIR", &self.base)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("GH_TOKEN");
        c
    }

    fn run(&self, args: &[&str], code: i32) -> (Value, String) {
        let out = self
            .ns()
            .args(args)
            .assert()
            .code(code)
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let v = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (v, stderr)
    }

    fn scorer(&self, name: &str, text: &str) {
        fs::write(
            self.root
                .join(".nightshift/scorers")
                .join(format!("{name}.md")),
            text,
        )
        .unwrap();
    }

    fn ledger(&self) -> String {
        fs::read_to_string(self.root.join(".git/ns/scores.jsonl")).unwrap_or_default()
    }

    fn prompt(&self) -> String {
        fs::read_to_string(self.base.join("prompt")).unwrap_or_default()
    }
}

#[test]
fn a_passing_label_is_scored_and_appended_to_the_ledger() {
    let e = Env::new("pass");
    let (v, _) = e.run(&["score", UNIT], 0);
    let line = serde_json::json!({"unit": UNIT, "scorer": "evidence", "label": "pass", "score": 1.0, "pass": true});
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["scores"], serde_json::json!([line]));
    assert_eq!(
        v["ledger"].as_str().unwrap(),
        e.root.join(".git/ns/scores.jsonl").to_str().unwrap()
    );
    assert_eq!(e.ledger(), format!("{line}\n"));
    let p = e.prompt();
    for want in [
        "Grade evidence.md against brief.md.",
        "the brief: AC1 works",
        "AC1: proved by run 1",
        "\"state\":\"MERGED\"",
        "\"review_rounds\":2",
        "\"reverted\":false",
        "label: <value>",
        "- partial (score 0.5)",
    ] {
        assert!(p.contains(want), "{want:?} not in prompt:\n{p}");
    }
}

#[test]
fn a_label_below_the_passing_score_fails_and_lines_append() {
    let e = Env::new("partial");
    let (v, _) = e.run(&["score", UNIT], 0);
    assert_eq!(v["scores"][0]["label"], "partial");
    assert_eq!(v["scores"][0]["score"], 0.5);
    assert_eq!(v["scores"][0]["pass"], false);
    e.run(&["score", UNIT], 0);
    assert_eq!(e.ledger().lines().count(), 2, "{}", e.ledger());
}

#[test]
fn an_unknown_label_is_an_error_and_appends_nothing() {
    let e = Env::new("excellent");
    let (v, err) = e.run(&["score", UNIT], 1);
    assert_eq!(v, Value::Null);
    assert!(err.contains("names no known label"), "{err}");
    assert!(err.contains("pass, partial, fail"), "{err}");
    assert_eq!(e.ledger(), "");
}

#[test]
fn dry_run_prints_the_prompt_and_writes_nothing() {
    let e = Env::new("pass");
    let (v, _) = e.run(&["score", UNIT, "--dry-run"], 0);
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["prompts"][0]["scorer"], "evidence");
    assert_eq!(v["prompts"][0]["role"], "review.scorer");
    let p = v["prompts"][0]["prompt"].as_str().unwrap();
    assert!(p.contains("AC1: proved by run 1"), "{p}");
    assert_eq!(e.prompt(), "", "the judge ran");
    assert_eq!(e.ledger(), "");
    assert!(!e.root.join(".git/ns").exists());
}

#[test]
fn a_commit_on_the_default_branch_that_reverts_the_merge_marks_it_reverted() {
    let e = Env::new("fail");
    e.run(&["score", UNIT, "--dry-run"], 0);
    git(
        &e.root,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Revert \"feat: x (#12)\"\n\nThis reverts commit abc1234.",
        ],
    );
    let (v, _) = e.run(&["score", UNIT, "--dry-run"], 0);
    let p = v["prompts"][0]["prompt"].as_str().unwrap();
    assert!(p.contains("\"reverted\":true"), "{p}");
    assert!(p.contains("\"state\":\"MERGED\""), "{p}");
}

#[test]
fn a_merge_commit_that_is_not_a_sha_is_not_looked_up() {
    let e = Env::new("fail");
    fs::write(
        e.base.join("pr.json"),
        r#"{"state":"OPEN","mergeCommit":null,"reviews":[]}"#,
    )
    .unwrap();
    let (v, _) = e.run(&["score", UNIT, "--dry-run"], 0);
    let p = v["prompts"][0]["prompt"].as_str().unwrap();
    assert!(p.contains("\"reverted\":false"), "{p}");
    assert!(p.contains("\"review_rounds\":0"), "{p}");
    assert!(p.contains("\"state\":\"OPEN\""), "{p}");
    git(
        &e.root,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Revert\n\nThis reverts commit not-a-sha.",
        ],
    );
    fs::write(
        e.base.join("pr.json"),
        r#"{"state":"MERGED","mergeCommit":{"oid":"not-a-sha"},"reviews":[]}"#,
    )
    .unwrap();
    let (v, _) = e.run(&["score", UNIT, "--dry-run"], 0);
    let p = v["prompts"][0]["prompt"].as_str().unwrap();
    assert!(p.contains("\"reverted\":false"), "{p}");
}

#[test]
fn a_unit_without_a_pr_is_graded_without_calling_gh() {
    let e = Env::new("fail");
    fs::remove_file(e.root.join(".ns").join(UNIT).join("pr.md")).unwrap();
    let (v, _) = e.run(&["score", UNIT], 0);
    assert_eq!(v["scores"][0]["pass"], false);
    assert!(e.prompt().contains("{\"pr\":null}"), "{}", e.prompt());
    assert!(!e.base.join("gh-calls").exists());
}

#[test]
fn gh_failing_is_an_error() {
    let e = Env::new("pass");
    fs::remove_file(e.base.join("pr.json")).unwrap();
    let (_, err) = e.run(&["score", UNIT], 1);
    assert!(err.contains("gh pr view 12"), "{err}");
    assert_eq!(e.ledger(), "");
}

#[test]
fn the_unit_worktree_artifacts_win_over_the_main_checkout() {
    let e = Env::new("pass");
    let wt = e.base.join("myrepo.worktrees").join(UNIT);
    git(
        &e.root,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            &format!("ns/{UNIT}"),
        ],
    );
    fs::create_dir_all(wt.join(".ns").join(UNIT)).unwrap();
    fs::write(
        wt.join(".ns").join(UNIT).join("evidence.md"),
        "from the worktree\n",
    )
    .unwrap();
    e.run(&["score", UNIT], 0);
    assert!(e.prompt().contains("from the worktree"), "{}", e.prompt());
    assert!(!e.prompt().contains("AC1: proved"), "{}", e.prompt());
}

#[test]
fn a_unit_with_no_artifacts_is_an_error() {
    let e = Env::new("pass");
    let (_, err) = e.run(&["score", "8-other"], 1);
    assert!(err.contains("no artifacts for unit 8-other"), "{err}");
    assert!(err.contains("ns worktree list"), "{err}");
    let (_, err) = e.run(&["score", "Bad_Id"], 2);
    assert!(err.contains("invalid unit id"), "{err}");
}

#[test]
fn every_scorer_runs_unless_one_is_named() {
    let e = Env::new("pass");
    e.scorer(
        "other",
        &SCORER.replace("Grade evidence.md", "Grade review.md"),
    );
    let (v, _) = e.run(&["score", UNIT], 0);
    let names: Vec<_> = v["scores"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["scorer"].clone())
        .collect();
    assert_eq!(names, ["evidence", "other"]);
    let (v, _) = e.run(&["score", UNIT, "--scorer", "other", "--dry-run"], 0);
    assert_eq!(v["prompts"].as_array().unwrap().len(), 1);
    assert_eq!(v["prompts"][0]["scorer"], "other");
    let (_, err) = e.run(&["score", UNIT, "--scorer", "nope"], 2);
    assert!(err.contains("no scorer \"nope\""), "{err}");
    assert!(err.contains("scorers: evidence, other"), "{err}");
    assert_eq!(e.ledger().lines().count(), 2);
}

#[test]
fn one_unknown_label_among_several_scorers_appends_nothing() {
    let e = Env::new("pass");
    e.scorer("other", &SCORER.replace("value: pass", "value: good"));
    let (_, err) = e.run(&["score", UNIT], 1);
    assert!(err.contains("scorer \"other\""), "{err}");
    assert_eq!(e.ledger(), "");
}

#[test]
fn a_failing_judge_is_an_error_and_appends_nothing() {
    let e = Env::new("pass");
    script(&e.bin.join("judge"), "cat >/dev/null\nexit 3");
    let (_, err) = e.run(&["score", UNIT], 1);
    assert!(err.contains("exited with 3"), "{err}");
    assert_eq!(e.ledger(), "");
}

#[test]
fn invalid_or_missing_scorers_are_errors() {
    let e = Env::new("pass");
    e.scorer("bad", "no frontmatter\n");
    let (_, err) = e.run(&["score", UNIT], 1);
    assert!(err.contains("invalid scorers"), "{err}");
    assert!(err.contains("scorers/bad.md"), "{err}");
    assert!(err.contains("ns factory validate"), "{err}");
    fs::remove_dir_all(e.root.join(".nightshift/scorers")).unwrap();
    let (_, err) = e.run(&["score", UNIT], 1);
    assert!(err.contains("no scorers in"), "{err}");
    assert_eq!(e.ledger(), "");
}

#[test]
fn run_from_a_unit_worktree_the_ledger_is_in_the_common_git_dir() {
    let e = Env::new("pass");
    let wt = e.base.join("myrepo.worktrees").join(UNIT);
    git(
        &e.root,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            &format!("ns/{UNIT}"),
        ],
    );
    let out = e
        .ns()
        .current_dir(&wt)
        .args(["score", UNIT])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["ledger"].as_str().unwrap(),
        e.root.join(".git/ns/scores.jsonl").to_str().unwrap()
    );
    assert_eq!(e.ledger().lines().count(), 1, "{}", e.ledger());
    assert!(!e.root.join(".git/worktrees").join(UNIT).join("ns").exists());
    assert!(!wt.join(".git/ns").exists());
}

#[test]
fn a_ledger_that_cannot_be_written_is_an_error() {
    let e = Env::new("pass");
    fs::write(e.root.join(".git/ns"), "in the way").unwrap();
    let out = e
        .ns()
        .args(["score", UNIT])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("cannot create"), "{err}");
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
}

#[test]
fn a_ledger_file_that_cannot_be_appended_to_is_an_error() {
    let e = Env::new("pass");
    fs::create_dir_all(e.root.join(".git/ns/scores.jsonl")).unwrap();
    let out = e
        .ns()
        .args(["score", UNIT])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("cannot append to"), "{err}");
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
}

#[test]
fn factory_selects_the_directory_the_scorers_come_from() {
    let e = Env::new("pass");
    fs::remove_dir_all(e.root.join(".nightshift/scorers")).unwrap();
    let other = e.base.join("other-factory");
    fs::create_dir_all(other.join("scorers")).unwrap();
    fs::write(
        other.join("scorers/custom.md"),
        SCORER.replace("Grade evidence.md", "Grade custom things"),
    )
    .unwrap();
    let (_, err) = e.run(&["score", UNIT], 1);
    assert!(err.contains("no scorers in"), "{err}");
    let (v, _) = e.run(&["score", UNIT, "--factory", other.to_str().unwrap()], 0);
    assert_eq!(v["scores"][0]["scorer"], "custom");
    assert!(e.prompt().contains("Grade custom things"), "{}", e.prompt());
    assert_eq!(e.ledger().lines().count(), 1);
}

#[test]
fn a_malformed_user_config_is_reported_not_swallowed() {
    let e = Env::new("pass");
    fs::write(e.base.join("config.toml"), "this is [not toml").unwrap();
    let out = e
        .ns()
        .args(["score", UNIT])
        .assert()
        .code(3)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("config.toml"), "{err}");
    assert!(!err.contains("role not configured"), "{err}");
    assert_eq!(e.ledger(), "");
}

#[test]
fn a_revert_of_a_different_commit_does_not_mark_the_merge_reverted() {
    let e = Env::new("fail");
    git(
        &e.root,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Revert \"feat: y\"\n\nThis reverts commit deadbee.",
        ],
    );
    let (v, _) = e.run(&["score", UNIT, "--dry-run"], 0);
    let p = v["prompts"][0]["prompt"].as_str().unwrap();
    assert!(p.contains("\"reverted\":false"), "{p}");
}

#[test]
fn a_role_that_is_not_configured_is_an_invalid_scorer_and_appends_nothing() {
    let e = Env::new("pass");
    fs::write(
        e.base.join("config.toml"),
        "[harness.judge]\ncommand = [\"judge\"]\n",
    )
    .unwrap();
    let (_, err) = e.run(&["score", UNIT], 1);
    assert!(err.contains("invalid scorers"), "{err}");
    assert!(err.contains("review.scorer"), "{err}");
    assert_eq!(e.ledger(), "");
    assert_eq!(e.prompt(), "", "the judge ran");
}

#[test]
fn a_harness_binary_that_is_not_on_path_exits_4_and_appends_nothing() {
    let e = Env::new("pass");
    fs::write(
        e.base.join("config.toml"),
        "[harness.judge]\ncommand = [\"no-such-judge-binary\"]\n\n[roles.\"review.scorer\"]\nharness = \"judge\"\n",
    )
    .unwrap();
    let (_, err) = e.run(&["score", UNIT], 4);
    assert!(err.contains("no-such-judge-binary"), "{err}");
    assert_eq!(e.ledger(), "");
}

#[test]
fn a_judge_that_replies_at_length_before_reading_still_gets_the_whole_prompt() {
    let e = Env::new("pass");
    script(
        &e.bin.join("judge"),
        "head -c 200000 /dev/zero | tr '\\0' 'x'\necho\ncat > \"$FAKE_DIR/prompt\"\nprintf 'label: pass\\n'",
    );
    let big = format!("{}\nTAIL-MARKER\n", "y".repeat(200 * 1024));
    fs::write(e.root.join(".ns").join(UNIT).join("big.md"), &big).unwrap();
    let (v, _) = e.run(&["score", UNIT], 0);
    assert_eq!(v["scores"][0]["label"], "pass", "{v}");
    let p = e.prompt();
    assert!(p.len() > 200 * 1024, "{}", p.len());
    assert!(p.contains(&big), "the artifact arrived truncated");
    assert!(p.contains("label: <value>"), "the prompt tail is missing");
}

fn unit_worktree(e: &Env) -> PathBuf {
    let wt = e.base.join("myrepo.worktrees").join(UNIT);
    git(
        &e.root,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            &format!("ns/{UNIT}"),
        ],
    );
    wt
}

fn dry_prompt_from(e: &Env, dir: &Path) -> String {
    let out = e
        .ns()
        .current_dir(dir)
        .args(["score", UNIT, "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["prompts"][0]["prompt"].as_str().unwrap().to_string()
}

const REVERT_MSG: &str = "Revert \"feat: x (#12)\"\n\nThis reverts commit abc1234.";

fn with_origin_main(e: &Env) {
    let origin = e.base.join("origin.git");
    git(&e.base, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git(
        &e.root,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&e.root, &["push", "-q", "origin", "main"]);
    git(&e.root, &["fetch", "-q", "origin"]);
    git(&e.root, &["remote", "set-head", "origin", "main"]);
}

#[test]
fn a_revert_on_the_default_branch_counts_whatever_the_main_checkout_has_out() {
    let e = Env::new("fail");
    with_origin_main(&e);
    let wt = unit_worktree(&e);
    git(
        &e.root,
        &["commit", "-q", "--allow-empty", "-m", REVERT_MSG],
    );
    git(&e.root, &["checkout", "-q", "-b", "side", "HEAD~1"]);
    let p = dry_prompt_from(&e, &wt);
    assert!(p.contains("\"reverted\":true"), "{p}");
}

#[test]
fn a_revert_only_on_another_branch_is_not_a_revert_on_the_default_branch() {
    let e = Env::new("fail");
    with_origin_main(&e);
    let wt = unit_worktree(&e);
    git(&e.root, &["checkout", "-q", "-b", "side"]);
    git(
        &e.root,
        &["commit", "-q", "--allow-empty", "-m", REVERT_MSG],
    );
    let p = dry_prompt_from(&e, &wt);
    assert!(p.contains("\"reverted\":false"), "{p}");
}

const API_ENV: &[&str] = &["ANTHROPIC_API_KEY", "OPENAI_API_KEY"];

fn judge_env_seen(e: &Env, billing: &str) -> String {
    script(
        &e.bin.join("claude"),
        "cat >/dev/null\nenv > \"$FAKE_DIR/judge-env\"\nprintf 'label: pass\\n'",
    );
    fs::write(
        e.base.join("config.toml"),
        "[harness.judge]\ncommand = [\"claude\"]\n\n[roles.\"review.scorer\"]\nharness = \"judge\"\n",
    )
    .unwrap();
    fs::write(
        e.root.join(".nightshift/nightshift.toml"),
        format!("[defaults]\nbilling = \"{billing}\"\n"),
    )
    .unwrap();
    let mut c = e.ns();
    for k in API_ENV {
        c.env(k, "x");
    }
    c.env("CLAUDE_CODE_OAUTH_TOKEN", "login");
    c.args(["score", UNIT]).assert().code(0);
    fs::read_to_string(e.base.join("judge-env")).unwrap()
}

fn has_var(env: &str, k: &str) -> bool {
    env.lines().any(|l| l.starts_with(&format!("{k}=")))
}

#[test]
fn the_judge_gets_no_api_keys_under_subscription_billing() {
    let e = Env::new("pass");
    let env = judge_env_seen(&e, "subscription");
    for k in API_ENV {
        assert_eq!(has_var(&env, k), *k == "OPENAI_API_KEY", "{k}");
    }
}

#[test]
fn the_judge_keeps_api_keys_under_api_billing() {
    let e = Env::new("pass");
    let env = judge_env_seen(&e, "api");
    for k in API_ENV {
        assert!(has_var(&env, k), "{k}");
    }
}

/// On GitLab the outcome is the merge request's: its squash commit when it has no merge commit,
/// and its approvals as its reviews, read with `glab`, never `gh`.
#[test]
fn on_gitlab_the_outcome_is_read_from_the_merge_request_and_its_approvals() {
    let e = Env::new("pass");
    fs::write(
        e.root.join(".nightshift/nightshift.toml"),
        "[queue]\nsource = \"gitlab\"\n",
    )
    .unwrap();
    script(
        &e.bin.join("glab"),
        "echo \"$@\" >> \"$FAKE_DIR/glab-calls\"\n\
         case \"$2\" in\n\
           */approvals) echo '{\"approved_by\":[{},{},{}]}' ;;\n\
           *) echo '{\"state\":\"merged\",\"merge_commit_sha\":null,\"squash_commit_sha\":\"abc1234\"}' ;;\n\
         esac",
    );
    git(
        &e.root,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Revert \"feat: x (!12)\"\n\nThis reverts commit abc1234.",
        ],
    );
    let (v, _) = e.run(&["score", UNIT, "--dry-run"], 0);
    let p = v["prompts"][0]["prompt"].as_str().unwrap();
    for want in [
        "\"state\":\"MERGED\"",
        "\"review_rounds\":3",
        "\"reverted\":true",
    ] {
        assert!(p.contains(want), "{want:?} not in prompt:\n{p}");
    }
    let calls = fs::read_to_string(e.base.join("glab-calls")).unwrap();
    assert!(
        calls.contains("api projects/:id/merge_requests/12"),
        "{calls}"
    );
    assert!(!e.base.join("gh-calls").exists());
}

#[test]
fn a_malformed_factory_file_fails_the_score_instead_of_running_gh() {
    let e = Env::new("pass");
    fs::write(
        e.root.join(".nightshift/nightshift.toml"),
        "[queue\nsource = \n",
    )
    .unwrap();
    let (_, err) = e.run(&["score", UNIT, "--dry-run"], 2);
    assert!(err.contains("cannot parse"), "{err}");
    assert!(!e.base.join("gh-calls").exists(), "gh ran");
}
