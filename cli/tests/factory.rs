//! End-to-end tests for `ns run` and `ns watch` with a fake harness (`claude`) and a fake `gh`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const NOW: i64 = 1_791_504_000; // 2026-10-09T00:00:00Z

fn git(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Env {
    _tmp: TempDir,
    base: PathBuf,
    root: PathBuf,
    bin: PathBuf,
    home: PathBuf,
    ctrl: PathBuf,
    ghd: PathBuf,
}

impl Env {
    fn new() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("myrepo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        fs::write(root.join("README"), "hi\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let bin = base.join("bin");
        fs::create_dir(&bin).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/factory");
        for f in ["claude", "gh"] {
            fs::copy(fixtures.join(f), bin.join(f)).unwrap();
        }
        let home = base.join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/.credentials.json"), "{}").unwrap();
        let ctrl = base.join("ctrl");
        let ghd = base.join("ghd");
        fs::create_dir(&ctrl).unwrap();
        fs::create_dir(&ghd).unwrap();
        fs::write(ghd.join("issues.jsonl"), "").unwrap();
        let e = Env {
            _tmp: tmp,
            base,
            root,
            bin,
            home,
            ctrl,
            ghd,
        };
        e.issue(7, "Fix the thing", "OPEN");
        e
    }

    /// Add a bare `origin` with `main` pushed and origin/HEAD set.
    fn with_remote(&self) -> PathBuf {
        let remote = self.base.join("remote.git");
        git(
            &self.base,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        );
        git(
            &self.root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&self.root, &["push", "-q", "origin", "main"]);
        git(&self.root, &["remote", "set-head", "origin", "main"]);
        remote
    }

    fn factory(&self, text: &str) {
        fs::create_dir_all(self.root.join(".nightshift")).unwrap();
        fs::write(self.root.join(".nightshift/nightshift.toml"), text).unwrap();
    }

    fn issue(&self, n: u64, title: &str, state: &str) {
        fs::write(
            self.ghd.join(format!("issue-{n}.json")),
            serde_json::json!({"number": n, "title": title, "url": format!("https://github.com/o/r/issues/{n}"), "state": state}).to_string(),
        )
        .unwrap();
    }

    fn ready(&self, n: u64, title: &str, labels: &[&str], body: &str) {
        self.ready_by(n, title, labels, body, Some("MEMBER"));
    }

    fn ready_by(&self, n: u64, title: &str, labels: &[&str], body: &str, assoc: Option<&str>) {
        self.issue(n, title, "OPEN");
        let mut all = vec!["status:ready-for-agent"];
        all.extend(labels);
        let line = serde_json::json!({
            "number": n,
            "title": title,
            "labels": all.iter().map(|l| serde_json::json!({"name": l})).collect::<Vec<_>>(),
            "body": body,
            "authorAssociation": assoc,
        });
        let mut text = fs::read_to_string(self.ghd.join("issues.jsonl")).unwrap();
        text.push_str(&format!("{line}\n"));
        fs::write(self.ghd.join("issues.jsonl"), text).unwrap();
    }

    fn queue(&self, phase: &str, actions: &[&str]) {
        fs::write(self.ctrl.join(phase), actions.join("\n") + "\n").unwrap();
    }

    fn ctl(&self, name: &str, text: &str) {
        fs::write(self.ctrl.join(name), text).unwrap();
    }

    fn gh_file(&self, name: &str, text: &str) {
        fs::write(self.ghd.join(name), text).unwrap();
    }

    fn ns(&self) -> Command {
        let mut c = Command::cargo_bin("ns").unwrap();
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        c.current_dir(&self.root)
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("NS_CONFIG", self.base.join("no-config.toml"))
            .env("FAKE_CTRL", &self.ctrl)
            .env("FAKE_GH_DIR", &self.ghd)
            .env("ANTHROPIC_API_KEY", "must-not-leak")
            .env("NS_NOW", NOW.to_string())
            .env("TZ", "UTC")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .env_remove("GH_TOKEN");
        c
    }

    fn run(&self, args: &[&str], code: i32) -> Value {
        let out = self
            .ns()
            .args(args)
            .assert()
            .code(code)
            .get_output()
            .clone();
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{e}: stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        })
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.ctrl.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn gh_calls(&self) -> String {
        fs::read_to_string(self.ghd.join("calls")).unwrap_or_default()
    }

    fn prompt(&self, n: usize, phase: &str) -> String {
        fs::read_to_string(self.ctrl.join(format!("prompt-{n}-{phase}.txt"))).unwrap()
    }

    fn config(&self, text: &str) {
        fs::write(self.base.join("no-config.toml"), text).unwrap();
    }

    /// An executable script in the temp dir, for `token_command`.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let p = self.base.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        StdCommand::new("chmod").arg("+x").arg(&p).status().unwrap();
        p
    }

    /// The distinct `GH_TOKEN` values the fake gh and the fake harness saw.
    fn tokens_seen(&self) -> Vec<String> {
        let mut seen: Vec<String> = [self.ghd.join("tokens"), self.ctrl.join("tokens")]
            .iter()
            .flat_map(|p| {
                fs::read_to_string(p)
                    .unwrap_or_default()
                    .lines()
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .collect();
        seen.sort();
        seen.dedup();
        seen
    }

    fn worktree(&self, unit: &str) -> PathBuf {
        self.base.join("myrepo.worktrees").join(unit)
    }
}

const UNIT: &str = "7-fix-the-thing";

#[test]
fn happy_path_triage_to_done() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["unit"], UNIT);
    assert_eq!(v["outcome"], "done");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    assert_eq!(v["cost_usd"], 2.5);
    assert_eq!(v["phases"].as_array().unwrap().len(), 5);
    assert!(
        !e.ctrl.join("leak").exists(),
        "ANTHROPIC_API_KEY reached claude"
    );
    let args = fs::read_to_string(e.ctrl.join("args")).unwrap();
    assert!(args.contains("bypassPermissions"), "{args}");
    assert!(
        args.contains("--output-format stream-json --verbose"),
        "{args}"
    );
    let common = e.root.join(".git");
    let log = fs::read_to_string(common.join("ns/runs.jsonl")).unwrap();
    assert!(log.lines().count() >= 7);
    assert!(log.contains("\"outcome\":\"done\""));
    assert!(common
        .join(format!("ns/transcripts/{UNIT}/verify-1.jsonl"))
        .is_file());
    assert!(e.prompt(1, "triage").contains("ns-triage"));
    assert!(e
        .prompt(1, "triage")
        .contains("https://github.com/o/r/issues/7"));
    assert!(!common.join("ns-run.lock").exists());
}

#[test]
fn verify_fail_goes_back_to_build_with_feedback() {
    let e = Env::new();
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("verify", &["fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "verify", "review", "ship"]
    );
    let p = e.prompt(4, "build");
    assert!(p.contains("attempt 2"), "{p}");
    assert!(p.contains("verify said fail"), "{p}");
}

#[test]
fn attempt_limit_gives_stuck() {
    let e = Env::new();
    e.queue("build", &["pass:commit", "pass:commit", "pass:commit"]);
    e.queue("verify", &["fail", "fail"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    assert!(
        v["reason"].as_str().unwrap().contains("out of attempts"),
        "{v}"
    );
    assert_eq!(e.calls(), ["triage", "build", "verify", "build", "verify"]);
}

#[test]
fn no_artifact_is_a_failed_attempt() {
    let e = Env::new();
    e.queue("build", &["none", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(v["phases"][1]["reason"], "no artifact written");
    assert_eq!(v["phases"][1]["written"], false);
    assert_eq!(&e.calls()[..3], ["triage", "build", "build"]);
}

#[test]
fn triage_without_brief_is_stuck() {
    let e = Env::new();
    e.queue("triage", &["none"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert!(v["reason"].as_str().unwrap().contains("no brief"), "{v}");
    assert_eq!(e.calls(), ["triage"]);
}

#[test]
fn review_commit_makes_evidence_stale_and_reverifies() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "verify", "ship"]
    );
}

#[test]
fn blocked_artifact_is_stuck() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["blocked:needs hardware"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert!(v["reason"]
        .as_str()
        .unwrap()
        .contains("evidence.md is blocked"));
    assert!(v["artifact"].as_str().unwrap().ends_with("evidence.md"));
}

#[test]
fn lock_refuses_a_second_runner_and_clears_a_stale_one() {
    let e = Env::new();
    let lock = e.root.join(".git/ns-run.lock");
    fs::write(
        &lock,
        format!("{{\"pid\":{},\"unit\":\"other\"}}\n", std::process::id()),
    )
    .unwrap();
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(5)
        .stderr(predicates::str::contains("holds"));
    assert!(e.calls().is_empty());

    fs::write(&lock, "{\"pid\":2147483000,\"unit\":\"other\"}\n").unwrap();
    e.queue("build", &["pass:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    assert!(!lock.exists());
}

#[test]
fn default_branch_guard_stops_a_push_to_main() {
    let e = Env::new();
    e.with_remote();
    e.queue("build", &["pass:push"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("default branch moved"),
        "{v}"
    );
    assert_eq!(e.calls(), ["triage", "build"]);
}

#[test]
fn dry_run_calls_no_harness() {
    let e = Env::new();
    let v = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(v["decision"]["phase"], "triage");
    assert!(v["prompt"].as_str().unwrap().contains("`ns-triage`"));
    assert_eq!(v["worktree_exists"], false);
    assert!(e.calls().is_empty());
    assert!(!e.worktree(UNIT).exists());
    let v = e.run(&["run", "nobrief", "--dry-run"], 0);
    assert_eq!(v["decision"]["action"], "stuck");
}

#[test]
fn missing_subscription_login_refuses_to_start() {
    let e = Env::new();
    fs::remove_file(e.home.join(".claude/.credentials.json")).unwrap();
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("subscription"));
    assert!(e.calls().is_empty());
    // billing = "api" disables the guard.
    e.factory("[defaults]\nbilling = \"api\"\n");
    e.queue("build", &["pass:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    assert!(e.ctrl.join("leak").exists(), "api billing keeps the key");
}

#[test]
fn usage_limit_pauses_without_using_an_attempt() {
    let e = Env::new();
    e.factory("[defaults]\nmax_attempts = 1\n");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("build", &["limit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 4);
    assert_eq!(v["outcome"], "paused");
    assert_eq!(v["reset_at"], NOW + 3600);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(v["phases"][0]["phase"], "build");
    assert_eq!(v["phases"][0]["attempt"], 1);
}

#[test]
fn budget_stops_new_phases() {
    let e = Env::new();
    e.factory("[limits]\nbudget_usd = 1.0\n");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 3);
    assert_eq!(v["outcome"], "budget");
    assert_eq!(e.calls(), ["triage", "build"]);
}

const AUTO: &str = "[merge]\npolicy = \"auto\"
human_review = [\".github/**\", \".nightshift/**\"]\nci_timeout_minutes = 1\n";
const GREEN: &str = r#"[{"name":"ci","state":"SUCCESS","bucket":"pass","link":"https://github.com/o/r/actions/runs/1/job/2"}]"#;
const RED: &str = r#"[{"name":"ci","state":"FAILURE","bucket":"fail","link":"https://github.com/o/r/actions/runs/99/job/2"}]"#;

#[test]
fn auto_merge_squashes_when_green_and_no_human_review_files() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "src/x.rs\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(
        calls.contains("pr merge 12 --squash --delete-branch"),
        "{calls}"
    );
    assert!(!calls.contains("update-branch"));
}

#[test]
fn auto_merge_updates_a_branch_behind_main_first() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    let upd = calls.find("pr update-branch 12").expect("update-branch");
    let watch = calls.find("pr checks 12 --watch").unwrap();
    let merge = calls.find("pr merge 12 --squash").unwrap();
    assert!(upd < watch && watch < merge, "{calls}");
}

#[test]
fn auto_merge_conflict_goes_back_to_build() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("update-12.fail", "");
    e.gh_file("checks-12.json", GREEN);
    // The second update attempt succeeds.
    let v = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&v.stdout).unwrap();
    let p = e.prompt(6, "build");
    assert!(p.contains("rebase onto main"), "{p}");
    // max_attempts 2: the second conflict leaves the unit stuck at build.
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn ci_failure_rebuilds_then_merges_on_the_same_pr() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12-1.json", RED);
    e.gh_file("checks-12-2.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "build", "verify", "review", "ship"]
    );
    let p = e.prompt(6, "build");
    assert!(p.contains("CI failed on PR #12: ci (fail)"), "{p}");
    assert!(p.contains("assert 1 == 2"), "{p}");
}

#[test]
fn human_review_files_need_a_human_merge() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "src/x.rs\n.github/workflows/ci.yml\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert!(v["reason"]
        .as_str()
        .unwrap()
        .starts_with("changes files that need human review; needs a human merge"));
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_pr_merged_by_a_phase_is_stuck() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:merge"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("PR merged by run"),
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn human_policy_never_merges() {
    let e = Env::new();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert!(!e.gh_calls().contains("pr merge"));
    assert!(!e.gh_calls().contains("pr checks"));
}

#[test]
fn factory_validate_reports_problems() {
    let e = Env::new();
    e.factory("gates = \"auto\"\n[phases.build]\nskill = \"ns-build\"\n");
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["phases"].as_array().unwrap().len(), 5);
    e.factory("[defaults]\nharnes = \"claude\"\n");
    e.ns().args(["factory", "validate"]).assert().code(1);
    e.factory("");
    fs::create_dir_all(e.root.join(".nightshift/agents/build")).unwrap();
    fs::write(
        e.root.join(".nightshift/agents/build/agent.md"),
        "---\nrole: build\n---\nRun {skill} for {unit}; {bogus}\n",
    )
    .unwrap();
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8_lossy(&out).contains("unknown placeholder {bogus}"));
}

#[test]
fn custom_agent_prompt_is_rendered() {
    let e = Env::new();
    fs::create_dir_all(e.root.join(".nightshift/agents/triage")).unwrap();
    fs::write(
        e.root.join(".nightshift/agents/triage/agent.md"),
        "---\nrole: triage\n---\nPhase: triage\nCustom {skill} {unit} #{issue} gates={gates}\n",
    )
    .unwrap();
    let v = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(
        v["prompt"],
        format!("Phase: triage\nCustom ns-triage {UNIT} #7 gates=auto\n")
    );
}

// ---------------------------------------------------------------- ns watch

#[test]
fn watch_dry_run_orders_and_skips() {
    let e = Env::new();
    e.ready(1, "Docs", &["type:docs"], "");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix blocked", &["type:fix"], "Blocked by: #9\n\nmore");
    e.ready(4, "Feat", &["type:feat"], "Blocked by: #8");
    e.ready(5, "Fix with PR", &["type:fix"], "");
    e.ready(6, "Unlabelled", &[], "");
    e.issue(9, "open blocker", "OPEN");
    e.issue(8, "closed blocker", "CLOSED");
    e.gh_file("prs.json", r#"[{"number":40,"body":"Closes #5"}]"#);
    let v = e.run(&["watch", "--dry-run"], 0);
    let order: Vec<u64> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    assert_eq!(order, [2, 4, 1, 6]);
    let skipped = v["skipped"].to_string();
    assert!(skipped.contains("blocked by open #9"), "{skipped}");
    assert!(skipped.contains("open PR #40 closes it"), "{skipped}");
    assert!(e.calls().is_empty());
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn watch_queues_only_team_authored_issues() {
    let e = Env::new();
    e.ready_by(1, "Owner", &[], "", Some("OWNER"));
    e.ready_by(2, "Member", &[], "", Some("MEMBER"));
    e.ready_by(3, "Collaborator", &[], "", Some("COLLABORATOR"));
    e.ready_by(4, "Contributor", &[], "", Some("CONTRIBUTOR"));
    e.ready_by(5, "Stranger", &[], "", Some("NONE"));
    e.ready_by(6, "Unknown", &[], "", None);
    let v = e.run(&["watch", "--dry-run"], 0);
    let order: Vec<u64> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    assert_eq!(order, [1, 2, 3]);
    let skipped: Vec<(u64, &str)> = v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["number"].as_u64().unwrap(), i["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        skipped,
        [
            (4, "author outside the team"),
            (5, "author outside the team"),
            (6, "author outside the team"),
        ]
    );
    assert!(e.gh_calls().contains(
        "api --paginate repos/{owner}/{repo}/issues?state=open&labels=status:ready-for-agent&per_page=100 --jq"
    ));
}

#[test]
fn watch_never_runs_a_non_team_issue() {
    let e = Env::new();
    e.ready_by(2, "Stranger", &["type:fix"], "", Some("NONE"));
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "queue empty");
    assert!(v["units"].as_array().unwrap().is_empty());
    assert!(e.calls().is_empty());
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn watch_once_swaps_labels_on_done() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"].as_array().unwrap().len(), 1);
    assert_eq!(v["units"][0]["outcome"], "done");
    assert_eq!(v["stopped"], "max_units");
    let calls = e.gh_calls();
    assert!(calls.contains(
        "issue edit 2 --remove-label status:ready-for-agent --add-label status:in-progress"
    ));
    assert!(calls
        .contains("issue edit 2 --remove-label status:in-progress --add-label status:in-review"));
    assert!(!calls.contains("issue edit 3"));
}

#[test]
fn watch_stuck_swaps_label_and_comments() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["none"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck");
    let calls = e.gh_calls();
    assert!(
        calls.contains("--add-label status:ready-for-human"),
        "{calls}"
    );
    let comment = calls
        .lines()
        .find(|l| l.starts_with("issue comment 2"))
        .expect("comment");
    assert!(comment.contains("triage wrote no brief"), "{comment}");
    let all = calls.split("issue comment 2").nth(1).unwrap();
    assert!(all.contains("written by an AI agent"));
    assert!(all.contains("Last artifact"));
}

#[test]
fn watch_respects_max_units() {
    let e = Env::new();
    for n in [2, 3, 4] {
        e.ready(n, &format!("Fix {n}"), &["type:fix"], "");
    }
    e.queue("build", &["pass:commit", "pass:commit", "pass:commit"]);
    let v = e.run(&["watch", "--max-units", "2"], 0);
    let issues: Vec<u64> = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["issue"].as_u64().unwrap())
        .collect();
    assert_eq!(issues, [2, 3]);
    assert_eq!(v["stopped"], "max_units");
}

#[test]
fn watch_stops_when_queue_empties() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "queue empty");
}

#[test]
fn watch_sleeps_through_a_usage_limit_before_until() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("build", &["limit", "pass:commit"]);
    let v = e.run(&["watch", "--once", "--until", "06:30"], 0);
    let units = v["units"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{v}");
    assert_eq!(units[0]["outcome"], "paused");
    assert_eq!(units[0]["reset_at"], "2026-10-09T01:00:00Z");
    assert_eq!(units[1]["outcome"], "done");
    assert!(!e.gh_calls().contains("--add-label status:ready-for-agent"));
}

#[test]
fn watch_gives_back_the_issue_when_the_limit_resets_after_until() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.ctl("reset", &(NOW + 8 * 3600).to_string());
    e.queue("build", &["limit"]);
    let v = e.run(&["watch", "--until", "06:30"], 0);
    assert_eq!(v["stopped"], "usage limit resets after --until");
    assert!(e.gh_calls().contains(
        "issue edit 2 --remove-label status:in-progress --add-label status:ready-for-agent"
    ));
    assert!(!e.gh_calls().contains("issue edit 3"));
}

#[test]
fn watch_merged_removes_in_progress_and_bases_on_origin() {
    let e = Env::new();
    let remote = e.with_remote();
    // Someone else lands a commit on origin/main; the local main lags behind.
    let other = e.base.join("other");
    git(
        &e.base,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    fs::write(other.join("NEW"), "x\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "upstream"]);
    git(&other, &["push", "-q", "origin", "main"]);
    let upstream = git(&other, &["rev-parse", "HEAD"]);

    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.issue(2, "Fix a", "OPEN");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(
        calls.contains("issue edit 2 --remove-label status:in-progress\n"),
        "{calls}"
    );
    let wt = e.worktree("2-fix-a");
    assert!(git(&wt, &["merge-base", "--is-ancestor", &upstream, "HEAD"]).is_empty());
}

#[test]
fn watch_has_no_unit_cap_by_default() {
    let e = Env::new();
    for n in [2, 3, 4, 5, 6] {
        e.ready(n, &format!("Fix {n}"), &["type:fix"], "");
    }
    e.queue("build", &["pass:commit"; 5]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["units"].as_array().unwrap().len(), 5, "{v}");
    assert_eq!(v["stopped"], "queue empty");
}

const FAKE_TOKEN: &str = "fake-token-4f2a9c";

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn forge_token_reaches_every_child_and_never_the_logs() {
    let e = Env::new();
    let cmd = e.script("token.sh", &format!("echo '  {FAKE_TOKEN}  '"));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let out = e
        .ns()
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!printed.contains(FAKE_TOKEN), "{printed}");
    let ns_dir = e.root.join(".git/ns");
    let logged = files_under(&ns_dir);
    assert!(logged.iter().any(|p| p.ends_with("runs.jsonl")));
    assert!(logged
        .iter()
        .any(|p| p.to_string_lossy().contains("transcripts")));
    for p in logged {
        let text = fs::read_to_string(&p).unwrap_or_default();
        assert!(!text.contains(FAKE_TOKEN), "token in {}", p.display());
    }
}

#[test]
fn forge_token_env_is_read_and_exported() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    let out = e
        .ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
}

#[test]
fn a_set_gh_token_wins_and_no_command_runs() {
    let e = Env::new();
    let marker = e.base.join("ran");
    let cmd = e.script("token.sh", &format!("touch {}; exit 1", marker.display()));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ns()
        .env("GH_TOKEN", "from-env")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(
        !marker.exists(),
        "token_command ran though GH_TOKEN was set"
    );
    assert_eq!(e.tokens_seen(), ["from-env"]);
}

#[test]
fn a_failing_token_command_stops_before_any_child_and_hides_its_output() {
    let e = Env::new();
    let cmd = e.script("token.sh", &format!("echo {FAKE_TOKEN}; exit 1"));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    for args in [&["run", "--issue", "7"][..], &["watch", "--once"][..]] {
        let out = e.ns().args(args).assert().code(2).get_output().clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("github"), "{err}");
        assert!(err.contains(cmd.to_str().unwrap()), "{err}");
        assert!(!err.contains(FAKE_TOKEN), "{err}");
        assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
    }
    assert!(e.gh_calls().is_empty());
    assert!(e.calls().is_empty());
}

#[test]
fn an_empty_token_is_an_error() {
    let e = Env::new();
    let cmd = e.script("token.sh", "echo '   '");
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    let out = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("printed no token"), "{err}");
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    let out = e
        .ns()
        .env("MY_GH_TOKEN", "")
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("MY_GH_TOKEN"), "{err}");
    assert!(e.calls().is_empty());
}

#[test]
fn forge_host_is_exported_unless_already_set() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\nhost = \"ghe.example.com\"\n");
    let hosts = || fs::read_to_string(e.ghd.join("hosts")).unwrap_or_default();
    e.ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .env_remove("GH_HOST")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!hosts().is_empty());
    assert!(
        hosts().lines().all(|h| h == "ghe.example.com"),
        "{}",
        hosts()
    );
    fs::remove_file(e.ghd.join("hosts")).unwrap();
    e.ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .env("GH_HOST", "mine.example.com")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!hosts().is_empty());
    assert!(
        hosts().lines().all(|h| h == "mine.example.com"),
        "{}",
        hosts()
    );
}

#[test]
fn a_configured_gitlab_forge_exports_its_token_and_host() {
    let e = Env::new();
    e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\nhost = \"gitlab.example.com\"\n");
    let seen = || fs::read_to_string(e.ctrl.join("gitlab")).unwrap_or_default();
    e.ns()
        .env("MY_GL_TOKEN", FAKE_TOKEN)
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!seen().is_empty());
    assert!(
        seen()
            .lines()
            .all(|l| l == format!("{FAKE_TOKEN} gitlab.example.com")),
        "{}",
        seen()
    );
}

#[test]
fn a_set_gitlab_token_wins_and_no_command_runs() {
    let e = Env::new();
    let marker = e.base.join("ran");
    let cmd = e.script("token.sh", &format!("touch {}; exit 1", marker.display()));
    e.config(&format!(
        "[forge.gitlab]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ns()
        .env("GITLAB_TOKEN", "from-env")
        .env_remove("GITLAB_HOST")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(
        !marker.exists(),
        "token_command ran though GITLAB_TOKEN was set"
    );
    let seen = fs::read_to_string(e.ctrl.join("gitlab")).unwrap();
    assert!(seen.lines().all(|l| l == "from-env "), "{seen}");
}

#[test]
fn an_empty_gh_token_does_not_count_as_set() {
    let e = Env::new();
    let cmd = e.script("token.sh", &format!("echo {FAKE_TOKEN}"));
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    e.ns()
        .env("GH_TOKEN", "")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
}

#[test]
fn a_token_env_value_is_exported_trimmed() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    e.ns()
        .env("MY_GH_TOKEN", format!("  {FAKE_TOKEN}\n"))
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), [FAKE_TOKEN]);
}

#[test]
fn a_token_with_a_nul_byte_is_an_error_that_hides_it() {
    let e = Env::new();
    let cmd = e.script("token.sh", "printf 'secretA\\000secretB'");
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    let out = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("forge github"), "{err}");
    assert!(!err.contains("secret"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(e.calls().is_empty());
}

#[test]
fn token_command_stderr_is_discarded() {
    let e = Env::new();
    let cmd = e.script(
        "token.sh",
        &format!("echo {FAKE_TOKEN} >&2; echo {FAKE_TOKEN}"),
    );
    e.config(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        cmd.to_str().unwrap()
    ));
    let out = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&out.stderr).contains(FAKE_TOKEN));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
}

#[test]
fn an_unsettable_host_is_an_error_that_hides_the_token() {
    let e = Env::new();
    for host in ["a\\u0000b", ""] {
        e.config(&format!(
            "[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\nhost = \"{host}\"\n"
        ));
        let out = e
            .ns()
            .env("MY_GH_TOKEN", FAKE_TOKEN)
            .env_remove("GH_HOST")
            .args(["run", "--issue", "7"])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("forge github"), "{err}");
        assert!(err.contains("host"), "{err}");
        assert!(!err.contains(FAKE_TOKEN), "{err}");
        assert!(!err.contains("panicked"), "{err}");
        assert!(e.calls().is_empty());
    }
}

#[test]
fn a_set_gitlab_host_wins_over_the_configured_one() {
    let e = Env::new();
    e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\nhost = \"gitlab.example.com\"\n");
    e.ns()
        .env("MY_GL_TOKEN", FAKE_TOKEN)
        .env_remove("GITLAB_TOKEN")
        .env("GITLAB_HOST", "mine.example.com")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let seen = fs::read_to_string(e.ctrl.join("gitlab")).unwrap();
    assert!(!seen.is_empty());
    assert!(
        seen.lines()
            .all(|l| l == format!("{FAKE_TOKEN} mine.example.com")),
        "{seen}"
    );
}
