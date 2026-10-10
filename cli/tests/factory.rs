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
        e.with_remote();
        e
    }

    /// Add a bare `origin` (`base/remote.git`) with `main` pushed and origin/HEAD set.
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
    let lock = common.join(format!("ns-run-{UNIT}.lock"));
    assert_eq!(fs::read_to_string(&lock).unwrap(), "");
    assert!(lock_is_free(&lock));
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
    // The unit ran in its own ns run, which ns watch started.
    let run = run_events(&e)
        .into_iter()
        .find(|ev| ev["event"] == "worker_start")
        .unwrap()["run_pid"]
        .clone();
    assert_ne!(run, pid);
    assert_eq!(phase_ids(&e)[0], format!("{run} {pid}"));
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

const QUALITY_BRANCH: &str = "refs/heads/nightshift/quality";

/// The quality records on the bare `origin`'s data branch.
fn quality_records(e: &Env) -> Vec<Value> {
    let remote = e.base.join("remote.git");
    if git(&remote, &["for-each-ref", QUALITY_BRANCH]).is_empty() {
        return Vec::new();
    }
    git(
        &remote,
        &["show", &format!("{QUALITY_BRANCH}:records.jsonl")],
    )
    .lines()
    .map(|l| serde_json::from_str(l).unwrap())
    .collect()
}

fn quality_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "quality_record")
        .collect()
}

const FINDING: &str = "## Important\n### I1. Untested branch\n- Location: `work.txt:1`\n- Axis: tests\n- Scope: changed\n- Cycle: 0\n- Status: fixed (cycle 1, abc1234)\n";

#[test]
fn a_unit_that_ends_pushes_its_quality_record() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.ctl("review.body", FINDING);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let records = quality_records(&e);
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert_eq!(r["unit"], UNIT);
    assert_eq!(
        (r["issue"].as_u64(), r["outcome"].as_str()),
        (Some(7), Some("done"))
    );
    assert_eq!(r["artifact"], format!(".ns/{UNIT}/review.md"));
    assert_eq!(r["status"], "pass");
    assert_eq!(r["findings"][0]["id"], "I1");
    assert_eq!(r["findings"][0]["axes"], serde_json::json!(["tests"]));
    let ev = quality_events(&e);
    assert_eq!(ev.len(), 1);
    assert_eq!(
        (ev[0]["status"].as_str(), ev[0]["records"].as_u64()),
        (Some("pushed"), Some(1))
    );
    assert_eq!(ev[0]["unit"], UNIT);
}

#[test]
fn every_review_attempt_gets_a_record() {
    let e = Env::new();
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("review", &["fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let got: Vec<_> = quality_records(&e)
        .iter()
        .map(|r| {
            (
                r["attempt"].as_u64().unwrap(),
                r["attempts"].as_u64().unwrap(),
                r["status"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [(1, 2, "fail".to_string()), (2, 2, "pass".to_string())]
    );
}

#[test]
fn a_blocked_review_is_recorded_as_stuck() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["blocked:Open after 3 fix cycles: I1."]);
    e.ctl("review.body", FINDING);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    let records = quality_records(&e);
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0]["outcome"].as_str(),
            records[0]["status"].as_str()
        ),
        (Some("stuck"), Some("blocked"))
    );
}

#[test]
fn a_unit_with_no_review_writes_no_record() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["blocked:needs hardware"]);
    e.run(&["run", "--issue", "7"], 1);
    assert!(quality_records(&e).is_empty());
    let ev = quality_events(&e);
    assert_eq!(
        (ev[0]["status"].as_str(), ev[0]["attempts"].as_u64()),
        (Some("nothing"), Some(0))
    );
}

#[test]
fn a_failed_push_keeps_the_record_in_the_outbox_and_the_next_unit_pushes_it() {
    let e = Env::new();
    let remote = e.base.join("remote.git");
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            e.base.join("gone.git").to_str().unwrap(),
        ],
    );
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let outbox = e.root.join(".git/ns/quality-outbox.jsonl");
    assert_eq!(fs::read_to_string(&outbox).unwrap().lines().count(), 1);
    assert_eq!(quality_events(&e)[0]["status"], "outbox");
    assert!(quality_records(&e).is_empty());

    git(
        &e.root,
        &["remote", "set-url", "origin", remote.to_str().unwrap()],
    );
    e.issue(8, "Other thing", "OPEN");
    let v = e.run(&["run", "--issue", "8"], 0);
    assert_eq!(v["outcome"], "done");
    let units: Vec<_> = quality_records(&e)
        .iter()
        .map(|r| r["unit"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(units, [UNIT, "8-other-thing"]);
    assert!(!outbox.exists());
}

#[test]
fn lock_refuses_a_second_runner_and_takes_over_a_free_one() {
    let e = Env::new();
    let lock = e.root.join(format!(".git/ns-run-{UNIT}.lock"));
    let held = hold(&lock, r#"{"pid":42,"unit":"7-fix-the-thing","issue":7}"#);
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(5)
        .stderr(predicates::str::contains(format!(
            "another ns run (pid 42, unit {UNIT}) holds"
        )));
    assert!(e.calls().is_empty());
    drop(held);

    // A file nobody holds is free, even when it names a live pid (this test's).
    fs::write(
        &lock,
        format!("{{\"pid\":{},\"unit\":\"{UNIT}\"}}\n", std::process::id()),
    )
    .unwrap();
    e.queue("build", &["pass:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    assert!(lock_is_free(&lock));
    assert_eq!(fs::read_to_string(&lock).unwrap(), "");
}

/// An exclusive `flock` on `path`, holding `text`, as a live `ns` holds its lock; released on
/// drop.
struct Hold {
    _file: fs::File,
}

fn hold(path: &Path, text: &str) -> Hold {
    use std::os::unix::io::AsRawFd;
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .unwrap();
    // SAFETY: flock(2) on a descriptor this function owns.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(r, 0, "cannot lock {}", path.display());
    fs::write(path, text).unwrap();
    Hold { _file: f }
}

/// Set issue `n`'s labels, as a night that ended mid-unit leaves them.
fn set_labels(e: &Env, n: u64, labels: &[&str]) {
    let lines: String = labels.iter().map(|l| format!("{l}\n")).collect();
    e.gh_file(&format!("labels-{n}"), &lines);
}

fn run_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

#[test]
fn watch_returns_a_stale_in_progress_issue_to_the_queue_and_resumes_its_unit() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["crash", "crash"]);
    e.run(&["run", "--issue", "2"], 1);
    assert_eq!(e.calls(), ["triage", "build", "build"]);
    fs::remove_file(e.ctrl.join("calls")).unwrap();
    // The crashed night's lock file names a live pid (this test's), but nobody holds it.
    fs::write(
        e.root.join(".git/ns-run-2-fix-a.lock"),
        format!(
            "{{\"pid\":{},\"unit\":\"2-fix-a\",\"issue\":2}}\n",
            std::process::id()
        ),
    )
    .unwrap();
    set_labels(&e, 2, &["type:fix", "status:in-progress"]);

    let out = e.ns().args(["watch", "--once"]).output();
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["requeued"], serde_json::json!([2]), "{v}");
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "ns watch: #2 was in progress with no live ns run; back to status:ready-for-agent"
        ),
        "{err}"
    );
    // The unit resumed from its brief: no second triage.
    assert_eq!(e.calls(), ["build", "verify", "review", "ship"]);
    assert!(e.gh_calls().contains(
        "issue edit 2 --remove-label status:in-progress --add-label status:ready-for-agent"
    ));
    assert!(!e.labels(2).contains(&"status:in-progress".to_string()));
    assert!(run_events(&e)
        .iter()
        .any(|ev| ev["event"] == "requeued" && ev["issue"] == 2));
}

#[test]
fn watch_leaves_an_in_progress_issue_a_live_run_holds() {
    let e = Env::new();
    let wip = ["type:fix", "status:in-progress"];
    e.open_by(2, "Fix a", &wip, "", Some("MEMBER"));
    e.open_by(3, "Fix b", &wip, "", Some("MEMBER"));
    e.open_by(4, "Fix c", &wip, "", Some("MEMBER"));
    let _held = hold(
        &e.root.join(".git/ns-run-3-fix-b.lock"),
        r#"{"pid":42,"unit":"3-fix-b","issue":3}"#,
    );
    // A run that has taken its lock but not yet written its record is known by the file name.
    let _starting = hold(&e.root.join(".git/ns-run-4-fix-c.lock"), "");
    let out = e.ns().args(["watch", "--max-units", "0"]).output();
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["requeued"], serde_json::json!([2]), "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    assert_eq!(e.labels(3), wip);
    assert_eq!(e.labels(4), wip);
    assert!(!e.gh_calls().contains("issue edit 3"));
    assert!(!e.gh_calls().contains("issue edit 4"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "ns watch: #3 is held by a live ns run (pid 42, unit 3-fix-b); left in progress"
        ),
        "{err}"
    );
    assert!(
        err.contains(
            "ns watch: #4 is held by a live ns run (pid ?, unit 4-fix-c); left in progress"
        ),
        "{err}"
    );
    assert!(e.calls().is_empty());
}

#[test]
fn watch_dry_run_lists_a_stale_in_progress_issue_and_changes_nothing() {
    let e = Env::new();
    let wip = ["type:fix", "status:in-progress"];
    e.open_by(2, "Fix a", &wip, "", Some("MEMBER"));
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["requeue"], serde_json::json!([2]), "{v}");
    assert_eq!(e.labels(2), wip);
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn a_second_watch_on_the_repo_is_refused() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.open_by(
        3,
        "Fix b",
        &["type:fix", "status:in-progress"],
        "",
        Some("MEMBER"),
    );
    let _held = hold(&e.root.join(".git/ns-watch.lock"), r#"{"pid":42}"#);
    e.ns()
        .args(["watch", "--once"])
        .assert()
        .code(5)
        .stderr(predicates::str::contains(
            "another ns watch (pid 42) is running",
        ));
    assert!(e.calls().is_empty());
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn two_runs_on_different_units_in_one_repo_run_together() {
    let e = Env::new();
    e.issue(8, "Fix the other", "OPEN");
    // Unit 7 holds its run lock, blocked in verify, while unit 8 runs from start to end.
    let held = Blocked::start(&e, "true", "true");
    let v = e.run(&["run", "--issue", "8"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(5)
        .stderr(predicates::str::contains(format!(
            "another ns run (pid {}, unit {UNIT}) holds",
            held.run.id()
        )));
    held.release();
    let mut run = held.run;
    assert!(run.wait().success());
    for unit in [UNIT, "8-fix-the-other"] {
        let lock = e.root.join(format!(".git/ns-run-{unit}.lock"));
        assert!(lock_is_free(&lock), "{}", lock.display());
    }
}

#[test]
fn worktrees_cut_at_once_from_a_remote_base_all_land() {
    let e = Env::new();
    git(&e.root, &["fetch", "-q", "origin"]);
    // `git worktree add` from a remote base writes the branch's upstream to .git/config, which
    // only one git at a time can lock.
    let mut runs: Vec<Group> = (0..12)
        .map(|i| {
            e.ns()
                .args(["worktree", "new", &format!("u{i}"), "--base", "origin/main"])
                .start()
        })
        .collect();
    for r in &mut runs {
        let err = read_all(r.take_stderr());
        let status = r.wait();
        let err = String::from_utf8_lossy(&r.recv(&err)).into_owned();
        assert!(status.success(), "{err}");
    }
    let list = git(&e.root, &["worktree", "list"]);
    assert_eq!(list.lines().count(), 13, "{list}");
    assert!(lock_is_free(&e.root.join(".git/ns-worktree.lock")));
}

/// Gh's CI wait blocks for whichever PR reaches it first, until `go` gets a line, and each
/// wait's start and end land in `ctrl/order`. Merging a PR puts every other open one in `prs`
/// behind main, reported as `UNKNOWN` for one view while GitHub works that out.
fn merges_in_order(e: &Env, prs: &[u64]) -> PathBuf {
    let go = e.ctrl.join("go");
    assert!(StdCommand::new("mkfifo")
        .arg(&go)
        .status()
        .unwrap()
        .success());
    let others: Vec<String> = prs.iter().map(u64::to_string).collect();
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$1 $2 $4\" in\n\
             \"pr checks --watch\")\n\
               echo \"in $3\" >> {order:?}\n\
               mkdir {first:?} 2>/dev/null && cat {go:?} > /dev/null\n\
               echo \"out $3\" >> {order:?} ;;\n\
             \"pr merge \"*)\n\
               for n in {others}; do\n\
                 [ \"$n\" != \"$3\" ] && [ \"$(cat \"$d/pr-$n.state\" 2>/dev/null)\" != MERGED ] \\\n\
                   && echo UNKNOWN > \"$d/pr-$n.merge\"\n\
               done ;;\n\
             \"pr view \"*)\n\
               if [ \"$(cat \"$d/pr-$3.merge\" 2>/dev/null)\" = UNKNOWN ]; then\n\
                 [ -f \"$d/unknown-$3\" ] && echo BEHIND > \"$d/pr-$3.merge\"\n\
                 touch \"$d/unknown-$3\"\n\
               fi ;;\n\
             esac\n",
            order = e.ctrl.join("order"),
            first = e.ctrl.join("first"),
            others = others.join(" "),
        ),
    );
    go
}

/// Start `ns run --issue <n>` for each of `issues`, their stderr lines merged into one channel,
/// with `None` when one of them closes its stderr.
fn start_runs(e: &Env, issues: &[u64]) -> (Vec<Group>, mpsc::Receiver<Option<String>>) {
    let (tx, rx) = mpsc::channel();
    let runs = issues
        .iter()
        .map(|n| {
            let mut g = e.ns().args(["run", "--issue", &n.to_string()]).start();
            send_lines(g.take_stderr(), tx.clone());
            g
        })
        .collect();
    (runs, rx)
}

/// Send each line `r` gives on `tx`, then `None` when it closes.
fn send_lines(r: impl Read + Send + 'static, tx: mpsc::Sender<Option<String>>) {
    thread::spawn(move || {
        for l in BufReader::new(r).lines().map_while(Result::ok) {
            let _ = tx.send(Some(l));
        }
        let _ = tx.send(None);
    });
}

/// Read `rx` until a line contains `want` (true) or a stream closes (false), keeping every
/// line read in `seen`.
fn line_until(
    g: &Group,
    rx: &mpsc::Receiver<Option<String>>,
    want: &str,
    seen: &mut Vec<String>,
) -> bool {
    while let Some(l) = g.recv(rx) {
        let hit = l.contains(want);
        seen.push(l);
        if hit {
            return true;
        }
    }
    false
}

#[test]
fn two_units_merge_one_at_a_time_each_against_the_main_the_other_left() {
    let e = Env::new();
    e.factory(AUTO);
    e.issue(8, "Fix the other", "OPEN");
    e.ctl(&format!("pr-{UNIT}"), "12");
    e.ctl("pr-8-fix-the-other", "13");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    let go = merges_in_order(&e, &[12, 13]);
    let (mut runs, rx) = start_runs(&e, &[7, 8]);
    // One run waits for the merge lock while the other is in its CI wait. Without the lock, the
    // second run's CI wait doesn't block, and it ends without waiting.
    let mut seen = Vec::new();
    let waited = line_until(
        &runs[0],
        &rx,
        "merge waits for the merge lock (held by pid ",
        &mut seen,
    );
    assert!(
        waited,
        "neither run waited for the merge lock:\n{}",
        seen.join("\n")
    );
    // A thread, since a fifo write blocks until someone reads, and a broken run may not.
    thread::spawn(move || fs::write(&go, "go\n"));
    for r in &mut runs {
        assert!(r.wait().success());
    }

    let order = fs::read_to_string(e.ctrl.join("order")).unwrap();
    let order: Vec<&str> = order.lines().collect();
    assert_eq!(order.len(), 4, "{order:?}");
    let pr = |l: &str| l.split(' ').nth(1).unwrap().to_string();
    let (first, second) = (pr(order[0]), pr(order[2]));
    assert_ne!(first, second, "{order:?}");
    assert_eq!(
        order,
        [
            format!("in {first}"),
            format!("out {first}"),
            format!("in {second}"),
            format!("out {second}")
        ],
    );
    // After the first merge, the second PR is read again, updated to the main that merge left,
    // and checked again before it merges.
    let calls: Vec<String> = e.gh_calls().lines().map(String::from).collect();
    let merged = calls
        .iter()
        .position(|c| c.starts_with(&format!("pr merge {first} ")))
        .unwrap_or_else(|| panic!("{calls:?}"));
    let after: Vec<&str> = calls[merged + 1..]
        .iter()
        .map(String::as_str)
        .filter(|c| c.split(' ').nth(2) == Some(second.as_str()))
        .collect();
    let want = [
        format!("pr view {second} --json mergeStateStatus"),
        format!("pr view {second} --json mergeStateStatus"),
        format!("pr update-branch {second}"),
        format!("pr checks {second} --watch"),
        format!("pr merge {second} --squash"),
    ];
    let mut at = 0;
    for c in &after {
        if at < want.len() && c.starts_with(&want[at]) {
            at += 1;
        }
    }
    assert_eq!(at, want.len(), "{after:?}");
    assert!(lock_is_free(&e.root.join(".git/ns-merge.lock")));
    let waits: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "lock_wait" && ev["lock"] == "merge")
        .collect();
    assert_eq!(waits.len(), 1, "{waits:?}");
    assert_eq!(waits[0]["phase"], "merge");
}

#[test]
fn a_unit_with_nothing_to_merge_does_not_wait_for_the_merge_lock() {
    let e = Env::new();
    e.factory(AUTO);
    let _held = hold(
        &e.root.join(".git/ns-merge.lock"),
        r#"{"pid":42,"unit":"other"}"#,
    );
    // ship's pr.md names no PR, so the merge step ends before it needs the lock.
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        v["reason"], "pr.md has no pr: number; needs a human merge",
        "{v}"
    );
    assert!(!err.contains("merge lock"), "{err}");
}

#[test]
fn a_merge_state_github_never_works_out_is_merged_on_after_the_register_timeout() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "UNKNOWN");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    let views = calls
        .lines()
        .filter(|l| *l == "pr view 12 --json mergeStateStatus")
        .count();
    assert!(views > 1, "{calls}");
    assert!(!calls.contains("pr update-branch"), "{calls}");
}

#[test]
fn a_worktree_waits_for_the_worktree_lock() {
    let e = Env::new();
    let held = hold(
        &e.root.join(".git/ns-worktree.lock"),
        r#"{"pid":42,"unit":"other"}"#,
    );
    let mut g = e.ns().args(["worktree", "new", "u1"]).start();
    let (tx, rx) = mpsc::channel();
    send_lines(g.take_stderr(), tx);
    let mut seen = Vec::new();
    let waited = line_until(
        &g,
        &rx,
        "ns worktree: u1 waits for the worktree lock (held by pid 42, unit other)",
        &mut seen,
    );
    assert!(waited, "{}", seen.join("\n"));
    assert!(!e.worktree("u1").exists());
    drop(held);
    assert!(g.wait().success());
    assert!(e.worktree("u1").is_dir());
}

#[test]
fn a_stop_during_a_merge_lock_wait_returns_the_issue_to_the_queue() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    let _held = hold(
        &e.root.join(".git/ns-merge.lock"),
        r#"{"pid":42,"unit":"other"}"#,
    );
    let mut g = e.ns().args(["watch", "--once"]).start_piped();
    let stdout = read_all(g.take_stdout());
    let (tx, rx) = mpsc::channel();
    send_lines(g.take_stderr(), tx);
    let mut seen = Vec::new();
    let waited = line_until(
        &g,
        &rx,
        "ns run: 2-fix-a merge waits for the merge lock (held by pid 42, unit other)",
        &mut seen,
    );
    assert!(waited, "{}", seen.join("\n"));
    // SAFETY: kill(2) on the ns process this test started.
    assert_eq!(
        unsafe { libc::kill(g.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    assert_eq!(g.wait().code(), Some(143));
    let v: Value = serde_json::from_slice(&g.recv(&stdout)).unwrap();
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert_eq!(v["units"][0]["outcome"], "interrupted", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    assert!(!e.gh_calls().contains("pr checks"));
}

fn read_all(mut r: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

struct Interrupted {
    code: Option<i32>,
    json: Value,
    stderr: String,
    /// The blocked process's parent, the process itself and its background `sleep`.
    pids: Vec<String>,
}

/// A fifo under `ctrl` that [`blocks`] writes to.
fn fifo(e: &Env) -> PathBuf {
    let f = e.ctrl.join("blocked.fifo");
    assert!(StdCommand::new("mkfifo")
        .arg(&f)
        .status()
        .unwrap()
        .success());
    f
}

/// Shell that starts a long `sleep`, tells the test through `fifo` which processes to watch
/// (`$PPID $$ <sleep>`) and waits, so it ends only when it is killed.
fn blocks(fifo: &Path) -> String {
    format!(
        "sleep 600 & echo \"$PPID $$ $!\" > '{}'; wait",
        fifo.display()
    )
}

/// Run `ns watch --once`. Once something runs [`blocks`] on `fifo`, send `sig`: to ns's process
/// group when `ctrl_c`, as a terminal's Ctrl-C does (a phase, in its own session, gets nothing),
/// else to ns alone.
fn interrupt(e: &Env, fifo: PathBuf, sig: i32, ctrl_c: bool) -> Interrupted {
    let mut g = e.ns().args(["watch", "--once"]).start_piped();
    let stdout = read_all(g.take_stdout());
    let stderr = read_all(g.take_stderr());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(&fifo));
    });
    let pids: Vec<String> = g
        .recv(&rx)
        .unwrap()
        .split_whitespace()
        .map(String::from)
        .collect();
    assert_eq!(pids.len(), 3, "{pids:?}");
    let pid = g.id() as libc::pid_t;
    let target = if ctrl_c { -pid } else { pid };
    // SAFETY: kill(2) on the ns process, or its process group, that this test started.
    assert_eq!(unsafe { libc::kill(target, sig) }, 0);
    let code = g.wait().code();
    let stderr = String::from_utf8_lossy(&g.recv(&stderr)).into_owned();
    let out = g.recv(&stdout);
    let json = serde_json::from_slice(&out).unwrap_or_else(|x| {
        panic!(
            "{x}: stdout={} stderr={stderr}",
            String::from_utf8_lossy(&out)
        )
    });
    Interrupted {
        code,
        json,
        stderr,
        pids,
    }
}

/// `ns watch` stopped on `sig` with issue 2's unit interrupted, nothing it started still
/// running, and the issue back in the queue.
fn assert_interrupted(e: &Env, r: &Interrupted, sig: &str, code: i32) {
    let (v, err) = (&r.json, &r.stderr);
    assert_eq!(r.code, Some(code), "{err}");
    assert_eq!(v["stopped"], sig, "{v}");
    assert_eq!(v["units"][0]["issue"], 2, "{v}");
    assert_eq!(v["units"][0]["outcome"], "interrupted", "{v}");
    for pid in &r.pids {
        assert!(exits(pid), "process {pid} outlived ns watch: {err}");
    }
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    assert!(
        run_events(e)
            .iter()
            .any(|ev| ev["event"] == "end" && ev["outcome"] == "interrupted"),
        "{err}"
    );
}

/// Issue 2 is ready, and its build phase writes a partial `build.md` and blocks.
fn blocking_build(e: &Env) -> PathBuf {
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(e);
    e.ctl(
        "build.sh",
        &format!("echo partial > \".ns/$NS_UNIT/build.md\"\n{}\n", blocks(&f)),
    );
    e.queue("build", &["pass:script"]);
    f
}

/// The interrupted build's partial `build.md` was set aside and its `phase` event logged.
fn assert_build_set_aside(e: &Env, event: &str, sig: &str, partial: &str) {
    let unit = e.worktree("2-fix-a").join(".ns/2-fix-a");
    assert!(unit.join("brief.md").is_file());
    assert!(!unit.join("build.md").exists());
    let kept = fs::read_to_string(unit.join("history/build-interrupted-1.md")).unwrap();
    assert!(kept.contains(partial), "{kept}");
    assert!(run_events(e)
        .iter()
        .any(|ev| ev["event"] == event && ev["phase"] == "build" && ev["interrupted"] == sig));
}

#[test]
fn sigterm_kills_the_phase_and_returns_the_issue_to_the_queue() {
    let e = Env::new();
    let f = blocking_build(&e);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert_build_set_aside(&e, "phase", "SIGTERM", "partial");

    // The next night resumes the unit at build.
    fs::remove_file(e.ctrl.join("calls")).unwrap();
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.calls(), ["build", "verify", "review", "ship"]);
}

#[test]
fn ctrl_c_kills_the_phase_and_returns_the_issue_to_the_queue() {
    let e = Env::new();
    let f = blocking_build(&e);
    let r = interrupt(&e, f, libc::SIGINT, true);
    assert_interrupted(&e, &r, "SIGINT", 130);
    assert_build_set_aside(&e, "phase", "SIGINT", "partial");
}

/// Whether `pid` has a handler installed for `sig`, from the `SigCgt` mask in its status.
fn catches(pid: u32, sig: i32) -> bool {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let mask = status
        .lines()
        .find_map(|l| l.strip_prefix("SigCgt:"))
        .map(|m| u64::from_str_radix(m.trim(), 16).unwrap())
        .unwrap();
    mask & (1 << (sig - 1)) != 0
}

#[test]
fn a_second_signal_ends_watch_at_once() {
    use std::os::unix::process::ExitStatusExt;
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    // ns hangs on a gh call, which a SIGTERM sent to ns alone doesn't reach.
    let f = fifo(&e);
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$*\" in *\"--add-label status:in-progress\"*) echo hung > '{}'; sleep 600 ;; esac\n",
            f.display()
        ),
    );
    let mut g = e.ns().args(["watch", "--once"]).start();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(&f));
    });
    g.recv(&rx).unwrap();
    assert!(catches(g.id(), libc::SIGTERM));
    // SAFETY: kill(2) on the ns process this test started.
    assert_eq!(
        unsafe { libc::kill(g.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    // The handler has run once SIGTERM is no longer caught.
    let deadline = std::time::Instant::now() + common::EXIT_WAIT;
    while catches(g.id(), libc::SIGTERM) {
        assert!(
            std::time::Instant::now() < deadline,
            "SIGTERM never handled"
        );
        thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: as above.
    assert_eq!(
        unsafe { libc::kill(g.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    assert_eq!(g.wait().signal(), Some(libc::SIGTERM));
}

#[test]
fn a_stop_during_the_build_gate_runs_the_build_again() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    e.factory(&format!("[phases.build]\ngate = {:?}\n", blocks(&f)));
    e.queue("build", &["pass:commit"]);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert_build_set_aside(&e, "gate", "SIGTERM", "status: pass");

    // The build the gate never passed runs again, and its gate with it.
    e.factory("[phases.build]\ngate = \"touch gate-ran\"\n");
    fs::remove_file(e.ctrl.join("calls")).unwrap();
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.calls(), ["build", "verify", "review", "ship"]);
    assert!(e.worktree("2-fix-a").join("gate-ran").exists());
}

#[test]
fn a_stop_during_the_gate_after_an_empty_build_leaves_the_last_build_md() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    // The gate is red after the first build, and blocks after the second.
    let gate = format!(
        "if [ -f '{seen}' ]; then {}; else touch '{seen}'; exit 1; fi",
        blocks(&f),
        seen = e.ctrl.join("gate-seen").display()
    );
    e.factory(&format!("[phases.build]\ngate = {gate:?}\n"));
    // The second build commits but writes nothing, so its moves are undone before the gate.
    e.queue("build", &["pass:commit", "none:commit"]);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    let unit = e.worktree("2-fix-a").join(".ns/2-fix-a");
    let build = fs::read_to_string(unit.join("build.md")).unwrap();
    assert!(build.contains("status: pass"), "{build}");
    assert!(!unit.join("history/build-interrupted-1.md").exists());
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "gate"
        && ev["attempt"] == 2
        && ev["interrupted"] == "SIGTERM"
        && ev.get("archived").is_none()));
}

#[test]
fn a_stop_during_the_ci_wait_resumes_at_the_merge_the_next_night() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    // The unit's own PR closes the issue, as ship's does.
    e.gh_file(
        "prs.json",
        r#"[{"number":12,"body":"Closes #2","headRefName":"ns/2-fix-a","isCrossRepository":false}]"#,
    );
    let f = fifo(&e);
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$*\" in *\"pr checks\"*--watch*) {}; exit 0 ;; esac\n",
            blocks(&f)
        ),
    );
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert!(!e.gh_calls().contains("pr merge"));

    fs::remove_file(e.ghd.join("hook.sh")).unwrap();
    fs::remove_file(e.ctrl.join("calls")).unwrap();
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert!(e.calls().is_empty(), "{:?}", e.calls());
    assert!(e.gh_calls().contains("pr merge 12"));
}

#[test]
fn watch_still_skips_an_issue_a_pr_from_another_branch_closes() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    // Another unit's branch, an older title's unit branch, and a fork's branch of the same name.
    for head in [
        r#""headRefName":"ns/21-other","isCrossRepository":false"#,
        r#""headRefName":"ns/2-old-title","isCrossRepository":false"#,
        r#""headRefName":"ns/2-fix-a","isCrossRepository":true"#,
    ] {
        e.gh_file(
            "prs.json",
            &format!(r#"[{{"number":12,"body":"Closes #2",{head}}}]"#),
        );
        let v = e.run(&["watch", "--dry-run"], 0);
        assert_eq!(v["queue"], serde_json::json!([]), "{head}: {v}");
        assert_eq!(v["skipped"][0]["reason"], "open PR #12 closes it", "{v}");
    }
}

/// `[worktree] setup` that logs each start to `setup.log` beside the repo, runs `cut` (which
/// ends setup partway) the first time only, then logs the finish to `setup.done`.
fn setup_cut_once(e: &Env, cut: &str) {
    let commands = [
        "echo run >> \"$NS_MAIN_ROOT/../setup.log\"".to_string(),
        format!(
            "if [ ! -e \"$NS_MAIN_ROOT/../cut\" ]; then touch \"$NS_MAIN_ROOT/../cut\"; {cut}; fi"
        ),
        "echo done >> \"$NS_MAIN_ROOT/../setup.done\"".to_string(),
    ];
    e.factory(&format!("[worktree]\nsetup = {commands:?}\n"));
}

/// How many times setup started and finished.
fn setup_runs(e: &Env) -> (usize, usize) {
    let count = |f: &str| {
        fs::read_to_string(e.base.join(f))
            .unwrap_or_default()
            .lines()
            .count()
    };
    (count("setup.log"), count("setup.done"))
}

#[test]
fn a_crash_during_setup_reruns_setup_on_the_next_run() {
    use std::os::unix::process::ExitStatusExt;
    let e = Env::new();
    // Setup's shell is a child of ns.
    setup_cut_once(&e, "kill -KILL \"$PPID\"");
    let out = e.ns().args(["run", "--issue", "7"]).output();
    assert_eq!(out.status.signal(), Some(libc::SIGKILL), "{out:?}");
    assert!(e.worktree(UNIT).is_dir());
    assert_eq!(setup_runs(&e), (1, 0));
    assert!(e.calls().is_empty());

    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(setup_runs(&e), (2, 1));

    // Finished setup is on record, so reusing the worktree doesn't run it again.
    let v = e.run(&["worktree", "new", UNIT], 0);
    assert_eq!(v["setup"], serde_json::json!([]));
    assert_eq!(setup_runs(&e), (2, 1));
}

#[test]
fn a_stop_during_setup_reruns_setup_on_the_next_run() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    setup_cut_once(&e, &blocks(&f));
    // SIGTERM to the group, as a service manager stopping ns watch sends it.
    let r = interrupt(&e, f, libc::SIGTERM, true);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert!(
        !run_events(&e)
            .iter()
            .any(|ev| ev["event"] == "end" && ev["outcome"] == "stuck"),
        "{}",
        r.stderr
    );
    assert_eq!(setup_runs(&e), (1, 0));
    assert!(e.calls().is_empty());

    let v = e.run(&["run", "--issue", "2"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(setup_runs(&e), (2, 1));
}

/// Run `ns watch --once` on ready issue 2 through to the merge step, with a `gh` that sends ns
/// SIGTERM when it is called with `call`, then answers normally.
fn stop_at_gh(e: &Env, call: &str) -> (std::process::Output, Value) {
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    // ns runs gh directly, so gh's parent is ns.
    e.gh_file(
        "hook.sh",
        &format!("case \"$*\" in \"{call}\"*) kill -TERM \"$PPID\" ;; esac\n"),
    );
    let out = e.ns().args(["watch", "--once"]).output();
    let v = serde_json::from_slice(&out.stdout).unwrap_or_else(|x| panic!("{x}: {out:?}"));
    (out, v)
}

#[test]
fn a_stop_that_lands_while_a_unit_ends_needing_a_human_returns_the_issue() {
    let e = Env::new();
    e.gh_file("diff-12.txt", ".github/workflows/ci.yml\n");
    let (out, v) = stop_at_gh(&e, "pr diff 12");
    assert_eq!(out.status.code(), Some(143), "{out:?}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert_eq!(v["units"][0]["outcome"], "interrupted", "{v}");
    assert!(v["units"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("needs a human merge"));
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    assert!(!e.gh_calls().contains("issue comment"));
}

#[test]
fn a_stop_that_lands_while_a_unit_merges_keeps_the_merge() {
    let e = Env::new();
    e.gh_file("diff-12.txt", "src/x.rs\n");
    let (out, v) = stop_at_gh(&e, "pr merge 12");
    assert_eq!(out.status.code(), Some(143), "{out:?}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(e.labels(2), ["type:fix"]);
    assert!(e.gh_calls().contains("issue close 2"));
}

#[test]
fn sigterm_ends_a_usage_limit_sleep_and_returns_the_issue() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    e.ctl("reset", &(now + 3600).to_string());
    e.queue("build", &["limit"]);
    let mut g = e
        .ns()
        .env_remove("NS_NOW")
        .args(["watch", "--once"])
        .start_piped();
    let stdout = read_all(g.take_stdout());
    let stderr = g.take_stderr();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if line.contains("usage limit, sleeping until") {
                let _ = tx.send(());
            }
        }
    });
    g.recv(&rx);
    // SAFETY: kill(2) on the ns process this test started.
    assert_eq!(
        unsafe { libc::kill(g.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    assert_eq!(g.wait().code(), Some(143));
    let v: Value = serde_json::from_slice(&g.recv(&stdout)).unwrap();
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    let outcomes: Vec<&Value> = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| &u["outcome"])
        .collect();
    assert_eq!(outcomes, ["paused", "interrupted"], "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
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
    let r = &quality_records(&e)[0];
    assert_eq!(
        (r["outcome"].as_str(), r["pr"].as_u64()),
        (Some("merged"), Some(12))
    );
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
    // The rebased branch carries the merged change, so it goes.
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
    assert!(!e.worktree(UNIT).exists());
}

/// The sha `ns run` merged at: the unit's HEAD when its merge step ran.
fn merged_sha(e: &Env) -> String {
    let ev = run_events(e);
    let merged = ev
        .iter()
        .find(|v| v["event"] == "merged")
        .expect("merged event");
    merged["sha"].as_str().unwrap().to_string()
}

#[test]
fn a_pr_head_from_before_the_rebase_merges_at_that_head() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let old = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    assert_ne!(old.trim(), merged_sha(&e));
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
    assert_ne!(head, merged_sha(&e));
    assert_ne!(head, UPDATED_HEAD);
}

#[test]
fn a_dry_run_on_a_rebased_unit_sees_its_reviewed_artifacts_as_current() {
    // A human merge keeps the worktree to look at.
    let (e, v, code) =
        rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| e.factory(""));
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
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
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
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
    git(&e.root, &["push", "-q", "origin", "main"]);
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
fn a_failed_fetch_of_the_default_branch_needs_a_human_merge() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.ctl("edit", "git remote set-url origin /nonexistent");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "work.txt\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("cannot check human-review regions: git fetch -q origin main failed"),
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
fn a_split_brief_ends_the_run_as_split_not_stuck() {
    let e = Env::new();
    e.queue(
        "triage",
        &["split:into #8 and #9.", "split:into #8 and #9."],
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(e.calls(), ["triage"]);
    assert_eq!(v["outcome"], "split", "{v}");
    assert_eq!(v["phase"], "triage", "{v}");
    assert_eq!(
        v["reason"], "brief.md is split: triage said split into #8 and #9.",
        "{v}"
    );
    let artifacts = Path::new(v["worktree"].as_str().unwrap()).join(".ns/7-fix-the-thing");
    assert_eq!(
        v["artifact"].as_str().unwrap(),
        artifacts.join("history/brief-1.md").to_str().unwrap(),
        "{v}"
    );
    assert!(!artifacts.join("brief.md").exists());
    // The split brief is history, so a parent sent back to the queue is triaged afresh.
    let again = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(again["outcome"], "split", "{again}");
    assert_eq!(e.calls(), ["triage", "triage"]);
}

#[test]
fn dry_run_reports_a_split_brief() {
    let e = Env::new();
    e.queue("triage", &["blocked"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    let brief = Path::new(v["worktree"].as_str().unwrap()).join(".ns/7-fix-the-thing/brief.md");
    fs::write(&brief, "---\nstatus: split\n---\nSplit into #8 and #9.\n").unwrap();
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(d["decision"]["action"], "split", "{d}");
    assert_eq!(
        d["decision"]["reason"], "brief.md is split: Split into #8 and #9.",
        "{d}"
    );
}

#[test]
fn watch_split_moves_the_parent_to_needs_define_without_a_comment() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["split:into #8 and #9."]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "split", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:needs-define"]);
    let calls = e.gh_calls();
    assert!(!calls.contains("issue comment 2"), "{calls}");
    assert!(!calls.contains("status:ready-for-human"), "{calls}");
}

#[test]
fn watch_split_sets_a_custom_split_label() {
    let e = Env::new();
    e.factory("[queue]\nsplit_label = \"tracking\"\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["split:into #8 and #9."]);
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(2), ["type:fix", "tracking"]);
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
    let remote = e.base.join("remote.git");
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
    let merged = merged_sha(&e);
    assert!(git(
        &e.root,
        &["merge-base", "--is-ancestor", &upstream, &merged]
    )
    .is_empty());
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
fn watch_dry_run_lists_triage_candidates_in_triage_order() {
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
    assert_eq!(v["triage"][0]["priority_label"], "priority:high");
    assert_eq!(v["triage"][1]["reason"], "status:needs-triage");
    assert!(v["triage"][2]["priority_label"].is_null(), "{v}");
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
    assert_eq!(v["triage_pass"], true, "{v}");
    // #2 is ready, so the next pass leaves the backlog for later passes.
    assert_eq!(v["triage_next_pass"], serde_json::json!([]), "{v}");
    assert!(v.get("triage_per_night").is_none(), "{v}");
    assert!(e.calls().is_empty());
    assert!(!e.gh_calls().contains("issue edit"));
    assert!(e
        .gh_calls()
        .contains("api --paginate repos/{owner}/{repo}/issues?state=open&per_page=100 --jq"));
}

#[test]
fn watch_dry_run_with_no_ready_issue_shows_the_next_pass_taking_the_backlog_in_order() {
    let e = Env::new();
    e.untriaged(5, "Low", &["status:needs-triage", "priority:low"]);
    e.untriaged(6, "High", &["status:needs-triage", "priority:high"]);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert!(v["queue"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["triage_next_pass"], serde_json::json!([6, 5]), "{v}");
}

#[test]
fn watch_dry_run_counts_a_stale_in_progress_issue_as_ready_for_the_next_pass() {
    let e = Env::new();
    // The night returns #2 to the queue before its first pass, so the pass takes no backlog.
    e.open_by(
        2,
        "Fix a",
        &["type:fix", "status:in-progress"],
        "",
        Some("MEMBER"),
    );
    e.untriaged(5, "Backlog", &["status:needs-triage"]);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert!(v["queue"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["requeue"], serde_json::json!([2]), "{v}");
    assert_eq!(numbers(&v["triage"]), [5], "{v}");
    assert_eq!(v["triage_next_pass"], serde_json::json!([]), "{v}");
}

#[test]
fn triage_takes_priority_labels_first_highest_first_then_issue_numbers() {
    let e = Env::new();
    // The queue's category order (`type:fix` first) plays no part in triage order.
    e.untriaged(
        2,
        "No priority, docs",
        &["status:needs-triage", "type:docs"],
    );
    e.untriaged(3, "No priority, fix", &["status:needs-triage", "type:fix"]);
    e.untriaged(4, "Low", &["status:needs-triage", "priority:low"]);
    e.untriaged(
        5,
        "High docs",
        &["status:needs-triage", "priority:high", "type:docs"],
    );
    e.untriaged(
        6,
        "High fix",
        &["status:needs-triage", "priority:high", "type:fix"],
    );
    e.untriaged(8, "Medium", &["status:needs-triage", "priority:medium"]);
    let dry = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(numbers(&dry["triage"]), [5, 6, 8, 4, 2, 3], "{dry}");
    e.queue("triage", &["none"; 6]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5, 6, 8, 4, 2, 3], "{v}");
    assert_eq!(v["stopped"], "queue empty");
}

/// Shell for a phase script that files issue `n` as `label`, as review files an escape.
fn files_issue(n: u64, title: &str, label: &str) -> String {
    let line = serde_json::json!({
        "number": n, "title": title, "body": "",
        "labels": [{"name": label}], "authorAssociation": "MEMBER",
    });
    format!(
        "echo '{line}' >> \"$FAKE_GH_DIR/issues.jsonl\"\necho {label} > \"$FAKE_GH_DIR/labels-{n}\"\necho '{{\"number\":{n},\"title\":\"{title}\",\"url\":\"u\",\"state\":\"OPEN\"}}' > \"$FAKE_GH_DIR/issue-{n}.json\"\n"
    )
}

#[test]
fn issues_filed_since_the_last_pass_are_triaged_before_the_backlog() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    // A backlog candidate that outranks the follow-up on priority.
    e.untriaged(4, "Old", &["status:needs-triage", "priority:high"]);
    e.ctl(
        "ship.sh",
        &files_issue(9, "Follow-up", "status:needs-triage"),
    );
    e.queue("ship", &["pass:script", "pass"]);
    e.queue("build", &["pass:commit", "pass:commit"]);
    // Unit 2's triage, follow-up 9's triage-only run, unit 3's triage, backlog 4's.
    e.queue("triage", &["pass", "none", "pass", "none"]);
    let v = e.run(&["watch"], 0);
    // #4 waits while units are ready; #9 is new before unit 3, so it goes first.
    assert_eq!(numbers(&v["triaged"]), [9, 4], "{v}");
    assert_eq!(numbers(&v["units"]), [2, 3], "{v}");
    let calls = e.calls();
    assert_eq!(calls[4..7], ["ship", "triage", "triage"], "{calls:?}");
    assert!(e.prompt(6, "triage").contains("unit `9-follow-up`"));
    assert!(!e.prompt(7, "triage").contains("Triage only"));
    assert_eq!(calls.last().unwrap(), "triage", "{calls:?}");
    assert!(e.prompt(calls.len(), "triage").contains("unit `4-old`"));
    assert_eq!(e.triage_only_runs(), 2);
}

#[test]
fn every_new_issue_is_triaged_even_with_a_ready_queue() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    // Unit 2's ship files two follow-ups; both are new at the next pass.
    let script = files_issue(8, "First", "status:needs-triage")
        + &files_issue(9, "Second", "status:needs-triage");
    e.ctl("ship.sh", &script);
    e.queue("ship", &["pass:script", "pass"]);
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("triage", &["pass", "none", "none", "pass"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [8, 9], "{v}");
    assert_eq!(numbers(&v["units"]), [2, 3], "{v}");
    assert_eq!(e.calls()[4..8], ["ship", "triage", "triage", "triage"]);
}

#[test]
fn the_backlog_stops_once_an_issue_is_ready_and_goes_on_at_the_next_pass() {
    let e = Env::new();
    e.untriaged(5, "A", &["status:needs-triage", "type:fix"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.untriaged(8, "C", &["status:needs-triage"]);
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage", &["pass:script", "none", "none"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5, 6, 8], "{v}");
    assert_eq!(numbers(&v["units"]), [5], "{v}");
    assert_eq!(v["stopped"], "queue empty");
    // #5 came back ready, so the pass stopped and unit 5 ran before #6 and #8 were triaged.
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "triage", "triage"]
    );
}

#[test]
fn a_blocked_ready_issue_does_not_stop_the_backlog() {
    let e = Env::new();
    e.ready(2, "Blocked", &["type:fix"], "Blocked by: #7");
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.queue("triage", &["none", "none"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
    assert!(v["units"].as_array().unwrap().is_empty(), "{v}");
}

#[test]
fn each_candidate_is_triaged_once_a_night() {
    let e = Env::new();
    e.untriaged(5, "Unclear", &["status:needs-triage", "priority:high"]);
    e.untriaged(6, "Clear", &["status:needs-triage"]);
    e.ctl(
        "triage.sh",
        "[ \"${NS_UNIT%%-*}\" = 6 ] && gh issue edit 6 --remove-label status:needs-triage --add-label status:ready-for-agent\ntrue\n",
    );
    // Unit 6's ship files #9, which triage leaves untriaged too.
    e.ctl(
        "ship.sh",
        &files_issue(9, "Follow-up", "status:needs-triage"),
    );
    e.queue("triage", &["none", "pass:script", "none"]);
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:script"]);
    let v = e.run(&["watch"], 0);
    // #5 and #9 stay untriaged and are candidates at every later pass, but run once each.
    assert_eq!(numbers(&v["triaged"]), [5, 6, 9], "{v}");
    assert_eq!(numbers(&v["units"]), [6], "{v}");
    assert_eq!(e.triage_only_runs(), 3);
    assert_eq!(e.labels(5), ["status:needs-triage", "priority:high"]);
    assert_eq!(e.labels(9), ["status:needs-triage"]);
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
    e.ready(3, "Low fix too", &["type:fix", "priority:low"], "");
    e.ctl("ship.sh", &files_issue(9, "Escape", "status:needs-triage"));
    e.triage_sets(&["status:ready-for-agent", "priority:high"]);
    e.queue("ship", &["pass:script", "pass"]);
    e.queue("triage", &["pass", "pass:script"]);
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["watch", "--max-units", "2"], 0);
    // Triage-only runs are not units: max_units counts units 2 and 9 only.
    assert_eq!(numbers(&v["triaged"]), [9], "{v}");
    assert_eq!(numbers(&v["units"]), [2, 9], "{v}");
    assert_eq!(v["stopped"], "max_units");
    assert!(!e.gh_calls().contains("issue edit 3"));
}

#[test]
fn a_triage_that_leaves_the_issue_untriaged_is_not_retried_that_night() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.untriaged(5, "Unclear", &["status:needs-triage"]);
    e.queue("triage", &["pass", "pass", "none"]);
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
fn an_old_triage_per_night_warns_and_is_ignored() {
    let e = Env::new();
    e.factory("[queue]\ntriage_per_night = 0\n");
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.queue("triage", &["none", "none"]);
    let out = e.ns().args(["watch"]).assert().code(0).get_output().clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("warning:") && err.contains("triage_per_night is no longer used"),
        "{err}"
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    // The old cap of 0 would have run none: the key changes nothing.
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
    let check = e.run(&["factory", "validate"], 0);
    assert_eq!(check["ok"], true, "{check}");
    let warnings = check["warnings"].to_string();
    assert!(warnings.contains("triage_per_night"), "{check}");
}

#[test]
fn a_paused_triage_sleeps_and_retries_the_same_issue() {
    let e = Env::new();
    e.untriaged(5, "Escape", &["status:needs-triage"]);
    e.ctl("reset", &(NOW + 3600).to_string());
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage", &["limit", "pass:script"]);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--until", "06:30"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["paused", "done"], "{v}");
    assert_eq!(numbers(&v["triaged"]), [5, 5], "{v}");
    assert_eq!(v["triaged"][0]["reset_at"], "2026-10-09T01:00:00+00:00");
    assert_eq!(numbers(&v["units"]), [5], "{v}");
    assert_eq!(v["units"][0]["outcome"], "done");
}

#[test]
fn a_triage_paused_past_until_ends_the_night_and_touches_no_label() {
    let e = Env::new();
    e.untriaged(5, "Escape", &["status:needs-triage"]);
    e.untriaged(6, "Next", &["status:needs-triage"]);
    e.ctl("reset", &(NOW + 8 * 3600).to_string());
    e.queue("triage", &["limit"]);
    let v = e.run(&["watch", "--until", "06:30"], 0);
    assert_eq!(v["stopped"], "usage limit resets after --until", "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["paused"], "{v}");
    assert!(v["units"].as_array().unwrap().is_empty(), "{v}");
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn no_triage_run_starts_after_until() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    // b's triage needs the bench a holds, so its first triage waits out the night.
    b.factory("[phases.triage]\nrunner = \"bench\"\n");
    let held = Blocked::start(&a, "true", "true");
    b.untriaged(5, "A", &["status:needs-triage"]);
    b.untriaged(6, "B", &["status:needs-triage"]);
    let out = b
        .ns()
        .args(["watch", "--until", "00:30"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    held.release();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["stopped"], "until", "{v}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["budget"], "{v}");
    assert!(b.calls().is_empty(), "{:?}", b.calls());
}

#[test]
fn triage_runs_that_keep_failing_instantly_stop_the_night() {
    let e = Env::new();
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.untriaged(8, "C", &["status:needs-triage"]);
    e.queue("triage", &["crash", "crash", "none"]);
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
    // Both attempts of unit 2's triage phase crash, 5 works, then 6 crashes.
    e.queue("triage", &["crash", "crash", "none", "crash"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(outcomes(&v["units"]), ["harness_failing"], "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["done", "harness_failing"], "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
}

#[test]
fn a_failing_triage_and_a_failing_unit_share_the_harness_breaker() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.queue("triage", &["crash", "crash", "crash"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(v["stopped"], "harness failing", "{v}");
    assert_eq!(outcomes(&v["units"]), ["harness_failing"], "{v}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["harness_failing"], "{v}");
}

#[test]
fn a_timed_out_triage_is_recorded_and_the_night_goes_on() {
    let e = Env::new();
    e.untriaged(5, "Slow", &["status:needs-triage", "priority:high"]);
    e.untriaged(6, "Next", &["status:needs-triage"]);
    e.queue("triage", &["pass:sleep", "none"]);
    let out = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "triage=1500")
        .args(["watch"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(outcomes(&v["triaged"]), ["stuck", "done"], "{v}");
    let reason = v["triaged"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("timed out"), "{reason}");
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
    assert_eq!(e.labels(5), ["status:needs-triage", "priority:high"]);
}

#[test]
fn a_killed_triage_is_recorded_and_the_night_goes_on() {
    let e = Env::new();
    e.untriaged(5, "Killed", &["status:needs-triage", "priority:high"]);
    e.untriaged(6, "Next", &["status:needs-triage"]);
    // Kill the harness itself, so ns sees a signal instead of an exit code.
    e.ctl("triage.sh", "kill -KILL $PPID\n");
    e.queue("triage", &["pass:script", "none"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["stuck", "done"], "{v}");
    assert_eq!(v["triaged"][0]["reason"], "was killed by SIGKILL");
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
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
    e.untriaged(5, "Unreadable", &["status:needs-triage"]);
    e.untriaged(6, "Fine", &["status:needs-triage"]);
    fs::remove_file(e.ghd.join("issue-5.json")).unwrap();
    e.queue("triage", &["none"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["error", "done"], "{v}");
    let reason = v["triaged"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("cannot read issue #5"), "{reason}");
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
    assert_eq!(dry["triage_pass"], false, "{dry}");
    assert!(dry["triage"].as_array().unwrap().is_empty(), "{dry}");
    assert_eq!(dry["triage_next_pass"], serde_json::json!([]), "{dry}");
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
    e.ctl(
        "ship.sh",
        &files_issue(9, "Follow-up", "status:needs-triage"),
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

// ---------------------------------------------------------------- memory cap

/// Allocates and touches 256 MB: past a 64 MB cap, never near a real machine's memory.
const HOG: &str = "python3 -c \"b = b'x' * (256 << 20)\"";

/// The `start` event's `memory_cap` for the first unit in the run log.
fn memory_cap(e: &Env) -> Value {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["event"] == "start")
        .unwrap()["memory_cap"]
        .clone()
}

/// Shadow `name` on the fakes' PATH with a command that fails, as if it were missing.
fn break_tool(e: &Env, name: &str) {
    let p = e.bin.join(name);
    fs::write(&p, "#!/bin/sh\necho \"$0: not here\" >&2\nexit 1\n").unwrap();
    StdCommand::new("chmod").arg("+x").arg(&p).status().unwrap();
}

/// The build script records how its process is capped: `ulimit -v` and its cgroup.
fn record_limits(e: &Env) {
    e.ctl(
        "build.sh",
        &format!(
            "ulimit -v > {c}/ulimit; cat /proc/self/cgroup > {c}/cgroup\n",
            c = e.ctrl.display()
        ),
    );
}

fn recorded(e: &Env, name: &str) -> String {
    fs::read_to_string(e.ctrl.join(name)).unwrap()
}

#[test]
fn a_phase_past_the_memory_cap_fails_its_attempt_and_watch_goes_on() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.ctl(
        "build.sh",
        &format!("case \"$NS_UNIT\" in 2-*) {HOG} ;; esac\n"),
    );
    e.queue("build", &["pass:script", "pass:script", "pass:script"]);
    let v = e.run(&["watch"], 0);
    let units = v["units"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{v}");
    assert_eq!(units[0]["outcome"], "stuck", "{v}");
    assert_eq!(
        units[0]["reason"],
        "build is out of attempts (2): the last attempt exceeded the 64 MB memory limit"
    );
    assert_eq!(units[1]["outcome"], "done", "{v}");
    let failed: Vec<Value> = phase_events(&e)
        .into_iter()
        .filter(|p| p["phase"] == "build" && p["unit"] == "2-fix-a")
        .collect();
    assert_eq!(failed.len(), 2);
    for p in failed {
        assert_eq!(p["reason"], "exceeded the 64 MB memory limit", "{p}");
        assert_eq!(p["written"], false);
    }
    assert_eq!(memory_cap(&e)["mb"], 64);
}

#[test]
fn a_capped_phase_runs_in_a_scope_or_under_an_rlimit_and_logs_which() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    record_limits(&e);
    e.queue("build", &["pass:script"]);
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(0);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    let cap = memory_cap(&e);
    match cap["via"].as_str().unwrap() {
        "systemd-run" => {
            assert!(cap["fallback"].is_null(), "{cap}");
            assert!(
                err.contains("64 MB memory cap (cgroup, systemd-run)"),
                "{err}"
            );
            let cg = recorded(&e, "cgroup");
            assert!(cg.contains("/ns-") && cg.contains(".scope"), "{cg}");
        }
        "prlimit" => {
            assert!(
                err.contains("64 MB memory cap (RLIMIT_AS, prlimit)"),
                "{err}"
            );
            assert_eq!(recorded(&e, "ulimit").trim(), "65536");
        }
        other => panic!("{other}"),
    }
}

#[test]
fn without_systemd_run_the_cap_falls_back_to_prlimit() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    break_tool(&e, "systemd-run");
    let flag = e.ctrl.join("hogged");
    e.ctl(
        "build.sh",
        &format!(
            "ulimit -v > {c}/ulimit\ntest -f {f} || {{ touch {f}; {HOG}; }}\n",
            c = e.ctrl.display(),
            f = flag.display()
        ),
    );
    e.queue("build", &["pass:script", "pass:script"]);
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(0);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        err.contains("64 MB memory cap (RLIMIT_AS, prlimit); no cgroup: systemd-run failed"),
        "{err}"
    );
    let cap = memory_cap(&e);
    assert_eq!(cap["via"], "prlimit", "{cap}");
    assert!(
        cap["fallback"].as_str().unwrap().contains("systemd-run"),
        "{cap}"
    );
    assert_eq!(recorded(&e, "ulimit").trim(), "65536");
    assert_eq!(
        build_event(&e, 1)["reason"],
        "exceeded the 64 MB memory limit"
    );
    assert_eq!(build_event(&e, 2)["written"], true);
}

#[test]
fn a_capped_phase_that_times_out_fails_on_the_timeout() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[limits]\nmemory_mb = 64\n[phases.build]\nmax_attempts = 3\ngate = \"test -f {f} || {{ touch {f}; exit 1; }}\"\n",
        f = flag.display()
    ));
    break_tool(&e, "systemd-run");
    // Under RLIMIT_AS this message alone would read as the cap; the timeout must win.
    e.ctl("build.sh", "echo MemoryError >&2\n");
    e.queue(
        "build",
        &["pass:commit", "pass:script+sleep", "pass:commit"],
    );
    let v = run_with_phase_timeout(&e, 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(build_event(&e, 2)["reason"], "timed out after 1.5 s");
    assert_eq!(
        build_event(&e, 3)["decision"],
        "timed out with the CI gate still red"
    );
}

#[test]
fn a_cap_nothing_here_can_enforce_is_a_usage_error_before_any_phase() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    break_tool(&e, "systemd-run");
    break_tool(&e, "prlimit");
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(2);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        err.contains("limits.memory_mb = 64, but no memory cap works here"),
        "{err}"
    );
    assert!(e.calls().is_empty());
}

#[test]
fn without_a_cap_phases_run_directly() {
    let e = Env::new();
    record_limits(&e);
    e.queue("build", &["pass:script"]);
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(0);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(!err.contains("memory cap"), "{err}");
    assert!(memory_cap(&e).is_null());
    let own = StdCommand::new("sh")
        .args(["-c", "ulimit -v"])
        .output()
        .unwrap();
    assert_eq!(recorded(&e, "ulimit"), String::from_utf8_lossy(&own.stdout));
    assert_eq!(
        recorded(&e, "cgroup"),
        fs::read_to_string("/proc/self/cgroup").unwrap()
    );
}

#[test]
fn a_gate_past_the_memory_cap_sends_build_back_and_names_the_cap() {
    let e = Env::new();
    e.factory(&format!(
        "[limits]\nmemory_mb = 64\n[phases.build]\ngate = {HOG:?}\n"
    ));
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate exceeded the 64 MB memory limit"
    );
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["memory_exceeded"], true);
    assert_eq!(g[0]["green"], false);
    let p = e.prompt(3, "build");
    assert!(p.contains("exceeded the 64 MB memory limit"), "{p}");
    assert_eq!(
        build_event(&e, 2)["decision"],
        "the CI gate exceeded the 64 MB memory limit after build"
    );
}

#[test]
fn a_phase_killed_by_a_signal_fails_its_attempt_naming_it() {
    let e = Env::new();
    e.queue("build", &["pass:commit+kill", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let p = build_event(&e, 1);
    assert_eq!(p["reason"], "was killed by SIGKILL", "{p}");
    assert_eq!(p["written"], false);
    let dir = e.worktree(UNIT).join(".ns").join(UNIT);
    assert!(fs::read_to_string(dir.join("history/build-killed-1.md"))
        .unwrap()
        .contains("status: pass"));
    assert_eq!(
        p["archived"],
        dir.join("history/build-killed-1.md").to_str().unwrap()
    );
}

#[test]
fn a_phase_killed_on_every_attempt_is_stuck_naming_the_signal() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 1\n");
    e.queue("build", &["pass:commit+kill"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (1): the last attempt was killed by SIGKILL"
    );
}

#[test]
fn a_gate_killed_by_a_signal_names_it() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"kill -9 $$\"\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate was killed by SIGKILL"
    );
    let g = gate_events(&e);
    assert_eq!(g[0]["signal"], "SIGKILL");
    assert_eq!(g[0]["memory_exceeded"], false);
    assert!(e.prompt(3, "build").contains("was killed by SIGKILL"));
}

#[test]
fn a_killed_rebuild_keeps_the_red_gate_feedback() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\nmax_attempts = 3\ngate = \"test -f {f} || {{ touch {f}; echo boom; exit 1; }}\"\n",
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "pass:commit+kill", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        build_event(&e, 3)["decision"],
        "was killed by SIGKILL with the CI gate still red"
    );
    assert!(e.prompt(4, "build").contains("boom"));
}

#[test]
fn a_gate_still_red_after_a_silent_build_still_names_the_signal() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"kill -9 $$\"\n");
    e.queue("build", &["pass:commit", "none"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate was killed by SIGKILL"
    );
    assert_eq!(gate_events(&e).len(), 1);
}

#[test]
fn a_systemd_run_that_does_not_enforce_memory_max_falls_back_to_prlimit() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    // Starts and exits 0 but sets no limit, as a systemd with no memory controller would.
    let p = e.bin.join("systemd-run");
    fs::write(&p, "#!/bin/sh\necho max\n").unwrap();
    StdCommand::new("chmod").arg("+x").arg(&p).status().unwrap();
    e.run(&["run", "--issue", "7"], 0);
    let cap = memory_cap(&e);
    assert_eq!(cap["via"], "prlimit", "{cap}");
    assert!(
        cap["fallback"]
            .as_str()
            .unwrap()
            .contains("did not set the scope's memory.max to 67108864"),
        "{cap}"
    );
}

#[test]
fn a_systemd_run_that_rejects_a_scope_property_falls_back_to_prlimit() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    // Sets memory.max but rejects OOMPolicy, as a systemd older than 253 does on a scope.
    let p = e.bin.join("systemd-run");
    fs::write(
        &p,
        "#!/bin/sh\nfor a; do [ \"$a\" = OOMPolicy=stop ] && { echo 'Unknown assignment' >&2; exit 1; }; done\necho 67108864\n",
    )
    .unwrap();
    StdCommand::new("chmod").arg("+x").arg(&p).status().unwrap();
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(memory_cap(&e)["via"], "prlimit");
}

#[test]
fn a_missing_harness_under_a_cap_is_still_a_usage_error() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n[defaults]\nharness = \"gone\"\n");
    e.config("[harness.gone]\ncommand = [\"ns-no-such-harness\"]\ncommand_write = [\"ns-no-such-harness\"]\n");
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(2);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        err.contains("harness binary `ns-no-such-harness` is not on PATH"),
        "{err}"
    );
}

#[test]
fn a_dry_run_probes_no_memory_cap() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    break_tool(&e, "systemd-run");
    break_tool(&e, "prlimit");
    e.run(&["run", "--issue", "7", "--dry-run"], 0);
}

#[test]
fn a_plain_gate_failure_names_no_signal() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"exit 1\"\n");
    e.run(&["run", "--issue", "7"], 1);
    let g = gate_events(&e);
    assert!(g[0]["signal"].is_null());
    assert_eq!(
        build_event(&e, 2)["decision"],
        "the CI gate failed after build"
    );
}

// ---------------------------------------------------------------- cleanup (#211)

fn branch_exists(e: &Env, unit: &str) -> bool {
    !git(&e.root, &["branch", "--list", &format!("ns/{unit}")]).is_empty()
}

/// Index of the first run-log event matching `pred`.
fn event_at(e: &Env, pred: impl Fn(&Value) -> bool) -> usize {
    run_events(e)
        .iter()
        .position(pred)
        .unwrap_or_else(|| panic!("no such event in {:?}", run_events(e)))
}

fn cleanup_events(e: &Env) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|v| v["event"] == "cleanup")
        .collect()
}

/// A pre-receive hook on `origin` that declines every push to the quality branch.
fn decline_quality_pushes(e: &Env) {
    let hook = e.base.join("remote.git/hooks/pre-receive");
    fs::write(
        &hook,
        "#!/bin/sh\nwhile read old new ref; do\n  [ \"$ref\" = refs/heads/nightshift/quality ] && { echo declined >&2; exit 1; }\ndone\nexit 0\n",
    )
    .unwrap();
    StdCommand::new("chmod")
        .arg("+x")
        .arg(&hook)
        .status()
        .unwrap();
}

/// Make the outbox unwritable too: its temp file's path is a directory.
fn break_outbox(e: &Env) {
    fs::create_dir_all(e.root.join(".git/ns/quality-outbox.jsonl.tmp")).unwrap();
}

fn auto_merge_env() -> Env {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.ctl("review.body", FINDING);
    e
}

#[test]
fn auto_merge_removes_the_worktree_and_branch_after_saving_the_record() {
    let e = auto_merge_env();
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
    assert!(!e.worktree(UNIT).exists());
    assert!(!branch_exists(&e, UNIT));
    assert!(!git(&e.root, &["worktree", "list"]).contains(UNIT));
    let r = &quality_records(&e)[0];
    assert_eq!(
        (r["unit"].as_str(), r["outcome"].as_str()),
        (Some(UNIT), Some("merged"))
    );
    let saved = event_at(&e, |v| v["event"] == "quality_record");
    let removed = event_at(&e, |v| v["event"] == "cleanup");
    assert!(saved < removed);
    let ev = &cleanup_events(&e)[0];
    assert_eq!(
        (
            ev["by"].as_str(),
            ev["removed"].as_bool(),
            ev["pr"].as_u64()
        ),
        (Some("run"), Some(true), Some(12))
    );
}

#[test]
fn auto_merge_with_the_record_in_the_outbox_still_cleans() {
    let e = auto_merge_env();
    decline_quality_pushes(&e);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
    assert!(!e.worktree(UNIT).exists());
    assert_eq!(quality_events(&e)[0]["status"], "outbox");
    let outbox = fs::read_to_string(e.root.join(".git/ns/quality-outbox.jsonl")).unwrap();
    assert!(outbox.contains(UNIT), "{outbox}");
    assert!(quality_records(&e).is_empty());
}

#[test]
fn auto_merge_keeps_the_worktree_when_its_record_is_not_saved() {
    let e = auto_merge_env();
    decline_quality_pushes(&e);
    break_outbox(&e);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["cleanup"]["removed"], false, "{v}");
    assert_eq!(
        v["cleanup"]["reason"], "its quality record is not saved",
        "{v}"
    );
    assert!(e
        .worktree(UNIT)
        .join(format!(".ns/{UNIT}/review.md"))
        .is_file());
    assert!(branch_exists(&e, UNIT));
}

#[test]
fn auto_merge_keeps_a_worktree_with_uncommitted_tracked_changes() {
    let e = auto_merge_env();
    e.queue("ship", &["pass:script"]);
    e.ctl("ship.sh", "echo edited >> README\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        v["cleanup"]["reason"], "uncommitted changes to tracked files (1 paths)",
        "{v}"
    );
    assert_eq!(
        fs::read_to_string(e.worktree(UNIT).join("README")).unwrap(),
        "hi\nedited\n"
    );
    assert!(branch_exists(&e, UNIT));
}

#[test]
fn a_merge_that_leaves_a_pr_head_not_here_keeps_the_worktree() {
    let e = auto_merge_env();
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file(
        "updated-12.head",
        "0123456789abcdef0123456789abcdef01234567",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let reason = v["cleanup"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("has changes merged PR #12 (head 0123456789abcdef0123456789abcdef01234567) does not hold"),
        "{v}"
    );
    assert!(e.worktree(UNIT).exists());
}

/// A unit worktree on `ns/<unit>` with one commit, a review and a `pr.md` naming PR `pr`,
/// issue `n` in `issue_state`, and the PR in `pr_state` with a body that closes the issue.
fn unit_with_pr(
    e: &Env,
    n: u64,
    unit: &str,
    pr: u64,
    issue_state: &str,
    pr_state: &str,
) -> PathBuf {
    let wt = e.worktree(unit);
    git(
        &e.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("ns/{unit}"),
            wt.to_str().unwrap(),
            "main",
        ],
    );
    fs::write(wt.join(format!("{unit}.txt")), "work\n").unwrap();
    git(&wt, &["add", &format!("{unit}.txt")]);
    git(&wt, &["commit", "-q", "-m", unit]);
    let dir = wt.join(".ns").join(unit);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("pr.md"),
        format!("---\nunit: {unit}\nphase: ship\nstatus: pass\npr: https://github.com/o/r/pull/{pr}\n---\n"),
    )
    .unwrap();
    fs::write(
        dir.join("review.md"),
        format!("---\nunit: {unit}\nphase: review\nstatus: pass\nupdated: 2026-10-08T00:00:00Z\n---\n{FINDING}"),
    )
    .unwrap();
    e.issue(n, unit, issue_state);
    e.gh_file(&format!("pr-{pr}.state"), pr_state);
    e.gh_file(&format!("pr-{pr}.body"), &format!("Closes #{n}"));
    wt
}

fn reasons(v: &Value) -> std::collections::BTreeMap<String, String> {
    v["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| {
            (
                k["unit"].as_str().unwrap().to_string(),
                k["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn units_in(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|u| u["unit"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn clean_removes_a_merged_unit_after_saving_its_record_and_keeps_open_ones() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let open = unit_with_pr(&e, 4, "4-open", 40, "OPEN", "MERGED");
    let v = e.run(&["clean"], 0);
    assert_eq!(v["status"], "cleaned", "{v}");
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert_eq!(v["removed"][0]["pr"], 30);
    assert_eq!(reasons(&v)["4-open"], "issue #4 is open", "{v}");
    assert!(!done.exists());
    assert!(!branch_exists(&e, "3-done"));
    assert!(open.join(".ns/4-open/pr.md").is_file());
    assert!(branch_exists(&e, "4-open"));
    let records = quality_records(&e);
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0]["unit"].as_str(),
            records[0]["outcome"].as_str(),
            records[0]["pr"].as_u64(),
            records[0]["issue"].as_u64()
        ),
        (Some("3-done"), Some("merged"), Some(30), Some(3))
    );
    assert!(
        event_at(&e, |v| v["event"] == "quality_record")
            < event_at(&e, |v| v["event"] == "cleanup" && v["unit"] == "3-done")
    );
    assert_eq!(cleanup_events(&e)[0]["by"], "clean");

    // A rerun has nothing to do.
    let v = e.run(&["clean"], 0);
    assert_eq!(v["status"], "nothing-to-clean", "{v}");
    assert_eq!(units_in(&v["removed"]), Vec::<String>::new());
}

#[test]
fn clean_keeps_units_it_cannot_prove_merged_and_says_why() {
    let e = Env::new();
    let dirty = unit_with_pr(&e, 5, "5-dirty", 50, "CLOSED", "MERGED");
    fs::write(dirty.join("5-dirty.txt"), "changed\n").unwrap();
    let ahead = unit_with_pr(&e, 6, "6-ahead", 60, "CLOSED", "MERGED");
    let pr_head = git(&ahead, &["rev-parse", "HEAD"]);
    e.gh_file("pr-60.head", &pr_head);
    fs::write(ahead.join("more.txt"), "more\n").unwrap();
    git(&ahead, &["add", "more.txt"]);
    git(&ahead, &["commit", "-q", "-m", "more"]);
    unit_with_pr(&e, 8, "8-open-pr", 80, "CLOSED", "OPEN");
    unit_with_pr(&e, 9, "9-other", 90, "CLOSED", "MERGED");
    e.gh_file("pr-90.body", "Closes #1");
    unit_with_pr(&e, 10, "10-elsewhere", 100, "CLOSED", "MERGED");
    e.gh_file("pr-100.ref", "someone/branch");
    let nopr = unit_with_pr(&e, 11, "11-nopr", 110, "CLOSED", "MERGED");
    fs::remove_file(nopr.join(".ns/11-nopr/pr.md")).unwrap();
    unit_with_pr(&e, 12, "scratch", 120, "CLOSED", "MERGED");
    unit_with_pr(&e, 13, "13-unread", 130, "CLOSED", "MERGED");
    fs::remove_file(e.ghd.join("issue-13.json")).unwrap();
    // Untracked files never keep a worktree: they are not work the PR could hold.
    let untracked = unit_with_pr(&e, 14, "14-untracked", 140, "CLOSED", "MERGED");
    fs::write(untracked.join("build.log"), "noise\n").unwrap();

    let v = e.run(&["clean"], 0);
    let r = reasons(&v);
    assert_eq!(
        r["5-dirty"], "uncommitted changes to tracked files (1 paths)",
        "{v}"
    );
    let tip = git(&ahead, &["rev-parse", "HEAD"]);
    assert_eq!(
        r["6-ahead"],
        format!("ns/6-ahead at {tip} has changes merged PR #60 (head {pr_head}) does not hold"),
        "{v}"
    );
    assert_eq!(r["8-open-pr"], "PR #80 is OPEN");
    assert_eq!(r["9-other"], "PR #90 does not close #9");
    assert_eq!(
        r["10-elsewhere"],
        "PR #100 is from branch \"someone/branch\", not ns/10-elsewhere"
    );
    assert_eq!(r["11-nopr"], "no PR in .ns/11-nopr/pr.md");
    assert_eq!(r["scratch"], "the unit id names no issue");
    assert!(r["13-unread"].starts_with("cannot read issue #13: "), "{v}");
    assert_eq!(r.len(), 8, "{v}");
    assert_eq!(units_in(&v["removed"]), ["14-untracked"], "{v}");
    for unit in [
        "5-dirty",
        "6-ahead",
        "8-open-pr",
        "9-other",
        "10-elsewhere",
        "11-nopr",
        "scratch",
        "13-unread",
    ] {
        assert!(e.worktree(unit).is_dir(), "{unit}");
        assert!(branch_exists(&e, unit), "{unit}");
    }
    assert_eq!(
        fs::read_to_string(dirty.join("5-dirty.txt")).unwrap(),
        "changed\n"
    );
}

#[test]
fn clean_dry_run_lists_the_plan_and_changes_nothing() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    unit_with_pr(&e, 4, "4-open", 40, "OPEN", "MERGED");
    let v = e.run(&["clean", "--dry-run"], 0);
    assert_eq!(
        (v["dry_run"].as_bool(), v["status"].as_str()),
        (Some(true), Some("planned")),
        "{v}"
    );
    assert_eq!(units_in(&v["planned"]), ["3-done"], "{v}");
    assert_eq!(reasons(&v)["4-open"], "issue #4 is open");
    assert!(v.get("removed").is_none(), "{v}");
    assert!(done.join(".ns/3-done/pr.md").is_file());
    assert!(branch_exists(&e, "3-done"));
    assert!(quality_records(&e).is_empty());
    assert!(run_events(&e).is_empty(), "{:?}", run_events(&e));
}

#[test]
fn clean_keeps_a_unit_a_live_run_holds_and_removes_the_rest() {
    let e = Env::new();
    let held = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let free = unit_with_pr(&e, 4, "4-done", 40, "CLOSED", "MERGED");
    let _held = hold(
        &e.root.join(".git/ns-run-3-done.lock"),
        r#"{"pid":42,"unit":"3-done","issue":3}"#,
    );
    let want = "a live ns run (pid 42) holds its run lock";
    let v = e.run(&["clean", "--dry-run"], 0);
    assert_eq!(units_in(&v["planned"]), ["4-done"], "{v}");
    assert_eq!(reasons(&v)["3-done"], want, "{v}");
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["4-done"], "{v}");
    assert_eq!(reasons(&v)["3-done"], want, "{v}");
    // Only the unit that goes is recorded.
    let units: Vec<_> = quality_records(&e)
        .iter()
        .map(|r| r["unit"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(units, ["4-done"]);
    assert!(held.is_dir());
    assert!(branch_exists(&e, "3-done"));
    assert!(!free.exists());
}

#[test]
fn clean_keeps_a_unit_whose_run_starts_after_the_plan() {
    let e = Env::new();
    let a = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let b = unit_with_pr(&e, 4, "4-done", 40, "CLOSED", "MERGED");
    // Reading one unit's PR starts a run on the other: whichever is read second was planned
    // with its lock free, and must find it taken when it is checked again.
    let lock = |unit: &str| e.root.join(format!(".git/ns-run-{unit}.lock"));
    let start = |unit: &str| {
        format!(
            "( flock {0} sleep 30 >/dev/null 2>&1 & ); until ! flock -n {0} true; do sleep 0.01; done",
            lock(unit).display()
        )
    };
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$*\" in\n  \"pr view 30 \"*) {} ;;\n  \"pr view 40 \"*) {} ;;\nesac\n",
            start("4-done"),
            start("3-done")
        ),
    );
    let v = e.run(&["clean"], 0);
    let r = reasons(&v);
    let want = "a live ns run (pid ?) holds its run lock";
    assert_eq!(
        (r["3-done"].as_str(), r["4-done"].as_str()),
        (want, want),
        "{v}"
    );
    // One of them got past the plan and was refused at the run lock.
    let refused = cleanup_events(&e);
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0]["reason"], want);
    // The lock is taken before the record: neither unit is recorded.
    assert!(quality_records(&e).is_empty());
    assert!(a.is_dir() && b.is_dir());
}

#[test]
fn clean_names_another_cleanup_and_a_run_lock_it_cannot_take() {
    let e = Env::new();
    let a = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let b = unit_with_pr(&e, 4, "4-done", 40, "CLOSED", "MERGED");
    let _other = hold(
        &e.root.join(".git/ns-run-3-done.lock"),
        r#"{"pid":42,"unit":"3-done","issue":3,"by":"watch"}"#,
    );
    fs::create_dir(e.root.join(".git/ns-run-4-done.lock")).unwrap();
    let v = e.run(&["clean"], 0);
    let r = reasons(&v);
    assert_eq!(r["3-done"], "ns watch (pid 42) is cleaning it up", "{v}");
    assert!(r["4-done"].starts_with("cannot take its run lock: "), "{v}");
    assert!(a.is_dir() && b.is_dir());
    assert!(quality_records(&e).is_empty());
}

#[test]
fn clean_with_the_record_in_the_outbox_still_removes() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    decline_quality_pushes(&e);
    let v = e.run(&["clean"], 0);
    assert_eq!(v["records"]["status"], "outbox", "{v}");
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert!(!done.exists());
    let outbox = fs::read_to_string(e.root.join(".git/ns/quality-outbox.jsonl")).unwrap();
    assert!(outbox.contains("\"unit\":\"3-done\""), "{outbox}");
}

#[test]
fn clean_keeps_a_unit_whose_record_is_not_saved() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    decline_quality_pushes(&e);
    break_outbox(&e);
    let v = e.run(&["clean"], 0);
    assert!(v["records"]["error"].is_string(), "{v}");
    let r = reasons(&v);
    assert!(
        r["3-done"].starts_with("its quality record is not saved: "),
        "{v}"
    );
    assert!(done.join(".ns/3-done/review.md").is_file());
    assert!(branch_exists(&e, "3-done"));
}

#[test]
fn clean_fails_when_a_removal_fails() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    // A locked worktree refuses a single --force.
    git(&e.root, &["worktree", "lock", done.to_str().unwrap()]);
    let v = e.run(&["clean"], 1);
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["kept"][0]["error"], true, "{v}");
    assert!(
        reasons(&v)["3-done"].starts_with("worktree not removed: "),
        "{v}"
    );
    assert!(done.is_dir());
}

#[test]
fn watch_cleans_a_unit_a_human_merged_at_start_and_keeps_open_ones() {
    let e = Env::new();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    // A stuck unit's issue stays open: its worktree stays.
    let stuck = unit_with_pr(&e, 4, "4-stuck", 40, "OPEN", "OPEN");

    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["clean"], serde_json::json!([]), "{v}");

    // A human merges PR 12, which closes #7.
    e.issue(7, "Fix the thing", "CLOSED");
    e.gh_file("pr-12.state", "MERGED");
    e.gh_file("pr-12.body", "Closes #7");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["clean"], serde_json::json!([UNIT]), "{v}");
    assert!(e.worktree(UNIT).is_dir());

    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["cleaned"], serde_json::json!([UNIT]), "{v}");
    assert!(!e.worktree(UNIT).exists());
    assert!(!branch_exists(&e, UNIT));
    assert!(stuck.is_dir());
    // The record is written again with the merge as its outcome.
    let last = quality_records(&e).pop().unwrap();
    assert_eq!(
        (last["unit"].as_str(), last["outcome"].as_str()),
        (Some(UNIT), Some("merged"))
    );
    assert_eq!(cleanup_events(&e)[0]["by"], "watch");
}

#[test]
fn watch_cleans_a_unit_merged_while_another_unit_ran() {
    let e = Env::new();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    e.ctl("pr", "13");
    e.ready(2, "Fix a", &["type:fix"], "");
    // A human merges PR 12 while unit 2 runs.
    e.gh_file(
        "hook.sh",
        "if [ \"$1 $2\" = \"issue edit\" ] && [ \"$3\" = 2 ]; then\n  sed -i 's/\"OPEN\"/\"CLOSED\"/' \"$d/issue-7.json\"\n  echo MERGED > \"$d/pr-12.state\"\n  echo 'Closes #7' > \"$d/pr-12.body\"\nfi\n",
    );
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["issue"], 2, "{v}");
    assert_eq!(v["cleaned"], serde_json::json!([UNIT]), "{v}");
    assert!(!e.worktree(UNIT).exists());
    let unit2_ended = event_at(&e, |v| v["event"] == "end" && v["unit"] == "2-fix-a");
    let cleaned = event_at(&e, |v| v["event"] == "cleanup" && v["unit"] == UNIT);
    assert!(unit2_ended < cleaned);
}

#[test]
fn auto_merge_keeps_the_worktree_when_the_merged_head_cannot_be_read() {
    let e = auto_merge_env();
    e.gh_file(
        "hook.sh",
        "[ \"$*\" = \"pr view 12 --json headRefOid\" ] && { echo 'HTTP 502' >&2; exit 1; }\n",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let reason = v["cleanup"]["reason"].as_str().unwrap();
    assert!(reason.starts_with("cannot read PR #12: "), "{v}");
    assert!(e.worktree(UNIT).is_dir());
}

#[test]
fn clean_keeps_the_worktree_it_runs_from() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let out = e.ns().current_dir(&done).args(["clean"]).output();
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        reasons(&v)["3-done"],
        "the current directory is inside it",
        "{v}"
    );
    assert!(done.is_dir());
}

#[test]
fn clean_keeps_the_main_checkout_and_a_worktree_git_cannot_read() {
    let e = Env::new();
    // The main checkout on a merged unit's branch.
    git(&e.root, &["checkout", "-qb", "ns/15-main"]);
    let art = e.root.join(".ns/15-main");
    fs::create_dir_all(&art).unwrap();
    fs::write(art.join("pr.md"), "---\nstatus: pass\npr: 150\n---\n").unwrap();
    e.issue(15, "main", "CLOSED");
    e.gh_file("pr-150.state", "MERGED");
    e.gh_file("pr-150.body", "Closes #15");
    // A worktree whose .git link is broken.
    let broken = unit_with_pr(&e, 16, "16-broken", 160, "CLOSED", "MERGED");
    let head = git(&broken, &["rev-parse", "HEAD"]);
    e.gh_file("pr-160.head", &head);
    e.gh_file("pr-160.ref", "ns/16-broken");
    fs::write(broken.join(".git"), "gitdir: /nonexistent\n").unwrap();

    let v = e.run(&["clean"], 0);
    let r = reasons(&v);
    assert_eq!(r["15-main"], "the main checkout is on its branch", "{v}");
    assert!(
        r["16-broken"].starts_with("cannot read its status: "),
        "{v}"
    );
    assert!(art.join("pr.md").is_file());
    assert!(broken.is_dir());
}

#[test]
fn clean_reports_a_branch_it_could_not_delete() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    fs::write(e.root.join(".git/refs/heads/ns/3-done.lock"), "").unwrap();
    let v = e.run(&["clean"], 1);
    let reason = &reasons(&v)["3-done"];
    assert!(
        reason.starts_with("worktree removed, branch ns/3-done kept: "),
        "{v}"
    );
    assert!(!done.exists());
    assert!(branch_exists(&e, "3-done"));
}

#[test]
fn auto_merge_in_the_main_checkout_keeps_it() {
    let e = auto_merge_env();
    git(&e.root, &["checkout", "-qb", &format!("ns/{UNIT}")]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["worktree"], e.root.to_str().unwrap(), "{v}");
    assert_eq!(
        v["cleanup"]["reason"], "the main checkout is on its branch",
        "{v}"
    );
    assert!(e.root.join(format!(".ns/{UNIT}/pr.md")).is_file());
    assert!(branch_exists(&e, UNIT));
}

#[test]
fn auto_merge_keeps_a_worktree_whose_head_left_the_branch() {
    // Detached, or on another branch at the same commit: what HEAD holds is not the branch's.
    for leave in ["git checkout -q --detach", "git checkout -qb elsewhere"] {
        let e = auto_merge_env();
        e.queue("ship", &["pass:script"]);
        e.ctl("ship.sh", &format!("{leave}\n"));
        let v = e.run(&["run", "--issue", "7"], 0);
        assert_eq!(v["outcome"], "merged", "{leave}: {v}");
        assert_eq!(
            v["cleanup"]["reason"],
            format!("its HEAD is not on ns/{UNIT}"),
            "{leave}: {v}"
        );
        assert!(e.worktree(UNIT).is_dir(), "{leave}");
        assert!(branch_exists(&e, UNIT), "{leave}");
    }
}

#[test]
fn auto_merge_keeps_a_worktree_with_a_review_artifact_no_record_holds() {
    let e = auto_merge_env();
    e.queue("ship", &["pass:script"]);
    e.ctl(
        "ship.sh",
        "mkdir -p .ns/$NS_UNIT/history && echo notes > .ns/$NS_UNIT/history/review-1.md\n",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        v["cleanup"]["reason"], "its quality record is not saved",
        "{v}"
    );
    assert_eq!(
        quality_events(&e)[0]["unparsed"][0]["reason"],
        "no frontmatter"
    );
    assert!(e
        .worktree(UNIT)
        .join(format!(".ns/{UNIT}/history/review-1.md"))
        .is_file());
}

#[test]
fn clean_keeps_a_unit_with_a_review_artifact_no_record_holds() {
    let e = Env::new();
    let wt = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    fs::write(wt.join(".ns/3-done/review.md"), "notes, no frontmatter\n").unwrap();
    let v = e.run(&["clean"], 0);
    assert_eq!(
        reasons(&v)["3-done"],
        "its quality record is not saved: cannot read .ns/3-done/review.md: no frontmatter",
        "{v}"
    );
    assert!(wt.join(".ns/3-done/review.md").is_file());
}

#[test]
fn watch_tries_a_unit_that_stays_once_a_night() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    git(&e.root, &["worktree", "lock", done.to_str().unwrap()]);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(4, "Fix b", &["type:fix"], "");
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["units"]), [2, 4], "{v}");
    assert_eq!(v["cleaned"], serde_json::json!([]), "{v}");
    let tries: Vec<Value> = cleanup_events(&e)
        .into_iter()
        .filter(|v| v["unit"] == "3-done")
        .collect();
    assert_eq!(tries.len(), 1, "{tries:?}");
    assert_eq!(tries[0]["error"], true);
    let writes = run_events(&e)
        .into_iter()
        .filter(|v| v["event"] == "quality_record" && v["units"] == serde_json::json!(["3-done"]))
        .count();
    assert_eq!(writes, 1);
    assert!(done.is_dir());
}

// ---------------------------------------------------------------- parallel units

const A: &str = "2-fix-a";
const B: &str = "3-fix-b";

fn two_ready(e: &Env) {
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
}

fn mkfifo(p: &Path) {
    assert!(StdCommand::new("mkfifo").arg(p).status().unwrap().success());
}

/// Hold `unit`'s next `phase` at its start: the fake harness writes its pid to the first fifo
/// returned, then waits for a line on the second.
fn hold_phase(e: &Env, unit: &str, phase: &str) -> (PathBuf, PathBuf) {
    let entered = e.ctrl.join(format!("entered-{unit}-{phase}"));
    let go = e.ctrl.join(format!("go-{unit}-{phase}"));
    mkfifo(&entered);
    mkfifo(&go);
    (entered, go)
}

/// Write a line to `go` from a thread, since a fifo write blocks until someone reads it.
fn release(go: &Path) {
    let go = go.to_path_buf();
    thread::spawn(move || fs::write(go, "go\n"));
}

/// A running `ns watch`, its stderr lines on a channel and its stdout collected.
struct Watching {
    g: Group,
    rx: mpsc::Receiver<Option<String>>,
    stdout: mpsc::Receiver<Vec<u8>>,
    seen: Vec<String>,
}

impl Watching {
    fn start(ns: &mut Ns) -> Watching {
        let mut g = ns.start_piped();
        let stdout = read_all(g.take_stdout());
        let (tx, rx) = mpsc::channel();
        send_lines(g.take_stderr(), tx);
        Watching {
            g,
            rx,
            stdout,
            seen: Vec::new(),
        }
    }

    /// Wait until the phase held on `entered` starts; the harness's pid.
    fn entered(&self, entered: &Path) -> String {
        let (tx, rx) = mpsc::channel();
        let f = entered.to_path_buf();
        thread::spawn(move || {
            let _ = tx.send(fs::read_to_string(f));
        });
        self.g.recv(&rx).unwrap().trim().to_string()
    }

    /// Read stderr until a line contains `want`; panics with what it read if watch ends first.
    fn line(&mut self, want: &str) -> String {
        if !line_until(&self.g, &self.rx, want, &mut self.seen) {
            panic!("no line with {want:?}:\n{}", self.seen.join("\n"));
        }
        self.seen.last().unwrap().clone()
    }

    fn signal(&self, sig: i32, group: bool) {
        let pid = self.g.id() as libc::pid_t;
        // SAFETY: kill(2) on the ns process, or its process group, that this test started.
        assert_eq!(
            unsafe { libc::kill(if group { -pid } else { pid }, sig) },
            0
        );
    }

    /// Wait for watch to end: its exit code, summary and whole stderr.
    fn finish(mut self) -> (Option<i32>, Value, String) {
        let code = self.g.wait().code();
        while let Some(l) = self.g.recv(&self.rx) {
            self.seen.push(l);
        }
        let err = self.seen.join("\n");
        let out = self.g.recv(&self.stdout);
        let v = serde_json::from_slice(&out).unwrap_or_else(|x| {
            panic!("{x}: stdout={} stderr={err}", String::from_utf8_lossy(&out))
        });
        (code, v, err)
    }
}

/// Each unit record's issue and outcome, sorted by issue.
fn by_issue(units: &[Value]) -> Vec<(u64, String)> {
    let mut o: Vec<(u64, String)> = units
        .iter()
        .map(|u| {
            (
                u["issue"].as_u64().unwrap(),
                u["outcome"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    o.sort();
    o
}

fn units(v: &Value) -> Vec<Value> {
    v["units"].as_array().unwrap().clone()
}

fn each(issues: &[u64], outcome: &str) -> Vec<(u64, String)> {
    issues.iter().map(|n| (*n, outcome.to_string())).collect()
}

/// The pid of each `ns run` that started a unit, by unit, from the run log's start events.
fn run_pids(e: &Env) -> Vec<(String, u64)> {
    let mut pids: Vec<(String, u64)> = run_events(e)
        .into_iter()
        .filter(|ev| ev["event"] == "start")
        .map(|ev| {
            (
                ev["unit"].as_str().unwrap().to_string(),
                ev["pid"].as_u64().unwrap(),
            )
        })
        .collect();
    pids.sort();
    pids
}

fn phase_count(e: &Env, phase: &str) -> usize {
    e.calls().iter().filter(|c| *c == phase).count()
}

#[test]
fn watch_parallel_runs_two_units_together_each_in_its_own_ns_run() {
    let e = Env::new();
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    let watch_pid = u64::from(w.g.id());
    // Both builds are in at once: one unit at a time never reaches the second.
    w.entered(&in_a);
    w.entered(&in_b);
    release(&go_a);
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert_eq!(v["cost_usd"], 5.0, "{v}");
    let pids = run_pids(&e);
    assert_eq!(pids.len(), 2, "{pids:?}");
    assert_ne!(pids[0].1, pids[1].1, "{pids:?}");
    assert!(pids.iter().all(|(_, p)| *p != watch_pid), "{pids:?}");
    // The output and the run log name the worker that ran each unit.
    let mut workers: Vec<u64> = units(&v)
        .iter()
        .map(|u| u["worker"].as_u64().unwrap())
        .collect();
    workers.sort();
    assert_eq!(workers, [1, 2], "{v}");
    for u in units(&v) {
        let title = if u["issue"] == 2 { "Fix a" } else { "Fix b" };
        let line = format!("ns watch: #{} {title} (worker {})", u["issue"], u["worker"]);
        assert!(err.contains(&line), "{line}\n{err}");
    }
    let starts: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "worker_start")
        .collect();
    assert_eq!(starts.len(), 2, "{starts:?}");
    for s in &starts {
        let unit = format!(
            "{}-fix-{}",
            s["issue"],
            if s["issue"] == 2 { "a" } else { "b" }
        );
        let run = pids.iter().find(|(u, _)| *u == unit).unwrap();
        assert_eq!(s["run_pid"], run.1, "{s}");
        assert!(s["worker"] == 1 || s["worker"] == 2, "{s}");
    }
}

#[test]
fn watch_parallel_units_take_a_runner_lock_one_at_a_time() {
    let e = Env::new();
    let locks = e.base.join("locks");
    e.bench(&locks);
    e.factory("[limits]\nparallel = 2\n[phases.verify]\nrunner = \"bench\"\n");
    two_ready(&e);
    let holds = [hold_phase(&e, A, "verify"), hold_phase(&e, B, "verify")];
    // A pinned clock would end a lock wait at once: watch runs on the real clock.
    let mut w = Watching::start(e.ns().env_remove("NS_NOW").arg("watch"));
    let line = w.line("verify waits for lock bench-1");
    let (waiter, holder) = if line.contains(&format!("ns run: {A} ")) {
        (0, 1)
    } else {
        (1, 0)
    };
    w.entered(&holds[holder].0);
    release(&holds[holder].1);
    // The waiter enters only once the holder's verify ends and lets the lock go.
    w.entered(&holds[waiter].0);
    release(&holds[waiter].1);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    let waits: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "lock_wait" && ev["lock"] == "bench-1")
        .collect();
    assert_eq!(waits.len(), 1, "{waits:?}");
}

#[test]
fn watch_parallel_units_merge_one_at_a_time() {
    let e = Env::new();
    e.factory(AUTO);
    two_ready(&e);
    e.ctl(&format!("pr-{A}"), "12");
    e.ctl(&format!("pr-{B}"), "13");
    e.queue(&format!("build.{A}"), &["pass:commit"]);
    e.queue(&format!("build.{B}"), &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    let go = merges_in_order(&e, &[12, 13]);
    let mut w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.line("merge waits for the merge lock (held by pid ");
    release(&go);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "merged"), "{v}");
    let order = fs::read_to_string(e.ctrl.join("order")).unwrap();
    let order: Vec<&str> = order.lines().collect();
    assert_eq!(order.len(), 4, "{order:?}");
    let pr = |l: &str| l.split(' ').nth(1).unwrap().to_string();
    assert_eq!(
        order,
        [
            format!("in {}", pr(order[0])),
            format!("out {}", pr(order[0])),
            format!("in {}", pr(order[2])),
            format!("out {}", pr(order[2])),
        ]
    );
    assert_ne!(pr(order[0]), pr(order[2]), "{order:?}");
}

#[test]
fn watch_parallel_never_takes_an_issue_twice() {
    let e = Env::new();
    two_ready(&e);
    // GitHub keeps listing a claimed issue as ready for a while.
    e.gh_file("ready-sticks", "");
    let reread = e.ctrl.join("reread");
    mkfifo(&reread);
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$1 $2 $3\" in \"api --paginate \"*labels=status:ready-for-agent*)\n\
               echo x >> \"$d/reads\"\n\
               [ \"$(wc -l < \"$d/reads\")\" -eq 2 ] && echo reread > {reread:?} ;;\n\
             esac\n"
        ),
    );
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.entered(&in_b);
    // Unit 2 ended and its slot refills from a queue that still lists #3, mid build.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(&reread));
    });
    w.g.recv(&rx).unwrap();
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    let calls = e.gh_calls();
    assert_eq!(
        calls.matches("--add-label status:in-progress").count(),
        2,
        "{calls}"
    );
    assert_eq!(run_pids(&e).len(), 2, "{:?}", run_pids(&e));
}

#[test]
fn watch_parallel_claims_an_issue_before_its_unit_starts() {
    let e = Env::new();
    two_ready(&e);
    let seen = e.ctrl.join("seen");
    e.ctl(
        "triage.sh",
        &format!("cat \"$FAKE_GH_DIR/labels-${{NS_UNIT%%-*}}\" >> {seen:?}\n"),
    );
    e.queue(&format!("triage.{A}"), &["pass:script"]);
    e.queue(&format!("triage.{B}"), &["pass:script"]);
    // Each claim leaves a mark in the run log, which watch also writes each unit's start to.
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *\"--add-label status:in-progress\"*)\n\
           echo \"{\\\"event\\\":\\\"test_claim\\\",\\\"issue\\\":$3}\" >> \"$(git rev-parse --git-common-dir)/ns/runs.jsonl\" ;;\n\
         esac\n",
    );
    let v = e.run(&["watch", "--parallel", "2"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    for n in [2, 3] {
        let claimed = event_at(&e, |v| v["event"] == "test_claim" && v["issue"] == n);
        let started = event_at(&e, |v| v["event"] == "worker_start" && v["issue"] == n);
        assert!(claimed < started, "#{n}: {:?}", run_events(&e));
    }
    let at_start = fs::read_to_string(&seen).unwrap();
    assert_eq!(
        at_start.matches("status:in-progress").count(),
        2,
        "{at_start}"
    );
    assert!(!at_start.contains("status:ready-for-agent"), "{at_start}");
}

#[test]
fn watch_parallel_pauses_every_unit_on_a_usage_limit_and_resumes_them_together() {
    let e = Env::new();
    two_ready(&e);
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue(&format!("build.{A}"), &["limit", "pass:commit"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(
        e.ns()
            .args(["watch", "--parallel", "2", "--until", "06:30"]),
    );
    // Unit 2's build hits its limit only once unit 3's build has begun.
    w.entered(&in_b);
    w.entered(&in_a);
    release(&go_a);
    w.line("ns watch: #2 paused on a usage limit until 2026-10-09T01:00:00+00:00");
    // Unit 3's build ends after the pause began; it pauses before its next phase.
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    let us = units(&v);
    assert_eq!(us.len(), 4, "{v}");
    assert_eq!(by_issue(&us[..1]), each(&[2], "paused"), "{v}");
    assert_eq!(by_issue(&us[1..2]), each(&[3], "paused"), "{v}");
    for u in &us[..2] {
        assert_eq!(u["reset_at"], "2026-10-09T01:00:00+00:00", "{v}");
    }
    assert!(
        us[1]["reason"].as_str().unwrap().contains("another unit"),
        "{v}"
    );
    assert_eq!(by_issue(&us[2..]), each(&[2, 3], "done"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert_eq!(
        err.matches("usage limit, sleeping until").count(),
        1,
        "{err}"
    );
    assert!(err.contains("usage limit, sleeping until 2026-10-09T01:00:00+00:00"));
    // Unit 3 resumed at verify; unit 2 ran its build again.
    assert_eq!(phase_count(&e, "build"), 3, "{:?}", e.calls());
    assert_eq!(phase_count(&e, "verify"), 2, "{:?}", e.calls());
    let calls = e.gh_calls();
    assert_eq!(
        calls.matches("--add-label status:in-progress").count(),
        2,
        "{calls}"
    );
}

#[test]
fn watch_parallel_resumes_at_the_later_of_two_resets() {
    let e = Env::new();
    two_ready(&e);
    // The later reset comes first: a pause must not shrink to the earlier one that follows.
    e.ctl("reset", &(NOW + 7200).to_string());
    e.ctl(&format!("reset.{B}"), &(NOW + 3600).to_string());
    e.queue(&format!("build.{A}"), &["limit", "pass:commit"]);
    e.queue(&format!("build.{B}"), &["limit", "pass:commit"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(
        e.ns()
            .args(["watch", "--parallel", "2", "--until", "06:30"]),
    );
    // Unit 2's build hits its limit only once unit 3's build has begun.
    w.entered(&in_b);
    w.entered(&in_a);
    release(&go_a);
    w.line("ns watch: #2 paused on a usage limit until 2026-10-09T02:00:00+00:00");
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    let us = units(&v);
    assert_eq!(us[1]["issue"], 3, "{v}");
    assert_eq!(us[1]["reset_at"], "2026-10-09T01:00:00+00:00", "{v}");
    assert!(
        err.contains("ns watch: #3 paused on a usage limit until 2026-10-09T02:00:00+00:00"),
        "{err}"
    );
    assert_eq!(by_issue(&us[2..]), each(&[2, 3], "done"), "{v}");
    assert!(
        err.contains("usage limit, sleeping until 2026-10-09T02:00:00+00:00"),
        "{err}"
    );
    assert!(!err.contains("sleeping until 2026-10-09T01:00:00"), "{err}");
}

#[test]
fn watch_parallel_gives_every_paused_unit_back_when_the_reset_is_past_until() {
    let e = Env::new();
    two_ready(&e);
    e.ctl("reset", &(NOW + 8 * 3600).to_string());
    e.queue(&format!("build.{A}"), &["limit"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(
        e.ns()
            .args(["watch", "--parallel", "2", "--until", "06:30"]),
    );
    // Unit 2's build hits its limit only once unit 3's build has begun.
    w.entered(&in_b);
    w.entered(&in_a);
    release(&go_a);
    w.line("ns watch: #2 paused on a usage limit until");
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(v["stopped"], "usage limit resets after --until", "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "paused"), "{v}");
    for n in [2, 3] {
        assert_eq!(e.labels(n), ["type:fix", "status:ready-for-agent"]);
    }
}

#[test]
fn watch_parallel_budget_stop_reaches_every_unit() {
    let e = Env::new();
    e.factory("[limits]\nbudget_usd = 2.0\nparallel = 2\n");
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(e.ns().args(["watch"]));
    // Both triages ran: $1.00 of $2.00 spent.
    w.entered(&in_a);
    w.entered(&in_b);
    // Unit 2's build and verify bring the night to $2.00; it stops before review.
    release(&go_a);
    w.line("ns watch: #2 budget");
    // Unit 3's build brings it to $2.50, and unit 3 stops before verify: it counts unit 2's spend.
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "budget"), "{v}");
    assert_eq!(v["stopped"], "budget", "{v}");
    assert_eq!(v["cost_usd"], 2.5, "{v}");
    assert_eq!(phase_count(&e, "verify"), 1, "{:?}", e.calls());
    assert_eq!(phase_count(&e, "review"), 0, "{:?}", e.calls());
    for n in [2, 3] {
        assert_eq!(e.labels(n), ["type:fix", "status:ready-for-agent"]);
    }
}

#[test]
fn watch_parallel_runs_the_triage_pass_when_a_worker_finishes() {
    let e = Env::new();
    two_ready(&e);
    e.untriaged(5, "Escape", &["status:needs-triage"]);
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-escape", &["pass:script"]);
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let (in_t, go_t) = hold_phase(&e, "5-escape", "triage");
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.entered(&in_b);
    // Unit 2 ended; the pass triages #5 before its slot refills, while unit 3 still builds.
    w.entered(&in_t);
    release(&go_t);
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3, 5], "done"), "{v}");
    // Units' harnesses ran at once and may share a call number, so count the prompts, not the
    // calls; the triage-only run ran with no other harness beside it.
    let triage_only = fs::read_dir(&e.ctrl)
        .unwrap()
        .filter_map(|f| fs::read_to_string(f.unwrap().path()).ok())
        .filter(|t| t.contains("Triage only") && t.contains("unit `5-escape`"))
        .count();
    assert_eq!(triage_only, 1, "{err}");
}

#[test]
fn watch_runs_one_unit_at_a_time_unless_told_otherwise() {
    for (factory, args) in [
        ("", vec!["watch"]),
        ("[limits]\nparallel = 3\n", vec!["watch", "--parallel", "1"]),
    ] {
        let e = Env::new();
        e.factory(factory);
        two_ready(&e);
        let v = e.run(&args, 0);
        assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
        let calls = e.gh_calls();
        let last_of_2 = calls.rfind("issue edit 2 ").unwrap();
        let claim_3 = calls
            .find(
                "issue edit 3 --remove-label status:ready-for-agent --add-label status:in-progress",
            )
            .unwrap();
        assert!(last_of_2 < claim_3, "{args:?}: {calls}");
        assert!(units(&v).iter().all(|u| u["worker"] == 1), "{v}");
    }
}

#[test]
fn watch_parallel_must_be_at_least_one_and_the_flag_wins() {
    let e = Env::new();
    e.ns()
        .args(["watch", "--parallel", "0"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--parallel"));
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["parallel"], 1, "{v}");
    e.factory("[limits]\nparallel = 3\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["parallel"], 3, "{v}");
    let v = e.run(&["watch", "--dry-run", "--parallel", "2"], 0);
    assert_eq!(v["parallel"], 2, "{v}");
    e.factory("[limits]\nparallel = 0\n");
    e.ns()
        .args(["watch", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("parallel"));
}

#[test]
fn watch_parallel_once_and_max_units_cap_the_units_started() {
    let e = Env::new();
    two_ready(&e);
    e.ready(4, "Fix c", &["type:fix"], "");
    let v = e.run(&["watch", "--parallel", "3", "--max-units", "2"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(v["stopped"], "max_units", "{v}");
    assert!(!e.gh_calls().contains("issue edit 4"), "{}", e.gh_calls());
    let v = e.run(&["watch", "--parallel", "3", "--once"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[4], "done"), "{v}");
}

#[test]
fn watch_parallel_settles_the_other_unit_when_one_ends_without_a_result() {
    let e = Env::new();
    two_ready(&e);
    // ns run can't name unit 2's worktree without its issue, and fails.
    fs::remove_file(e.ghd.join("issue-2.json")).unwrap();
    let out = e.ns().args(["watch", "--parallel", "2"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("cannot read issue #2"), "{err}");
    assert!(err.contains("ns run --issue 2 ended with exit 2"), "{err}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    let three = e.labels(3);
    assert!(
        !three
            .iter()
            .any(|l| l == "status:in-progress" || l == "status:ready-for-agent"),
        "{three:?}"
    );
    // The summary still reports the unit that finished, and the error.
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|x| panic!("{x}: {err}"));
    assert_eq!(v["stopped"], "error", "{v}");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("ns run --issue 2 ended with exit 2"),
        "{v}"
    );
    assert_eq!(by_issue(&units(&v)), each(&[3], "done"), "{v}");
}

#[test]
fn watch_parallel_starts_an_issue_listed_twice_once() {
    let e = Env::new();
    // The listing shifted between pages and names #2 twice.
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(2, "Fix a", &["type:fix"], "");
    let v = e.run(&["watch", "--parallel", "2"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
    assert_eq!(run_pids(&e).len(), 1, "{:?}", run_pids(&e));
    let calls = e.gh_calls();
    assert_eq!(
        calls.matches("--add-label status:in-progress").count(),
        1,
        "{calls}"
    );
}

#[test]
fn cleanup_between_units_leaves_the_worktree_of_a_unit_still_running() {
    let e = Env::new();
    // Unit 3's worktree looks merged to cleanup once its setup starts: issue closed, PR merged.
    unit_with_pr(&e, 3, "3-done", 30, "OPEN", "OPEN");
    e.ready(3, "done", &["type:fix"], "");
    e.ready(2, "Fix a", &["type:fix"], "");
    let entered = e.ctrl.join("entered-setup");
    let go = e.ctrl.join("go-setup");
    mkfifo(&entered);
    mkfifo(&go);
    let setup = format!(
        "if [ \"$NS_UNIT\" = 3-done ]; then sed -i 's/OPEN/CLOSED/' \"$FAKE_GH_DIR/issue-3.json\"; \
         echo MERGED > \"$FAKE_GH_DIR/pr-30.state\"; echo in > {entered:?}; cat {go:?} > /dev/null; fi"
    );
    e.factory(&format!("[worktree]\nsetup = [{setup:?}]\n"));
    let mut w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    // Unit 3 holds its run lock in setup while unit 2 runs to its end and cleanup runs after it.
    w.entered(&entered);
    w.line("ns watch: #2 done (worker");
    let looked = e.ctrl.join("looked");
    mkfifo(&looked);
    e.gh_file(
        "hook.sh",
        &format!(
            "[ \"$*\" = \"pr view 30 --json state,headRefOid,headRefName,body\" ] && echo x > {looked:?}\n"
        ),
    );
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(&looked));
    });
    w.g.recv(&rx).unwrap();
    fs::remove_file(e.ghd.join("hook.sh")).unwrap();
    release(&go);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    // The worktree went only after unit 3's run ended and let its lock go.
    let ended = event_at(&e, |v| v["event"] == "end" && v["unit"] == "3-done");
    let cleaned = event_at(&e, |v| v["event"] == "cleanup" && v["unit"] == "3-done");
    assert!(ended < cleaned, "{:?}", run_events(&e));
    assert_eq!(v["cleaned"], serde_json::json!(["3-done"]), "{v}");
}

/// Both units mid build when `sig` reaches `ns watch` (its process group, when `group`): every
/// unit goes back to the queue and no process it started outlives it.
fn assert_a_stop_requeues_every_running_unit(sig: i32, group: bool, name: &str) {
    let e = Env::new();
    two_ready(&e);
    let (in_a, _go_a) = hold_phase(&e, A, "build");
    let (in_b, _go_b) = hold_phase(&e, B, "build");
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    let harnesses = [w.entered(&in_a), w.entered(&in_b)];
    w.signal(sig, group);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(128 + sig), "{err}");
    assert_eq!(v["stopped"], name, "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "interrupted"), "{v}");
    for n in [2, 3] {
        assert_eq!(e.labels(n), ["type:fix", "status:ready-for-agent"]);
    }
    let runs: Vec<String> = run_pids(&e).iter().map(|(_, p)| p.to_string()).collect();
    assert_eq!(runs.len(), 2, "{err}");
    for pid in harnesses.iter().chain(&runs) {
        assert!(exits(pid), "process {pid} outlived ns watch: {err}");
    }
    let ends = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "end" && ev["outcome"] == "interrupted")
        .count();
    assert_eq!(ends, 2, "{err}");
}

#[test]
fn sigterm_returns_every_running_unit_to_the_queue() {
    assert_a_stop_requeues_every_running_unit(libc::SIGTERM, false, "SIGTERM");
}

#[test]
fn ctrl_c_returns_every_running_unit_to_the_queue() {
    assert_a_stop_requeues_every_running_unit(libc::SIGINT, true, "SIGINT");
}

/// A night directory as `ns watch` writes it, with `spend` already spent and the hold `hold`.
fn night_dir(e: &Env, spend: f64, hold: Option<i64>) -> PathBuf {
    let dir = e.base.join("night");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("night.json"),
        format!(
            "{{\"watch_pid\":{},\"until\":null,\"gate\":null}}",
            std::process::id()
        ),
    )
    .unwrap();
    fs::write(
        dir.join("spend.jsonl"),
        format!("{{\"unit\":\"other\",\"usd\":{spend}}}\n"),
    )
    .unwrap();
    if let Some(r) = hold {
        fs::write(dir.join("hold.json"), format!("{{\"reset_at\":{r}}}")).unwrap();
    }
    dir
}

#[test]
fn a_run_in_a_night_counts_every_unit_s_spend_against_the_budget() {
    let e = Env::new();
    e.factory("[limits]\nbudget_usd = 40.0\n");
    let night = night_dir(&e, 39.75, None);
    let v = e.run(
        &["run", "--issue", "7", "--night", night.to_str().unwrap()],
        3,
    );
    assert_eq!(v["outcome"], "budget", "{v}");
    assert_eq!(v["reason"], "spent $40.25 of the $40.00 budget", "{v}");
    assert_eq!(v["cost_usd"], 0.5, "{v}");
    assert_eq!(e.calls(), ["triage"]);
    let spend = fs::read_to_string(night.join("spend.jsonl")).unwrap();
    assert!(
        spend.ends_with(&format!("{{\"unit\":\"{UNIT}\",\"usd\":0.5}}\n")),
        "{spend}"
    );
}

#[test]
fn a_run_in_a_night_stops_before_a_phase_while_a_hold_lasts() {
    let e = Env::new();
    let night = night_dir(&e, 0.0, Some(NOW + 3600));
    let v = e.run(
        &["run", "--issue", "7", "--night", night.to_str().unwrap()],
        4,
    );
    assert_eq!(v["outcome"], "paused", "{v}");
    assert_eq!(v["reset_at"], NOW + 3600, "{v}");
    assert!(
        v["reason"].as_str().unwrap().contains("another unit"),
        "{v}"
    );
    assert!(e.calls().is_empty(), "{:?}", e.calls());
    // A hold whose reset has come holds nothing.
    let night = night_dir(&e, 0.0, Some(NOW));
    let v = e.run(
        &["run", "--issue", "7", "--night", night.to_str().unwrap()],
        0,
    );
    assert_eq!(v["outcome"], "done", "{v}");
}

#[test]
fn cleanup_at_a_resume_leaves_the_worktree_of_the_paused_unit() {
    let e = Env::new();
    // Unit 2's worktree has a PR from an earlier night; a human merges it during the pause.
    unit_with_pr(&e, 2, A, 20, "OPEN", "OPEN");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue(&format!("triage.{A}"), &["limit"]);
    let (entered, go) = hold_phase(&e, A, "triage");
    let w = Watching::start(e.ns().args(["watch", "--once", "--until", "06:30"]));
    w.entered(&entered);
    e.issue(2, "Fix a", "CLOSED");
    e.gh_file("pr-20.state", "MERGED");
    release(&go);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    let us = units(&v);
    assert_eq!(us[0]["outcome"], "paused", "{v}");
    assert_eq!(us[1]["issue"], 2, "{v}");
    // The unit resumed in its own worktree; cleanup removed it only once the unit had ended.
    let starts: Vec<usize> = run_events(&e)
        .iter()
        .enumerate()
        .filter(|(_, v)| v["event"] == "worker_start" && v["issue"] == 2)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(starts.len(), 2, "{err}");
    let removed = run_events(&e)
        .iter()
        .position(|v| v["event"] == "cleanup" && v["unit"] == A && v["removed"] == true);
    assert!(
        removed.is_none_or(|r| r > starts[1]),
        "{:?}",
        run_events(&e)
    );
}

// ---------------------------------------------------------------- self-update

/// Make the repo the nightshift source, `cli/Cargo.toml` naming the package `nightshift`, put the
/// fake cargo on PATH, and return the commit pushed, which the `ns` under test counts as its
/// build commit.
fn nightshift_repo(e: &Env) -> String {
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/factory/cargo"),
        e.bin.join("cargo"),
    )
    .unwrap();
    fs::create_dir_all(e.root.join("cli")).unwrap();
    fs::write(
        e.root.join("cli/Cargo.toml"),
        "[package]\nname = \"nightshift\"\n",
    )
    .unwrap();
    land(e, "cli/x", "old\n")
}

/// Commit `text` to `path` and push it to origin's main, as a unit merged tonight would; the
/// commit.
fn land(e: &Env, path: &str, text: &str) -> String {
    let p = e.root.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
    git(&e.root, &["add", "."]);
    git(&e.root, &["commit", "-q", "-m", path]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    git(&e.root, &["rev-parse", "HEAD"])
}

/// `ns watch` built from `from`, whose stub successor execs the `ns` under test.
fn watch_from(e: &Env, from: &str) -> Ns {
    let mut c = e.ns();
    c.env("NS_BUILD_COMMIT", from)
        .env("REAL_NS", common::ns_path());
    c
}

fn cargo_calls(e: &Env) -> usize {
    fs::read_to_string(e.ctrl.join("cargo-calls"))
        .unwrap_or_default()
        .lines()
        .count()
}

fn events_named(e: &Env, name: &str) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|ev| ev["event"] == name)
        .collect()
}

#[test]
fn watch_hands_off_to_a_new_build_between_units_and_the_night_goes_on() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    two_ready(&e);
    e.ready(4, "Fix c", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let w = Watching::start(watch_from(&e, &from).args([
        "watch",
        "--until",
        "06:30",
        "--max-units",
        "2",
    ]));
    w.entered(&in_a);
    let to = land(&e, "cli/x", "new\n");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    // The night's state crossed the hand-off: both units are in one summary, with all the spend
    // and the same deadline, and unit 2 counts toward --max-units.
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(v["cost_usd"], 5.0, "{v}");
    assert_eq!(v["until"], "2026-10-09T06:30:00+00:00", "{v}");
    assert_eq!(v["stopped"], "max_units", "{v}");
    // Built once, from the new commit, and exec'd with the same arguments and the state.
    assert_eq!(cargo_calls(&e), 1);
    assert_eq!(
        fs::read_to_string(e.ctrl.join("cargo-built")).unwrap(),
        "new\n"
    );
    let args = fs::read_to_string(e.ctrl.join("handed-off")).unwrap();
    assert!(
        args.starts_with("watch --until 06:30 --max-units 2 --resume-night "),
        "{args}"
    );
    let state = args.trim().rsplit(' ').next().unwrap();
    assert!(!Path::new(state).exists(), "{state} left behind");
    let ev = events_named(&e, "self_update");
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(
        (&ev[0]["from"], &ev[0]["to"]),
        (&serde_json::json!(from), &serde_json::json!(to))
    );
    assert!(
        err.contains(&format!("self-update {} -> {}", &from[..12], &to[..12])),
        "{err}"
    );
    // Unit 3 started after the hand-off, from the new binary.
    let updated = event_at(&e, |ev| ev["event"] == "self_update");
    let started_3 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 3);
    assert!(updated < started_3, "{:?}", run_events(&e));
}

#[test]
fn draining_starts_no_unit_and_waits_for_the_running_one() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    two_ready(&e);
    e.ready(4, "Fix c", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(watch_from(&e, &from).args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    land(&e, "cli/x", "new\n");
    release(&go_b);
    w.line("#3 done");
    w.line("draining");
    // Unit 2 is still in its build: nothing is built, and unit 4 waits for the update.
    assert_eq!(cargo_calls(&e), 0);
    assert!(events_named(&e, "self_update").is_empty());
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3, 4], "done"), "{v}");
    assert_eq!(cargo_calls(&e), 1);
    assert_eq!(err.matches("draining").count(), 1, "{err}");
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    let updated = event_at(&e, |ev| ev["event"] == "self_update");
    let started_4 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 4);
    assert!(
        ended_2 < updated && updated < started_4,
        "{:?}",
        run_events(&e)
    );
}

/// A night on a nightshift repo whose origin has a `cli/` change the build commit lacks, with
/// the fake cargo in `mode`; its summary and stderr.
fn night_with_a_failed_update(mode: &str) -> (Env, String, Value, String) {
    let e = Env::new();
    let from = nightshift_repo(&e);
    let to = land(&e, "cli/x", "new\n");
    e.ctl("cargo-mode", mode);
    two_ready(&e);
    let out = watch_from(&e, &from)
        .args(["watch"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    (e, to, v, err)
}

fn assert_kept_the_old_binary(e: &Env, to: &str, v: &Value, err: &str, why: &str) {
    // The night went on without a hand-off and tried the commit once.
    assert_eq!(by_issue(&units(v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(cargo_calls(e), 1, "{err}");
    assert!(!e.ctrl.join("handed-off").exists());
    assert!(events_named(e, "self_update").is_empty());
    let warning = format!("ns watch: warning: self-update to {} failed", &to[..12]);
    assert!(err.contains(&warning) && err.contains(why), "{err}");
    let failed = events_named(e, "self_update_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["to"], to);
    assert!(
        failed[0]["error"].as_str().unwrap().contains(why),
        "{failed:?}"
    );
}

#[test]
fn a_failed_build_keeps_the_old_binary_and_is_not_retried() {
    let (e, to, v, err) = night_with_a_failed_update("fail");
    assert_kept_the_old_binary(&e, &to, &v, &err, "could not compile nightshift");
}

#[test]
fn a_failed_check_keeps_the_old_binary_and_is_not_retried() {
    let (e, to, v, err) = night_with_a_failed_update("badcheck");
    assert_kept_the_old_binary(&e, &to, &v, &err, "dry run failed");
    let (e, to, v, err) = night_with_a_failed_update("versionfails");
    assert_kept_the_old_binary(&e, &to, &v, &err, "--version said");
    let (e, to, v, err) = night_with_a_failed_update("badversion");
    assert_kept_the_old_binary(&e, &to, &v, &err, "--version said \"ns 0.1.0 (0000000)\"");
}

/// A night with a `cli/` change on origin the build commit lacks, which never updates; its
/// stderr.
fn assert_never_updates(e: &Env, ns: &mut Ns) -> String {
    two_ready(e);
    let out = ns.assert().code(0).get_output().clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    assert!(!err.contains("draining"), "{err}");
    assert_eq!(cargo_calls(e), 0, "{err}");
    assert!(events_named(e, "self_update").is_empty());
    assert!(events_named(e, "self_update_failed").is_empty());
    err.into_owned()
}

#[test]
fn watch_on_another_repo_never_drains_or_rebuilds() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    fs::write(
        e.root.join("cli/Cargo.toml"),
        "[package]\nname = \"other\"\n",
    )
    .unwrap();
    land(&e, "cli/x", "new\n");
    assert_never_updates(&e, watch_from(&e, &from).args(["watch"]));
}

#[test]
fn no_self_update_turns_it_off() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    land(&e, "cli/x", "new\n");
    assert_never_updates(
        &e,
        watch_from(&e, &from).args(["watch", "--no-self-update"]),
    );
}

#[test]
fn changes_outside_cli_do_not_drain() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    land(&e, "docs/x.md", "new\n");
    assert_never_updates(&e, watch_from(&e, &from).args(["watch"]));
}

#[test]
fn an_ns_with_no_build_commit_or_one_the_repo_lacks_never_updates() {
    let off = "ns watch: self-update off: this ns has no build commit";
    let missing = "0123456789abcdef0123456789abcdef01234567";
    for (from, says_off) in [("unknown", true), ("", true), (missing, false)] {
        let e = Env::new();
        nightshift_repo(&e);
        land(&e, "cli/x", "new\n");
        let err = assert_never_updates(&e, watch_from(&e, from).args(["watch"]));
        assert_eq!(err.contains(off), says_off, "{from:?}: {err}");
    }
}

#[test]
fn a_unit_paused_while_draining_resumes_and_finishes_before_the_update() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    two_ready(&e);
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue(&format!("build.{A}"), &["limit", "pass"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(watch_from(&e, &from).args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    land(&e, "cli/x", "new\n");
    release(&go_b);
    w.line("draining");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        by_issue(&units(&v)),
        [(2, "done".into()), (2, "paused".into()), (3, "done".into())],
        "{v}"
    );
    let done_2 = event_at(&e, |ev| {
        ev["event"] == "worker_end" && ev["issue"] == 2 && ev["outcome"] == "done"
    });
    let updated = event_at(&e, |ev| ev["event"] == "self_update");
    assert!(done_2 < updated, "{:?}", run_events(&e));
}

#[test]
fn a_stop_while_draining_ends_the_night_without_an_update() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    // Unit 3 spends 2.5 and unit 2 reaches the budget at its review.
    e.factory("[limits]\nbudget_usd = 4.0\n");
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(watch_from(&e, &from).args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    land(&e, "cli/x", "new\n");
    release(&go_b);
    w.line("draining");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(v["stopped"], "budget", "{v}");
    assert_eq!(cargo_calls(&e), 0, "{err}");
    assert!(events_named(&e, "self_update").is_empty());
}

#[test]
fn the_harness_breaker_and_finished_issues_survive_the_hand_off() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    for n in [2, 3, 4] {
        e.ready(n, &format!("Fix {n}"), &["type:fix"], "");
    }
    // Each unit's triage phase fails instantly on both attempts.
    e.queue("triage", &["crash"; 4]);
    let (in_2, go_2) = hold_phase(&e, "2-fix-2", "triage");
    let w = Watching::start(watch_from(&e, &from).args(["watch"]));
    w.entered(&in_2);
    land(&e, "cli/x", "new\n");
    release(&go_2);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(events_named(&e, "self_update").len(), 1, "{err}");
    // Unit 2's failure counts toward the breaker after the hand-off, and unit 2, back to ready,
    // is not taken again.
    assert_eq!(v["stopped"], "harness failing", "{v}");
    assert_eq!(
        by_issue(&units(&v)),
        each(&[2, 3], "harness_failing"),
        "{v}"
    );
}

#[test]
fn the_triage_pass_record_survives_the_hand_off() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage", &["pass:script"]);
    let (in_5, go_5) = hold_phase(&e, "5-a", "build");
    let w = Watching::start(watch_from(&e, &from).args(["watch"]));
    w.entered(&in_5);
    land(&e, "cli/x", "new\n");
    release(&go_5);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(events_named(&e, "self_update").len(), 1, "{err}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[5], "done"), "{v}");
    assert_eq!(e.triage_only_runs(), 1);
}
