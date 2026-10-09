//! `ns eval` end to end against a fake harness that emits canned claude-stream-json.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

/// Reads the prompt, logs the call, and acts like an agent: with ns-demo installed it fixes
/// the adder and adds a test (a trivial one when the prompt says WEAK); without it, it
/// edits README.md. It loads the skill when the prompt mentions "demo".
const FAKE_HARNESS: &str = r#"#!/bin/sh
prompt=$(cat)
cred=no
[ -L "$HOME/.fake-cred" ] && cred=yes
echo "call model=$1 pwd=$PWD home=$HOME mode=$CALC_MODE cred=$cred bg=${CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS-unset}" >> "$FAKE_LOG"
echo '{"type":"system","subtype":"init"}'
if [ -d "$HOME/.claude/skills/ns-demo" ] && [ -d "$HOME/.agents/skills/ns-demo" ]; then
  case "$prompt" in
    *demo*) echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Skill","input":{"skill":"ns-demo"}}]}}' ;;
  esac
  if [ -f src/calc.sh ]; then
    printf 'echo $(($1 + $2))\n' > src/calc.sh
    case "$prompt" in
      *WEAK*) echo 'true' > tests/test_trivial.sh ;;
      *) printf '[ "$(sh src/calc.sh 2 3)" = "5" ] || exit 1\n' > tests/test_add.sh ;;
    esac
    mkdir -p .ns/demo
    printf -- '---\nstatus: pass\n---\n' > .ns/demo/build.md
    git add -A >/dev/null && git commit -qm fix >/dev/null
  fi
elif [ -f README.md ]; then
  echo "more" >> README.md
fi
echo '{"type":"result","subtype":"success","result":"done","total_cost_usd":0.01,"duration_ms":10,"num_turns":2,"usage":{"input_tokens":100,"output_tokens":20}}'
"#;

struct Env {
    tmp: TempDir,
    repo: PathBuf,
    cfg: PathBuf,
    log: PathBuf,
    home: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let ok = StdCommand::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for e in fs::read_dir(src).unwrap().flatten() {
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            fs::copy(e.path(), &to).unwrap();
        }
    }
}

fn setup_with(output: &str) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("repo");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/eval-repo"),
        &repo,
    );
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);

    let fake = base.join("fake.sh");
    fs::write(&fake, FAKE_HARNESS).unwrap();
    let claude = base.join("bin/claude");
    fs::create_dir_all(claude.parent().unwrap()).unwrap();
    fs::write(&claude, FAKE_HARNESS).unwrap();
    StdCommand::new("chmod")
        .arg("+x")
        .arg(&claude)
        .status()
        .unwrap();
    let home = base.join("realhome");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join(".fake-cred"), "secret").unwrap();
    let cfg = base.join("config.toml");
    fs::write(
        &cfg,
        format!(
            r#"
[eval]
harness = "fake"
model = "m1"
trials = 1
max_trials = 2
budget_usd = 100.0
max_runs = 100
transcripts = "{t}"

[eval.harnesses.fake]
command = ["sh", "{fake}", "{{model}}"]
carry = [".fake-cred"]
output = "{output}"

[eval.harnesses.claude]
command = ["{claude}", "{{model}}"]
carry = [".fake-cred"]
output = "{output}"

[harness.judge]
command = ["sh", "-c", "cat > /dev/null; echo PASS; echo root cause fixed"]
[roles."eval.judge"]
harness = "judge"
"#,
            t = base.join("transcripts").display(),
            fake = fake.display(),
            claude = claude.display(),
        ),
    )
    .unwrap();
    Env {
        log: base.join("calls.log"),
        tmp,
        repo,
        cfg,
        home,
    }
}

fn setup() -> Env {
    setup_with("claude-stream-json")
}

impl Env {
    fn ns(&self) -> Command {
        let mut c = Command::cargo_bin("ns").unwrap();
        c.env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS")
            .env("NS_CONFIG", &self.cfg)
            .env("FAKE_LOG", &self.log)
            .env("HOME", &self.home)
            .current_dir(&self.repo)
            .arg("eval");
        c
    }

    fn eval(&self, args: &[&str]) -> Value {
        let out = self.ns().args(args).assert().success().get_output().clone();
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{e}: {}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        })
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

fn case<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["skills"][0]["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["case"] == id)
        .unwrap_or_else(|| panic!("no case {id} in {v}"))
}

fn check<'a>(trial: &'a Value, kind: &str) -> &'a Value {
    trial["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == kind)
        .unwrap()
}

#[test]
fn full_trial_with_and_without_then_cache_hit() {
    let e = setup();
    let v = e.eval(&["ns-demo", "--case", "fix-add", "--human"]);
    let c = case(&v, "fix-add");
    assert_eq!(c["status"], "ran");

    let with = &c["arms"]["with"]["trial_results"][0];
    assert_eq!(with["passed"], true, "{with}");
    for ch in with["checks"].as_array().unwrap() {
        assert_eq!(ch["passed"], true, "{ch}");
    }
    assert_eq!(check(with, "judge")["required"], false);
    assert!(check(with, "judge")["detail"]
        .as_str()
        .unwrap()
        .contains("root cause"));
    assert_eq!(with["cost_usd"], 0.01);
    assert_eq!(with["input_tokens"], 100);
    assert_eq!(with["num_turns"], 2);

    let without = &c["arms"]["without"]["trial_results"][0];
    assert_eq!(without["passed"], false);
    assert_eq!(check(without, "command")["passed"], false);
    assert_eq!(check(without, "fails_on_base")["passed"], false);
    let scope = check(without, "diff_scope");
    assert_eq!(scope["passed"], false);
    assert!(scope["detail"].as_str().unwrap().contains("README.md"));

    // Both arms unanimous and different: no adaptive extra trials.
    assert_eq!(c["arms"]["with"]["trials"], 1);
    assert_eq!(v["skills"][0]["metrics"]["uplift"], 1.0);
    assert_eq!(v["runs"], 2);

    // Isolation: scratch cwd, throwaway home with the carry symlink, fixture env.
    let calls = e.calls();
    assert_eq!(calls.len(), 2);
    let tmp = std::env::temp_dir().canonicalize().unwrap();
    for l in &calls {
        assert!(l.contains("model=m1"), "{l}");
        assert!(l.contains("mode=strict"), "{l}");
        assert!(l.contains("cred=yes"), "{l}");
        assert!(
            l.contains("bg=unset"),
            "only claude gets a bg wait ceiling: {l}"
        );
        assert!(!l.contains(&format!("home={} ", e.home.display())), "{l}");
        let pwd = l.split("pwd=").nth(1).unwrap().split(' ').next().unwrap();
        assert!(Path::new(pwd).starts_with(&tmp), "{l}");
        assert!(
            !Path::new(pwd).exists(),
            "scratch dir not cleaned up: {pwd}"
        );
    }

    let files = v["results_files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let results: Value =
        serde_json::from_str(&fs::read_to_string(files[0].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(results["skill"], "ns-demo");
    assert_eq!(results["config"]["harness"], "fake");
    assert!(results["commit"].as_str().unwrap().len() >= 7);
    assert!(results["cases"][0]["arms"]["with"]
        .get("trial_results")
        .is_none());
    let name = Path::new(files[0].as_str().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.ends_with(".json")
            && name.len() == "2026-10-08-".len() + results["commit"].as_str().unwrap().len() + 5
    );

    // Second run: the without baseline comes from the cache.
    let v = e.eval(&["ns-demo", "--case", "fix-add", "--no-write-results"]);
    let c = case(&v, "fix-add");
    assert_eq!(c["arms"]["without"]["cached"], 1);
    assert_eq!(c["arms"]["without"]["pass_rate"], 0.0);
    assert_eq!(v["runs"], 1);
    assert_eq!(e.calls().len(), 3);
    assert!(v["results_files"].as_array().unwrap().is_empty());

    // --no-cache reruns it.
    let v = e.eval(&[
        "ns-demo",
        "--case",
        "fix-add",
        "--no-cache",
        "--no-write-results",
    ]);
    assert_eq!(case(&v, "fix-add")["arms"]["without"]["cached"], 0);
    assert_eq!(v["runs"], 2);
}

/// The same setup with the harness named `claude`, logged in by token.
fn setup_claude() -> Env {
    let e = setup();
    let cfg = fs::read_to_string(&e.cfg).unwrap();
    fs::write(
        &e.cfg,
        cfg.replacen("harness = \"fake\"", "harness = \"claude\"", 1),
    )
    .unwrap();
    e
}

#[test]
fn claude_trials_wait_for_background_tasks() {
    let e = setup_claude();
    e.ns()
        .env("CLAUDE_CODE_OAUTH_TOKEN", "t")
        .args([
            "ns-demo",
            "--case",
            "fix-add",
            "--arms",
            "with",
            "--no-write-results",
        ])
        .assert()
        .success();
    let calls = e.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].ends_with(" bg=0"), "{}", calls[0]);
}

#[test]
fn a_bg_wait_ceiling_the_user_set_wins_in_trials() {
    let e = setup_claude();
    e.ns()
        .env("CLAUDE_CODE_OAUTH_TOKEN", "t")
        .env("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS", "900000")
        .args([
            "ns-demo",
            "--case",
            "fix-add",
            "--arms",
            "with",
            "--no-write-results",
        ])
        .assert()
        .success();
    let calls = e.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].ends_with(" bg=900000"), "{}", calls[0]);
}

#[test]
fn fails_on_base_rejects_a_test_that_passes_on_the_bug() {
    let e = setup();
    let out = e.tmp.path().join("out.json");
    let v = e.eval(&[
        "ns-demo",
        "--case",
        "weak-test",
        "--arms",
        "with",
        "--out",
        out.to_str().unwrap(),
    ]);
    let t = &case(&v, "weak-test")["arms"]["with"]["trial_results"][0];
    assert_eq!(check(t, "command")["passed"], true);
    let fob = check(t, "fails_on_base");
    assert_eq!(fob["passed"], false, "{fob}");
    assert!(fob["detail"].as_str().unwrap().contains("test_trivial.sh"));
    assert_eq!(t["passed"], false);
    // Unanimous single arm: one trial only.
    assert_eq!(e.calls().len(), 1);
    let written: Value = serde_json::from_str(&fs::read_to_string(out).unwrap()).unwrap();
    assert_eq!(written["run_id"], v["run_id"]);
}

#[test]
fn budget_and_max_runs_stop_new_trials() {
    let e = setup();
    let v = e.eval(&["ns-demo", "--case", "fix-add", "--budget-usd", "0.01"]);
    assert_eq!(v["runs"], 1);
    assert_eq!(v["budget"]["exhausted"], true);
    let c = case(&v, "fix-add");
    assert_eq!(c["reason"], "budget");
    assert_eq!(c["skipped_trials"], 1);
    assert!(v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["reason"] == "budget"));
    assert_eq!(e.calls().len(), 1);

    let v = e.eval(&[
        "ns-demo",
        "--triggers-only",
        "--max-runs",
        "2",
        "--no-write-results",
    ]);
    assert_eq!(v["runs"], 2);
    let statuses: Vec<&str> = v["skills"][0]["triggers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses.iter().filter(|s| **s == "skipped: budget").count(),
        2
    );
}

#[test]
fn dry_run_plans_without_calling_the_harness() {
    let e = setup();
    let v = e.eval(&["--dry-run"]);
    assert!(e.calls().is_empty());
    assert_eq!(v["dry_run"], true);
    let skills: Vec<&str> = v["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["skill"].as_str().unwrap())
        .collect();
    assert_eq!(skills, ["ns-demo", "ns-manual"]);
    let demo = &v["skills"][0];
    assert_eq!(demo["triggers"]["planned"], 4);
    let fix = case(&v, "fix-add");
    assert_eq!(
        fix["arms"][0]["skills"],
        serde_json::json!(["ns-demo", "ns-helper"])
    );
    assert_eq!(fix["arms"][1]["skills"], serde_json::json!(["ns-helper"]));
    assert_eq!(fix["env"]["CALC_MODE"], "strict");
    assert!(case(&v, "needs-cap")["skipped"]
        .as_str()
        .unwrap()
        .contains("missing capability: zephyr"));
    assert!(v["skills"][1]["triggers"]["skipped"]
        .as_str()
        .unwrap()
        .contains("user-invoked"));
    // 4 triggers + 3 cases x 2 arms x 1 trial (up to 2 trials each).
    assert_eq!(v["estimated_runs"]["min"], 10);
    assert_eq!(v["estimated_runs"]["max"], 16);
    assert!(!e.tmp.path().join("transcripts").exists());

    // With the capability configured, the case is planned.
    let mut text = fs::read_to_string(&e.cfg).unwrap();
    text.push_str("[eval.capability.zephyr]\npath_prepend = [\"/opt/zbin\"]\n");
    fs::write(&e.cfg, text).unwrap();
    let v = e.eval(&["ns-demo", "--case", "needs-cap", "--dry-run"]);
    let c = case(&v, "needs-cap");
    assert!(c["skipped"].is_null());
    assert!(c["env"]["PATH"].as_str().unwrap().starts_with("/opt/zbin:"));
    assert!(e.calls().is_empty());
}

#[test]
fn trigger_evals_detect_skill_loads() {
    let e = setup();
    let v = e.eval(&["ns-demo", "--triggers-only"]);
    let m = &v["skills"][0]["metrics"]["triggers"];
    assert_eq!(m["status"], "ok");
    assert_eq!(
        (
            m["tp"].as_u64(),
            m["fp"].as_u64(),
            m["tn"].as_u64(),
            m["fn"].as_u64()
        ),
        (Some(1), Some(1), Some(1), Some(1))
    );
    assert_eq!(m["precision"], 0.5);
    assert_eq!(m["recall"], 0.5);
    let t = &v["skills"][0]["triggers"][0];
    assert_eq!(t["loaded"], true);
    assert!(Path::new(t["transcript"].as_str().unwrap()).is_file());
    assert_eq!(e.calls().len(), 4);
    assert!(v["skills"][0]["cases"].as_array().unwrap().is_empty());
}

#[test]
fn harness_without_parser_reports_triggers_unsupported() {
    let e = setup_with("none");
    let v = e.eval(&["ns-demo", "--dry-run"]);
    assert!(v["skills"][0]["triggers"]["skipped"]
        .as_str()
        .unwrap()
        .starts_with("unsupported"));
    let v = e.eval(&[
        "ns-demo",
        "--case",
        "fix-add",
        "--arms",
        "with",
        "--no-write-results",
    ]);
    let t = &case(&v, "fix-add")["arms"]["with"]["trial_results"][0];
    assert_eq!(t["passed"], true);
    assert!(t["cost_usd"].is_null());
}

#[test]
fn refuses_a_fixture_outside_evals_fixtures() {
    let e = setup();
    let dir = e.repo.join("skills/ns-demo/evals/cases/escape");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("case.toml"),
        "fixture = \"../../skills\"\nprompt = \"x\"\n",
    )
    .unwrap();
    e.ns()
        .args(["ns-demo", "--dry-run"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("refusing case ns-demo/escape"));
}

#[test]
fn usage_errors_and_changed_since() {
    let e = setup();
    e.ns()
        .args(["ns-nope", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ns eval ns-tdd --dry-run"));
    e.ns()
        .args(["--arms", "sideways", "--dry-run"])
        .assert()
        .code(2);
    e.ns()
        .args(["--harness", "ghost", "--dry-run"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("[eval.harnesses.ghost]"));

    let v = e.eval(&["--changed-since", "HEAD", "--dry-run"]);
    assert!(v["skills"].as_array().unwrap().is_empty());
    fs::write(e.repo.join("skills/ns-manual/evals/new.txt"), "x").unwrap();
    let v = e.eval(&["--changed-since", "HEAD", "--dry-run"]);
    assert_eq!(v["skills"].as_array().unwrap().len(), 1);
    assert_eq!(v["skills"][0]["skill"], "ns-manual");
}

#[test]
fn compare_installs_the_skill_at_a_ref() {
    let e = setup();
    let skill = e.repo.join("skills/ns-demo/SKILL.md");
    let text = fs::read_to_string(&skill).unwrap();
    fs::write(&skill, text.replace("# Demo", "# Demo v2")).unwrap();
    git(&e.repo, &["commit", "-qam", "v2"]);
    let v = e.eval(&[
        "ns-demo",
        "--case",
        "fix-add",
        "--compare",
        "HEAD~1",
        "--dry-run",
    ]);
    let fix = case(&v, "fix-add");
    assert_eq!(fix["arms"][1]["arm"], "old");
    assert_eq!(
        fix["arms"][1]["skills"],
        serde_json::json!(["ns-demo@HEAD~1", "ns-helper"])
    );
    let v = e.eval(&[
        "ns-demo",
        "--case",
        "fix-add",
        "--compare",
        "HEAD~1",
        "--no-write-results",
    ]);
    let c = case(&v, "fix-add");
    // The fake harness keys on the installed directory name, so both versions pass.
    assert_eq!(c["arms"]["old"]["pass_rate"], 1.0);
    assert_eq!(v["skills"][0]["metrics"]["uplift"], 0.0);
    assert_eq!(v["skills"][0]["metrics"]["baseline"], "old");
}

#[test]
fn fails_on_base_rejects_a_runner_that_cannot_start() {
    let e = setup();
    let v = e.eval(&["ns-demo", "--case", "runner-missing", "--arms", "with"]);
    let t = &case(&v, "runner-missing")["arms"]["with"]["trial_results"][0];
    let fob = check(t, "fails_on_base");
    assert_eq!(fob["passed"], false, "{fob}");
    assert!(fob["detail"]
        .as_str()
        .unwrap()
        .contains("must pass on the trial's final state"));
}
