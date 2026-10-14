//! End-to-end tests for `ns run` and `ns watch` with a fake harness (`claude`) and a fake `gh`.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

#[path = "../common/mod.rs"]
mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::{copy_exe, exits, write_exe, Group, Ns};
use serde_json::{json, Value};
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
        Env::with_repo("main", |root| {
            fs::write(root.join("README"), "hi\n").unwrap();
        })
    }

    /// An env whose repo holds what `fill` writes and a demo skill, committed on `branch`,
    /// which is also `origin`'s default branch.
    fn with_repo(branch: &str, fill: impl FnOnce(&Path)) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("myrepo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", branch]);
        fill(&root);
        // A repo that pins no skills is refused, so every unit has one to pin.
        fs::create_dir_all(root.join("skills/ns-demo")).unwrap();
        fs::write(root.join("skills/ns-demo/SKILL.md"), "demo\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let bin = base.join("bin");
        fs::create_dir(&bin).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/factory");
        for f in ["claude", "gh", "glab"] {
            copy_exe(&fixtures.join(f), &bin.join(f));
        }
        let home = base.join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/.credentials.json"), "{}").unwrap();
        let ctrl = base.join("ctrl");
        let ghd = base.join("ghd");
        fs::create_dir(base.join("systmp")).unwrap();
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
        e.with_remote(branch);
        e
    }

    /// Add a bare `origin` (`base/remote.git`) with `branch` pushed and origin/HEAD set to it.
    fn with_remote(&self, branch: &str) {
        let remote = self.base.join("remote.git");
        git(
            &self.base,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                branch,
                remote.to_str().unwrap(),
            ],
        );
        git(
            &self.root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&self.root, &["push", "-q", "origin", branch]);
        git(&self.root, &["remote", "set-head", "origin", branch]);
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

    /// An open GitLab issue by member `author`, listed by the fake glab, ready unless `labels`
    /// says otherwise.
    fn gl_issue(&self, n: u64, labels: &[&str], body: &str, author: u64) {
        let line = serde_json::json!({
            "iid": n,
            "title": format!("Issue {n}"),
            "labels": labels,
            "description": body,
            "author": {"id": author, "username": format!("u{author}")},
        });
        let p = self.ghd.join("gl-issues.jsonl");
        let mut text = fs::read_to_string(&p).unwrap_or_default();
        text.push_str(&format!("{line}\n"));
        fs::write(&p, text).unwrap();
        let lines: String = labels.iter().map(|l| format!("{l}\n")).collect();
        fs::write(self.ghd.join(format!("labels-{n}")), lines).unwrap();
    }

    /// GitLab project members as `(id, access level)`, plus any `extra` member rows.
    fn gl_members(&self, members: &[(u64, u64)], extra: &[Value]) {
        let mut v: Vec<Value> = members
            .iter()
            .map(|(id, level)| serde_json::json!({"id": id, "username": format!("u{id}"), "access_level": level}))
            .collect();
        v.extend(extra.iter().cloned());
        self.gh_file("gl-members.json", &Value::from(v).to_string());
    }

    fn glab_calls(&self) -> String {
        fs::read_to_string(self.ghd.join("glab-calls")).unwrap_or_default()
    }

    fn queue(&self, phase: &str, actions: &[&str]) {
        fs::write(self.ctrl.join(phase), actions.join("\n") + "\n").unwrap();
    }

    fn ctl(&self, name: &str, text: &str) {
        fs::write(self.ctrl.join(name), text).unwrap();
    }

    fn gh_file(&self, name: &str, text: &str) {
        let tmp = self.ghd.join(format!("{name}.tmp"));
        fs::write(&tmp, text).unwrap();
        fs::rename(&tmp, self.ghd.join(name)).unwrap();
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
            .env("TMPDIR", self.base.join("systmp"))
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
            .env_remove("GH_TOKEN")
            .env_remove("GH_HOST");
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
        write_exe(&p, format!("#!/bin/sh\n{body}\n"));
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

// The tests, by feature. They share this file's namespace, so a helper defined in one part is
// visible in every other.
include!("run_phases_and_skills.rs");
include!("run_rebases_and_gates.rs");
include!("run_ratchets_and_worktrees.rs");
include!("run_locks_and_merge_order.rs");
include!("run_setup_signals.rs");
include!("run_runners_and_github_checks.rs");
include!("merge_conflicts_and_validate.rs");
include!("validate_owner_warning.rs");
include!("watch_fix_main.rs");
include!("watch_nights_and_splits.rs");
include!("forge_tokens.rs");
include!("forge_access_and_triage.rs");
include!("gitlab_guards_and_cleanup.rs");
include!("triage_pass_and_memory_cap.rs");
include!("run_signals_and_cleanup.rs");
include!("cleanup_and_cpus.rs");
include!("cpus_and_self_update.rs");
include!("self_update_and_drains.rs");
include!("skills_sync_and_merge_gate.rs");
include!("merge_gate_and_labels.rs");
