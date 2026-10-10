//! End-to-end tests for `ns run` and `ns watch` with a fake harness (`claude`) and a fake `gh`.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use common::{exits, Group, Ns};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const NOW: i64 = 1_791_504_000; // 2026-10-09T00:00:00Z

fn git(dir: &Path, args: &[&str]) -> String {
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
        let mut all = vec!["status:ready-for-agent"];
        all.extend(labels);
        self.open_by(n, title, &all, body, assoc);
    }

    /// A team-authored open issue that is not ready: a triage candidate when its labels allow.
    fn untriaged(&self, n: u64, title: &str, labels: &[&str]) {
        self.open_by(n, title, labels, "", Some("MEMBER"));
    }

    /// The triage phase's script (run by the `pass:script` or `none:script` action) moves the
    /// unit's issue from `status:needs-triage` to these labels.
    fn triage_sets(&self, labels: &[&str]) {
        let adds: String = labels.iter().map(|l| format!(" --add-label {l}")).collect();
        self.ctl(
            "triage.sh",
            &format!(
                "gh issue edit \"${{NS_UNIT%%-*}}\" --remove-label status:needs-triage{adds}\n"
            ),
        );
    }

    /// How many triage-only runs the harness saw.
    fn triage_only_runs(&self) -> usize {
        self.calls()
            .iter()
            .enumerate()
            .filter(|(i, p)| *p == "triage" && self.prompt(i + 1, "triage").contains("Triage only"))
            .count()
    }

    fn open_by(&self, n: u64, title: &str, all: &[&str], body: &str, assoc: Option<&str>) {
        self.issue(n, title, "OPEN");
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
        let lines: String = all.iter().map(|l| format!("{l}\n")).collect();
        fs::write(self.ghd.join(format!("labels-{n}")), lines).unwrap();
    }

    fn labels(&self, n: u64) -> Vec<String> {
        fs::read_to_string(self.ghd.join(format!("labels-{n}")))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
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

    /// `ns` with the fakes on PATH.
    fn ns(&self) -> Ns {
        let mut c = common::ns();
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
            .env_remove("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS")
            .env_remove("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
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

    /// verify runs on runner `bench`, which names lock `bench-1`, kept under `locks`.
    fn bench(&self, locks: &Path) {
        self.factory("[phases.verify]\nrunner = \"bench\"\n");
        self.runner("bench", "kind = \"bench\"\nlocks = [\"bench-1\"]\n");
        self.config(&format!(
            "[runners]\nlock_dir = {:?}\n",
            locks.to_str().unwrap()
        ));
    }

    fn runner(&self, name: &str, text: &str) {
        let dir = self.root.join(".nightshift/runners");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.toml")), text).unwrap();
    }

    fn worktree(&self, unit: &str) -> PathBuf {
        self.base.join("myrepo.worktrees").join(unit)
    }
}

const UNIT: &str = "7-fix-the-thing";

#[test]
fn claude_phases_wait_for_background_tasks() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    let seen = fs::read_to_string(e.ctrl.join("bgwait")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["0"; 5], "{seen}");
}

#[test]
fn a_non_claude_harness_gets_no_bg_wait_ceiling() {
    let e = Env::new();
    let agent = e.bin.join("agent");
    fs::copy(e.bin.join("claude"), &agent).unwrap();
    e.config(&format!(
        "[harness.other]\ncommand = [{a:?}]\ncommand_write = [{a:?}]\n",
        a = agent.to_str().unwrap()
    ));
    e.factory("[defaults]\nharness = \"other\"\n");
    e.run(&["run", "--issue", "7"], 0);
    let seen = fs::read_to_string(e.ctrl.join("bgwait")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["unset"; 5], "{seen}");
    let seen = fs::read_to_string(e.ctrl.join("automem")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["unset"; 5], "{seen}");
}

#[test]
fn claude_phases_wait_for_background_tasks_under_api_billing() {
    let e = Env::new();
    e.factory("[defaults]\nbilling = \"api\"\n");
    e.run(&["run", "--issue", "7"], 0);
    let seen = fs::read_to_string(e.ctrl.join("bgwait")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["0"; 5], "{seen}");
}

#[test]
fn a_bg_wait_ceiling_the_user_set_wins() {
    let e = Env::new();
    e.ns()
        .env("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS", "900000")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let seen = fs::read_to_string(e.ctrl.join("bgwait")).unwrap();
    assert!(seen.lines().all(|l| l == "900000"), "{seen}");
}

#[test]
fn claude_phases_run_with_auto_memory_off_whatever_the_operator_set() {
    let e = Env::new();
    e.ns()
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "0")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let seen = fs::read_to_string(e.ctrl.join("automem")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["1"; 5], "{seen}");
}

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

/// The build phase records its `NS_RUN_PID`, `NS_WATCH_PID` and the sid and pgid of itself and of `ns`.
fn record_phase_ids(e: &Env) {
    e.ctl(
        "build.sh",
        r#"{
  echo "${NS_RUN_PID-unset} ${NS_WATCH_PID-unset}"
  ps -o sid=,pgid= -p $$
  ps -o sid=,pgid= -p "$NS_RUN_PID"
} > "$FAKE_CTRL/ids""#,
    );
    e.queue("build", &["pass:script"]);
}

fn phase_ids(e: &Env) -> Vec<String> {
    fs::read_to_string(e.ctrl.join("ids"))
        .unwrap()
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// Run `ns`, which must succeed, and return the pid it ran as.
fn ok_pid(ns: &mut Ns) -> u32 {
    let (out, pid) = ns.output_with_pid();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    pid
}

#[test]
fn a_phase_runs_in_its_own_session_and_knows_the_run_pid() {
    let e = Env::new();
    record_phase_ids(&e);
    let pid = ok_pid(
        e.ns()
            .env("NS_WATCH_PID", "999999")
            .args(["run", "--issue", "7"]),
    );
    let ids = phase_ids(&e);
    assert_eq!(
        ids[0],
        pid.to_string(),
        "an inherited NS_WATCH_PID leaked: {ids:?}"
    );
    let phase: Vec<&str> = ids[1].split(' ').collect();
    let ns: Vec<&str> = ids[2].split(' ').collect();
    assert_ne!(phase[0], ns[0], "phase shares ns's session: {ids:?}");
    assert_ne!(phase[1], ns[1], "phase shares ns's process group: {ids:?}");
}

#[test]
fn a_phase_under_watch_knows_the_watch_pid() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    record_phase_ids(&e);
    let pid = ok_pid(e.ns().args(["watch", "--once"]));
    assert_eq!(phase_ids(&e)[0], format!("{pid} {pid}"));
}

#[test]
fn a_phase_that_kills_its_own_group_is_a_failed_attempt_not_a_dead_watch() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("build.sh", "kill 0");
    e.queue("build", &["pass:script", "pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(&e.calls()[..3], ["triage", "build", "build"]);
    assert_eq!(build_event(&e, 1)["written"], false);
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

/// The `gate` events in the run log.
fn gate_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "gate")
        .collect()
}

#[test]
fn a_green_gate_continues_to_verify_and_is_logged() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"touch gate-ran && echo gate ok\"\n");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    assert!(e.worktree(UNIT).join("gate-ran").exists());
    let g = gate_events(&e);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0]["command"], "touch gate-ran && echo gate ok");
    assert_eq!(g[0]["exit"], 0);
    assert_eq!(g[0]["green"], true);
    assert_eq!(g[0]["phase"], "build");
    assert!(g[0]["wall_s"].is_number(), "{g:?}");
    assert_eq!(
        g[0]["sha"].as_str(),
        Some(git(&e.worktree(UNIT), &["rev-parse", "--short", "HEAD"]).as_str())
    );
}

#[test]
fn a_red_gate_goes_back_to_build_with_its_output_tail() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\ngate = \"test -f {f} || {{ touch {f}; echo boom from the gate; exit 1; }}\"\n",
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "verify", "review", "ship"]
    );
    let p = e.prompt(3, "build");
    assert!(p.contains("attempt 2"), "{p}");
    assert!(p.contains("boom from the gate"), "{p}");
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["exit"], 1);
    assert_eq!(g[0]["green"], false);
    assert_eq!(g[1]["green"], true);
}

#[test]
fn a_gate_red_until_attempts_run_out_is_stuck() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"exit 1\"\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    assert_eq!(v["reason"], "build is out of attempts (2)");
    assert_eq!(e.calls(), ["triage", "build", "build"]);
    assert_eq!(gate_events(&e).len(), 2);
}

#[test]
fn a_committing_silent_rebuild_reruns_the_gate_and_can_go_green() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\nmax_attempts = 3\ngate = \"test -f {f} || {{ touch {f}; echo boom; exit 1; }}\"\n",
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "none:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(e.calls().contains(&"verify".to_string()), "{:?}", e.calls());
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["green"], false);
    assert_eq!(g[1]["green"], true);
    assert_ne!(g[0]["sha"], g[1]["sha"]);
}

#[test]
fn a_review_that_commits_runs_the_gate_again() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    e.factory(&format!(
        "[phases.build]\ngate = \"echo ran >> {}\"\n",
        log.display()
    ));
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "verify", "ship"]
    );
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "review");
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);
}

#[test]
fn a_red_gate_after_a_review_commit_goes_back_to_build() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"test ! -f broken\"\n");
    let commit = "git -c user.name=f -c user.email=f@f commit -qm";
    e.ctl(
        "review.sh",
        &format!("touch broken && git add broken && {commit} break"),
    );
    e.ctl("build.sh", &format!("git rm -q broken && {commit} fix"));
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("review", &["pass:script", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(5, "build").contains("test ! -f broken"));
    let g: Vec<_> = gate_events(&e)
        .iter()
        .map(|g| (g["phase"].clone(), g["green"].clone()))
        .collect();
    assert_eq!(
        g,
        [
            ("build".into(), true.into()),
            ("review".into(), false.into()),
            ("build".into(), true.into())
        ]
    );
}

#[test]
fn the_gate_falls_back_to_ci_local_in_stack_md() {
    let e = Env::new();
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command |\n|---|---|\n| ci-local | `echo from stack` | 1 s |\n",
    )
    .unwrap();
    git(&e.root, &["add", "."]);
    git(&e.root, &["commit", "-q", "-m", "stack"]);
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command |\n|---|---|\n| ci-local | `echo from main` | 1 s |\n",
    )
    .unwrap();
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(d["gate"], "echo from main");
    e.run(&["run", "--issue", "7"], 0);
    let g = gate_events(&e);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0]["command"], "echo from main");
    let in_worktree = fs::read_to_string(e.worktree(UNIT).join("docs/agents/stack.md")).unwrap();
    assert!(in_worktree.contains("echo from stack"), "{in_worktree}");
}

#[test]
fn a_rebuild_that_leaves_head_alone_skips_a_green_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit", "pass"]);
    e.queue("verify", &["fail", "pass"]);
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(&e.calls()[..4], ["triage", "build", "verify", "build"]);
    assert_eq!(gate_events(&e).len(), 1);
}

#[test]
fn a_gate_that_times_out_goes_back_to_build_and_uses_an_attempt() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"sleep 30\"\n");
    let out = e
        .ns()
        .env("NS_GATE_TIMEOUT_MS", "300")
        .args(["run", "--issue", "7"])
        .output();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["reason"], "build is out of attempts (2)");
    assert_eq!(e.calls(), ["triage", "build", "build"]);
    assert!(e.prompt(3, "build").contains("timed out after 0.3 s"));
    assert!(e.prompt(3, "build").contains("attempt 2"));
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["timed_out"], true);
    assert_eq!(g[0]["green"], false);
}

fn phase_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "phase")
        .collect()
}

fn run_with_phase_timeout(e: &Env, code: i32) -> Value {
    run_with_timeouts(e, "build=1500", code)
}

fn run_with_timeouts(e: &Env, timeouts: &str, code: i32) -> Value {
    let out = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", timeouts)
        .args(["run", "--issue", "7"])
        .assert()
        .code(code)
        .get_output()
        .clone();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn build_event(e: &Env, attempt: u64) -> Value {
    phase_events(e)
        .into_iter()
        .find(|p| p["phase"] == "build" && p["attempt"] == attempt)
        .unwrap()
}

#[test]
fn a_timed_out_phase_ignores_its_artifact_and_retries() {
    let e = Env::new();
    e.queue("build", &["blocked:sleep", "pass:commit"]);
    let v = run_with_phase_timeout(&e, 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "verify", "review", "ship"]
    );
    let dir = e.worktree(UNIT).join(".ns").join(UNIT);
    let archived = dir.join("history/build-timeout-1.md");
    assert!(fs::read_to_string(&archived)
        .unwrap()
        .contains("status: blocked"));
    assert!(fs::read_to_string(dir.join("build.md"))
        .unwrap()
        .contains("status: pass"));
    let p = build_event(&e, 1);
    assert_eq!(p["timed_out"], true);
    assert_eq!(p["written"], false);
    assert_eq!(p["reason"], "timed out after 1.5 s");
    assert_eq!(p["archived"], archived.to_str().unwrap());
    assert!(e.prompt(3, "build").contains("attempt 2"));
}

#[test]
fn a_phase_that_times_out_on_every_attempt_is_stuck_on_the_timeout() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 1\n");
    e.queue("build", &["blocked:sleep"]);
    let v = run_with_phase_timeout(&e, 1);
    assert_eq!(v["outcome"], "stuck");
    let reason = v["reason"].as_str().unwrap();
    assert_eq!(
        reason,
        "build is out of attempts (1): the last attempt timed out after 1.5 s; raise its timeout_minutes or split the unit"
    );
    assert_eq!(e.calls(), ["triage", "build"]);
    let dir = e.worktree(UNIT).join(".ns").join(UNIT);
    assert!(!dir.join("build.md").exists());
    assert!(dir.join("history/build-timeout-1.md").exists());
}

#[test]
fn a_timeout_then_a_plain_failure_is_not_stuck_on_a_timeout() {
    let e = Env::new();
    e.queue("build", &["blocked:sleep", "none"]);
    let v = run_with_phase_timeout(&e, 1);
    assert_eq!(v["outcome"], "stuck");
    assert_eq!(v["reason"], "build is out of attempts (2)");
    assert_eq!(build_event(&e, 2)["reason"], "no artifact written");
}

#[test]
fn repeated_timeouts_archive_each_artifact_separately() {
    let e = Env::new();
    e.queue("build", &["blocked:sleep", "fail:sleep"]);
    let v = run_with_phase_timeout(&e, 1);
    assert_eq!(v["outcome"], "stuck");
    let hist = e.worktree(UNIT).join(".ns").join(UNIT).join("history");
    assert!(fs::read_to_string(hist.join("build-timeout-1.md"))
        .unwrap()
        .contains("status: blocked"));
    assert!(fs::read_to_string(hist.join("build-timeout-2.md"))
        .unwrap()
        .contains("status: fail"));
}

#[test]
fn a_timed_out_review_that_moved_head_still_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["blocked:commit+sleep", "pass"]);
    let v = run_with_timeouts(&e, "review=1500", 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "review");
    assert_eq!(g[1]["attempt"], 1);
}

#[test]
fn a_timed_out_ship_keeps_its_pr_number_for_the_merged_guard() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:merge+sleep"]);
    let v = run_with_timeouts(&e, "ship=1500", 1);
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("PR merged by run"),
        "{v}"
    );
    let dir = e.worktree(UNIT).join(".ns").join(UNIT);
    assert!(!dir.join("pr.md").exists());
    assert!(dir.join("history/pr-timeout-1.md").exists());
}

#[test]
fn a_timed_out_build_does_not_run_the_gate() {
    let e = Env::new();
    let runs = e.ctrl.join("gate-runs");
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\nmax_attempts = 3\ngate = \"echo ran >> {r}; test -f {f} || {{ touch {f}; exit 1; }}\"\n",
        r = runs.display(),
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "blocked:sleep", "pass:commit"]);
    let v = run_with_phase_timeout(&e, 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "build", "verify", "review", "ship"]
    );
    assert_eq!(fs::read_to_string(&runs).unwrap().lines().count(), 2);
    let attempts: Vec<_> = gate_events(&e)
        .iter()
        .map(|g| g["attempt"].clone())
        .collect();
    assert_eq!(attempts, [1, 3]);
    assert_eq!(build_event(&e, 2)["reason"], "timed out after 1.5 s");
    assert_eq!(
        build_event(&e, 3)["decision"],
        "timed out with the CI gate still red"
    );
    assert!(e.prompt(4, "build").contains("attempt 3"));
}

#[test]
fn a_red_gate_is_not_skipped_when_the_rebuild_writes_no_artifact() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\nmax_attempts = 3\ngate = \"test -f {f} || {{ touch {f}; echo boom; exit 1; }}\"\n",
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "none", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(4, "build").contains("boom"));
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["green"], true);
}

#[test]
fn a_red_gate_with_a_silent_rebuild_never_reaches_verify() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"exit 1\"\n");
    e.queue("build", &["pass:commit", "none"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["reason"], "build is out of attempts (2)");
    assert_eq!(e.calls(), ["triage", "build", "build"]);
}

#[test]
fn a_rebuild_that_passes_at_the_same_head_reruns_a_red_gate() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\ngate = \"test -f {f} || {{ touch {f}; echo boom; exit 1; }}\"\n",
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "verify", "review", "ship"]
    );
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["green"], false);
    assert_eq!(g[1]["green"], true);
    assert_eq!(g[0]["sha"], g[1]["sha"]);
}

#[test]
fn a_red_gate_with_a_committing_rebuild_that_writes_no_artifact_never_reaches_verify() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 3\ngate = \"exit 1\"\n");
    e.queue("build", &["pass:commit", "none:commit", "none:commit"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    assert!(
        !e.calls().contains(&"verify".to_string()),
        "{:?}",
        e.calls()
    );
}

#[test]
fn a_review_that_fails_after_moving_head_still_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "review");
}

#[test]
fn a_build_that_did_not_pass_runs_no_gate() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 5\ngate = \"true\"\n");
    e.queue("build", &["none", "fail", "blocked:no"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    assert_eq!(e.calls(), ["triage", "build", "build", "build"]);
    assert!(gate_events(&e).is_empty());
}

#[test]
fn without_a_gate_none_runs() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    assert!(gate_events(&e).is_empty());
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

/// Whether another process could take `lock` right now, asked through flock(1).
fn lock_is_free(lock: &Path) -> bool {
    StdCommand::new("flock")
        .args(["-n", lock.to_str().unwrap(), "true"])
        .status()
        .unwrap()
        .success()
}

#[test]
fn runner_locks_are_held_for_the_phase_and_released_on_failure() {
    let e = Env::new();
    let locks = e.base.join("locks");
    let lock = locks.join("bench-1.lock");
    e.bench(&locks);
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("verify", &["fail:script", "fail:script"]);
    e.ctl(
        "verify.sh",
        &format!(
            "flock -n {l:?} true && echo free >> {s:?} || echo held >> {s:?}; cat {l:?} >> {s:?}",
            l = lock,
            s = e.ctrl.join("seen"),
        ),
    );
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    let seen = fs::read_to_string(e.ctrl.join("seen")).unwrap();
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(lines.len(), 4, "{seen}");
    for pair in lines.chunks(2) {
        assert_eq!(pair[0], "held", "{seen}");
        let holder: Value = serde_json::from_str(pair[1]).unwrap();
        assert_eq!(holder["unit"], UNIT);
        assert!(holder["pid"].as_u64().is_some(), "{seen}");
    }
    assert!(lock_is_free(&lock));
    assert_eq!(fs::read_to_string(&lock).unwrap(), "");
}

#[test]
fn the_default_lock_dir_is_under_the_git_common_dir() {
    let e = Env::new();
    e.factory("[phases.verify]\nrunner = \"bench\"\n");
    e.runner("bench", "kind = \"bench\"\nlocks = [\"bench-1\"]\n");
    let lock = e.root.join(".git/ns/locks/bench-1.lock");
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["pass:script"]);
    e.ctl(
        "verify.sh",
        &format!("cat {:?} > {:?}", lock, e.ctrl.join("seen")),
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let holder: Value =
        serde_json::from_str(&fs::read_to_string(e.ctrl.join("seen")).unwrap()).unwrap();
    assert_eq!(holder["unit"], UNIT);
    assert!(lock.exists());
    assert!(lock_is_free(&lock));
}

#[test]
fn leftover_content_in_a_free_runner_lock_is_overwritten() {
    let e = Env::new();
    let locks = e.base.join("locks");
    let lock = locks.join("bench-1.lock");
    e.bench(&locks);
    fs::create_dir_all(&locks).unwrap();
    let stale = serde_json::json!({"pid": 2147483000, "unit": "x".repeat(200)});
    fs::write(&lock, format!("{stale}\n")).unwrap();
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["pass:script"]);
    e.ctl(
        "verify.sh",
        &format!("cat {:?} > {:?}", lock, e.ctrl.join("seen")),
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let holder: Value =
        serde_json::from_str(&fs::read_to_string(e.ctrl.join("seen")).unwrap()).unwrap();
    assert_eq!(holder["unit"], UNIT);
    assert!(lock_is_free(&lock));
}

enum Event {
    Entered,
    Exited(String),
}

/// A run whose verify phase holds its lock until `release` is called.
struct Blocked {
    run: Group,
    go: PathBuf,
}

impl Blocked {
    /// Returns once the run's verify phase is inside the harness, or panics with its stderr if
    /// the run ended first.
    fn start(e: &Env, before: &str, after: &str) -> Blocked {
        let entered = e.ctrl.join("entered");
        let go = e.ctrl.join("go");
        for f in [&entered, &go] {
            assert!(StdCommand::new("mkfifo").arg(f).status().unwrap().success());
        }
        e.queue("verify", &["pass:script"]);
        e.ctl(
            "verify.sh",
            &format!("{before}; echo in > {entered:?}; cat {go:?} > /dev/null; {after}"),
        );
        let mut run = e.ns().args(["run", "--issue", "7"]).start();
        let mut stderr = run.take_stderr();
        let (tx, rx) = mpsc::channel();
        let tx_err = tx.clone();
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            let _ = tx_err.send(Event::Exited(text));
        });
        thread::spawn(move || {
            let _ = fs::read_to_string(&entered);
            let _ = tx.send(Event::Entered);
        });
        match run.recv(&rx) {
            Event::Entered => Blocked { run, go },
            Event::Exited(text) => panic!("the run ended before its verify phase began:\n{text}"),
        }
    }

    fn release(&self) {
        fs::write(&self.go, "go\n").unwrap();
    }
}

#[test]
fn dropping_a_background_run_kills_the_phase_in_its_own_session() {
    let e = Env::new();
    let pid = e.ctrl.join("phase.pid");
    let held = Blocked::start(&e, &format!("echo $$ > {pid:?}"), "true");
    drop(held);
    let pid = fs::read_to_string(&pid).unwrap();
    assert!(exits(&pid), "phase {pid} outlived its run");
}

#[test]
fn a_run_past_its_timeout_fails_and_kills_the_phase_in_its_own_session() {
    let e = Env::new();
    let pid = e.ctrl.join("phase.pid");
    e.queue("build", &["pass:script"]);
    e.ctl("build.sh", &format!("echo $$ > {pid:?}; sleep 30"));
    let panic = std::panic::catch_unwind(|| {
        e.ns()
            .args(["run", "--issue", "7"])
            .timeout(Duration::from_secs(3))
            .output()
    })
    .unwrap_err();
    let msg = panic.downcast_ref::<String>().unwrap();
    assert!(msg.contains("timed out after 3s"), "{msg}");
    let pid = fs::read_to_string(&pid).unwrap();
    assert!(exits(&pid), "phase {pid} outlived its run");
}

#[test]
fn a_runner_lock_held_by_a_killed_run_is_recovered() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    let lock = locks.join("bench-1.lock");
    a.bench(&locks);
    b.bench(&locks);
    let mut held = Blocked::start(&a, "true", "true");
    assert!(!lock_is_free(&lock));
    held.run.kill();
    held.run.wait();
    held.release();
    assert!(lock_is_free(&lock));

    b.queue("build", &["pass:commit"]);
    b.queue("verify", &["pass:script"]);
    b.ctl(
        "verify.sh",
        &format!("cat {:?} > {:?}", lock, b.ctrl.join("seen")),
    );
    let v = b.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let holder: Value =
        serde_json::from_str(&fs::read_to_string(b.ctrl.join("seen")).unwrap()).unwrap();
    assert_ne!(holder["pid"].as_u64().unwrap(), u64::from(held.run.id()));
    assert!(lock_is_free(&lock));
}

#[test]
fn a_runner_with_two_locks_holds_both_for_the_phase() {
    let e = Env::new();
    let locks = e.base.join("locks");
    e.bench(&locks);
    e.runner(
        "bench",
        "kind = \"bench\"\nlocks = [\"b-lock\", \"a-lock\"]\n",
    );
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["pass:script"]);
    let seen = e.ctrl.join("seen");
    e.ctl(
        "verify.sh",
        &format!(
            "for n in a-lock b-lock; do flock -n {l:?}/$n.lock true && echo free >> {s:?} || echo held >> {s:?}; done",
            l = locks,
            s = seen,
        ),
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(fs::read_to_string(&seen).unwrap(), "held\nheld\n");
    assert!(lock_is_free(&locks.join("a-lock.lock")));
    assert!(lock_is_free(&locks.join("b-lock.lock")));
}

#[test]
fn two_runs_needing_one_lock_serialise() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    let order = a.base.join("order");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(
        &a,
        &format!("echo A-in >> {order:?}"),
        &format!("echo A-out >> {order:?}"),
    );
    b.queue("verify", &["pass:script"]);
    b.ctl("verify.sh", &format!("echo B >> {order:?}"));

    // A pinned clock would end B's wait at once: B waits on the real clock.
    let mut run_b = b
        .ns()
        .env_remove("NS_NOW")
        .args(["run", "--issue", "7"])
        .start();
    let b_err = run_b.take_stderr();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut lines = BufReader::new(b_err).lines().map_while(Result::ok);
        let _ = tx.send(lines.any(|l| l.contains("verify waits for lock bench-1")));
        lines.for_each(drop);
    });
    let waited = run_b.recv(&rx);
    let calls_while_waiting = b.calls();
    held.release();
    assert!(run_b.wait().success());
    let mut a_run = held.run;
    assert!(a_run.wait().success());

    assert!(waited, "B never waited for bench-1");
    assert_eq!(calls_while_waiting, ["triage", "build"]);
    assert_eq!(
        fs::read_to_string(&order)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["A-in", "A-out", "B"]
    );
    let log = fs::read_to_string(b.root.join(".git/ns/runs.jsonl")).unwrap();
    assert!(log.contains("\"event\":\"lock_wait\""), "{log}");
}

#[test]
fn a_runner_lock_wait_ends_each_attempt_at_the_phase_timeout() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    b.queue("build", &["pass:commit"]);
    b.queue("verify", &["pass:script"]);
    let out = b
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "verify=60000")
        .args(["run", "--issue", "7"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    held.release();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.starts_with("verify is out of attempts"), "{reason}");
    assert!(
        reason.contains("gave up waiting for lock bench-1 (held by pid ")
            && reason.ends_with("at the phase timeout"),
        "{reason}"
    );
    assert_eq!(b.calls(), ["triage", "build"]);
    let log = fs::read_to_string(b.root.join(".git/ns/runs.jsonl")).unwrap();
    let gave_up: Vec<Value> = log
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|ev| ev["event"] == "lock_wait_timeout")
        .collect();
    assert!(!gave_up.is_empty(), "{log}");
    assert_eq!(gave_up[0]["lock"], "bench-1");
    assert_eq!(gave_up[0]["bound"], "timeout");
    assert_eq!(gave_up[0]["held_by"]["unit"], UNIT);
    let attempts: Vec<_> = gave_up.iter().map(|ev| ev["attempt"].clone()).collect();
    assert_eq!(attempts, [1, 2], "{log}");
}

#[test]
fn a_runner_lock_timeout_retries_the_phase_it_was_given() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    let v = b.run(&["run", "--issue", "7", "--from", "verify"], 1);
    held.release();
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.starts_with("verify is out of attempts"), "{reason}");
    assert_eq!(b.calls(), Vec::<String>::new());
    let phases = v["phases"].as_array().unwrap();
    assert_eq!(phases.len(), 2, "{v}");
    for (i, p) in phases.iter().enumerate() {
        assert_eq!(p["phase"], "verify", "{v}");
        assert_eq!(p["attempt"], i + 1, "{v}");
        assert_eq!(p["written"], false, "{v}");
    }
}

#[test]
fn a_runner_lock_wait_past_until_ends_the_night() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    b.ready(2, "Fix a", &["type:fix"], "");
    b.queue("build", &["pass:commit"]);
    b.queue("verify", &["pass:script"]);
    let out = b
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "verify=86400000")
        .args(["watch", "--until", "00:30"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    held.release();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["stopped"], "until", "{v}");
    assert_eq!(v["units"][0]["outcome"], "budget", "{v}");
    let reason = v["units"][0]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("gave up waiting for lock bench-1") && reason.ends_with("at --until"),
        "{reason}"
    );
    assert_eq!(b.calls(), ["triage", "build"]);
    assert!(b.gh_calls().contains(
        "issue edit 2 --remove-label status:in-progress --add-label status:ready-for-agent"
    ));
    let log = fs::read_to_string(b.root.join(".git/ns/runs.jsonl")).unwrap();
    assert!(
        log.contains("\"event\":\"lock_wait_timeout\"") && log.contains("\"bound\":\"until\""),
        "{log}"
    );
}

#[test]
fn runner_problems_fail_validate_and_run() {
    let e = Env::new();
    e.factory("[phases.verify]\nrunner = \"bench\"\n");
    let validate = |e: &Env| -> String {
        let out = e
            .ns()
            .args(["factory", "validate"])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    };
    let out = validate(&e);
    assert!(
        out.contains("runner \\\"bench\\\" has no runners/bench.toml"),
        "{out}"
    );
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("runners/bench.toml"));

    e.runner("bench", "kind = \"remote\"\n");
    let out = validate(&e);
    assert!(
        out.contains("runners/bench.toml: unknown variant `remote`"),
        "{out}"
    );
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unknown variant `remote`"));
    assert!(e.calls().is_empty());

    e.runner("bench", "locks = [\"bench-1\"]\n");
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["phases"][2]["runner"], "bench");
    assert_eq!(v["phases"][2]["locks"][0], "bench-1");
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

fn update_branch_events(e: &Env) -> usize {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .filter(|l| serde_json::from_str::<Value>(l).unwrap()["event"] == "update_branch")
        .count()
}

fn head_polls(e: &Env) -> usize {
    e.gh_calls()
        .lines()
        .filter(|l| *l == "pr view 12 --json headRefOid,mergeStateStatus")
        .count()
}

#[test]
fn a_lagging_head_after_update_branch_is_waited_for() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("updated-12.head", UPDATED_HEAD);
    e.gh_file("updated-12.lag", "2");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert_eq!(head_polls(&e), 3, "{calls}");
    assert_eq!(update_branch_events(&e), 1);
    let polls = register_polls(&e);
    assert!(!polls.is_empty(), "{calls}");
    assert!(
        polls
            .iter()
            .all(|p| p.contains(&format!("/commits/{UPDATED_HEAD}/"))),
        "{calls}"
    );
    assert!(
        calls.contains(&format!("--match-head-commit {UPDATED_HEAD}")),
        "{calls}"
    );
}

#[test]
fn a_head_that_never_changes_after_update_branch_needs_a_human_merge() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("updated-12.lag", "1000");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let head = git(&e.worktree(UNIT), &["rev-parse", "HEAD"]);
    assert_eq!(
        v["reason"],
        format!(
            "PR #12 head {head} did not change after update-branch within 3 min; needs a human merge"
        )
    );
    let calls = e.gh_calls();
    assert_eq!(head_polls(&e), 9, "{calls}");
    assert_eq!(update_branch_events(&e), 0);
    assert!(register_polls(&e).is_empty(), "{calls}");
    assert!(!calls.contains("pr merge"), "{calls}");
}

#[test]
fn a_dirty_view_while_waiting_for_the_head_goes_back_to_build() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("updated-12.merge", "DIRTY");
    e.gh_file("updated-12.lag", "1000");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let p = e.prompt(6, "build");
    assert!(p.contains("rebase onto main"), "{p}");
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

fn register_polls(e: &Env) -> Vec<String> {
    e.gh_calls()
        .lines()
        .filter(|l| l.ends_with("/check-runs --jq .total_count"))
        .map(String::from)
        .collect()
}

#[test]
fn checks_that_register_late_are_watched_then_merged() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "2");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    let third = calls
        .match_indices("/check-runs")
        .nth(2)
        .expect("3 polls")
        .0;
    let watch = calls.find("pr checks 12 --watch").unwrap();
    assert!(third < watch, "{calls}");
    assert_eq!(register_polls(&e).len(), 3, "{calls}");
}

#[test]
fn no_checks_registered_within_the_timeout_needs_a_human_merge() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "1000");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        v["reason"],
        "no CI checks registered within 3 min on PR #12; needs a human merge"
    );
    let calls = e.gh_calls();
    assert!(!calls.contains("--watch"), "{calls}");
    assert!(!calls.contains("pr merge"), "{calls}");
    assert_eq!(register_polls(&e).len(), 9, "{calls}");
}

#[test]
fn a_shorter_register_timeout_polls_on_the_capped_schedule() {
    let e = Env::new();
    e.factory(&format!("{AUTO}ci_register_timeout = 1\n"));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "1000");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        v["reason"],
        "no CI checks registered within 1 min on PR #12; needs a human merge"
    );
    assert_eq!(register_polls(&e).len(), 5, "{}", e.gh_calls());
}

#[test]
fn status_only_ci_registers_and_is_watched() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "1000");
    e.gh_file("status.after", "1");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(calls.contains("pr checks 12 --watch"), "{calls}");
    assert_eq!(register_polls(&e).len(), 2, "{calls}");
}

#[test]
fn a_failing_check_query_is_reported_not_called_no_ci() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("check-runs.fail", "HTTP 403: rate limit exceeded");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("could not query CI checks on PR #12: "),
        "{reason}"
    );
    assert!(reason.contains("rate limit exceeded"), "{reason}");
    assert!(reason.ends_with("; needs a human merge"), "{reason}");
    assert!(!e.gh_calls().contains("--watch"), "{}", e.gh_calls());
}

#[test]
fn malformed_check_query_output_is_reported_not_called_no_ci() {
    for output in ["", "null", "<html>Bad Gateway</html>"] {
        let e = Env::new();
        e.factory(AUTO);
        e.ctl("pr", "12");
        e.queue("build", &["pass:commit"]);
        e.gh_file("check-runs.output", output);
        let v = e.run(&["run", "--issue", "7"], 0);
        assert_eq!(v["outcome"], "done", "{v}");
        let reason = v["reason"].as_str().unwrap();
        assert!(
            reason.starts_with("could not query CI checks on PR #12: "),
            "{output:?}: {reason}"
        );
        assert!(reason.ends_with("; needs a human merge"), "{reason}");
        assert!(!e.gh_calls().contains("--watch"), "{}", e.gh_calls());
    }
}

#[test]
fn zero_check_runs_and_a_failing_status_query_is_reported_not_called_no_ci() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "1000");
    e.gh_file("status.fail", "HTTP 500: server error");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("could not query CI checks on PR #12: "),
        "{reason}"
    );
    assert!(reason.contains("server error"), "{reason}");
    assert!(!e.gh_calls().contains("--watch"), "{}", e.gh_calls());
}

#[test]
fn a_status_with_checks_proceeds_though_the_check_runs_query_fails() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("check-runs.fail", "HTTP 403: rate limit exceeded");
    e.gh_file("status.after", "0");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(calls.contains("pr checks 12 --watch"), "{calls}");
    assert!(calls.contains("pr merge 12"), "{calls}");
}

const PRINT_HEAD_SHA_CMD: &str = "git rev-parse HEAD";

/// main gains an unrelated commit after review. `pass:script` in ship rebases the unit onto
/// it; `pass:commit` changes the unit's content instead. The PR head is the sha `print_pr_head_sha`
/// prints before the rebase; `setup` runs before the unit does.
fn rebased_after_review(
    ship: &[&str],
    print_pr_head_sha: &str,
    setup: impl FnOnce(&Env),
) -> (Env, Value, i32) {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", ship);
    git(&e.root, &["checkout", "-qb", "upstream"]);
    fs::write(e.root.join("UPSTREAM"), "x\n").unwrap();
    git(&e.root, &["add", "UPSTREAM"]);
    git(&e.root, &["commit", "-q", "-m", "upstream"]);
    let upstream = git(&e.root, &["rev-parse", "HEAD"]);
    git(&e.root, &["checkout", "-q", "main"]);
    e.ctl(
        "ship.sh",
        &format!(
            "({print_pr_head_sha}) | tee \"$FAKE_GH_DIR/pr-12.head\" > \"$FAKE_GH_DIR/pr-12.shipped\"\n\
             git update-ref refs/heads/main {upstream}\n\
             git -c user.name=f -c user.email=f@f rebase -q main\n"
        ),
    );
    e.gh_file("checks-12.json", GREEN);
    setup(&e);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let v = serde_json::from_slice(&out.stdout).unwrap();
    (e, v, out.status.code().unwrap())
}

#[test]
fn a_rebase_after_review_still_merges() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    let wt = e.worktree(UNIT);
    let review = fs::read_to_string(wt.join(format!(".ns/{UNIT}/review.md"))).unwrap();
    let head = git(&wt, &["rev-parse", "--short", "HEAD"]);
    assert!(!review.contains(&format!("sha: {head}")), "{review}");
}

#[test]
fn a_pr_head_from_before_the_rebase_merges_at_that_head() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let old = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    let wt = e.worktree(UNIT);
    assert_ne!(old.trim(), git(&wt, &["rev-parse", "HEAD"]));
    let want = format!("--match-head-commit {}", old.trim());
    assert!(e.gh_calls().contains(&want), "{}", e.gh_calls());
}

const UPDATED_HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn a_behind_pr_merges_at_the_head_update_branch_returns() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.gh_file("pr-12.merge", "BEHIND");
        e.gh_file("updated-12.head", UPDATED_HEAD);
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let calls = e.gh_calls();
    assert!(calls.contains("pr update-branch 12"), "{calls}");
    let want = format!("--match-head-commit {UPDATED_HEAD}");
    assert!(calls.contains(&want), "{calls}");
    let polls = register_polls(&e);
    assert!(!polls.is_empty(), "{calls}");
    let update = calls.find("pr update-branch 12").unwrap();
    let first_poll = calls.find("/check-runs").unwrap();
    let watch = calls.find("pr checks 12 --watch").unwrap();
    assert!(update < first_poll && first_poll < watch, "{calls}");
    assert!(
        polls
            .iter()
            .all(|p| p.contains(&format!("/commits/{UPDATED_HEAD}/"))),
        "{calls}"
    );
}

#[test]
fn a_behind_pr_without_an_updated_head_merges_at_the_pre_update_head() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.gh_file("pr-12.merge", "BEHIND");
        e.gh_file("updated-12.head", "none");
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let old = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    assert_eq!(old.trim(), "none");
    let calls = e.gh_calls();
    let merge = calls.lines().find(|l| l.starts_with("pr merge")).unwrap();
    let head = merge.rsplit(' ').next().unwrap();
    let shipped = fs::read_to_string(e.ghd.join("pr-12.shipped")).unwrap();
    assert_eq!(head, shipped.trim(), "{merge}");
    assert_ne!(head, git(&e.worktree(UNIT), &["rev-parse", "HEAD"]));
    assert_ne!(head, UPDATED_HEAD);
}

#[test]
fn a_dry_run_on_a_rebased_unit_sees_its_reviewed_artifacts_as_current() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let v = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(v["decision"]["action"], "done", "{v}");
}

#[test]
fn a_pr_head_with_other_content_needs_a_human_merge() {
    let other = "echo other > other.txt; git add other.txt; \
                 git -c user.name=f -c user.email=f@f commit -qm other; \
                 git rev-parse HEAD; git reset -q --hard HEAD~1";
    let (e, v, code) = rebased_after_review(&["pass:script"], other, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.contains("is not current with HEAD") && reason.ends_with("needs a human merge"),
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_content_change_after_review_reverifies_reviews_and_ships() {
    let (e, v, code) = rebased_after_review(&["pass:commit", "pass"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "verify", "review", "ship"]
    );
    let hist = e.worktree(UNIT).join(format!(".ns/{UNIT}/history"));
    assert!(hist.join("pr-1.md").is_file());
}

#[test]
fn a_content_change_after_every_review_runs_out_of_attempts() {
    let (e, v, code) =
        rebased_after_review(&["pass:commit", "pass:commit"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["reason"], "verify is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "verify", "review", "ship"]
    );
    assert!(!e.gh_calls().contains("pr merge"));
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

/// main holds work.txt with a human-review region; the build commit runs `edit` on it.
fn marked(edit: &str) -> Value {
    let e = Env::new();
    // Spelled out at run time so this file holds no real markers.
    let (start, end) = ("ns:human-review start", "ns:human-review end");
    fs::write(
        e.root.join("work.txt"),
        format!("top\n# {start}: brake torque limits\nlimit = 5\n# {end}\nbottom\n"),
    )
    .unwrap();
    git(&e.root, &["add", "work.txt"]);
    git(&e.root, &["commit", "-q", "-m", "marked"]);
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.ctl("edit", edit);
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "work.txt\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(
        v["outcome"] == "merged",
        e.gh_calls().contains("pr merge"),
        "{v}"
    );
    v
}

#[test]
fn an_edit_in_a_human_review_region_needs_a_human_merge() {
    let v = marked("sed -i 's/limit = 5/limit = 9/' work.txt");
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        v["reason"],
        "changes code in a human-review region; needs a human merge (work.txt:2-4: brake torque limits)"
    );
}

#[test]
fn an_edit_outside_a_human_review_region_merges() {
    let v = marked("sed -i 's/bottom/BOTTOM/' work.txt");
    assert_eq!(v["outcome"], "merged", "{v}");
}

#[test]
fn an_unreadable_merge_base_needs_a_human_merge() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.ctl("edit", "git update-ref -d refs/heads/main");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "work.txt\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("cannot check human-review regions:"),
        "{v}"
    );
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

/// `ns watch` stdout and stderr with `TZ` and `NS_NOW` overridden.
fn watch_in(e: &Env, tz: &str, now: i64, args: &[&str]) -> (Value, String) {
    let out = e
        .ns()
        .env("TZ", tz)
        .env("NS_NOW", now.to_string())
        .arg("watch")
        .args(args)
        .assert()
        .code(0)
        .get_output()
        .clone();
    (
        serde_json::from_slice(&out.stdout).unwrap(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn watch_until_is_local_time_with_its_offset() {
    let e = Env::new();
    // 2026-10-08T20:00 EDT.
    let (v, _) = watch_in(
        &e,
        "America/Toronto",
        NOW,
        &["--dry-run", "--until", "06:30"],
    );
    assert_eq!(v["until"], "2026-10-09T06:30:00-04:00");
    // 2026-01-14T19:00 EST.
    let (v, _) = watch_in(
        &e,
        "America/Toronto",
        1_768_435_200,
        &["--dry-run", "--until", "06:30"],
    );
    assert_eq!(v["until"], "2026-01-15T06:30:00-05:00");
    let (v, _) = watch_in(&e, "UTC", NOW, &["--dry-run", "--until", "06:30"]);
    assert_eq!(v["until"], "2026-10-09T06:30:00+00:00");
}

#[test]
fn watch_until_keeps_half_hour_and_positive_offsets() {
    let e = Env::new();
    let (v, _) = watch_in(&e, "Asia/Kolkata", NOW, &["--dry-run", "--until", "06:30"]);
    assert_eq!(v["until"], "2026-10-09T06:30:00+05:30");
    let (v, _) = watch_in(
        &e,
        "America/St_Johns",
        NOW,
        &["--dry-run", "--until", "06:30"],
    );
    assert_eq!(v["until"], "2026-10-09T06:30:00-02:30");
}

#[test]
fn watch_reports_until_and_the_usage_reset_in_local_time() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("build", &["limit", "pass:commit"]);
    let (v, err) = watch_in(&e, "America/Toronto", NOW, &["--once", "--until", "06:30"]);
    assert_eq!(v["until"], "2026-10-09T06:30:00-04:00", "{v}");
    assert_eq!(
        v["units"][0]["reset_at"], "2026-10-08T21:00:00-04:00",
        "{v}"
    );
    assert!(
        err.contains("sleeping until 2026-10-08T21:00:00-04:00"),
        "{err}"
    );
    let log = fs::read_to_string(e.root.join(".git/ns/runs.jsonl")).unwrap();
    assert!(log.lines().count() > 0);
    for line in log.lines() {
        let ts = serde_json::from_str::<Value>(line).unwrap()["ts"].clone();
        assert!(ts.as_str().unwrap().ends_with('Z'), "{line}");
    }
}

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
    assert!(v["until"].is_null());
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

fn queue_order(v: &serde_json::Value) -> Vec<(u64, String, String)> {
    v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["number"].as_u64().unwrap(),
                i["priority_label"].as_str().unwrap_or("").to_string(),
                i["order_label"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

#[test]
fn watch_sorts_by_priority_before_type() {
    let e = Env::new();
    e.ready(1, "Low fix", &["type:fix", "priority:low"], "");
    e.ready(2, "High docs", &["type:docs", "priority:high"], "");
    e.ready(3, "Unprioritised fix", &["type:fix"], "");
    e.ready(4, "Medium feat", &["type:feat", "priority:medium"], "");
    e.ready(5, "Medium fix", &["priority:medium", "type:fix"], "");
    e.ready(6, "High fix", &["type:fix", "priority:high"], "");
    let v = e.run(&["watch", "--dry-run"], 0);
    let s = |x: &str| x.to_string();
    assert_eq!(
        queue_order(&v),
        [
            (6, s("priority:high"), s("type:fix")),
            (2, s("priority:high"), s("type:docs")),
            (5, s("priority:medium"), s("type:fix")),
            (4, s("priority:medium"), s("type:feat")),
            (1, s("priority:low"), s("type:fix")),
            (3, s(""), s("type:fix")),
        ]
    );
    assert!(v["queue"][5]["priority_label"].is_null());
}

#[test]
fn watch_uses_a_custom_priority_list() {
    let e = Env::new();
    e.factory("[queue]\npriority = [\"p0\", \"p1\"]\n");
    e.ready(1, "p1 fix", &["type:fix", "p1"], "");
    e.ready(2, "p0 docs", &["type:docs", "p0"], "");
    e.ready(3, "default label", &["type:fix", "priority:high"], "");
    let v = e.run(&["watch", "--dry-run"], 0);
    let order: Vec<u64> = queue_order(&v).iter().map(|q| q.0).collect();
    assert_eq!(order, [2, 1, 3]);
    assert_eq!(v["queue"][0]["priority_label"], "p0");
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
fn watch_stuck_comment_quotes_the_blocker_and_a_relative_path() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["blocked:needs a board"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck");
    let unit = v["units"][0]["unit"].as_str().unwrap();
    let calls = e.gh_calls();
    let comment = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(
        comment.contains("evidence.md is blocked: verify said blocked needs a board"),
        "{comment}"
    );
    assert!(
        comment.contains(&format!("`.ns/{unit}/evidence.md` on branch `ns/{unit}`")),
        "{comment}"
    );
    assert!(!comment.contains(e.base.to_str().unwrap()), "{comment}");
}

#[test]
fn a_blocked_review_stops_at_once_and_the_comment_quotes_it() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["blocked:C1 open after 3 cycles"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review"]);
    let calls = e.gh_calls();
    let comment = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(
        comment.contains("review.md is blocked: review said blocked C1 open after 3 cycles"),
        "{comment}"
    );
}

#[test]
fn a_blocked_review_comment_lists_its_open_findings() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["blocked:Open after 3 fix cycles: C1, I2."]);
    e.ctl(
        "review.body",
        "\n## Critical\n### C1. cancel_all drops every SKU\n- Location: `src/w.py:119`\n- Status: open\n\
         \n## Important\n### I1. Fixed thing\n- Location: `src/w.py:3`\n- Status: fixed (cycle 1, abc)\n\
         ### I2. No test for UnknownItem\n- Location: `src/w.py:118`\n- Status: open. The fix did not land.\n\
         ### I3. Old code smell\n- Location: `src/old.py:9`\n- Status: deferred: #77\n\
         \n## Suggestion\n### S1. Rename x\n- Location: `src/w.py:5`\n- Status: open\n",
    );
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck");
    let calls = e.gh_calls();
    let comment = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(
        comment.contains(
            "Open findings in `review.md`:\n```text\nC1. cancel_all drops every SKU (src/w.py:119)\nI2. No test for UnknownItem (src/w.py:118)\n```"
        ),
        "{comment}"
    );
    for gone in ["I1.", "I3.", "S1."] {
        assert!(!comment.contains(gone), "{gone} in {comment}");
    }
}

#[test]
fn a_run_stopped_by_an_earlier_blocked_review_names_it() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["blocked:Open after 3 fix cycles: I2."]);
    let first = e.run(&["run", "--issue", "7"], 1);
    let again = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(e.calls(), ["triage", "build", "verify", "review"]);
    assert_eq!(again["outcome"], "stuck");
    assert_eq!(again["artifact"], first["artifact"]);
    assert!(
        again["artifact"].as_str().unwrap().ends_with("/review.md"),
        "{again}"
    );
}

#[test]
fn a_failed_review_goes_back_to_build() {
    let e = Env::new();
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("review", &["fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "build", "verify", "review", "ship"]
    );
    let p = e.prompt(5, "build");
    assert!(p.contains("review said fail"), "{p}");
}

const READY: &str = "status:ready-for-agent";

fn labels_after_triage_adds(e: &Env, stray: &str, args: &[&str]) -> Vec<String> {
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl(
        "triage.sh",
        &format!("gh issue edit 2 --add-label {stray}\n"),
    );
    e.queue("triage", &["pass:script"]);
    e.run(args, 0);
    let labels = e.labels(2);
    assert!(labels.contains(&"type:fix".to_string()), "{labels:?}");
    labels
}

#[test]
fn watch_done_leaves_only_the_done_label() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:in-review"]);
}

#[test]
fn watch_stuck_leaves_only_the_stuck_label() {
    let e = Env::new();
    e.queue("verify", &["blocked:needs a board"]);
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:ready-for-human"]);
}

#[test]
fn watch_merged_leaves_no_status_label() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once"]);
    assert_eq!(labels, ["type:fix"]);
}

#[test]
fn watch_given_back_leaves_only_the_ready_label() {
    let e = Env::new();
    e.ctl("reset", &(NOW + 8 * 3600).to_string());
    e.queue("build", &["limit"]);
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once", "--until", "06:30"]);
    assert_eq!(labels, ["type:fix", "status:ready-for-agent"]);
}

#[test]
fn watch_budget_leaves_only_the_ready_label() {
    let e = Env::new();
    e.factory("[limits]\nbudget_usd = 1.0\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl(
        "triage.sh",
        &format!("gh issue edit 2 --add-label {READY}\n"),
    );
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "budget", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
}

#[test]
fn watch_done_removes_an_unconfigured_status_label() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    let labels = labels_after_triage_adds(&e, "status:needs-info", &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:in-review"]);
}

#[test]
fn watch_stuck_removes_an_unconfigured_status_label() {
    let e = Env::new();
    e.queue("verify", &["blocked:needs a board"]);
    let labels = labels_after_triage_adds(&e, "status:needs-info", &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:ready-for-human"]);
}

#[test]
fn watch_merged_removes_an_unconfigured_status_label() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let labels = labels_after_triage_adds(&e, "status:needs-info", &["watch", "--once"]);
    assert_eq!(labels, ["type:fix"]);
}

#[test]
fn watch_restores_the_ready_label_when_a_run_errors() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl(
        "triage.sh",
        "gh issue edit 2 --add-label status:ready-for-agent\n",
    );
    e.ctl("verify.sh", "touch \".ns/$NS_UNIT/history\"\n");
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["fail:script"]);
    let out = e.ns().args(["watch", "--once"]).output();
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
}

#[test]
fn watch_needing_a_human_leaves_only_the_stuck_label() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", ".github/workflows/ci.yml\n");
    let labels = labels_after_triage_adds(&e, READY, &["watch", "--once"]);
    assert_eq!(labels, ["type:fix", "status:ready-for-human"]);
    let calls = e.gh_calls();
    let comment = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(comment.contains("needs a human on unit"), "{comment}");
    assert!(!comment.contains("got stuck"), "{comment}");
}

#[test]
fn watch_removes_a_stray_custom_ready_label() {
    let e = Env::new();
    e.factory(
        "[queue]\nready_label = \"ready\"\nin_progress_label = \"wip\"\ndone_label = \"shipped\"\nstuck_label = \"blocked\"\n",
    );
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("labels-2", "type:fix\nready\n");
    e.ctl("triage.sh", "gh issue edit 2 --add-label ready\n");
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(2), ["type:fix", "shipped"]);
}

#[test]
fn watch_retries_a_label_edit_that_did_not_land() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("edit.drop-once", "");
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(2), ["type:fix", "status:in-review"]);
    let dropped =
        "issue edit 2 --remove-label status:ready-for-agent --add-label status:in-progress";
    assert!(
        e.gh_calls().matches(dropped).count() >= 2,
        "{}",
        e.gh_calls()
    );
}

#[test]
fn watch_fails_loudly_when_a_label_edit_does_not_land() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("edit.noop", "");
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("#2") && err.contains("status:ready-for-agent"),
        "{err}"
    );
    assert!(e.calls().is_empty());
}

#[test]
fn watch_fails_loudly_when_the_end_label_edit_does_not_land() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("triage.sh", "touch \"$FAKE_GH_DIR/edit.noop\"\n");
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("#2") && err.contains("missing status:in-review"),
        "{err}"
    );
}

#[test]
fn watch_posts_the_stuck_comment_even_when_the_label_edit_fails() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("triage.sh", "touch \"$FAKE_GH_DIR/edit.noop\"\n");
    e.queue("triage", &["pass:script"]);
    e.queue("verify", &["blocked:needs a board"]);
    e.ns().args(["watch", "--once"]).assert().code(1);
    assert!(e.gh_calls().contains("issue comment 2"), "{}", e.gh_calls());
}

#[test]
fn watch_closes_the_merged_issue_even_when_the_label_edit_fails() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.ctl("triage.sh", "touch \"$FAKE_GH_DIR/edit.noop\"\n");
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.ns().args(["watch", "--once"]).assert().code(1);
    assert!(e.gh_calls().contains("issue close 2"), "{}", e.gh_calls());
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
fn watch_keeps_its_start_config_when_the_files_break_between_units() {
    let e = Env::new();
    e.config("[forge.github]\ntoken_env = \"MY_GH_TOKEN\"\n");
    e.factory("[defaults]\nmax_attempts = 3\n");
    let factory = e.root.join(".nightshift/nightshift.toml");
    let config = e.base.join("no-config.toml");
    let sha = |p: &Path| {
        Sha256::digest(fs::read(p).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let config_sha = sha(&config);
    let factory_sha = sha(&factory);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.ctl(
        "triage.sh",
        &format!(
            "echo '[forge]\nbogus = 1' > {0}\necho 'not = [toml' > {1}\n",
            config.display(),
            factory.display()
        ),
    );
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit", "pass:commit"]);
    let out = e
        .ns()
        .env("MY_GH_TOKEN", FAKE_TOKEN)
        .args(["watch", "--max-units", "2"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let outcomes: Vec<&str> = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["done", "done"], "{v}");
    assert!(fs::read_to_string(&config).unwrap().contains("bogus"));
    let started = &v["started_with"];
    assert_eq!(started["config"]["path"], config.to_str().unwrap());
    assert_eq!(started["factory"]["path"], factory.to_str().unwrap());
    assert_eq!(started["config"]["sha256"], config_sha.as_str());
    assert_eq!(started["factory"]["sha256"], factory_sha.as_str());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(config.to_str().unwrap()), "{err}");
    assert!(
        err.contains(started["config"]["sha256"].as_str().unwrap()),
        "{err}"
    );
    assert!(err.contains(factory.to_str().unwrap()), "{err}");
    assert!(err.contains(&factory_sha), "{err}");
    assert!(!err.contains(FAKE_TOKEN), "{err}");
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
    assert_eq!(units[0]["reset_at"], "2026-10-09T01:00:00+00:00");
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
fn watch_closes_a_merged_issue_and_never_restarts_it() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("ready-sticks", "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    // A cap above one, so a restart fails the test instead of looping.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(v["units"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(v["stopped"], "queue empty");
    let calls = e.gh_calls();
    assert_eq!(
        calls.matches("labels=status:ready-for-agent").count(),
        2,
        "{calls}"
    );
    // The merged issue carries no status label now, but it is not triaged again.
    assert!(v["triaged"].as_array().unwrap().is_empty(), "{v}");
    assert!(
        calls.contains("issue close 2 --reason completed"),
        "{calls}"
    );
}

#[test]
fn watch_keeps_going_when_the_merged_issue_is_already_closed() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("close.fail", "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert!(e.gh_calls().contains("issue close 2"));
}

#[test]
fn watch_never_restarts_an_issue_it_finished() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("ready-sticks", "");
    e.queue("build", &["pass:commit", "pass:commit"]);
    // A cap above one, so a restart fails the test instead of looping.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(v["units"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["units"][0]["outcome"], "done");
    assert_eq!(v["stopped"], "queue empty");
    assert!(!e.gh_calls().contains("issue close"));
}

#[test]
fn watch_skips_an_issue_a_merged_pr_closes() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.gh_file("prs-merged.json", r#"[{"number":41,"body":"Fixes #3"}]"#);
    let v = e.run(&["watch", "--dry-run"], 0);
    let order: Vec<u64> = queue_order(&v).iter().map(|q| q.0).collect();
    assert_eq!(order, [2]);
    assert_eq!(v["skipped"][0]["number"], 3);
    assert_eq!(v["skipped"][0]["reason"], "merged PR #41 closes it");
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

#[test]
fn watch_pauses_on_a_session_limit_reported_only_in_structured_fields() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("build", &["session", "pass:commit"]);
    let v = e.run(&["watch", "--once", "--until", "06:30"], 0);
    let units = v["units"].as_array().unwrap();
    assert_eq!(units[0]["outcome"], "paused", "{v}");
    assert_eq!(units[0]["reset_at"], "2026-10-09T01:00:00+00:00");
    assert_eq!(units[1]["outcome"], "done");
}

#[test]
fn watch_stops_the_night_when_the_harness_keeps_failing_instantly() {
    let e = Env::new();
    for n in [2, 3, 4] {
        e.ready(n, &format!("Fix {n}"), &["type:fix"], "");
    }
    e.queue("triage", &["crash"; 6]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "harness failing", "{v}");
    let units = v["units"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{v}");
    assert!(units.iter().all(|u| u["outcome"] == "harness_failing"));
    let calls = e.gh_calls();
    assert!(
        calls.contains(
            "issue edit 2 --remove-label status:in-progress --add-label status:ready-for-agent"
        ),
        "{calls}"
    );
    assert!(
        calls.contains(
            "issue edit 3 --remove-label status:in-progress --add-label status:ready-for-agent"
        ),
        "{calls}"
    );
    assert!(!calls.contains("issue edit 4"), "{calls}");
    assert!(!calls.contains("got stuck"), "{calls}");
}

// ---------------------------------------------------------------- the triage pass

fn numbers(v: &Value) -> Vec<u64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().or(i["issue"].as_u64()).unwrap())
        .collect()
}

fn outcomes(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i["outcome"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn watch_triages_a_needs_triage_issue_ready_and_builds_it_the_same_night() {
    let e = Env::new();
    e.untriaged(5, "Fix a follow-up", &["status:needs-triage", "type:fix"]);
    e.triage_sets(&["status:ready-for-agent", "priority:high"]);
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(v["triaged"][0]["outcome"], "done", "{v}");
    assert_eq!(
        v["triaged"][0]["state"],
        serde_json::json!(["status:ready-for-agent"])
    );
    assert_eq!(numbers(&v["units"]), [5], "{v}");
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(v["stopped"], "queue empty");
    // The unit picks up the brief the triage-only run wrote, so triage runs once.
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship"],
        "{v}"
    );
    assert!(e.prompt(1, "triage").contains("Triage only"));
    assert!(!e.prompt(2, "build").contains("Triage only"));
    let calls = e.gh_calls();
    let triaged = calls
        .find("issue edit 5 --remove-label status:needs-triage --add-label status:ready-for-agent")
        .expect(&calls);
    let started = calls.find("--add-label status:in-progress").expect(&calls);
    assert!(triaged < started, "{calls}");
    assert_eq!(
        e.labels(5),
        ["type:fix", "priority:high", "status:in-review"]
    );
}

#[test]
fn watch_dry_run_lists_triage_candidates_and_the_cap() {
    let e = Env::new();
    e.ready(2, "Ready", &["type:fix"], "");
    e.untriaged(5, "Needs triage", &["status:needs-triage", "priority:low"]);
    e.untriaged(6, "No status", &["type:fix", "priority:high"]);
    e.untriaged(8, "Needs info", &["status:needs-info"]);
    e.untriaged(9, "In review", &["status:in-review"]);
    e.open_by(10, "Stranger", &["status:needs-triage"], "", Some("NONE"));
    e.untriaged(11, "Open PR", &[]);
    e.untriaged(12, "Merged PR", &[]);
    e.untriaged(13, "Plain", &[]);
    e.gh_file("prs.json", r#"[{"number":40,"body":"Closes #11"}]"#);
    e.gh_file("prs-merged.json", r#"[{"number":41,"body":"Fixes #12"}]"#);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(numbers(&v["queue"]), [2], "{v}");
    assert_eq!(numbers(&v["triage"]), [6, 5, 13], "{v}");
    assert_eq!(v["triage"][0]["reason"], "no status label");
    assert_eq!(v["triage"][1]["reason"], "status:needs-triage");
    let skipped: Vec<(u64, &str)> = v["triage_skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["number"].as_u64().unwrap(), i["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        skipped,
        [
            (10, "author outside the team"),
            (11, "open PR #40 closes it"),
            (12, "merged PR #41 closes it"),
        ]
    );
    assert_eq!(v["triage_per_night"], 10);
    assert!(e.calls().is_empty());
    assert!(!e.gh_calls().contains("issue edit"));
    assert!(e
        .gh_calls()
        .contains("api --paginate repos/{owner}/{repo}/issues?state=open&per_page=100 --jq"));
}

#[test]
fn watch_triages_a_custom_triage_label() {
    let e = Env::new();
    e.factory("[queue]\ntriage_label = \"needs-triage\"\n");
    e.untriaged(5, "Custom", &["needs-triage"]);
    e.untriaged(6, "Default label only", &["status:needs-triage"]);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(numbers(&v["triage"]), [5], "{v}");
    assert_eq!(v["triage"][0]["reason"], "needs-triage");
}

#[test]
fn a_newly_ready_issue_sorts_in_by_priority() {
    let e = Env::new();
    e.ready(2, "Low fix", &["type:fix", "priority:low"], "");
    e.untriaged(5, "Escape", &["status:needs-triage", "type:fix"]);
    e.triage_sets(&["status:ready-for-agent", "priority:high"]);
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    // Triage-only runs are not units: --once still builds one.
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(numbers(&v["units"]), [5], "{v}");
    assert_eq!(v["stopped"], "max_units");
    assert!(!e.gh_calls().contains("issue edit 2"));
}

#[test]
fn a_triage_that_leaves_the_issue_untriaged_is_not_retried_that_night() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.untriaged(5, "Unclear", &["status:needs-triage"]);
    e.queue("triage", &["none"]);
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(v["triaged"][0]["outcome"], "done");
    assert_eq!(
        v["triaged"][0]["state"],
        serde_json::json!(["status:needs-triage"])
    );
    assert_eq!(numbers(&v["units"]), [2, 3], "{v}");
    assert_eq!(e.triage_only_runs(), 1);
    assert_eq!(e.labels(5), ["status:needs-triage"]);
}

#[test]
fn triage_per_night_caps_the_triage_only_runs() {
    let e = Env::new();
    e.factory("[queue]\ntriage_per_night = 1\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Low", &["status:needs-triage", "priority:low"]);
    e.untriaged(6, "High", &["status:needs-triage", "priority:high"]);
    e.queue("triage", &["none"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [6], "{v}");
    assert_eq!(numbers(&v["units"]), [2], "{v}");
    assert_eq!(e.triage_only_runs(), 1);
}

#[test]
fn a_zero_cap_runs_no_triage() {
    let e = Env::new();
    e.factory("[queue]\ntriage_per_night = 0\n");
    e.untriaged(5, "Untriaged", &["status:needs-triage"]);
    let v = e.run(&["watch"], 0);
    assert!(v["triaged"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["stopped"], "queue empty");
    assert!(e.calls().is_empty());
}

#[test]
fn a_paused_triage_sleeps_and_retries_without_spending_the_cap() {
    let e = Env::new();
    e.factory("[queue]\ntriage_per_night = 1\n");
    e.untriaged(5, "Escape", &["status:needs-triage"]);
    e.ctl("reset", &(NOW + 3600).to_string());
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage", &["limit", "pass:script"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--until", "06:30"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["paused", "done"], "{v}");
    assert_eq!(v["triaged"][0]["reset_at"], "2026-10-09T01:00:00+00:00");
    assert_eq!(numbers(&v["units"]), [5], "{v}");
    assert_eq!(v["units"][0]["outcome"], "done");
}

#[test]
fn a_triage_paused_past_until_ends_the_night_and_touches_no_label() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Escape", &["status:needs-triage"]);
    e.ctl("reset", &(NOW + 8 * 3600).to_string());
    e.queue("triage", &["limit"]);
    let v = e.run(&["watch", "--until", "06:30"], 0);
    assert_eq!(v["stopped"], "usage limit resets after --until", "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["paused"], "{v}");
    assert!(v["units"].as_array().unwrap().is_empty(), "{v}");
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn triage_runs_that_keep_failing_instantly_stop_the_night() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.queue("triage", &["crash", "crash"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "harness failing", "{v}");
    assert_eq!(
        outcomes(&v["triaged"]),
        ["harness_failing", "harness_failing"]
    );
    assert_eq!(v["triaged"][0]["reason"], "exited 1");
    assert!(v["units"].as_array().unwrap().is_empty(), "{v}");
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn a_triage_that_works_resets_the_harness_failure_count() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    // 5 crashes, 6 works, then every attempt of unit 2's triage phase crashes.
    e.queue("triage", &["crash", "none", "crash", "crash"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["harness_failing", "done"], "{v}");
    assert_eq!(outcomes(&v["units"]), ["harness_failing"], "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
}

#[test]
fn a_failing_triage_and_a_failing_unit_share_the_harness_breaker() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.queue("triage", &["crash", "crash", "crash"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "harness failing", "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["harness_failing"], "{v}");
    assert_eq!(numbers(&v["units"]), [2], "{v}");
    assert!(!e.gh_calls().contains("issue edit 3"));
}

#[test]
fn a_timed_out_triage_is_recorded_and_the_night_goes_on() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Slow", &["status:needs-triage"]);
    e.queue("triage", &["pass:sleep"]);
    e.queue("build", &["pass:commit"]);
    let out = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "triage=1500")
        .args(["watch"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(outcomes(&v["triaged"]), ["stuck"], "{v}");
    let reason = v["triaged"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("timed out"), "{reason}");
    assert_eq!(numbers(&v["units"]), [2], "{v}");
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.labels(5), ["status:needs-triage"]);
}

#[test]
fn a_killed_triage_is_recorded_and_the_night_goes_on() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Killed", &["status:needs-triage"]);
    // Kill the harness itself, so ns sees a signal instead of an exit code.
    e.ctl("triage.sh", "kill -KILL $PPID\n");
    e.queue("triage", &["pass:script"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["stuck"], "{v}");
    assert_eq!(v["triaged"][0]["reason"], "killed by a signal");
    assert_eq!(numbers(&v["units"]), [2], "{v}");
}

#[test]
fn an_unreadable_state_after_triage_is_recorded_as_null() {
    let e = Env::new();
    e.untriaged(5, "Escape", &["status:needs-triage"]);
    e.gh_file("labels-5.fail", "");
    e.queue("triage", &["none"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["done"], "{v}");
    assert!(v["triaged"][0]["state"].is_null(), "{v}");
    assert_eq!(v["stopped"], "queue empty");
}

#[test]
fn the_budget_stops_the_triage_pass() {
    let e = Env::new();
    e.factory("[limits]\nbudget_usd = 0.4\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.queue("triage", &["none", "none"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "budget", "{v}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert!(v["units"].as_array().unwrap().is_empty(), "{v}");
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn a_triage_run_that_errors_is_recorded_and_the_night_goes_on() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Unreadable", &["status:needs-triage"]);
    e.untriaged(6, "Fine", &["status:needs-triage"]);
    fs::remove_file(e.ghd.join("issue-5.json")).unwrap();
    e.queue("triage", &["none"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["error", "done"], "{v}");
    let reason = v["triaged"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("cannot read issue #5"), "{reason}");
    assert_eq!(numbers(&v["units"]), [2], "{v}");
    assert_eq!(e.triage_only_runs(), 1);
}

#[test]
fn a_triage_only_run_triages_again_over_an_earlier_brief() {
    let e = Env::new();
    e.untriaged(5, "Escape", &["status:needs-triage"]);
    e.queue("triage", &["pass", "none"]);
    e.run(&["watch"], 0);
    // The next night the issue is still untriaged, and its worktree holds the first brief.
    let v = e.run(&["watch"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["done"], "{v}");
    assert_eq!(e.calls(), ["triage", "triage"]);
}

#[test]
fn the_same_start_error_twice_turns_the_triage_pass_off() {
    let e = Env::new();
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.untriaged(8, "C", &["status:needs-triage"]);
    // A global failure: no subscription login, whichever issue the run is for.
    fs::remove_file(e.home.join(".claude/.credentials.json")).unwrap();
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["error", "error"], "{v}");
    assert_eq!(v["triaged"][0]["reason"], v["triaged"][1]["reason"]);
    assert_eq!(v["stopped"], "queue empty");
    assert!(e.calls().is_empty());
}

#[test]
fn different_start_errors_keep_the_triage_pass_on() {
    let e = Env::new();
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.untriaged(8, "C", &["status:needs-triage"]);
    for n in [5, 6] {
        fs::remove_file(e.ghd.join(format!("issue-{n}.json"))).unwrap();
    }
    e.queue("triage", &["none"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["error", "error", "done"], "{v}");
}

#[test]
fn gates_stop_runs_no_triage() {
    let e = Env::new();
    e.factory("gates = \"stop\"\n");
    e.untriaged(5, "Untriaged", &["status:needs-triage"]);
    let dry = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(dry["triage_per_night"], 0, "{dry}");
    assert!(dry["triage"].as_array().unwrap().is_empty(), "{dry}");
    assert!(
        dry["triage_skipped"].as_array().unwrap().is_empty(),
        "{dry}"
    );
    let v = e.run(&["watch"], 0);
    assert!(v["triaged"].as_array().unwrap().is_empty(), "{v}");
    assert!(e.calls().is_empty());
}

#[test]
fn a_follow_up_filed_by_a_unit_is_triaged_before_the_next_unit() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    // Unit 2's ship files follow-up #9 as needs-triage, as review files an escape.
    let line = serde_json::json!({
        "number": 9, "title": "Follow-up", "body": "",
        "labels": [{"name": "status:needs-triage"}], "authorAssociation": "MEMBER",
    });
    e.ctl(
        "ship.sh",
        &format!(
            "echo '{line}' >> \"$FAKE_GH_DIR/issues.jsonl\"\necho status:needs-triage > \"$FAKE_GH_DIR/labels-9\"\necho '{{\"number\":9,\"title\":\"Follow-up\",\"url\":\"u\",\"state\":\"OPEN\"}}' > \"$FAKE_GH_DIR/issue-9.json\"\n"
        ),
    );
    e.queue("ship", &["pass:script", "pass"]);
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("triage", &["pass", "none", "pass"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [9], "{v}");
    assert_eq!(numbers(&v["units"]), [2, 3], "{v}");
    let calls = e.calls();
    assert_eq!(calls[4..7], ["ship", "triage", "triage"], "{calls:?}");
    assert!(e.prompt(6, "triage").contains("Triage only"));
    assert!(e.prompt(6, "triage").contains("unit `9-follow-up`"));
    assert!(!e.prompt(7, "triage").contains("Triage only"));
}
