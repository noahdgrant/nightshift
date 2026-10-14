//! End-to-end tests for `ns run` and `ns watch` with a fake harness (`claude`) and a fake `gh`.

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

#[test]
fn run_by_issue_without_a_unit_refuses_when_the_issue_is_unreadable() {
    let e = Env::new();
    e.ns()
        .args(["run", "--issue", "8"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "cannot read issue #8 to name the unit",
        ));
    assert!(e.calls().is_empty());
}

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
    copy_exe(&e.bin.join("claude"), &agent);
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

/// `ns run` on a codex harness with every OpenAI API variable set; the variables codex saw.
fn codex_phases_saw(factory: &str) -> String {
    let e = Env::new();
    let codex = e.bin.join("codex");
    copy_exe(&e.bin.join("claude"), &codex);
    e.config(&format!(
        "[harness.codex]\ncommand = [{c:?}]\ncommand_write = [{c:?}]\n",
        c = codex.to_str().unwrap()
    ));
    e.factory(&format!("[defaults]\nharness = \"codex\"\n{factory}"));
    e.ns()
        .env("OPENAI_API_KEY", "k")
        .env("OPENAI_BASE_URL", "u")
        .env("CODEX_API_KEY", "c")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(
        e.ctrl.join("leak").exists(),
        "the codex scrub removed ANTHROPIC_API_KEY"
    );
    fs::read_to_string(e.ctrl.join("openai")).unwrap_or_default()
}

#[test]
fn a_codex_phase_sees_the_snapshot_through_agents_skills_but_gets_no_claude_settings() {
    let e = Env::new();
    let codex = e.bin.join("codex");
    copy_exe(&e.bin.join("claude"), &codex);
    e.config(&format!(
        "[harness.codex]\ncommand = [{c:?}]\ncommand_write = [{c:?}]\n",
        c = codex.to_str().unwrap()
    ));
    e.factory("[defaults]\nharness = \"codex\"\n");
    e.run(&["run", "--issue", "7"], 0);
    let args = fs::read_to_string(e.ctrl.join("args")).unwrap();
    for flag in ["--settings", "--setting-sources", "--add-dir"] {
        assert!(!args.split_whitespace().any(|a| a == flag), "{args}");
    }
    let wt = e.worktree(UNIT);
    for link in [".claude/skills", ".agents/skills"] {
        let target = fs::read_link(wt.join(link)).unwrap();
        assert!(
            target.ends_with(format!(".ns/{UNIT}/skills")),
            "{link}: {target:?}"
        );
        assert!(wt.join(link).join("ns-demo").is_dir(), "{link}");
    }
}

#[test]
fn codex_phases_on_a_subscription_run_without_openai_api_keys() {
    assert_eq!(codex_phases_saw(""), "");
}

#[test]
fn codex_phases_under_api_billing_keep_openai_api_keys() {
    let seen = codex_phases_saw("billing = \"api\"\n");
    assert_eq!(
        seen.lines().collect::<Vec<_>>(),
        ["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"].repeat(5),
        "{seen}"
    );
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

/// Run the `PreToolUse` hook from a claude phase's `--settings` on one Bash command; the
/// reason it denied the command, or `None` when it let it through.
fn hook_denies(settings: &Value, command: &str) -> Option<String> {
    let input = serde_json::json!({"tool_name": "Bash", "tool_input": {"command": command}});
    hook_says(settings, &input.to_string())
}

/// The deny reason the `PreToolUse` hook prints for hook input `input`, if any.
fn hook_says(settings: &Value, input: &str) -> Option<String> {
    let hook = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let out = common::command("sh")
        .args(["-c", hook])
        .write_stdin(input)
        .output();
    assert!(out.status.success(), "{input}: {out:?}");
    if out.stdout.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let o = &v["hookSpecificOutput"];
    assert_eq!(o["hookEventName"], "PreToolUse");
    assert_eq!(o["permissionDecision"], "deny");
    Some(o["permissionDecisionReason"].as_str().unwrap().to_string())
}

/// Run unit 7 through every phase and return the last phase's argv and the settings file its
/// `--settings` names.
fn phase_settings(e: &Env) -> (Vec<String>, Value) {
    e.run(&["run", "--issue", "7"], 0);
    let args = fs::read_to_string(e.ctrl.join("args")).unwrap();
    let argv: Vec<String> = args.split_whitespace().map(String::from).collect();
    let i = argv
        .iter()
        .position(|a| a == "--settings")
        .expect("--settings");
    let text = fs::read_to_string(&argv[i + 1]).unwrap();
    (argv, serde_json::from_str(&text).unwrap())
}

#[test]
fn a_claude_phase_hook_denies_every_git_hook_bypass() {
    let e = Env::new();
    let (_, settings) = phase_settings(&e);
    assert_eq!(
        settings["hooks"]["PreToolUse"][0]["matcher"],
        "Bash|Edit|Write|MultiEdit|NotebookEdit"
    );
    for bypass in [
        "git commit --no-verify -m x",
        "git commit -n -m x",
        "git push --no-verify",
        "git merge --no-verify main",
        "git rebase --no-verify origin/main",
        "git -c core.hooksPath=/dev/null commit -m x",
        "git config core.hooksPath /tmp/none",
        "rm .githooks/pre-commit",
        "echo 'exit 0' > .githooks/pre-push",
        "HUSKY=0 git commit -m x",
        "SKIP=ci git commit -m x",
    ] {
        let why = hook_denies(&settings, bypass).unwrap_or_else(|| panic!("allowed: {bypass}"));
        assert!(why.contains("skills/ns-contract"), "{bypass}: {why}");
        assert!(why.contains("Git hooks"), "{bypass}: {why}");
    }
    for ok in [
        "git commit -m x",
        "git push origin ns/x",
        "echo 'git commit --no-verify'",
    ] {
        assert_eq!(hook_denies(&settings, ok), None, "{ok}");
    }
    assert_eq!(hook_says(&settings, "not json"), None);
}

#[test]
fn claude_phases_read_only_project_settings_and_the_factory_settings_file() {
    let e = Env::new();
    fs::write(
        e.home.join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(git remote set-url:*)"]}}"#,
    )
    .unwrap();
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    let file = e
        .root
        .join(".git/ns/units")
        .join(UNIT)
        .join("phase-settings.json");
    let skills = e.worktree(UNIT).join(".ns").join(UNIT).join("skills");
    let dry: Vec<&str> = d["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let tail = [
        "--setting-sources",
        "project,local",
        "--settings",
        file.to_str().unwrap(),
        "--add-dir",
        skills.to_str().unwrap(),
    ];
    assert!(dry.windows(6).any(|w| w == tail), "{dry:?}");

    let (argv, settings) = phase_settings(&e);
    assert!(argv.windows(6).any(|w| w == tail), "{argv:?}");
    let deny: Vec<&str> = settings["permissions"]["deny"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    for rule in [
        "Bash(sudo:*)",
        "Bash(terraform apply:*)",
        "Read(//**/.env)",
        "Read(//**/secrets/**)",
    ] {
        assert!(deny.contains(&rule), "{rule}: {deny:?}");
    }
    let sources: Vec<&String> = argv
        .iter()
        .enumerate()
        .filter(|(_, a)| a.starts_with("--setting-sources"))
        .map(|(i, _)| &argv[i + 1])
        .collect();
    assert_eq!(sources, ["project,local"], "{argv:?}");
}

/// Commit `files` (path, text) in the main checkout and push them to `origin/main`.
fn commit_files(e: &Env, files: &[(&str, &str)], msg: &str) {
    for (path, text) in files {
        let p = e.root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    git(&e.root, &["add", "-A"]);
    git(&e.root, &["commit", "-q", "-m", msg]);
    git(&e.root, &["push", "-q", "origin", "main"]);
}

#[test]
fn phases_run_the_base_commits_skills_whatever_the_worktree_or_main_hold_later() {
    let e = Env::new();
    commit_files(
        &e,
        &[
            ("skills/ns-x/SKILL.md", "v1\n"),
            ("skills/ns-x/references/r.md", "ref\n"),
            (".agents/skills/verify-x/SKILL.md", "verify\n"),
        ],
        "skills v1",
    );
    e.run(&["worktree", "new", UNIT], 0);
    let wt = e.worktree(UNIT);
    fs::write(wt.join("skills/ns-x/SKILL.md"), "worktree edit\n").unwrap();
    git(&wt, &["commit", "-qam", "edit a skill in the unit"]);
    commit_files(&e, &[("skills/ns-x/SKILL.md", "v2\n")], "skills v2");
    e.run(&["run", "--issue", "7"], 0);

    let seen = |p: &str| fs::read_to_string(wt.join(".claude/skills").join(p)).unwrap();
    assert_eq!(seen("ns-x/SKILL.md"), "v1\n");
    assert_eq!(seen("ns-x/references/r.md"), "ref\n");
    assert_eq!(seen("verify-x/SKILL.md"), "verify\n");
    let snap = wt.join(".ns").join(UNIT).join("skills/ns-x/SKILL.md");
    assert!(!fs::symlink_metadata(&snap).unwrap().is_symlink());
    // A resume keeps the snapshot it wrote first, even once the unit has merged main's v2.
    git(
        &wt,
        &["merge", "-q", "--no-edit", "origin/main", "-X", "ours"],
    );
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(seen("ns-x/SKILL.md"), "v1\n");
    // The repo tracks .agents/skills, so the worktree keeps its own copy there.
    assert!(!fs::symlink_metadata(wt.join(".agents/skills"))
        .unwrap()
        .is_symlink());
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
}

#[test]
fn a_skill_in_both_skills_and_agents_skills_pins_the_agents_copy() {
    let e = Env::new();
    commit_files(
        &e,
        &[
            ("skills/ns-x/SKILL.md", "from skills\n"),
            (".agents/skills/ns-x/SKILL.md", "from agents\n"),
        ],
        "clashing skill",
    );
    e.run(&["run", "--issue", "7"], 0);
    let snap = e.worktree(UNIT).join(".ns").join(UNIT).join("skills");
    assert_eq!(
        fs::read_to_string(snap.join("ns-x/SKILL.md")).unwrap(),
        "from agents\n"
    );
    assert!(!snap.with_extension("tmp").exists());
}

#[test]
fn the_snapshot_links_are_excluded_from_git() {
    let e = Env::new();
    e.run(&["worktree", "new", UNIT], 0);
    let wt = e.worktree(UNIT);
    let leftover = wt.join(".ns").join(UNIT).join("skills.tmp/ns-junk");
    fs::create_dir_all(&leftover).unwrap();
    e.run(&["run", "--issue", "7"], 0);
    assert!(!wt.join(".ns").join(UNIT).join("skills/ns-junk").exists());
    for link in [".claude/skills", ".agents/skills"] {
        let target = fs::read_link(wt.join(link)).unwrap();
        assert_eq!(
            wt.join(link)
                .parent()
                .unwrap()
                .join(&target)
                .canonicalize()
                .unwrap(),
            wt.join(".ns")
                .join(UNIT)
                .join("skills")
                .canonicalize()
                .unwrap(),
            "{link}"
        );
    }
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
}

/// Install an `ns-y` skill under the fake home's `~/.claude/skills`, as a symlink into a
/// checkout, and return the checkout's skill dir.
fn install_skill_y(e: &Env) -> PathBuf {
    let src = e.base.join("checkout/skills/ns-y");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), "installed\n").unwrap();
    fs::create_dir_all(e.home.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink(&src, e.home.join(".claude/skills/ns-y")).unwrap();
    src
}

/// Remove the repo's `skills/` from `main` and `origin/main`.
fn drop_repo_skills(e: &Env) {
    git(&e.root, &["rm", "-rq", "skills"]);
    git(&e.root, &["commit", "-q", "-m", "drop skills"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
}

fn run_failing(e: &Env, args: &[&str]) -> String {
    let out = e.ns().args(args).output();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn without_skills_in_the_repo_the_snapshot_copies_the_installed_ns_skills() {
    let e = Env::new();
    drop_repo_skills(&e);
    let src = install_skill_y(&e);
    e.run(&["run", "--issue", "7"], 0);
    fs::write(src.join("SKILL.md"), "changed later\n").unwrap();
    let snap = e.worktree(UNIT).join(".ns").join(UNIT).join("skills/ns-y");
    assert!(!fs::symlink_metadata(&snap).unwrap().is_symlink());
    assert_eq!(
        fs::read_to_string(snap.join("SKILL.md")).unwrap(),
        "installed\n"
    );
    let pinned = events_named(&e, "skills_pinned");
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0]["source"], "installed");
}

#[test]
fn a_skills_tree_without_ns_skills_falls_back_to_the_installed_ones() {
    let e = Env::new();
    drop_repo_skills(&e);
    commit_files(
        &e,
        &[("skills/other/SKILL.md", "not ours\n")],
        "other skills",
    );
    install_skill_y(&e);
    e.run(&["run", "--issue", "7"], 0);
    let skills = e.worktree(UNIT).join(".ns").join(UNIT).join("skills");
    assert_eq!(
        fs::read_to_string(skills.join("ns-y/SKILL.md")).unwrap(),
        "installed\n"
    );
    assert_eq!(events_named(&e, "skills_pinned")[0]["source"], "installed");
}

#[test]
fn a_repo_with_no_ns_skills_to_pin_is_refused_and_leaves_no_snapshot() {
    let e = Env::new();
    drop_repo_skills(&e);
    let err = run_failing(&e, &["run", "--issue", "7"]);
    assert!(err.contains("no ns-* skills to pin"), "{err}");
    let unit = e.worktree(UNIT).join(".ns").join(UNIT);
    assert!(!unit.join("skills").exists());
    assert!(!unit.join("skills.tmp").exists());
    assert!(e.calls().is_empty(), "{:?}", e.calls());
}

#[test]
fn an_empty_snapshot_from_an_older_run_is_rebuilt() {
    let e = Env::new();
    e.run(&["worktree", "new", UNIT], 0);
    let skills = e.worktree(UNIT).join(".ns").join(UNIT).join("skills");
    fs::create_dir_all(&skills).unwrap();
    e.run(&["run", "--issue", "7"], 0);
    assert!(skills.join("ns-demo/SKILL.md").is_file());
}

#[test]
fn an_unresolvable_base_errors_and_leaves_no_snapshot() {
    let e = Env::new();
    e.run(&["worktree", "new", UNIT], 0);
    let err = run_failing(&e, &["run", "--issue", "7", "--base", "no-such-base"]);
    assert!(err.contains("no-such-base"), "{err}");
    assert!(!e
        .worktree(UNIT)
        .join(".ns")
        .join(UNIT)
        .join("skills")
        .exists());
    assert!(!e
        .worktree(UNIT)
        .join(".ns")
        .join(UNIT)
        .join("skills.tmp")
        .exists());
}

#[test]
fn a_repo_with_skills_but_no_agents_skills_pins_both_links() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    let wt = e.worktree(UNIT);
    assert!(!wt.join("../myrepo/.agents/skills").exists());
    for link in [".claude/skills", ".agents/skills"] {
        assert!(
            wt.join(link).join("ns-demo/SKILL.md").is_file(),
            "{link} shows the snapshot"
        );
    }
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
}

#[test]
fn the_pin_logs_its_sha_source_and_skill_count_once_and_not_on_a_resume() {
    let e = Env::new();
    commit_files(
        &e,
        &[(".agents/skills/verify-x/SKILL.md", "v\n")],
        "local skill",
    );
    e.run(&["run", "--issue", "7"], 0);
    let pinned = events_named(&e, "skills_pinned");
    assert_eq!(pinned.len(), 1, "{pinned:?}");
    assert_eq!(pinned[0]["sha"], git(&e.root, &["rev-parse", "main"]));
    assert_eq!(pinned[0]["source"], "base");
    assert_eq!(pinned[0]["skills"], 2);
    let unpinned = events_named(&e, "skills_unpinned");
    assert_eq!(unpinned.len(), 1, "{unpinned:?}");
    assert_eq!(unpinned[0]["path"], ".agents/skills");
    assert!(unpinned[0]["why"].as_str().unwrap().contains("tracked"));
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(events_named(&e, "skills_pinned").len(), 1);
}

/// `ns run` stops with a usage error (2) before any phase.
fn run_unpinnable(e: &Env) -> String {
    let out = e.ns().args(["run", "--issue", "7"]).output();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_tracked_claude_skills_dir_or_symlink_stops_the_run() {
    let e = Env::new();
    commit_files(
        &e,
        &[(".claude/skills/own/SKILL.md", "own\n")],
        "tracked dir",
    );
    let err = run_unpinnable(&e);
    assert!(err.contains(".claude/skills"), "{err}");
    assert!(err.contains("real directory"), "{err}");
    assert!(e.calls().is_empty());

    let e = Env::new();
    fs::create_dir_all(e.root.join(".claude")).unwrap();
    std::os::unix::fs::symlink("../elsewhere", e.root.join(".claude/skills")).unwrap();
    git(&e.root, &["add", "-f", ".claude/skills"]);
    git(&e.root, &["commit", "-q", "-m", "tracked symlink"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let err = run_unpinnable(&e);
    assert!(err.contains("not the unit's snapshot"), "{err}");
    assert_eq!(
        fs::read_link(e.worktree(UNIT).join(".claude/skills")).unwrap(),
        Path::new("../elsewhere")
    );
}

#[test]
fn a_tracked_agents_skills_symlink_is_kept_and_reported() {
    let e = Env::new();
    fs::create_dir_all(e.root.join(".agents")).unwrap();
    std::os::unix::fs::symlink("../elsewhere", e.root.join(".agents/skills")).unwrap();
    git(&e.root, &["add", "-f", ".agents/skills"]);
    git(&e.root, &["commit", "-q", "-m", "tracked symlink"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("warning: .agents/skills is a symlink that is not the unit's snapshot"));
    let wt = e.worktree(UNIT);
    assert_eq!(
        fs::read_link(wt.join(".agents/skills")).unwrap(),
        Path::new("../elsewhere")
    );
    let ev = events_named(&e, "skills_unpinned");
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(ev[0]["path"], ".agents/skills");
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
}

#[test]
fn a_stale_snapshot_link_is_pointed_at_the_unit_again() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    let wt = e.worktree(UNIT);
    fs::remove_file(wt.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink("../.ns/other/skills", wt.join(".claude/skills")).unwrap();
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(
        fs::read_link(wt.join(".claude/skills")).unwrap(),
        Path::new("../.ns").join(UNIT).join("skills")
    );
    assert!(events_named(&e, "skills_unpinned").is_empty());
}

#[test]
fn the_phase_settings_file_is_written_again_on_a_later_run_and_before_each_phase() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    let file = e
        .root
        .join(".git/ns/units")
        .join(UNIT)
        .join("phase-settings.json");
    assert!(!e
        .worktree(UNIT)
        .join(".ns")
        .join(UNIT)
        .join("phase-settings.json")
        .exists());
    let good = fs::read_to_string(&file).unwrap();
    fs::write(&file, "{}").unwrap();
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(fs::read_to_string(&file).unwrap(), good);

    // Build deletes the file; verify and review must still find it whole.
    let e = Env::new();
    let seen = e.ctrl.join("seen");
    let check =
        format!("cat \"$NS_PHASE_SETTINGS\" >> {seen:?} 2>/dev/null || echo MISSING >> {seen:?}\n");
    e.ctl("build.sh", "rm -f \"$NS_PHASE_SETTINGS\"\n");
    e.ctl("verify.sh", &check);
    e.ctl("review.sh", &check);
    e.queue("build", &["pass:script"]);
    e.queue("verify", &["pass:script"]);
    e.queue("review", &["pass:script"]);
    e.run(&["run", "--issue", "7"], 0);
    let seen = fs::read_to_string(seen).unwrap();
    assert!(!seen.contains("MISSING"), "{seen}");
    assert_eq!(seen.matches("PreToolUse").count(), 2, "{seen}");
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

/// Fail, naming the tool, rather than let a missing tool time out a phase or pass as a clean pack.
fn need_python_tools() {
    for tool in ["pytest", "ruff", "ast-grep"] {
        let found = StdCommand::new(tool).arg("--version").output();
        assert!(
            found.is_ok_and(|o| o.status.success()),
            "the py-inventory tests need `{tool}` on PATH; install it with scripts/ci-tools.sh install {tool}"
        );
    }
}

/// A copy of the py-inventory eval fixture on default branch `branch`, with the Python rule pack
/// installed as `ns-setup` installs it, the fixture's existing violations baselined, and a
/// `ci-local` row that runs its tests.
fn py_inventory(branch: &str) -> Env {
    need_python_tools();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    Env::with_repo(branch, |root| {
        common::copy_dir(&repo.join("evals/fixtures/py-inventory"), root);
        let pack = root.join(".nightshift/rules/python");
        fs::create_dir_all(&pack).unwrap();
        for f in ["ruff.toml", "sgconfig.yml", "file-size.sh"] {
            common::copy_exe(&repo.join("rules/python").join(f), &pack.join(f));
        }
        for d in ["rules", "utils"] {
            common::copy_dir(&repo.join("rules/python").join(d), &pack.join(d));
        }
        let allowed = pack.join("utils/allowed-package.yml");
        let text = fs::read_to_string(&allowed).unwrap();
        fs::write(
            &allowed,
            text.replace("(?:)", "(?:inventory|pytest|conftest)"),
        )
        .unwrap();
        let row = fs::read_to_string(repo.join("rules/python/stack-row.md")).unwrap();
        let row = row
            .replace("{pack}", ".nightshift/rules/python")
            .replace("{target}", ".");
        let stack = root.join("docs/agents/stack.md");
        let text = fs::read_to_string(&stack).unwrap();
        let typecheck = "| typecheck | none | |\n";
        assert!(text.contains(typecheck), "{text}");
        let rows = format!("{typecheck}{row}| ci-local | `pytest -q` | 1 s |\n");
        fs::write(&stack, text.replace(typecheck, &rows)).unwrap();
        common::ns()
            .current_dir(root)
            .args(["rules", "baseline", "--allow-growth"])
            .assert()
            .success();
        let baseline = fs::read_to_string(root.join(".nightshift/rules-baseline.json")).unwrap();
        let v: Value = serde_json::from_str(&baseline).unwrap();
        assert!(
            v["violations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["message"]
                    .as_str()
                    .unwrap()
                    .contains("[suppression-reason]")),
            "the baseline holds more than the fixture's unexplained suppressions: {baseline}"
        );
    })
}

/// The log of every gate run, for a failure message.
fn gate_logs(e: &Env) -> String {
    gate_events(e)
        .iter()
        .filter_map(|g| g["log"].as_str())
        .map(|l| format!("--- {l}\n{}\n", fs::read_to_string(l).unwrap_or_default()))
        .collect()
}

/// Every artifact the unit's phases wrote, for a failure message.
fn artifacts(e: &Env, unit: &str) -> String {
    let dir = e.worktree(unit).join(".ns").join(unit);
    let mut out = String::new();
    for f in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let text = fs::read_to_string(f.path()).unwrap_or_default();
        out.push_str(&format!("--- {}\n{text}\n", f.path().display()));
    }
    out
}

/// `ns run` takes one unit of py-inventory, on default branch `branch`, to done: the gate after
/// build runs the fixture's own `ci-local` row and the Python rule pack, and no step makes a
/// `main` branch.
fn py_inventory_reaches_done(branch: &str) {
    let e = py_inventory(branch);
    e.queue("build", &["pass:commit"]);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    assert!(
        out.status.success() && v["outcome"] == "done",
        "{}\nstdout={v}\nstderr={}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        artifacts(&e, UNIT) + &gate_logs(&e)
    );
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    let g = gate_events(&e);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0]["green"], true, "{g:?}");
    let cmd = g[0]["command"].as_str().unwrap();
    assert!(
        cmd.starts_with("(pytest -q) && ") && cmd.ends_with(" rules check"),
        "{cmd}"
    );
    let log = fs::read_to_string(g[0]["log"].as_str().unwrap()).unwrap();
    for proof in [
        " passed in ",
        "ns rules: ruff check --config .nightshift/rules/python/ruff.toml",
        "ns rules: ast-grep scan",
        "ns rules: .nightshift/rules/python/file-size.sh .",
    ] {
        assert!(log.contains(proof), "no {proof:?} in the gate log:\n{log}");
    }
    let refs = git(&e.root, &["for-each-ref", "--format=%(refname)"]);
    assert!(!refs.lines().any(|r| r.ends_with("/main")), "{refs}");
}

#[test]
fn py_inventory_on_branch_trunk_reaches_done() {
    py_inventory_reaches_done("trunk");
}

#[test]
fn py_inventory_on_branch_develop_reaches_done() {
    py_inventory_reaches_done("develop");
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

/// Shell that adds a commit to `origin`'s main, as another actor merging a PR would.
fn move_main_sh(e: &Env) -> String {
    let r = e.base.join("remote.git");
    format!(
        "r={r:?}; c=$(git --git-dir=\"$r\" commit-tree \"$(git --git-dir=\"$r\" rev-parse main^{{tree}})\" -p main -m other) && git --git-dir=\"$r\" update-ref refs/heads/main \"$c\"",
        r = r.display()
    )
}

fn remote_main(e: &Env) -> String {
    git(&e.base.join("remote.git"), &["rev-parse", "main"])
}

fn rebase_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "rebase_after_base_moved")
        .collect()
}

/// Build 1 commits and another actor moves main; verify gets five attempts, so only build's cap
/// can end the unit.
fn base_moves_during_build(e: &Env, extra: &str) {
    e.factory(&format!("[phases.verify]\nmax_attempts = 5\n{extra}"));
    e.ctl(
        "build.sh",
        &format!(
            "echo b >> work.txt && git add work.txt && git -c user.name=f -c user.email=f@f commit -qm b && {}",
            move_main_sh(e)
        ),
    );
}

#[test]
fn a_verify_no_go_for_base_movement_rebases_onto_the_newest_main_at_no_attempts_cost() {
    let e = Env::new();
    base_moves_during_build(&e, "");
    e.queue("build", &["pass:script", "pass:commit", "pass:commit"]);
    e.queue("verify", &["fail:base moved", "fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "verify", "build", "verify", "review", "ship"]
    );
    let main = remote_main(&e);
    let p = e.prompt(4, "build");
    assert!(p.contains("base moved"), "{p}");
    assert!(
        p.contains(&format!("rebase onto the newest origin/main ({main})")),
        "{p}"
    );
    assert!(!e.prompt(6, "build").contains("rebase onto the newest"));
    let wt = e.worktree(UNIT);
    assert_eq!(git(&wt, &["rev-parse", "origin/main"]), main, "fetched");
    let r = rebase_events(&e);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["attempt"], 2);
    assert_eq!(r[0]["onto"], main.as_str());
    assert_eq!(r[0]["cause"], "evidence.md failed");
}

#[test]
fn a_ship_no_go_for_base_movement_goes_to_build_at_no_attempts_cost() {
    let e = Env::new();
    base_moves_during_build(&e, "[phases.ship]\nmax_attempts = 5\n");
    e.queue("build", &["pass:script", "pass:commit", "pass:commit"]);
    e.queue("verify", &["pass", "fail", "pass"]);
    e.queue("ship", &["fail:base moved", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        &e.calls()[..6],
        ["triage", "build", "verify", "review", "ship", "build"]
    );
    let r = rebase_events(&e);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["cause"], "pr.md failed: base moved");
}

#[test]
fn the_ship_after_a_free_rebase_build_is_not_free() {
    let e = Env::new();
    base_moves_during_build(&e, "");
    e.queue("build", &["pass:script", "pass:commit"]);
    e.queue("verify", &["fail:base moved", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "verify", "review", "ship"]
    );
    assert!(!run_events(&e)
        .iter()
        .any(|ev| ev["event"] == "ship_after_base_moved"));
}

#[test]
fn a_free_rebase_build_that_fails_leaves_the_unit_stuck_at_the_normal_cap() {
    let e = Env::new();
    base_moves_during_build(&e, "");
    e.queue("build", &["pass:script", "fail", "pass:commit"]);
    e.queue("verify", &["fail:base moved"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["reason"], "build is out of attempts (2)", "{v}");
    assert_eq!(e.calls(), ["triage", "build", "verify", "build"]);
}

#[test]
fn a_review_no_go_for_base_movement_goes_to_build_at_no_attempts_cost() {
    let e = Env::new();
    base_moves_during_build(&e, "[phases.review]\nmax_attempts = 5\n");
    e.queue("build", &["pass:script", "pass:commit", "pass:commit"]);
    e.queue("verify", &["pass", "fail", "pass"]);
    e.queue("review", &["fail:base moved", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        &e.calls()[..6],
        ["triage", "build", "verify", "review", "build", "verify"]
    );
    let r = rebase_events(&e);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["cause"], "review.md failed");
}

#[test]
fn a_free_rebase_build_that_times_out_leaves_the_unit_stuck_at_the_normal_cap() {
    let e = Env::new();
    base_moves_during_build(&e, "");
    e.queue("build", &["pass:script", "blocked:sleep", "pass:commit"]);
    e.queue("verify", &["fail:base moved"]);
    let out = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "build=1500")
        .args(["run", "--issue", "7"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("build is out of attempts (2)"),
        "{v}"
    );
    assert_eq!(e.calls(), ["triage", "build", "verify", "build"]);
}

#[test]
fn a_free_rebase_build_that_writes_no_artifact_leaves_the_unit_stuck_at_the_normal_cap() {
    let e = Env::new();
    base_moves_during_build(&e, "");
    e.queue("build", &["pass:script", "none", "pass:commit"]);
    e.queue("verify", &["fail:base moved"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("build is out of attempts (2)"),
        "{v}"
    );
    assert_eq!(e.calls(), ["triage", "build", "verify", "build"]);
}

#[test]
fn a_free_rebase_that_gives_up_waiting_for_a_lock_still_rebases_on_the_retry() {
    let e = Env::new();
    let lock = e.base.join("locks/bench-1.lock");
    let log = e.root.join(".git/ns/runs.jsonl");
    e.factory("[phases.build]\nrunner = \"bench\"\n");
    e.runner("bench", "kind = \"bench\"\nlocks = [\"bench-1\"]\n");
    e.config(&format!(
        "[runners]\nlock_dir = {:?}\n",
        lock.parent().unwrap().to_str().unwrap()
    ));
    // After the first build lets go of the lock, a holder takes it and keeps it until the
    // run has logged its first give-up, so the rebase build's first wait ends at the timeout.
    e.ctl(
        "build.sh",
        &format!(
            "echo b >> work.txt && git add work.txt && git -c user.name=f -c user.email=f@f commit -qm b && {}; \
             (setsid timeout 30 flock -x {lock:?} sh -c 'until grep -q lock_wait_timeout {log:?}; do sleep 0.05; done' </dev/null >/dev/null 2>&1 &)",
            move_main_sh(&e)
        ),
    );
    e.queue("build", &["pass:script", "pass:commit", "pass:commit"]);
    e.queue("verify", &["fail:base moved", "pass"]);
    let out = e
        .ns()
        .env_remove("NS_NOW")
        .env("NS_PHASE_TIMEOUT_MS", "build=4000")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    let main = remote_main(&e);
    let p = e.prompt(4, "build");
    assert!(
        p.contains(&format!("rebase onto the newest origin/main ({main})")),
        "{p}"
    );
    assert_eq!(rebase_events(&e).len(), 1);
}

#[test]
fn a_red_gate_after_a_rebase_with_main_moved_since_rebases_once_more() {
    let e = Env::new();
    let n = e.ctrl.join("gates");
    let n = n.display();
    // Gate 2 (after the rebase) and gate 3 (after the extra rebase) go red and move main.
    base_moves_during_build(
        &e,
        &format!(
            "[phases.build]\ngate = \"n=$(($(cat {n} 2>/dev/null || echo 0)+1)); echo $n > {n}; case $n in 2|3) {m}; echo boom; exit 1;; esac\"\n",
            m = move_main_sh(&e).replace('"', "\\\"")
        ),
    );
    e.queue(
        "build",
        &["pass:script", "pass:commit", "pass:commit", "pass:commit"],
    );
    e.queue("verify", &["fail:base moved", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "build", "build", "verify", "review", "ship"]
    );
    let r = rebase_events(&e);
    assert_eq!(r.len(), 2, "one extra rebase, never a second: {r:?}");
    assert_eq!(r[1]["attempt"], 3);
    assert_eq!(
        r[1]["cause"],
        "the CI gate failed after a rebase and main moved since"
    );
    let p = e.prompt(5, "build");
    assert!(p.contains("boom"), "{p}");
    assert!(p.contains("rebase onto the newest origin/main"), "{p}");
    // The third red spent build's second attempt.
    assert!(!e.prompt(6, "build").contains("rebase onto the newest"));
}

#[test]
fn a_red_gate_after_a_rebase_with_main_still_spends_an_attempt() {
    let e = Env::new();
    let n = e.ctrl.join("gates");
    let n = n.display();
    base_moves_during_build(
        &e,
        &format!(
            "[phases.build]\ngate = \"n=$(($(cat {n} 2>/dev/null || echo 0)+1)); echo $n > {n}; [ $n -ne 2 ] || {{ echo boom; exit 1; }}\"\n"
        ),
    );
    e.queue("build", &["pass:script", "pass:commit", "pass:commit"]);
    e.queue("verify", &["fail:base moved", "fail"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["reason"], "build is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "build", "verify"]
    );
    assert_eq!(rebase_events(&e).len(), 1);
}

#[test]
fn a_build_that_fails_saying_base_moved_still_spends_an_attempt() {
    let e = Env::new();
    e.queue("build", &["fail:base moved", "fail:base moved"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["reason"], "build is out of attempts (2)", "{v}");
    assert_eq!(e.calls(), ["triage", "build", "build"]);
    assert!(rebase_events(&e).is_empty());
}

#[test]
fn without_an_origin_a_base_moved_no_go_spends_an_attempt() {
    let e = Env::new();
    git(&e.root, &["remote", "remove", "origin"]);
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("verify", &["fail:base moved", "fail"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert!(
        v["reason"].as_str().unwrap().contains("out of attempts"),
        "{v}"
    );
    assert!(rebase_events(&e).is_empty());
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

/// Each build phase records its `TMPDIR` and what was in it, and the modes of it and its parent,
/// then leaves a file there.
fn record_tmpdir(e: &Env) {
    e.ctl(
        "build.sh",
        r#"{ echo "$TMPDIR"; ls -A "$TMPDIR"; } >> "$FAKE_CTRL/tmpdir-$NS_UNIT"
stat -c %a "$TMPDIR" "$TMPDIR/.." >> "$FAKE_CTRL/mode-$NS_UNIT"
touch "$TMPDIR/scratch""#,
    );
}

/// The lines `record_tmpdir` wrote for `unit`.
fn phase_tmpdirs(e: &Env, unit: &str) -> Vec<String> {
    fs::read_to_string(e.ctrl.join(format!("tmpdir-{unit}")))
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

/// The directory of `unit`'s phase `TMPDIR`s, in the system temp dir `ns` inherited:
/// `ns-<repo>-<8 hex of sha256 of the git common dir>-<unit>`.
fn unit_tmp(e: &Env, unit: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(e.root.join(".git").as_os_str().as_encoded_bytes());
    let hash: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
    e.base
        .join("systmp")
        .join(format!("ns-myrepo-{hash}-{unit}"))
}

#[test]
fn each_phase_gets_its_own_tmpdir_and_a_passing_phase_leaves_none_behind() {
    let e = Env::new();
    e.issue(8, "Fix other", "OPEN");
    record_tmpdir(&e);
    for (n, unit) in [("7", UNIT), ("8", "8-fix-other")] {
        e.queue(&format!("build.{unit}"), &["pass:script"]);
        let v = e.run(&["run", "--issue", n], 0);
        assert_eq!(v["outcome"], "done", "{v}");
    }
    let dir = unit_tmp(&e, UNIT).join("build-1");
    assert_eq!(phase_tmpdirs(&e, UNIT), [dir.to_str().unwrap()]);
    let other = unit_tmp(&e, "8-fix-other").join("build-1");
    assert_eq!(phase_tmpdirs(&e, "8-fix-other"), [other.to_str().unwrap()]);
    for unit in [UNIT, "8-fix-other"] {
        let left: Vec<_> = fs::read_dir(unit_tmp(&e, unit)).unwrap().collect();
        assert!(left.is_empty(), "{unit}: {left:?}");
        let modes = fs::read_to_string(e.ctrl.join(format!("mode-{unit}"))).unwrap();
        assert_eq!(modes, "700\n700\n", "{unit}");
    }
    let worktrees = e.base.join("myrepo.worktrees");
    assert!(worktrees.join(UNIT).is_dir());
    for d in [&dir, &other] {
        assert!(!d.starts_with(&e.root), "{} is in the repo", d.display());
        assert!(
            !d.starts_with(&worktrees),
            "{} is in a worktree",
            d.display()
        );
    }
}

#[test]
fn a_phase_that_times_out_keeps_its_tmpdir() {
    let e = Env::new();
    record_tmpdir(&e);
    e.queue("build", &["pass:script+sleep", "pass:script"]);
    let v = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "build=1500")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&v).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    let tmp = unit_tmp(&e, UNIT);
    // `ns` makes the directory before it starts the phase, so the kill cannot beat it; what the
    // phase wrote there depends on how soon it started, so is not checked.
    assert!(tmp.join("build-1").is_dir());
    assert!(!tmp.join("build-2").exists());
}

#[test]
fn a_passing_phase_whose_tmpdir_will_not_go_is_logged_not_failed() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let e = Env::new();
    e.ctl(
        "build.sh",
        r#"mkdir "$TMPDIR/locked" && touch "$TMPDIR/locked/f" && chmod 500 "$TMPDIR/locked""#,
    );
    e.queue("build", &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    let dir = unit_tmp(&e, UNIT).join("build-1");
    fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(dir.join("locked/f").is_file());
    let ev = run_events(&e)
        .into_iter()
        .find(|ev| ev["event"] == "tmpdir_not_removed")
        .expect("a tmpdir_not_removed event");
    assert_eq!(ev["phase"], "build", "{ev}");
    assert_eq!(ev["path"], dir.to_str().unwrap(), "{ev}");
    assert!(ev["error"].as_str().is_some_and(|m| !m.is_empty()), "{ev}");
}

#[test]
fn a_phase_tmpdir_parent_another_user_could_use_stops_the_run_before_the_phase() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    let tmp = unit_tmp(&e, UNIT);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let calls = e.calls();
    let review = e.worktree(UNIT).join(format!(".ns/{UNIT}/review.md"));
    assert!(review.is_file());
    let kept = fs::read_dir(&tmp).unwrap().count();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o777)).unwrap();
    e.ns()
        .args(["run", "--issue", "7", "--from", "review"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(format!(
            "{} is not a directory only this user can use",
            tmp.display()
        )));
    assert_eq!(e.calls(), calls);
    assert_eq!(fs::read_dir(&tmp).unwrap().count(), kept);
    assert!(review.is_file(), "the refused phase's artifact stays put");
}

#[test]
fn a_failed_phase_keeps_its_tmpdir() {
    let e = Env::new();
    record_tmpdir(&e);
    e.queue("build", &["fail:script", "pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let tmp = unit_tmp(&e, UNIT);
    let dirs = [tmp.join("build-1"), tmp.join("build-2")];
    assert_eq!(
        phase_tmpdirs(&e, UNIT),
        dirs.iter().map(|d| d.to_str().unwrap()).collect::<Vec<_>>()
    );
    assert!(dirs[0].join("scratch").is_file());
    assert!(!dirs[1].exists());
}

#[test]
fn a_blocked_phase_keeps_its_tmpdir() {
    let e = Env::new();
    record_tmpdir(&e);
    e.queue("build", &["blocked:script"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let dir = unit_tmp(&e, UNIT).join("build-1");
    assert_eq!(phase_tmpdirs(&e, UNIT), [dir.to_str().unwrap()]);
    assert!(dir.join("scratch").is_file());
}

#[test]
fn a_rerun_keeps_the_tmpdirs_a_failed_run_kept() {
    let e = Env::new();
    record_tmpdir(&e);
    e.queue("build", &["fail:script", "fail:script"]);
    e.run(&["run", "--issue", "7"], 1);
    let tmp = unit_tmp(&e, UNIT);
    let kept = [tmp.join("build-1"), tmp.join("build-2")];
    for dir in &kept {
        assert!(dir.join("scratch").is_file(), "{}", dir.display());
    }
    e.queue("build", &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let dirs = ["build-1", "build-2", "build-3"].map(|d| tmp.join(d));
    assert_eq!(
        phase_tmpdirs(&e, UNIT),
        dirs.iter().map(|d| d.to_str().unwrap()).collect::<Vec<_>>()
    );
    for dir in &kept {
        assert!(dir.join("scratch").is_file(), "{}", dir.display());
    }
    assert!(!tmp.join("build-3").exists());
}

#[test]
fn a_tmpdir_that_cannot_be_created_stops_the_run_before_the_phase() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    let tmp = unit_tmp(&e, UNIT);
    fs::create_dir(&tmp).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o500)).unwrap();
    let out = e.ns().args(["run", "--issue", "7"]).assert().failure();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();
    out.stderr(predicates::str::contains(format!(
        "cannot create {}",
        tmp.join("triage-1").display()
    )));
    assert!(e.calls().is_empty());
}

/// The `GIT_SSH_COMMAND` the build phase saw, or `unset`, after `ns run` under `setup`.
fn phase_ssh_command(setup: impl FnOnce(&Env, &mut Ns)) -> String {
    let e = Env::new();
    e.ctl(
        "build.sh",
        r#"echo "${GIT_SSH_COMMAND-unset}" > "$FAKE_CTRL/ssh""#,
    );
    e.queue("build", &["pass:script"]);
    let mut ns = e.ns();
    ns.env_remove("GIT_SSH_COMMAND").env_remove("GIT_SSH");
    setup(&e, &mut ns);
    ok_pid(ns.args(["run", "--issue", "7"]));
    fs::read_to_string(e.ctrl.join("ssh"))
        .unwrap()
        .trim_end()
        .to_string()
}

#[test]
fn a_phase_pushes_with_ssh_keepalive_unless_the_user_chose_an_ssh_command() {
    let keepalive = "ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=40";
    assert_eq!(phase_ssh_command(|_, _| {}), keepalive);
    assert_eq!(
        phase_ssh_command(|_, ns| {
            ns.env("GIT_SSH_COMMAND", "ssh -i key");
        }),
        "ssh -i key"
    );
    assert_eq!(
        phase_ssh_command(|_, ns| {
            ns.env("GIT_SSH", "/usr/bin/ssh");
        }),
        "unset"
    );
    assert_eq!(
        phase_ssh_command(|e, _| {
            git(&e.root, &["config", "core.sshCommand", "ssh -i key"]);
        }),
        "unset"
    );
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
    assert!(g.iter().all(|g| g["where"] == "local_full"), "{g:?}");
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

/// `[gate] local = "build-only"` with `extra` before it, a build gate that appends `full` or
/// `fast` to `ci-runs`, a `ci-affected` row that appends the files it got to `affected` and runs
/// `affected.sh` when it exists, and a build script that commits `built.txt`. Returns the two logs.
fn build_only(e: &Env, extra: &str) -> (PathBuf, PathBuf) {
    let runs = e.ctrl.join("ci-runs");
    let affected = e.ctrl.join("affected");
    e.ctl(
        "ci.sh",
        &format!(
            "if [ \"$1\" = --fast ]; then echo fast; else echo full; fi >> {}\n",
            runs.display()
        ),
    );
    e.factory(&format!(
        "{extra}[gate]\nlocal = \"build-only\"\n[phases.build]\ngate = \"sh {}\"\n",
        e.ctrl.join("ci.sh").display()
    ));
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        format!(
            "| Task | Command |\n|---|---|\n| ci-affected | `echo \"[${{NS_GATE_FILES-UNSET}}]\" >> {}; test ! -f {c}/affected.sh || sh {c}/affected.sh` | 1 s |\n",
            affected.display(),
            c = e.ctrl.display()
        ),
    )
    .unwrap();
    git(&e.root, &["add", "."]);
    git(&e.root, &["commit", "-q", "-m", "stack"]);
    e.ctl(
        "build.sh",
        "date >> built.txt && git add built.txt && git -c user.name=f -c user.email=f@f commit -qm built",
    );
    (runs, affected)
}

fn gate_where(e: &Env) -> Vec<(Value, Value)> {
    gate_events(e)
        .iter()
        .map(|g| (g["phase"].clone(), g["where"].clone()))
        .collect()
}

#[test]
fn build_only_runs_one_full_gate_after_build_and_fast_ones_with_the_files_changed_since() {
    let e = Env::new();
    let (runs, affected) = build_only(&e, AUTO);
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:script"]);
    e.queue("review", &["pass:commit"]);
    main_moves(&e, "from-main", "x\n");
    e.gh_file("pr-12.merge", "BEHIND");
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(d["gate"], format!("sh {}", e.ctrl.join("ci.sh").display()));
    assert!(d["gate_rest"]
        .as_str()
        .unwrap()
        .contains("ci.sh --fast) && ("));
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\n");
    assert_eq!(
        gate_where(&e),
        [
            ("build".into(), "local_full".into()),
            ("review".into(), "local_fast".into())
        ]
    );
    assert_eq!(fs::read_to_string(&affected).unwrap(), "[work.txt]\n");
    assert!(e.gh_calls().contains("pr update-branch 12"));
}

#[test]
fn build_only_skips_a_fast_gate_where_a_gate_already_went_green_but_never_a_full_one() {
    let e = Env::new();
    let (runs, _) = build_only(&e, AUTO);
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\n");

    let e = Env::new();
    let (runs, _) = build_only(&e, AUTO);
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:script"]);
    e.queue("review", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\n");

    let e = Env::new();
    let (runs, _) = build_only(&e, "");
    e.queue("build", &["pass:script", "pass"]);
    e.queue("review", &["pass:commit", "pass"]);
    e.queue("verify", &["pass", "fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "verify", "build", "verify", "review", "ship"]
    );
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\nfull\n");
}

#[test]
fn build_only_without_a_full_gate_this_run_tests_the_files_changed_since_the_base() {
    let e = Env::new();
    let (runs, affected) = build_only(&e, "");
    e.queue("build", &["pass:script"]);
    e.queue("review", &["pass", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let v = e.run(&["run", "--issue", "7", "--from", "review"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\n");
    assert_eq!(
        fs::read_to_string(&affected).unwrap(),
        "[built.txt\nwork.txt]\n"
    );
}

#[test]
fn build_only_fast_gate_runs_everything_when_git_cannot_find_the_changed_files() {
    let e = Env::new();
    let (runs, affected) = build_only(&e, "");
    e.ctl(
        "review.sh",
        "git update-ref -d refs/heads/main && date >> work.txt && git add work.txt && git -c user.name=f -c user.email=f@f commit -qm fix",
    );
    e.queue("build", &["pass:script"]);
    e.queue("review", &["pass", "pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let v = e.run(&["run", "--issue", "7", "--from", "review"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\n");
    assert_eq!(fs::read_to_string(&affected).unwrap(), "[UNSET]\n");
}

#[test]
fn build_only_sends_a_red_fast_gate_back_to_build_and_the_rebuild_runs_the_full_one() {
    let e = Env::new();
    let (runs, _) = build_only(&e, "");
    let failing = e.ctrl.join("affected.sh");
    let commit = "git add work.txt && git -c user.name=f -c user.email=f@f commit -qm fix";
    e.ctl(
        "review.sh",
        &format!(
            "echo 'echo affected tests failed; exit 1' > {}; date >> work.txt && {commit}",
            failing.display()
        ),
    );
    e.ctl(
        "build.sh",
        &format!(
            "rm -f {}; date >> built.txt && git add built.txt && git -c user.name=f -c user.email=f@f commit -qm built",
            failing.display()
        ),
    );
    e.queue("build", &["pass:script", "pass:script"]);
    e.queue("review", &["pass:script", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(5, "build").contains("affected tests failed"));
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\nfull\n");
    let g: Vec<_> = gate_events(&e)
        .iter()
        .map(|g| (g["phase"].clone(), g["green"].clone(), g["where"].clone()))
        .collect();
    assert_eq!(
        g,
        [
            ("build".into(), true.into(), "local_full".into()),
            ("review".into(), false.into(), "local_fast".into()),
            ("build".into(), true.into(), "local_full".into())
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

/// A factory with one ratchet, `flash`, recorded at 100, whose command prints `ctrl/flash`.
fn ratchet_env(measured: &str) -> Env {
    let e = Env::new();
    e.factory(&format!(
        "[ratchets.flash]\ncommand = \"cat {}\"\nbetter = \"lower\"\ntolerance = \"5%\"\n",
        e.ctrl.join("flash").display()
    ));
    fs::write(
        e.root.join(".nightshift/ratchets.json"),
        r#"{"version": 1, "values": {"flash": 100}}"#,
    )
    .unwrap();
    e.ctl("flash", measured);
    e
}

#[test]
fn a_ratchet_makes_the_gate_run_ns_ratchet_check() {
    let e = ratchet_env("104\n");
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert!(
        d["gate"]
            .as_str()
            .unwrap()
            .ends_with(" ratchet check --human"),
        "{d}"
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let g = gate_events(&e);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0]["green"], true);
}

#[test]
fn a_unit_that_stays_over_a_ratchet_ends_stuck_naming_its_values() {
    let e = ratchet_env("120\n");
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    let line = "ratchet flash is worse: measured 120, recorded 100, tolerance 5%, limit 105, lower is better";
    assert_eq!(v["outcome"], "stuck");
    assert_eq!(
        v["reason"],
        format!("build is out of attempts (2): the CI gate failed on {line}")
    );
    assert_eq!(e.calls(), ["triage", "build", "build"]);
    let p = e.prompt(3, "build");
    assert!(p.contains(&format!("It failed on {line}.")), "{p}");
    assert!(p.contains("never edit ratchets.json"), "{p}");
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["green"], false);
    let report = g[0]["log"]
        .as_str()
        .unwrap()
        .replace(".log", ".ratchets.json");
    let r: Value = serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
    assert_eq!(r["ratchets"][0]["status"], "worse");
}

#[test]
fn a_build_that_gets_back_under_the_line_goes_on() {
    let e = ratchet_env("120\n");
    e.ctl(
        "build.sh",
        &format!("echo 99 > {}\n", e.ctrl.join("flash").display()),
    );
    e.queue("build", &["pass:commit", "pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(
        (g[0]["green"].as_bool(), g[1]["green"].as_bool()),
        (Some(false), Some(true))
    );
}

#[test]
fn a_build_that_writes_nothing_after_a_red_ratchet_is_stuck_naming_its_values() {
    let e = ratchet_env("120\n");
    e.queue("build", &["pass:commit", "none"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate failed on ratchet flash is worse: measured 120, recorded 100, tolerance 5%, limit 105, lower is better"
    );
    assert_eq!(gate_events(&e).len(), 1);
}

/// A factory whose ratchet `steady` prints 5, recorded at 5, and logs each run to `ctrl/count`.
fn counted_ratchet() -> String {
    "[ratchets.steady]\ncommand = \"echo x >> $FAKE_CTRL/count; echo 5\"\nbetter = \"lower\"\n"
        .into()
}

fn ratchet_runs(e: &Env) -> usize {
    fs::read_to_string(e.ctrl.join("count"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn without_a_night_every_gate_runs_a_ratchet_twice_whatever_the_environment_says() {
    let e = Env::new();
    e.factory(&counted_ratchet());
    fs::write(
        e.root.join(".nightshift/ratchets.json"),
        r#"{"version": 1, "values": {"steady": 5}}"#,
    )
    .unwrap();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["pass:commit"]);
    let steady = e.base.join("leaked-steady.json");
    let out = e
        .ns()
        .env("NS_RATCHET_STEADY", &steady)
        .args(["run", "--issue", "7"])
        .output();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(gate_events(&e).len(), 2);
    assert_eq!(ratchet_runs(&e), 4);
    assert!(!steady.exists());
}

#[test]
fn under_ns_watch_only_a_ratchets_first_check_of_the_night_runs_it_twice() {
    let e = Env::new();
    e.factory(&counted_ratchet());
    fs::write(
        e.root.join(".nightshift/ratchets.json"),
        r#"{"version": 1, "values": {"steady": 5}}"#,
    )
    .unwrap();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(gate_events(&e).len(), 2);
    assert_eq!(ratchet_runs(&e), 3);
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
fn a_verify_that_fails_after_moving_head_still_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["fail:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "verify");
}

#[test]
fn a_ship_that_fails_after_moving_head_still_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["fail:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "ship");
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
    e.queue(
        "review",
        &["blocked:Open after at least 2 fix attempts: I1."],
    );
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

/// Issue `n`'s team comment: an Agent Brief whose `touches` are `globs`.
fn brief_comment(e: &Env, n: u64, globs: &str) {
    let c = serde_json::json!({
        "number": 1,
        "body": format!("## Agent Brief\n**Touches:** {globs}\n"),
        "authorAssociation": "MEMBER",
    });
    e.gh_file(&format!("comments-{n}.jsonl"), &format!("{c}\n"));
}

#[test]
fn watch_dry_run_names_the_running_units_each_ready_issue_overlaps() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    brief_comment(&e, 2, "`cli/src/run.rs`");
    e.ready(4, "Fix c", &["type:fix"], "");
    brief_comment(&e, 4, "`docs/**`");
    e.ready(5, "Fix d", &["type:fix"], "");
    let overlaps = |v: &Value| -> Vec<(u64, Value)> {
        v["queue"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["number"].as_u64().unwrap(), i["overlaps"].clone()))
            .collect()
    };
    let none = serde_json::json!([]);
    let v = e.run(&["watch", "--dry-run"], 0);
    let nothing_runs = [(2, none.clone()), (4, none.clone()), (5, none.clone())];
    assert_eq!(overlaps(&v), nothing_runs, "{v}");

    let wip = ["type:fix", "status:in-progress"];
    // A run lock record with no unit: the unit is named from its issue, its touches read from
    // the issue's brief, as no worktree has a brief.md.
    e.open_by(6, "Fix e", &wip, "", Some("MEMBER"));
    let _held_e = hold(
        &e.root.join(".git/ns-run-6-fix-e.lock"),
        r#"{"pid":43,"issue":6}"#,
    );
    brief_comment(&e, 6, "`docs/FACTORY.md`");
    e.open_by(3, "Fix b", &wip, "", Some("MEMBER"));
    // A unit named apart from its issue's title, as `ns run 3-uart --issue 3` names it.
    let _held = hold(
        &e.root.join(".git/ns-run-3-uart.lock"),
        r#"{"pid":42,"unit":"3-uart","issue":3}"#,
    );
    // The running unit's own brief.md wins over its issue's comment.
    brief_comment(&e, 3, "`docs/**`");
    let name = e.root.file_name().unwrap().to_str().unwrap();
    let artifacts = e.base.join(format!("{name}.worktrees/3-uart/.ns/3-uart"));
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(
        artifacts.join("brief.md"),
        "---\ntouches: [\"cli/**\"]\n---\n",
    )
    .unwrap();
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(
        overlaps(&v),
        [
            (2, serde_json::json!(["3-uart"])),
            (4, serde_json::json!(["6-fix-e"])),
            (5, none)
        ],
        "{v}"
    );
}

#[test]
fn watch_starts_a_ready_issue_clear_of_running_units_before_a_higher_one() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    brief_comment(&e, 2, "`cli/**`");
    e.ready(3, "Fix b", &["type:fix"], "");
    brief_comment(&e, 3, "`cli/src/run.rs`");
    e.ready(4, "Fix c", &["type:fix"], "");
    brief_comment(&e, 4, "`docs/**`");
    // #2 holds at its triage, so the next pick reads its prediction from its issue.
    let (in_a, go_a) = hold_phase(&e, A, "triage");
    let w = Watching::start(
        e.ns()
            .args(["watch", "--parallel", "2", "--max-units", "2"]),
    );
    w.entered(&in_a);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 4], "done"), "{v}");
}

#[test]
fn watch_dry_run_leaves_out_overlap_on_files_overlap_ignore_names() {
    let e = Env::new();
    e.factory("[queue]\noverlap_ignore = [\"docs/FACTORY.md\"]\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    brief_comment(&e, 2, "`docs/FACTORY.md`, `cli/src/run.rs`");
    e.open_by(
        6,
        "Fix e",
        &["type:fix", "status:in-progress"],
        "",
        Some("MEMBER"),
    );
    let _held = hold(
        &e.root.join(".git/ns-run-6-fix-e.lock"),
        r#"{"pid":43,"issue":6}"#,
    );
    brief_comment(&e, 6, "`docs/FACTORY.md`");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["queue"][0]["overlaps"], serde_json::json!([]), "{v}");
}

#[test]
fn watch_keeps_the_queue_order_over_overlap_on_files_overlap_ignore_names() {
    let e = Env::new();
    e.factory("[queue]\noverlap_ignore = [\"docs/FACTORY.md\"]\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    brief_comment(&e, 2, "`docs/FACTORY.md`");
    e.ready(3, "Fix b", &["type:fix"], "");
    brief_comment(&e, 3, "`docs/FACTORY.md`");
    e.ready(4, "Fix c", &["type:fix"], "");
    brief_comment(&e, 4, "`docs/DESIGN.md`");
    let (in_a, go_a) = hold_phase(&e, A, "triage");
    let w = Watching::start(
        e.ns()
            .args(["watch", "--parallel", "2", "--max-units", "2"]),
    );
    w.entered(&in_a);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
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
    two_units_merge_one_at_a_time(AUTO);
}

#[test]
fn under_build_only_two_units_still_merge_one_at_a_time() {
    two_units_merge_one_at_a_time(&format!("{AUTO}[gate]\nlocal = \"build-only\"\n"));
}

fn two_units_merge_one_at_a_time(factory: &str) {
    let e = Env::new();
    e.factory(factory);
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
    let entered = e.ctrl.join("entered");
    mkfifo(&entered);
    e.queue("build", &["pass:script"]);
    e.ctl("build.sh", &format!("echo $$ > {entered:?}; sleep 600"));
    let mut run = e
        .ns()
        .args(["run", "--issue", "7"])
        .timeout(Duration::from_secs(3))
        .start();
    // The run's timeout starts at the wait, once the phase is in, however slow the start.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(entered));
    });
    let pid = rx.recv_timeout(common::TIMEOUT).unwrap().unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.wait())).unwrap_err();
    let msg = panic.downcast_ref::<String>().unwrap();
    assert!(msg.contains("timed out after 3s"), "{msg}");
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
    let main = git(&e.base.join("remote.git"), &["rev-parse", "main"]);
    assert!(
        e.gh_calls().contains(&format!(
            "api repos/{{owner}}/{{repo}}/commits/{main}/pulls"
        )),
        "{}",
        e.gh_calls()
    );
}

/// The build phase commits to the unit, runs `pre` in the worktree, then another actor lands
/// `land` on origin's main from a clone, GitHub links the new main commit to `pulls`, and the
/// build rebases onto it.
fn another_actor_lands(e: &Env, pre: &str, land: &str, pulls: &str) {
    e.ctl(
        "build.sh",
        &format!(
            r#"set -e
c="git -c user.name=f -c user.email=f@f"
echo own >> work.txt && git add work.txt && $c commit -qm own
{pre}
o="$FAKE_CTRL/other"
rm -rf "$o" && git clone -q "$(git remote get-url origin)" "$o"
(cd "$o" && {land} && git push -q origin main &&
  printf '%s' '{pulls}' > "$FAKE_GH_DIR/pulls-$(git rev-parse HEAD).json")
git fetch -q origin && $c rebase -q origin/main"#
        ),
    );
    e.queue("build", &["pass:script"]);
}

const SQUASH_B: &str = r#"echo b > b.txt && git add b.txt && $c commit -qm "Fix b (#3)""#;
const MERGED_B: &str =
    r#"[{"number":3,"merged_at":"2026-10-09T00:00:00Z","head":{"ref":"ns/3-fix-b"}}]"#;

fn run_log(e: &Env) -> String {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl")).unwrap()
}

#[test]
fn another_units_merge_then_a_rebase_is_not_this_units_push() {
    let e = Env::new();
    another_actor_lands(&e, "", SQUASH_B, MERGED_B);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let main = git(&e.base.join("remote.git"), &["rev-parse", "main"]);
    let wt = e.worktree(UNIT);
    assert!(git(&wt, &["merge-base", "--is-ancestor", &main, "HEAD"]).is_empty());
    let log = run_log(&e);
    assert!(
        log.contains(&format!("\"to\":\"{main}\""))
            && log.contains("\"event\":\"default_moved_by_other_actor\"")
            && log.contains("\"prs\":[3]")
            && !log.contains("\"breach\""),
        "{log}"
    );
}

#[test]
fn a_human_merge_commit_then_a_rebase_is_not_this_units_push() {
    let e = Env::new();
    another_actor_lands(
        &e,
        "",
        r#"git checkout -qb human-fix && echo h > h.txt && git add h.txt && $c commit -qm h &&
  git checkout -q main && $c merge -q --no-ff human-fix -m "Merge pull request #50""#,
        r#"[{"number":50,"merged_at":"2026-10-09T00:00:00Z","head":{"ref":"human-fix"}}]"#,
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let log = run_log(&e);
    assert!(
        log.contains("\"prs\":[50]") && !log.contains("\"breach\""),
        "{log}"
    );
}

#[test]
fn a_push_of_its_own_under_another_merge_is_still_stuck() {
    let e = Env::new();
    another_actor_lands(&e, "git push -q origin HEAD:main", SQUASH_B, MERGED_B);
    let v = e.run(&["run", "--issue", "7"], 1);
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.starts_with("default branch moved"), "{v}");
    assert!(
        reason.contains("is in no merged PR of another branch"),
        "{v}"
    );
    assert_eq!(e.calls(), ["triage", "build"]);
}

#[test]
fn a_merged_pr_from_this_units_branch_is_still_stuck() {
    let e = Env::new();
    another_actor_lands(
        &e,
        "",
        SQUASH_B,
        &MERGED_B.replace("ns/3-fix-b", &format!("ns/{UNIT}")),
    );
    let v = e.run(&["run", "--issue", "7"], 1);
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.contains("is in no merged PR of another branch"),
        "{v}"
    );
}

#[test]
fn a_merge_github_cannot_attribute_is_stuck() {
    let e = Env::new();
    another_actor_lands(&e, "", SQUASH_B, MERGED_B);
    e.gh_file("pulls.fail", "");
    let v = e.run(&["run", "--issue", "7"], 1);
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("cannot tell who made"), "{v}");
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
/// `[merge]` with the local merge queue off: update-branch, then the gate on the PR head.
const LEGACY: &str = "local_queue = false\n";
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
        calls.contains("pr merge 12 --squash --match-head-commit"),
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
    assert_eq!(update_branch_vias(&e), ["api"]);
}

#[test]
fn build_only_with_no_fast_gate_updates_a_branch_behind_main_and_gates_only_after_build() {
    let e = Env::new();
    e.factory(&format!("{AUTO}[gate]\nlocal = \"build-only\"\n"));
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command |\n|---|---|\n| rules | `true` | 1 s |\n",
    )
    .unwrap();
    git(&e.root, &["add", "."]);
    git(&e.root, &["commit", "-q", "-m", "stack"]);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert!(e.gh_calls().contains("pr update-branch 12"));
    assert_eq!(gate_where(&e), [("build".into(), "local_full".into())]);
}

/// `[gate] local = "build-only"` with auto merge, its fast and full gates logged to the returned
/// file, a build that commits, and PR #12 green on GitHub.
fn remote_queue(e: &Env, merge: &str) -> PathBuf {
    let (runs, _) = build_only(e, merge);
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:script", "pass:script"]);
    runs
}

const UPDATED: &str = "00000000000000000000000000000000000000aa";

#[test]
fn build_only_merges_on_github_updating_the_pr_head_to_main_and_waiting_for_its_checks() {
    let e = Env::new();
    let runs = remote_queue(&e, AUTO);
    // GitHub says CLEAN, as with no "up to date" rule, but the PR head lacks main's tip.
    main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    let upd = calls.find("pr update-branch 12").expect("update-branch");
    let watch = calls.find("pr checks 12 --watch").unwrap();
    let merge = calls
        .find(&format!(
            "pr merge 12 --squash --match-head-commit {UPDATED}"
        ))
        .unwrap_or_else(|| panic!("{calls}"));
    assert!(upd < watch && watch < merge, "{calls}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\n");
    assert_eq!(gate_where(&e), [("build".into(), "local_full".into())]);
    let passed: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "ci_passed")
        .collect();
    assert_eq!(passed.len(), 1, "{passed:?}");
    assert_eq!(
        (&passed[0]["where"], &passed[0]["sha"]),
        (&json!("github"), &json!(UPDATED))
    );
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn build_only_does_not_update_a_pr_head_that_holds_main() {
    let e = Env::new();
    remote_queue(&e, AUTO);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(!calls.contains("pr update-branch"), "{calls}");
    assert!(calls.contains("pr checks 12 --watch"), "{calls}");
}

#[test]
fn build_only_runs_no_local_gate_after_a_ship_that_moves_head() {
    let e = Env::new();
    let runs = remote_queue(&e, AUTO);
    e.queue("ship", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\n");
    assert_eq!(gate_where(&e), [("build".into(), "local_full".into())]);
    assert!(e.gh_calls().contains("pr checks 12 --watch"));
}

#[test]
fn build_only_runs_no_local_gate_after_a_cut_short_ship_that_moves_head() {
    let e = Env::new();
    let runs = remote_queue(&e, "");
    e.queue("ship", &["pass:commit+sleep", "pass"]);
    let v = run_with_timeouts(&e, "ship=1500", 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\n");
    assert_eq!(gate_where(&e), [("build".into(), "local_full".into())]);
}

#[test]
fn every_step_still_gates_after_a_ship_that_moves_head() {
    let e = Env::new();
    let runs = e.ctrl.join("ci-runs");
    e.factory(&format!(
        "{AUTO}[phases.build]\ngate = \"echo ran >> {}\"\n",
        runs.display()
    ));
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let phases: Vec<Value> = gate_events(&e).iter().map(|g| g["phase"].clone()).collect();
    assert!(phases.contains(&json!("ship")), "{phases:?}");
}

#[test]
fn build_only_sends_red_github_checks_back_to_build_naming_the_check() {
    let e = Env::new();
    let runs = remote_queue(&e, AUTO);
    e.gh_file("checks-12-1.json", RED);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.calls();
    let rebuild = calls.iter().rposition(|c| c == "build").unwrap();
    assert!(
        rebuild > calls.iter().position(|c| c == "ship").unwrap(),
        "{calls:?}"
    );
    let p = e.prompt(rebuild + 1, "build");
    assert!(p.contains("CI failed on PR #12: ci (fail)"), "{p}");
    let failed: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "ci_failed")
        .collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["where"], "github");
    assert_eq!(failed[0]["checks"], json!(["ci (fail)"]));
    // The rebuild's gate is the full one; the merge step runs no local gate.
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfull\n");
}

#[test]
fn build_only_sends_a_conflict_with_main_back_to_ship() {
    for (file, text) in [("pr-12.merge", "DIRTY"), ("update-12.fail", "")] {
        let e = Env::new();
        remote_queue(&e, AUTO);
        let tip = main_moves(&e, "from-main", "x\n");
        e.gh_file(file, text);
        let v = e.run(&["run", "--issue", "7"], 1);
        assert_eq!(v["outcome"], "stuck", "{file}: {v}");
        assert_eq!(v["reason"], "ship is out of attempts (2)", "{file}: {v}");
        // The first conflict's reship is free. Main stays where it was, so the next one uses
        // ship's second attempt and the unit ends stuck instead of looping.
        let calls = e.calls();
        assert_eq!(
            calls,
            ["triage", "build", "verify", "review", "ship", "ship", "ship"],
            "{file}"
        );
        let p = e.prompt(6, "ship");
        assert!(
            p.contains("PR #12 conflicts with main: rebase onto main and resolve conflicts"),
            "{file}: {p}"
        );
        assert!(
            p.contains(&format!(
                "merge in the newest origin/main ({tip}). This ship costs no attempt."
            )),
            "{file}: {p}"
        );
        assert!(!e.prompt(7, "ship").contains("costs no attempt"), "{file}");
        let r = rebase_events(&e);
        assert_eq!(r.len(), 1, "{file}: {r:?}");
        assert_eq!(
            (&r[0]["phase"], &r[0]["cause"]),
            (&json!("ship"), &json!("the PR conflicts with its base")),
            "{file}"
        );
        assert!(!e.gh_calls().contains("pr merge"), "{file}");
        assert!(
            run_events(&e)
                .iter()
                .any(|ev| ev["event"] == "conflict" && ev["pr"] == 12),
            "{file}"
        );
    }
}

/// A ship script that commits and clears PR #12's conflict, except on its run `move_on`, where
/// another actor moves main and the PR conflicts again.
fn ship_brings_in_main(e: &Env, move_on: u32) {
    e.ctl(
        "ship.sh",
        &format!(
            "n=$(( $(cat \"$FAKE_CTRL/ships\" 2>/dev/null || echo 0) + 1 )); echo $n > \"$FAKE_CTRL/ships\"\n\
             if [ $n = {move_on} ]; then {} && echo DIRTY > \"$FAKE_GH_DIR/pr-12.merge\"\n\
             else echo $n >> main-in.txt && git add main-in.txt && git -c user.name=f -c user.email=f@f commit -qm \"merge main $n\" && echo CLEAN > \"$FAKE_GH_DIR/pr-12.merge\"; fi\n",
            move_main_sh(e)
        ),
    );
}

#[test]
fn build_only_ships_that_follow_a_moved_base_cost_ship_no_attempts() {
    let e = Env::new();
    remote_queue(&e, AUTO);
    e.gh_file("pr-12.merge", "DIRTY");
    ship_brings_in_main(&e, 2);
    // Each reship merges main in; review's rerun after it commits a fix, so verify and ship run
    // again. The first of those ships finds main moved and the PR conflicting again.
    e.queue(
        "ship",
        &["pass", "pass:script", "pass:script", "pass:script", "pass"],
    );
    e.queue("review", &["pass", "pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        e.calls(),
        [
            "triage", "build", "verify", "review", "ship", "ship", "verify", "review", "verify",
            "ship", "ship", "verify", "review", "verify", "ship"
        ]
    );
    let free: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "ship_after_base_moved")
        .collect();
    assert_eq!(free.len(), 2, "{free:?}");
    assert_eq!(rebase_events(&e).len(), 2);
}

#[test]
fn build_only_ships_that_fail_after_a_moved_base_still_run_out_of_attempts() {
    let e = Env::new();
    remote_queue(&e, AUTO);
    e.gh_file("pr-12.merge", "DIRTY");
    e.queue("ship", &["pass", "fail:review", "fail:review"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(v["reason"], "ship is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "ship", "review"]
    );
}

#[test]
fn build_only_a_failing_ship_after_a_moved_base_gives_its_free_attempt_back() {
    let e = Env::new();
    remote_queue(&e, AUTO);
    e.gh_file("pr-12.merge", "DIRTY");
    ship_brings_in_main(&e, 0);
    e.queue("ship", &["pass", "pass:script", "fail:review"]);
    e.queue("review", &["pass", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["reason"], "ship is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        [
            "triage", "build", "verify", "review", "ship", "ship", "verify", "review", "verify",
            "ship", "review"
        ]
    );
}

#[test]
fn build_only_a_build_after_a_moved_base_makes_the_next_ship_cost_an_attempt() {
    let e = Env::new();
    remote_queue(&e, AUTO);
    e.gh_file("pr-12.merge", "DIRTY");
    ship_brings_in_main(&e, 0);
    e.queue("ship", &["pass", "pass:script", "pass"]);
    e.queue("review", &["pass", "fail"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        e.calls(),
        [
            "triage", "build", "verify", "review", "ship", "ship", "verify", "review", "build",
            "verify", "review", "ship"
        ]
    );
    let free = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "ship_after_base_moved")
        .count();
    assert_eq!(free, 0);
}

#[test]
fn build_only_a_reship_that_changes_nothing_does_not_make_the_next_reship_a_chain_ship() {
    let e = Env::new();
    remote_queue(&e, AUTO);
    e.gh_file("pr-12.merge", "DIRTY");
    e.ctl("ship.sh", &move_main_sh(&e));
    e.queue(
        "ship",
        &[
            "pass:script",
            "pass:script",
            "pass:script",
            "fail:review",
            "fail:review",
        ],
    );
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(v["reason"], "ship is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "ship", "ship", "ship", "review"]
    );
    assert!(!run_events(&e)
        .iter()
        .any(|ev| ev["event"] == "ship_after_base_moved"));
}

#[test]
fn build_only_a_credited_ship_that_gives_up_on_a_lock_is_credited_again_on_its_retry() {
    let e = Env::new();
    let locks = e.base.join("locks");
    fs::create_dir_all(&locks).unwrap();
    remote_queue(&e, &format!("{AUTO}[phases.ship]\nrunner = \"bench\"\n"));
    e.runner("bench", "kind = \"bench\"\nlocks = [\"bench-1\"]\n");
    e.config(&format!(
        "[runners]\nlock_dir = {:?}\n",
        locks.to_str().unwrap()
    ));
    e.gh_file("pr-12.merge", "DIRTY");
    ship_brings_in_main(&e, 0);
    // The review that precedes the credited ship tells the test it is running and waits, so the
    // test takes the ship's lock before the ship asks for it.
    let ready = e.ctrl.join("ready");
    let go = e.ctrl.join("go");
    for f in [&ready, &go] {
        assert!(StdCommand::new("mkfifo").arg(f).status().unwrap().success());
    }
    e.ctl(
        "review.sh",
        &format!("echo in > {ready:?}; cat {go:?} > /dev/null\n"),
    );
    e.queue("ship", &["pass", "pass:script"]);
    e.queue("review", &["pass", "pass:script"]);
    let mut run = e.ns().args(["run", "--issue", "7"]).start();
    let mut stderr = run.take_stderr();
    let (tx, rx) = mpsc::channel();
    let tx_err = tx.clone();
    thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        let _ = tx_err.send(Some(text));
    });
    thread::spawn(move || {
        let _ = fs::read_to_string(&ready);
        let _ = tx.send(None);
    });
    assert!(run.recv(&rx).is_none(), "the run ended before its review");
    let lock = fs::File::create(locks.join("bench-1.lock")).unwrap();
    lock.lock().unwrap();
    fs::write(&go, "go\n").unwrap();
    let stderr = run.recv(&rx).expect("stderr");
    drop(lock);
    assert_eq!(run.wait().code(), Some(1), "{stderr}");
    let log = run_events(&e);
    let attempts = |event: &str| -> Vec<Value> {
        log.iter()
            .filter(|ev| ev["event"] == event)
            .map(|ev| ev["attempt"].clone())
            .collect()
    };
    assert_eq!(attempts("lock_wait_timeout"), [3, 4], "{stderr}");
    assert_eq!(attempts("ship_after_base_moved"), [3, 4, 5], "{stderr}");
}

#[test]
fn build_only_stops_reshipping_for_a_base_that_moves_on_every_check() {
    let e = Env::new();
    remote_queue(&e, AUTO);
    e.gh_file("pr-12.merge", "DIRTY");
    e.ctl("ship.sh", &move_main_sh(&e));
    e.queue("ship", &["pass:script"; 7]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(
        v["reason"], "ship was sent back 5 times because the base moved (cap 5)",
        "{v}"
    );
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "ship", "ship", "ship", "ship", "ship"]
    );
    assert_eq!(rebase_events(&e).len(), 5);
    let capped: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "reship_cap")
        .collect();
    assert_eq!(capped.len(), 1, "{capped:?}");
    assert_eq!(
        (&capped[0]["reships"], &capped[0]["cap"]),
        (&json!(5), &json!(5))
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn build_only_a_reship_earns_no_credit_when_origin_default_cannot_be_read() {
    let e = Env::new();
    remote_queue(&e, AUTO);
    e.gh_file("pr-12.merge", "DIRTY");
    e.ctl(
        "review.sh",
        "git remote set-url origin \"$FAKE_CTRL/gone.git\"",
    );
    e.queue("review", &["pass:script"]);
    e.queue("ship", &["pass", "fail:review", "fail:review"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(v["reason"], "ship is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "ship", "review"]
    );
    assert!(!run_events(&e).iter().any(|ev| {
        ev["event"] == "ship_after_base_moved" || ev["event"] == "rebase_after_base_moved"
    }));
}

#[test]
fn build_only_needs_a_human_merge_when_main_cannot_be_fetched() {
    let e = Env::new();
    remote_queue(&e, AUTO);
    e.queue("ship", &["pass:script"]);
    e.ctl(
        "ship.sh",
        "git remote set-url origin \"$FAKE_CTRL/gone.git\"",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("cannot fetch main to update PR #12: git fetch origin main failed"),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn build_only_waits_for_github_checks_at_most_ci_timeout_minutes() {
    let e = Env::new();
    remote_queue(
        &e,
        &AUTO.replace("ci_timeout_minutes = 1", "ci_timeout_minutes = 0"),
    );
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *\"pr checks\"*--watch*) sleep 30; exit 0 ;; esac\n",
    );
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(
        v["reason"], "CI on PR #12 did not finish within 0 min",
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
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
    // The first conflict's rebuild is free. Main stays where it was, so the later ones use
    // build's two attempts and the unit ends stuck instead of looping.
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(v["reason"], "build is out of attempts (2)", "{v}");
    assert!(!e.gh_calls().contains("pr merge"));
    let r = rebase_events(&e);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["cause"], "PR #12 conflicts with main");
}

fn update_branch_events(e: &Env) -> usize {
    update_branch_vias(e).len()
}

/// How each logged branch update was made: `api` or `git`.
fn update_branch_vias(e: &Env) -> Vec<String> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "update_branch")
        .map(|v| v["via"].as_str().unwrap_or_default().to_string())
        .collect()
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

fn update_failures(e: &Env) -> Vec<Value> {
    run_log(e)
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "update_branch_failed")
        .collect()
}

#[test]
fn a_failed_update_branch_logs_its_cause() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file(
        "update-12.fail",
        "GraphQL: merge conflict between base and head",
    );
    e.run(&["run", "--issue", "7"], 1);
    let failed = update_failures(&e);
    // The first failure's rebuild is a free rebase; the next two use build's attempts.
    assert_eq!(failed.len(), 3, "{failed:?}");
    let err = failed[0]["error"].as_str().unwrap();
    assert!(
        err.contains("merge conflict between base and head"),
        "{err}"
    );
    assert!(failed[0].get("workflow_scope").is_none(), "{failed:?}");
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_pr_still_behind_after_update_branch_logs_its_merge_state() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("updated-12.merge", "BEHIND");
    e.gh_file("updated-12.lag", "1000");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let failed = update_failures(&e);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["merge_state"], "BEHIND", "{failed:?}");
    let err = failed[0]["error"].as_str().unwrap();
    assert!(err.contains("did not change"), "{err}");
}

const WORKFLOW_REFUSAL: &str = "GraphQL: refusing to allow an OAuth App to create or update workflow `.github/workflows/ci.yml` without `workflow` scope (updatePullRequestBranch)";

/// Ship pushes the unit's branch, then another actor lands `land` on origin's main from a clone,
/// and `after` runs in the worktree. GitHub reports the PR `BEHIND` and refuses `update-branch`
/// for the workflow scope.
fn workflow_refused(land: &str, after: &str) -> (Env, Value, i32) {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("ship", &["pass:script", "pass:script"]);
    e.ctl(
        "ship.sh",
        &format!(
            r#"set -e
c="git -c user.name=f -c user.email=f@f"
git push -q -f origin HEAD
o="$FAKE_CTRL/other"
if [ ! -d "$o" ]; then
  git clone -q "$(git remote get-url origin)" "$o"
  (cd "$o" && {land} && git push -q origin main)
fi
{after}"#
        ),
    );
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("update-12.fail", WORKFLOW_REFUSAL);
    e.gh_file("checks-12.json", GREEN);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let v = serde_json::from_slice(&out.stdout).unwrap();
    (e, v, out.status.code().unwrap())
}

const LAND_UP: &str = r#"echo up > UP && git add UP && $c commit -qm up"#;

#[test]
fn a_workflow_refusal_merges_main_with_git_and_pushes() {
    let (e, v, code) = workflow_refused(LAND_UP, "");
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let remote = e.base.join("remote.git");
    let head = git(&remote, &["rev-parse", &format!("ns/{UNIT}")]);
    let main = git(&remote, &["rev-parse", "main"]);
    git(&remote, &["merge-base", "--is-ancestor", &main, &head]);
    assert_eq!(
        git(&remote, &["rev-list", "--count", "--merges", "-1", &head]),
        "1"
    );
    let calls = e.gh_calls();
    let polls = register_polls(&e);
    assert!(!polls.is_empty(), "{calls}");
    assert!(
        polls
            .iter()
            .all(|p| p.contains(&format!("/commits/{head}/"))),
        "{calls}"
    );
    let upd = calls.find("pr update-branch 12").unwrap();
    let watch = calls.find("pr checks 12 --watch").unwrap();
    assert!(upd < watch, "{calls}");
    assert!(
        calls.contains(&format!("--match-head-commit {head}")),
        "{calls}"
    );
    let failed = update_failures(&e);
    assert_eq!(failed[0]["workflow_scope"], true, "{failed:?}");
    assert!(
        failed[0]["error"]
            .as_str()
            .unwrap()
            .contains("without `workflow` scope"),
        "{failed:?}"
    );
    assert_eq!(update_branch_vias(&e), ["git"]);
}

#[test]
fn a_conflict_in_the_git_fallback_goes_back_to_build() {
    let (e, v, code) = workflow_refused(
        r#"echo theirs > work.txt && git add work.txt && $c commit -qm theirs"#,
        "",
    );
    assert_eq!((code, &v["outcome"]), (1, &"stuck".into()), "{v}");
    let p = e.prompt(6, "build");
    assert!(p.contains("conflicts with main"), "{p}");
    assert_eq!(update_failures(&e)[0]["workflow_scope"], true);
    let wt = e.worktree(UNIT);
    assert_eq!(
        git(&wt, &["status", "--porcelain"]),
        "",
        "the failed merge is aborted"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_failed_fetch_in_the_git_fallback_needs_a_human_merge() {
    let (e, v, code) = workflow_refused(LAND_UP, "git remote set-url origin /nonexistent");
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("`workflow` scope"), "{reason}");
    assert!(reason.contains("git fetch"), "{reason}");
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_rebased_pr_head_is_not_overwritten_by_the_git_fallback() {
    let rebased = r#"git rev-parse HEAD > "$FAKE_GH_DIR/pr-12.head"
git push -q -f origin HEAD
$c commit -q --amend -m reworded"#;
    let (e, v, code) = workflow_refused(LAND_UP, rebased);
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("`workflow` scope"), "{reason}");
    assert!(reason.contains("needs a human merge"), "{reason}");
    let pushed = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    let remote = e.base.join("remote.git");
    assert_eq!(
        git(&remote, &["rev-parse", &format!("ns/{UNIT}")]),
        pushed.trim()
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_merge_that_fails_without_a_conflict_needs_a_human_merge() {
    let landed = r#"echo up > UNTRACKED && git add UNTRACKED && $c commit -qm up"#;
    let (e, v, code) = workflow_refused(landed, "echo mine > UNTRACKED");
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("`workflow` scope"), "{reason}");
    assert!(reason.contains("UNTRACKED"), "{reason}");
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_failed_push_in_the_git_fallback_needs_a_human_merge() {
    // Someone else moves the PR branch after ship, so the lease on the PR head fails.
    let moved = r#"(cd "$o" && git fetch -q origin "$(git -C "$OLDPWD" symbolic-ref --short HEAD)" \
  && git checkout -q FETCH_HEAD && echo other > OTHER && git add OTHER && $c commit -qm other \
  && git push -q origin "HEAD:$(git -C "$OLDPWD" symbolic-ref --short HEAD)")"#;
    let (e, v, code) = workflow_refused(LAND_UP, moved);
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("`workflow` scope"), "{reason}");
    assert!(reason.contains("git push"), "{reason}");
    assert!(reason.contains("needs a human merge"), "{reason}");
    assert!(!reason.contains("required status checks"), "{reason}");
    let wt = e.worktree(UNIT);
    let shipped = git(&wt, &["rev-parse", "HEAD"]);
    assert_eq!(
        git(&wt, &["rev-list", "--count", "--merges", "-1", &shipped]),
        "0"
    );
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
    let ci: Vec<_> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "ci_failed" || ev["event"] == "ci_passed")
        .map(|ev| (ev["event"].clone(), ev["where"].clone()))
        .collect();
    assert_eq!(
        ci,
        [
            ("ci_failed".into(), "github".into()),
            ("ci_passed".into(), "github".into())
        ]
    );
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
    let rebase = "git update-ref refs/heads/main upstream\n\
                  git -c user.name=f -c user.email=f@f rebase -q main\n";
    main_moves_after_review(ship, print_pr_head_sha, rebase, setup)
}

/// The commit main gains after review is on branch `upstream`; `pass:script` in ship runs
/// `bring_in` to take it into the unit.
fn main_moves_after_review(
    ship: &[&str],
    print_pr_head_sha: &str,
    bring_in: &str,
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
    git(&e.root, &["checkout", "-q", "main"]);
    e.ctl(
        "ship.sh",
        &format!(
            "({print_pr_head_sha}) | tee \"$FAKE_GH_DIR/pr-12.head\" > \"$FAKE_GH_DIR/pr-12.shipped\"\n\
             {bring_in}"
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
    assert!(gate_events(&e).is_empty());
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

/// Ship merges main's new commit as `git pull origin main` would: `origin/main` moves, local main
/// stays behind.
const MERGE_UPSTREAM: &str =
    "git -c user.name=f -c user.email=f@f merge -q --no-ff --no-edit upstream\n\
     git update-ref refs/remotes/origin/main upstream\n";

#[test]
fn a_merge_of_main_after_review_still_merges() {
    let (e, v, code) =
        main_moves_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, MERGE_UPSTREAM, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
}

#[test]
fn a_merge_of_main_that_changes_the_units_diff_reverifies() {
    let resolve = "git merge -q --no-ff --no-commit upstream\n\
                   echo resolved >> work.txt && git add work.txt\n\
                   git -c user.name=f -c user.email=f@f commit -q --no-edit\n\
                   git rev-parse HEAD > \"$FAKE_GH_DIR/pr-12.head\"\n";
    let (e, v, code) = main_moves_after_review(
        &["pass:script", "pass"],
        PRINT_HEAD_SHA_CMD,
        resolve,
        |_| {},
    );
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "verify", "review", "ship"]
    );
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
fn rerunning_a_stale_pass_uses_no_attempt() {
    // Review's fix commit, then ship's, each leave a passing evidence.md behind HEAD: verify runs
    // three times under max_attempts 2, as on #156.
    let (e, v, code) = rebased_after_review(&["pass:commit", "pass"], PRINT_HEAD_SHA_CMD, |e| {
        e.queue("review", &["pass:commit", "pass"]);
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "verify", "ship", "verify", "review", "ship"]
    );
}

#[test]
fn only_the_first_rerun_after_a_pass_is_free() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 5\n");
    e.queue("build", &["pass:commit", "pass:commit", "pass:commit"]);
    e.queue("verify", &["pass", "fail", "fail", "fail"]);
    e.queue("review", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["reason"], "verify is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "verify", "build", "verify", "build"]
    );
}

#[test]
fn a_content_change_after_every_review_runs_out_of_attempts() {
    let (e, v, code) =
        rebased_after_review(&["pass:commit", "pass:commit"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!(code, 1, "{v}");
    // Each ship moves HEAD; the reruns of verify and review are free, so ship's cap stops it.
    assert_eq!(v["reason"], "ship is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        [
            "triage", "build", "verify", "review", "ship", "verify", "review", "ship", "verify",
            "review"
        ]
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
fn factory_validate_warns_when_build_only_has_no_fast_ci_local() {
    let e = Env::new();
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command | Takes |\n|---|---|---|\n| ci-local | `make ci` | 2 min |\n",
    )
    .unwrap();
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["errors"], serde_json::json!([]), "{v}");
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("ci-local") && warnings.contains("`make ci --fast`"),
        "{v}"
    );
    e.factory("");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
}

#[test]
fn factory_validate_build_only_fast_warning_edge_cases() {
    let e = Env::new();
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command | Takes |\n|---|---|---|\n| ci-local | `scripts/ci-local.sh --fast` | 1 min |\n",
    )
    .unwrap();
    e.factory("[gate]\nlocal = \"build-only\"\n[phases.build]\ngate = \"make ci\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("[phases.build] gate") && warnings.contains("`make ci --fast`"),
        "{v}"
    );
}

/// An env whose `origin` is the GitHub repo `o/r` and whose fake gh reports its owner as
/// `owner_type`, or prints nothing when `None`.
fn github_owned_by(owner_type: Option<&str>) -> Env {
    let e = Env::new();
    git(
        &e.root,
        &["remote", "set-url", "origin", "https://github.com/o/r.git"],
    );
    if let Some(t) = owner_type {
        e.gh_file("owner-type", &format!("{t}\n"));
    }
    e
}

#[test]
fn factory_validate_warns_when_build_only_runs_on_a_personal_account_repo() {
    let e = github_owned_by(Some("User"));
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["errors"], serde_json::json!([]), "{v}");
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{v}");
    let w = warnings[0].as_str().unwrap();
    assert!(
        w.contains("organisation-owned") && w.contains("local = \"every-step\""),
        "{w}"
    );
    assert!(e.gh_calls().contains("api repos/o/r --jq .owner.type"));
}

#[test]
fn factory_validate_with_factory_path_looks_up_the_owner_of_the_cwd_repo() {
    let e = github_owned_by(Some("User"));
    let other = e.base.join("other-checkout");
    fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q"]);
    git(
        &other,
        &["remote", "add", "origin", "https://github.com/x/y.git"],
    );
    fs::write(
        other.join("nightshift.toml"),
        "[gate]\nlocal = \"build-only\"\n",
    )
    .unwrap();
    let v = e.run(
        &["factory", "validate", "--factory", other.to_str().unwrap()],
        0,
    );
    assert_eq!(v["ok"], true, "{v}");
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{v}");
    assert!(
        warnings[0].as_str().unwrap().contains("organisation-owned"),
        "{v}"
    );
    let calls = e.gh_calls();
    assert!(calls.contains("api repos/o/r --jq .owner.type"), "{calls}");
    assert!(!calls.contains("repos/x/y"), "{calls}");
}

#[test]
fn factory_validate_owner_warning_joins_the_fast_warning() {
    let e = github_owned_by(Some("User"));
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command | Takes |\n|---|---|---|\n| ci-local | `make ci` | 2 min |\n",
    )
    .unwrap();
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    let warnings = v["warnings"].to_string();
    assert!(warnings.contains("`make ci --fast`"), "{v}");
    assert!(warnings.contains("organisation-owned"), "{v}");
}

#[test]
fn factory_validate_gives_no_owner_warning_without_a_personal_account_answer() {
    for owner in [Some("Organization"), Some("not json {"), None] {
        let e = github_owned_by(owner);
        e.factory("[gate]\nlocal = \"build-only\"\n");
        let v = e.run(&["factory", "validate"], 0);
        assert_eq!(v["warnings"], serde_json::json!([]), "{owner:?}: {v}");
    }
    let e = github_owned_by(Some("User"));
    e.gh_file("repo.fail", "HTTP 404: Not Found\n");
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    fs::remove_file(e.ghd.join("repo.fail")).unwrap();
    e.gh_file(
        "hook.sh",
        "[[ \" $* \" == *\" .owner.type \"* ]] && { echo User; exit 1; }\n",
    );
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "a failed lookup: {v}");
}

#[test]
fn factory_validate_skips_the_owner_lookup_off_build_only_or_github() {
    let e = github_owned_by(Some("User"));
    for text in ["", "[gate]\nlocal = \"every-step\"\n"] {
        e.factory(text);
        let v = e.run(&["factory", "validate"], 0);
        assert_eq!(v["warnings"], serde_json::json!([]), "{text:?}: {v}");
    }
    assert!(!e.gh_calls().contains(".owner.type"), "{}", e.gh_calls());
    git(
        &e.root,
        &["remote", "set-url", "origin", "https://gitlab.com/o/r.git"],
    );
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    assert!(!e.gh_calls().contains(".owner.type"), "{}", e.gh_calls());
}

#[test]
fn factory_validate_gives_no_owner_warning_when_gh_is_missing() {
    let e = github_owned_by(Some("User"));
    let only_git = e.base.join("only-git");
    fs::create_dir(&only_git).unwrap();
    let real_git = String::from_utf8(
        std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::os::unix::fs::symlink(real_git.trim(), only_git.join("git")).unwrap();
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e
        .ns()
        .env("PATH", &only_git)
        .args(["factory", "validate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&v).unwrap();
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    assert_eq!(e.gh_calls(), "", "the fake gh is off PATH");
}

#[test]
fn factory_validate_warns_on_auto_merge_with_no_human_review() {
    let e = Env::new();
    e.factory("[merge]\npolicy = \"auto\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert!(
        v["warnings"]
            .to_string()
            .contains("merge.human_review is empty"),
        "{v}"
    );
    e.factory("[merge]\npolicy = \"auto\"\nhuman_review = [\".nightshift/**\"]\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    e.factory("[merge]\npolicy = \"human\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
}

#[test]
fn factory_validate_prints_the_supervise_settings() {
    let e = Env::new();
    let validate = || -> Value {
        let out = e
            .ns()
            .args(["factory", "validate"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&out).unwrap()
    };
    e.factory("");
    let v = validate();
    assert_eq!(v["supervise"]["notify"], Value::Null);
    assert_eq!(v["supervise"]["silence_minutes"], 45);
    e.factory("[supervise]\nnotify = \"notify-send ns\"\nsilence_minutes = 20\n");
    let v = validate();
    assert_eq!(v["supervise"]["notify"], "notify-send ns");
    assert_eq!(v["supervise"]["silence_minutes"], 20);
    e.factory("[supervise]\nsilence = 20\n");
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["supervise"], Value::Null);

    e.factory("[supervise]\nsilence_minutes = 0\n");
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["supervise"]["silence_minutes"], 0);
    assert!(
        v["errors"]
            .to_string()
            .contains("[supervise] silence_minutes must be at least 1"),
        "{v}"
    );

    fs::remove_file(e.root.join(".nightshift/nightshift.toml")).unwrap();
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["supervise"], Value::Null);
}

#[test]
fn factory_validate_checks_scorers() {
    let e = Env::new();
    e.factory("");
    e.config("[roles.review]\nharness = \"claude\"\n");
    fs::create_dir_all(e.root.join(".nightshift/scorers")).unwrap();
    let scorer = "---\nrole: review\npassing_score: 1\nsampling_rate: 0.5\nlabels:\n  - value: pass\n    score: 1\n---\nGrade it.\n";
    fs::write(e.root.join(".nightshift/scorers/quality.md"), scorer).unwrap();
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["scorers"][0]["name"], "quality");
    assert_eq!(v["scorers"][0]["sampling_rate"], 0.5);

    fs::write(
        e.root.join(".nightshift/scorers/quality.md"),
        scorer.replace("role: review", "role: judge"),
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
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert!(
        v["errors"][0]
            .as_str()
            .unwrap()
            .starts_with("scorers/quality.md: role \"judge\" is not configured"),
        "{v}"
    );
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

/// `gh issue view` reports a pull request's state too: a merged PR as `MERGED`.
#[test]
fn watch_dry_run_clears_a_merged_pr_blocker() {
    let e = Env::new();
    e.ready(1, "After merged PR", &["type:fix"], "Blocked by: #40");
    e.ready(2, "After open PR", &["type:fix"], "Blocked by: #41");
    e.issue(40, "merged PR", "MERGED");
    e.issue(41, "open PR", "OPEN");
    let v = e.run(&["watch", "--dry-run"], 0);
    let order: Vec<u64> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    assert_eq!(order, [1]);
    let skipped = v["skipped"].to_string();
    assert!(skipped.contains("blocked by open #41"), "{skipped}");
    assert!(!skipped.contains("#40"), "{skipped}");
}

/// `ns watch --dry-run`'s output and stderr.
fn dry_run(e: &Env) -> (Value, String) {
    let out = e.ns().args(["watch", "--dry-run"]).output();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{err}");
    (serde_json::from_slice(&out.stdout).unwrap(), err)
}

const MAIN_SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

/// GitHub's answer for main's CI: head `MAIN_SHA`, `required` named by a ruleset, and check runs
/// `(name, status, conclusion)`.
fn main_ci(e: &Env, required: &[&str], runs: &[(&str, &str, &str)]) {
    e.gh_file("main-ci.json", &main_ci_at(MAIN_SHA, required, runs));
}

/// GitHub's answer for main's CI at head `sha`; each check run's job id is 40 plus its index.
fn main_ci_at(sha: &str, required: &[&str], runs: &[(&str, &str, &str)]) -> String {
    let rules = serde_json::json!([{"parameters": {"requiredStatusChecks":
        required.iter().map(|c| serde_json::json!({"context": c})).collect::<Vec<_>>()}}]);
    let nodes: Vec<Value> = runs
        .iter()
        .enumerate()
        .map(|(i, (n, s, c))| {
            serde_json::json!({"name": n, "status": s, "conclusion": c, "databaseId": 40 + i})
        })
        .collect();
    serde_json::json!({"data": {"repository": {"defaultBranchRef": {
        "rules": {"nodes": rules},
        "target": {"oid": sha, "statusCheckRollup": {"contexts": {"nodes": nodes}}},
    }}}})
    .to_string()
}

/// Main red at `MAIN_SHA`: check `test` failed.
fn main_red(e: &Env) {
    main_ci(e, &[], &[("test", "COMPLETED", "FAILURE")]);
}

const FIX_TITLE: &str = "fix: main is red at abcdef0: test";

#[test]
fn watch_with_main_green_runs_the_queue_and_files_nothing() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    main_ci(&e, &[], &[("test", "COMPLETED", "SUCCESS")]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
    assert!(events_named(&e, "main_red").is_empty());
    assert!(events_named(&e, "main_green").is_empty());
}

#[test]
fn watch_with_main_red_files_one_fix_issue_and_starts_only_it() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    main_red(&e);
    // A cap above one, so a second unit or a second filing fails the test instead of looping.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(v["triaged"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(e.triage_only_runs(), 0);
    let calls = e.gh_calls();
    assert_eq!(calls.matches("issue create").count(), 1, "{calls}");
    assert!(
        calls.contains(&format!("issue create --title {FIX_TITLE} --body ")),
        "{calls}"
    );
    assert!(
        calls.contains("--label type:fix --label priority:critical --label status:ready-for-agent"),
        "{calls}"
    );
    assert!(!calls.contains("issue edit 2 "), "{calls}");
    for l in ["type:fix", "priority:critical", "status:ready-for-agent"] {
        assert!(
            calls.contains(&format!("label create {l} --color d73a4a --description ")),
            "{calls}"
        );
    }
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.contains("### test"), "{body}");
    assert!(body.contains("untrusted output from CI"), "{body}");
    assert!(body.contains("FAILED tests/test_x.py::test_y"), "{body}");
    assert!(
        body.contains("No green run on the default branch found"),
        "{body}"
    );
    assert!(body.starts_with(DISCLOSED), "{body}");
    assert!(calls.contains("run view --job 40 --log-failed"), "{calls}");
}

#[test]
fn watch_with_main_red_runs_no_triage_only_run() {
    let e = Env::new();
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    main_red(&e);
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(e.triage_only_runs(), 0);
    assert!(v["triaged"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
}

#[test]
fn watch_stops_triaging_when_main_turns_red_after_a_triage_only_run() {
    let e = Env::new();
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    e.untriaged(6, "Fix f", &["status:needs-triage", "type:fix"]);
    main_ci(&e, &[], &[("test", "COMPLETED", "SUCCESS")]);
    e.gh_file(
        "main-ci.red",
        &main_ci_at(MAIN_SHA, &[], &[("test", "COMPLETED", "FAILURE")]),
    );
    // #5's triage-only run edits its issue, and main is red from then on.
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *\"issue edit 5 \"*) cp \"$d/main-ci.red\" \"$d/main-ci.json\" ;; esac\n",
    );
    e.triage_sets(&["status:needs-info"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    e.queue("triage.6-fix-f", &["none"]);
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(e.triage_only_runs(), 1, "{:?}", e.calls());
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    assert_eq!(e.gh_calls().matches("issue create").count(), 1);
}

#[test]
fn a_resumed_night_holds_at_its_first_fill_when_main_is_red() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    main_red(&e);
    let path = empty_night(&e);
    let out = e
        .ns()
        .args(["watch", "--max-units", "3", "--resume-night"])
        .arg(&path)
        .output();
    let err = String::from_utf8_lossy(&out.stderr);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    assert_eq!(e.triage_only_runs(), 0);
    assert_eq!(e.gh_calls().matches("issue create").count(), 1);
}

#[test]
fn a_unit_running_when_main_turns_red_finishes_and_the_backlog_waits() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w =
        Watching::start(
            e.ns()
                .env("NS_REFILL_MS", "100")
                .args(["watch", "--parallel", "2"]),
        );
    w.entered(&in_a);
    // Main is red before the backlog's next issue appears, so no fill sees one without the other.
    main_red(&e);
    e.ready(3, "Fix b", &["type:fix"], "");
    w.line("ns watch: main is red");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        by_issue(&units(&v)),
        [(2, "done".into()), (100, "done".into())],
        "{v}"
    );
    assert_eq!(e.gh_calls().matches("issue create").count(), 1);
    assert!(!e.gh_calls().contains("issue edit 3 "));
}

#[test]
fn watch_starts_a_fix_issue_it_just_filed_before_github_lists_it() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("create.unlisted", "");
    main_red(&e);
    // A cap above one: a fill after the fix ends must reuse it, not file a second.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    let calls = e.gh_calls();
    assert_eq!(calls.matches("issue create").count(), 1, "{calls}");
}

#[test]
fn a_fix_issue_that_cannot_be_filed_ends_the_night_with_the_error() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("create.fail", "");
    main_red(&e);
    let out = e.ns().args(["watch", "--once"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!out.status.success());
    assert_eq!(v["stopped"], "error", "{v}");
    assert!(v["error"].as_str().unwrap().contains("HTTP 502"), "{v}");
    assert!(units(&v).is_empty(), "{v}");

    let e = gitlab_env();
    e.gl_issue(2, &["type:fix", READY], "", 1);
    e.gh_file(
        "gl-statuses.json",
        &serde_json::json!([{"name": "test", "status": "failed"}]).to_string(),
    );
    e.gh_file("gl-create.raw", "{}");
    let out = e.ns().args(["watch", "--once"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("GitLab named no number"),
        "{v}"
    );
}

#[test]
fn a_fix_issue_is_filed_when_its_labels_already_exist() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("label.exists", "");
    main_red(&e);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    let calls = e.gh_calls();
    assert_eq!(calls.matches("label create").count(), 3, "{calls}");
    assert!(
        calls.contains("--label type:fix --label priority:critical --label status:ready-for-agent"),
        "{calls}"
    );
}

#[test]
fn a_label_that_cannot_be_created_ends_the_night_and_files_nothing() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file("label.fail", "HTTP 403: forbidden\n");
    main_red(&e);
    let out = e.ns().args(["watch", "--once"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!out.status.success());
    assert_eq!(v["stopped"], "error", "{v}");
    assert!(v["error"].as_str().unwrap().contains("HTTP 403"), "{v}");
    assert!(units(&v).is_empty(), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
}

#[test]
fn a_closed_fix_issue_at_the_same_red_sha_is_not_filed_again() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    // The fix issue is closed, as by a person, once its unit has begun; main stays red.
    e.gh_file(
        "hook.sh",
        "[ \"$1 $2 $3\" = \"issue edit 100\" ] && { sed -i '/\"number\":100/d' \"$d/issues.jsonl\"; \
         sed -i 's/OPEN/CLOSED/' \"$d/issue-100.json\"; }\n",
    );
    let out = e.ns().args(["watch", "--max-units", "3"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    let calls = e.gh_calls();
    assert_eq!(calls.matches("issue create").count(), 1, "{calls}");
    assert_eq!(err.matches("already filed for it").count(), 1, "{err}");
    assert!(!calls.contains("issue edit 2 "), "{calls}");
}

#[test]
fn a_unit_paused_before_main_turned_red_resumes_and_ends_as_it_would() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue(&format!("build.{A}"), &["limit", "pass:commit"]);
    // Main is green for the night's start and the first fill, red from the fourth read: the fill
    // after the pause. The unit paused before then resumes and finishes beside the fix.
    main_ci(&e, &[], &[("test", "COMPLETED", "SUCCESS")]);
    e.gh_file(
        "main-ci.red",
        &main_ci_at(MAIN_SHA, &[], &[("test", "COMPLETED", "FAILURE")]),
    );
    e.gh_file(
        "hook.sh",
        "if [ \"$1 $2\" = \"api graphql\" ]; then echo x >> \"$d/graphql\"; \
         [ \"$(wc -l < \"$d/graphql\")\" -ge 4 ] && cp \"$d/main-ci.red\" \"$d/main-ci.json\"; fi\n",
    );
    let v = e.run(&["watch", "--max-units", "3"], 0);
    let outcomes: Vec<(u64, String)> = by_issue(&units(&v));
    assert_eq!(
        outcomes,
        [
            (2, "done".into()),
            (2, "paused".into()),
            (100, "done".into())
        ],
        "{v}"
    );
}

#[test]
fn watch_lists_the_commits_since_main_was_last_green_in_the_fix_issue() {
    let e = Env::new();
    let green = git(&e.root, &["rev-parse", "HEAD"]);
    fs::write(e.root.join("README"), "broken\n").unwrap();
    git(&e.root, &["commit", "-qam", "Break the build"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let red = git(&e.root, &["rev-parse", "HEAD"]);
    e.gh_file(
        "main-ci.json",
        &main_ci_at(&red, &[], &[("test", "COMPLETED", "FAILURE")]),
    );
    // Another workflow passed on the red head itself: it is not the last green.
    e.gh_file(
        "runs-green.json",
        &format!(r#"[{{"headSha":"{red}"}},{{"headSha":"{green}"}}]"#),
    );
    e.run(&["watch", "--once"], 0);
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.contains(&format!("Since {}:", &green[..7])), "{body}");
    assert!(
        body.contains(&format!("- {} Break the build", &red[..7])),
        "{body}"
    );
    assert!(e
        .gh_calls()
        .contains("run list --branch main --status success"));
}

#[test]
fn the_commit_list_takes_the_default_branch_from_the_forge_not_from_origins_head() {
    let e = Env::new();
    let green = git(&e.root, &["rev-parse", "HEAD"]);
    fs::write(e.root.join("README"), "broken\n").unwrap();
    git(&e.root, &["commit", "-qam", "Break the build"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let red = git(&e.root, &["rev-parse", "HEAD"]);
    // The remote's HEAD names a branch it lacks, so `git ls-remote --symref` can't name main.
    git(
        &e.base.join("remote.git"),
        &["symbolic-ref", "HEAD", "refs/heads/trunk"],
    );
    e.gh_file(
        "main-ci.json",
        &main_ci_at(&red, &[], &[("test", "COMPLETED", "FAILURE")]),
    );
    e.gh_file("runs-green.json", &format!(r#"[{{"headSha":"{green}"}}]"#));
    e.run(&["watch", "--once"], 0);
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.contains(&format!("Since {}:", &green[..7])), "{body}");
    assert!(e.gh_calls().contains("run list --branch main "));
}

#[test]
fn watch_reuses_the_open_fix_issue_instead_of_filing_another() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    // Outside the team: never reused, since anyone may file a title.
    e.ready_by(
        8,
        "fix: main is red at 1234567: lint",
        &[],
        "",
        Some("NONE"),
    );
    e.ready(
        9,
        "fix: main is red at 1234567: lint",
        &["priority:critical"],
        "",
    );
    main_red(&e);
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[9], "done"), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
    let red = events_named(&e, "main_red");
    assert_eq!(red.len(), 1, "{red:?}");
    assert_eq!(red[0]["issue"], 9);
}

#[test]
fn watch_reuses_a_parked_fix_issue_and_files_no_duplicate() {
    let e = Env::new();
    e.open_by(
        9,
        "fix: main is red at 1234567: lint",
        &["status:needs-info"],
        "",
        Some("MEMBER"),
    );
    main_red(&e);
    let out = e.ns().args(["watch", "--max-units", "3"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{err}");
    assert!(units(&v).is_empty(), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
    assert!(
        err.contains("fix issue #9 is open but the queue will not start it"),
        "{err}"
    );
    let red = events_named(&e, "main_red");
    assert_eq!(red.len(), 1, "{red:?}");
    assert_eq!(red[0]["issue"], 9);
}

#[test]
fn watch_resumes_the_queue_once_the_fix_merges_and_main_is_green() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    let green_sha = "1111111111111111111111111111111111111111";
    e.gh_file(
        "main-ci.green",
        &main_ci_at(green_sha, &[], &[("test", "COMPLETED", "SUCCESS")]),
    );
    // Merging the fix's PR turns main green.
    e.gh_file(
        "hook.sh",
        "[ \"$1 $2 $3\" = \"pr merge 12\" ] && cp \"$d/main-ci.green\" \"$d/main-ci.json\"\n",
    );
    e.ctl("pr-100-fix-main-is-red-at-abcdef0-test", "12");
    e.ctl("pr-2-fix-a", "13");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(
        units(&v)
            .iter()
            .map(|u| (
                u["issue"].as_u64().unwrap(),
                u["outcome"].as_str().unwrap().to_string()
            ))
            .collect::<Vec<_>>(),
        [(100, "merged".to_string()), (2, "merged".to_string())],
        "{v}"
    );
    let red = events_named(&e, "main_red");
    assert_eq!(red.len(), 1, "{red:?}");
    assert_eq!(red[0]["sha"], MAIN_SHA);
    assert_eq!(red[0]["failing"], serde_json::json!(["test"]));
    assert_eq!(red[0]["issue"], 100);
    let green = events_named(&e, "main_green");
    assert_eq!(green.len(), 1, "{green:?}");
    assert_eq!(green[0]["sha"], green_sha);
    assert_eq!(green[0]["state"], "green");
}

#[test]
fn watch_on_gitlab_lists_the_commits_since_the_last_green_pipeline() {
    let e = gitlab_env();
    e.gl_issue(2, &["type:fix", READY], "", 1);
    let green = git(&e.root, &["rev-parse", "HEAD"]);
    fs::write(e.root.join("README"), "broken\n").unwrap();
    git(&e.root, &["commit", "-qam", "Break the build"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let red = git(&e.root, &["rev-parse", "HEAD"]);
    e.gh_file(
        "gl-statuses.json",
        &serde_json::json!([{"name": "test", "status": "failed", "allow_failure": false}])
            .to_string(),
    );
    e.gh_file(
        "gl-pipelines-green.json",
        &serde_json::json!([{"sha": red}, {"sha": green}]).to_string(),
    );
    e.run(&["watch", "--once"], 0);
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.contains(&format!("Since {}:", &green[..7])), "{body}");
    assert!(
        body.contains(&format!("- {} Break the build", &red[..7])),
        "{body}"
    );
}

/// The hook line that turns main to `file`'s answer when PR `pr` merges.
fn main_becomes(pr: u64, file: &str) -> String {
    format!(
        "[ \"$1 $2 $3\" = \"pr merge {pr}\" ] && cp \"$d/main-ci.{file}\" \"$d/main-ci.json\"\n"
    )
}

#[test]
fn watch_holds_nothing_when_main_ci_cannot_be_read_mid_night() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    main_red(&e);
    e.gh_file(
        "hook.sh",
        "[ \"$1 $2 $3\" = \"pr merge 12\" ] && echo 'HTTP 502' > \"$d/main-ci.fail\"\n",
    );
    e.factory(AUTO);
    e.ctl("pr-100-fix-main-is-red-at-abcdef0-test", "12");
    e.ctl("pr-2-fix-a", "13");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    let out = e.ns().args(["watch", "--max-units", "2"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        units(&v)
            .iter()
            .map(|u| u["issue"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [100, 2],
        "{v}\n{err}"
    );
    assert!(err.contains("warning: cannot read main's CI"), "{err}");
    assert!(events_named(&e, "main_green").is_empty());
    assert_eq!(e.gh_calls().matches("issue create").count(), 1);
}

#[test]
fn watch_logs_a_second_main_red_for_a_new_sha_and_again_after_green() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    let sha = |c: char| c.to_string().repeat(40);
    let at = |c: char, run: (&str, &str, &str)| main_ci_at(&sha(c), &[], &[run]);
    let (fail, pass) = (
        ("test", "COMPLETED", "FAILURE"),
        ("test", "COMPLETED", "SUCCESS"),
    );
    main_red(&e);
    e.gh_file("main-ci.second", &at('2', fail));
    e.gh_file("main-ci.green", &at('3', pass));
    e.gh_file("main-ci.third", &at('4', fail));
    // Each merge moves main on: red at a new head, green, then red again.
    let moves = [(12, "second", 100), (13, "green", 101), (14, "third", 2)];
    let mut hook = String::new();
    for (pr, file, n) in moves {
        hook.push_str(&main_becomes(pr, file));
        // Merging closes the fix issue, as the forge does with a closing PR.
        hook.push_str(&format!(
            "[ \"$1 $2 $3\" = \"pr merge {pr}\" ] && sed -i 's/OPEN/CLOSED/' \"$d/issue-{n}.json\" && sed -i '/\"number\":{n},/d' \"$d/issues.jsonl\"\n"
        ));
    }
    e.gh_file("hook.sh", &hook);
    e.ctl("pr-100-fix-main-is-red-at-abcdef0-test", "12");
    e.ctl("pr-101-fix-main-is-red-at-2222222-test", "13");
    e.ctl("pr-2-fix-a", "14");
    e.ctl("pr-102-fix-main-is-red-at-4444444-test", "15");
    e.queue(
        "build",
        &["pass:commit", "pass:commit", "pass:commit", "pass:commit"],
    );
    for pr in 12..=15 {
        e.gh_file(&format!("checks-{pr}.json"), GREEN);
    }
    let v = e.run(&["watch", "--max-units", "4"], 0);
    assert_eq!(
        units(&v)
            .iter()
            .map(|u| u["issue"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [100, 101, 2, 102],
        "{v}"
    );
    let red: Vec<(String, u64)> = events_named(&e, "main_red")
        .iter()
        .map(|r| {
            (
                r["sha"].as_str().unwrap().to_string(),
                r["issue"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        red,
        [
            (MAIN_SHA.to_string(), 100),
            (sha('2'), 101),
            (sha('4'), 102)
        ]
    );
    let green = events_named(&e, "main_green");
    assert_eq!(green.len(), 1, "{green:?}");
    assert_eq!(green[0]["sha"], sha('3'));
}

#[test]
fn watch_resumes_the_queue_when_main_is_pending_with_nothing_failing() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    e.gh_file(
        "main-ci.pending",
        &main_ci_at(MAIN_SHA, &[], &[("test", "IN_PROGRESS", "")]),
    );
    e.gh_file("hook.sh", &main_becomes(12, "pending"));
    e.ctl("pr-100-fix-main-is-red-at-abcdef0-test", "12");
    e.ctl("pr-2-fix-a", "13");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    let v = e.run(&["watch", "--max-units", "2"], 0);
    assert_eq!(
        units(&v)
            .iter()
            .map(|u| u["issue"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [100, 2],
        "{v}"
    );
    let green = events_named(&e, "main_green");
    assert_eq!(green.len(), 1, "{green:?}");
    assert_eq!(green[0]["state"], "pending");
}

/// A night with main red, #2 ready and the fix issue #100, set up further by `setup`.
fn fix_ends(setup: impl Fn(&Env)) -> (Env, Value) {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    setup(&e);
    // A cap above one: a night that goes on would file or start more.
    let v = e.run(&["watch", "--max-units", "3"], 0);
    (e, v)
}

/// The fix issue's unit, set up by `setup`, ends `outcome` with main still red: the night stops.
fn stops_unfixed(outcome: &str, setup: impl Fn(&Env)) {
    let (e, v) = fix_ends(setup);
    assert_eq!(by_issue(&units(&v)), each(&[100], outcome), "{v}");
    assert_eq!(v["stopped"], "main_red_unfixed", "{v}");
    let unfixed = events_named(&e, "main_red_unfixed");
    assert_eq!(unfixed.len(), 1, "{unfixed:?}");
    assert_eq!(unfixed[0]["sha"], MAIN_SHA);
    assert_eq!(unfixed[0]["issue"], 100);
    assert_eq!(unfixed[0]["outcome"], outcome);
    let calls = e.gh_calls();
    assert_eq!(calls.matches("issue create").count(), 1, "{calls}");
    assert!(!calls.contains("issue edit 2 "), "{calls}");
}

#[test]
fn a_fix_unit_that_ends_unfixed_stops_the_night() {
    stops_unfixed("stuck", |e| e.queue("triage", &["none"]));
    stops_unfixed("split", |e| e.queue("triage", &["split:into #8 and #9."]));
    stops_unfixed("budget", |e| {
        e.factory("[limits]\nbudget_usd = 1.0\n");
        e.ctl(
            "triage.sh",
            &format!("gh issue edit 100 --add-label {READY}\n"),
        );
        e.queue("triage", &["pass:script"]);
        e.queue("build", &["pass:commit"]);
    });
}

#[test]
fn a_fix_unit_cut_short_by_until_stops_the_night_as_until() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    b.ready(2, "Fix a", &["type:fix"], "");
    main_red(&b);
    b.ctl(
        "triage.sh",
        &format!("gh issue edit 100 --add-label {READY}\n"),
    );
    b.queue("triage", &["pass:script"]);
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
    assert_eq!(by_issue(&units(&v)), each(&[100], "budget"), "{v}");
    assert_eq!(v["stopped"], "until", "{v}");
    assert!(events_named(&b, "main_red_unfixed").is_empty());
}

#[test]
fn a_fix_unit_that_ends_stuck_after_until_passed_still_logs_main_red_unfixed() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    b.ready(2, "Fix a", &["type:fix"], "");
    b.queue(&format!("build.{A}"), &["pass:commit"]);
    b.queue(&format!("verify.{A}"), &["pass:script"]);
    let fix = "100-fix-main-is-red-at-abcdef0-test";
    b.queue(&format!("triage.{fix}"), &["none"]);
    let (in_a, go_a) = hold_phase(&b, A, "build");
    let (in_t, go_t) = hold_phase(&b, fix, "triage");
    let mut w = Watching::start(
        b.ns()
            .env("NS_REFILL_MS", "100")
            .env("NS_PHASE_TIMEOUT_MS", "verify=86400000")
            .args(["watch", "--parallel", "2", "--until", "00:30"]),
    );
    w.entered(&in_a);
    main_red(&b);
    w.entered(&in_t);
    // #2 waits for the held lock until --until and ends budget; the fix unit is still running.
    release(&go_a);
    w.line("#2 budget");
    release(&go_t);
    let (code, v, err) = w.finish();
    held.release();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        by_issue(&units(&v)),
        [(2, "budget".into()), (100, "stuck".into())],
        "{v}"
    );
    assert_eq!(v["stopped"], "until", "{v}");
    let unfixed = events_named(&b, "main_red_unfixed");
    assert_eq!(unfixed.len(), 1, "{unfixed:?}");
    assert_eq!(unfixed[0]["outcome"], "stuck");
}

#[test]
fn a_fix_unit_that_ends_unfixed_after_main_turned_green_stops_nothing() {
    let (e, v) = fix_ends(|e| {
        e.queue("triage", &["none", "none"]);
        e.gh_file(
            "main-ci.green",
            &main_ci_at(MAIN_SHA, &[], &[("test", "COMPLETED", "SUCCESS")]),
        );
        // Main turns green, by another hand, while the fix unit runs.
        e.gh_file(
            "hook.sh",
            "[ \"$1 $2 $3\" = \"issue edit 100\" ] && cp \"$d/main-ci.green\" \"$d/main-ci.json\"\n",
        );
    });
    assert_eq!(by_issue(&units(&v)), each(&[2, 100], "stuck"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(events_named(&e, "main_red_unfixed").is_empty());
}

#[test]
fn a_fix_unit_that_ends_unfixed_when_main_ci_cannot_be_read_stops_nothing() {
    let (e, v) = fix_ends(|e| {
        e.queue("triage", &["none", "none"]);
        e.gh_file(
            "hook.sh",
            "[ \"$1 $2 $3\" = \"issue edit 100\" ] && echo 'HTTP 502' > \"$d/main-ci.fail\"\n",
        );
    });
    assert_eq!(by_issue(&units(&v)), each(&[2, 100], "stuck"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(events_named(&e, "main_red_unfixed").is_empty());
}

#[test]
fn a_fix_unit_the_harness_fails_does_not_stop_the_night() {
    let (e, v) = fix_ends(|e| e.queue("triage", &["crash", "crash"]));
    assert_eq!(by_issue(&units(&v)), each(&[100], "harness_failing"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(events_named(&e, "main_red_unfixed").is_empty());
}

#[test]
fn a_stuck_unit_that_is_not_the_fix_does_not_stop_the_night() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue(&format!("build.{A}"), &["pass:commit", "pass:commit"]);
    e.queue(&format!("verify.{A}"), &["fail", "fail"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = Watching::start(e.ns().env("NS_REFILL_MS", "100").args([
        "watch",
        "--parallel",
        "2",
        "--max-units",
        "3",
    ]));
    w.entered(&in_a);
    main_red(&e);
    // #2 ends stuck after the fix issue is filed and started.
    w.line("ns watch: main is red");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        by_issue(&units(&v)),
        [(2, "stuck".into()), (100, "done".into())],
        "{v}"
    );
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert!(events_named(&e, "main_red_unfixed").is_empty());
}

#[test]
fn a_unit_running_when_the_fix_unit_ends_unfixed_finishes_and_nothing_new_starts() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage.100-fix-main-is-red-at-abcdef0-test", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = Watching::start(e.ns().env("NS_REFILL_MS", "100").args([
        "watch",
        "--parallel",
        "2",
        "--max-units",
        "4",
    ]));
    w.entered(&in_a);
    main_red(&e);
    e.ready(3, "Fix b", &["type:fix"], "");
    w.line("the night stops");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        by_issue(&units(&v)),
        [(2, "done".into()), (100, "stuck".into())],
        "{v}"
    );
    assert_eq!(v["stopped"], "main_red_unfixed", "{v}");
    assert_eq!(events_named(&e, "main_red_unfixed").len(), 1);
    assert!(!e.gh_calls().contains("issue edit 3 "));
}

#[test]
fn watch_with_fix_main_first_off_runs_the_queue_while_main_is_red() {
    let e = Env::new();
    e.factory("[queue]\nfix_main_first = false\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    main_red(&e);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
    assert!(!e.gh_calls().contains("issue create"));
    assert!(events_named(&e, "main_red").is_empty());
}

#[test]
fn watch_dry_run_shows_main_ci_and_holds_the_queue_when_it_is_red() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let (v, err) = dry_run(&e);
    assert_eq!(v["main_ci"]["state"], "none", "{v}");
    assert_eq!(v["queue_held"], false);
    assert!(err.contains("main's head has no CI"), "{err}");
    let calls = e.gh_calls();
    assert!(
        calls.contains("api graphql -f query=query($owner: String!, $repo: String!)"),
        "{calls}"
    );
    assert!(calls.contains("-F owner={owner} -F repo={repo}"), "{calls}");

    main_ci(
        &e,
        &["test"],
        &[
            ("test", "COMPLETED", "FAILURE"),
            ("bench", "COMPLETED", "FAILURE"),
        ],
    );
    let (v, err) = dry_run(&e);
    assert_eq!(
        v["main_ci"],
        serde_json::json!({"state": "red", "sha": MAIN_SHA, "failing": ["test"]})
    );
    assert_eq!(v["queue_held"], true);
    assert_eq!(queued(&v), [2], "the queue is only shown held, not emptied");
    assert!(!err.contains("fix_main_first"), "{err}");

    main_ci(
        &e,
        &["test"],
        &[
            ("test", "IN_PROGRESS", ""),
            ("lint", "COMPLETED", "SUCCESS"),
        ],
    );
    let (v, _) = dry_run(&e);
    assert_eq!(
        v["main_ci"],
        serde_json::json!({"state": "pending", "sha": MAIN_SHA})
    );
    assert_eq!(v["queue_held"], false);

    main_ci(&e, &[], &[("test", "COMPLETED", "SUCCESS")]);
    let (v, _) = dry_run(&e);
    assert_eq!(v["main_ci"]["state"], "green");
    assert_eq!(v["queue_held"], false);
}

#[test]
fn watch_dry_run_with_fix_main_first_off_reads_no_ci_and_says_so() {
    let e = Env::new();
    e.factory("[queue]\nfix_main_first = false\n");
    main_ci(&e, &[], &[("test", "COMPLETED", "FAILURE")]);
    let (v, err) = dry_run(&e);
    assert_eq!(v["main_ci"], serde_json::json!({"state": "off"}));
    assert_eq!(v["queue_held"], false);
    assert!(err.contains("fix_main_first is off"), "{err}");
    assert!(!e.gh_calls().contains("graphql"), "{}", e.gh_calls());
}

#[test]
fn a_night_with_fix_main_first_off_says_so_once_and_reads_no_ci() {
    let e = Env::new();
    e.factory("[queue]\nfix_main_first = false\n");
    let out = e.ns().args(["watch", "--once"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert_eq!(err.matches("fix_main_first is off").count(), 1, "{err}");
    assert!(!e.gh_calls().contains("graphql"), "{}", e.gh_calls());
}

#[test]
fn a_night_says_once_at_start_when_main_has_no_ci_and_runs_on_when_it_cannot_read_it() {
    let e = Env::new();
    let night = |e: &Env| {
        let out = e.ns().args(["watch", "--once"]).output();
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{err}");
        err
    };
    let err = night(&e);
    assert_eq!(err.matches("main's head has no CI").count(), 1, "{err}");

    main_ci(&e, &[], &[("test", "COMPLETED", "FAILURE")]);
    let err = night(&e);
    assert!(!err.contains("fix_main_first"), "{err}");

    e.gh_file("main-ci.fail", "HTTP 502");
    let err = night(&e);
    assert!(
        err.contains("warning: cannot read main's CI") && err.contains("HTTP 502"),
        "{err}"
    );
}

#[test]
fn watch_dry_run_fails_when_main_ci_cannot_be_read() {
    let e = Env::new();
    e.gh_file("main-ci.fail", "HTTP 502");
    let out = e.ns().args(["watch", "--dry-run"]).output();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("HTTP 502"), "{err}");
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
    let all = calls.split("issue comment 2").nth(1).expect("comment");
    assert!(all.starts_with(&format!(" --body {DISCLOSED}")), "{all}");
    assert!(all.contains("triage wrote no brief"), "{all}");
    assert!(all.contains("Last artifact"));
}

const HUMAN_MERGE: &str = "needs-human-merge";

/// How every body nightshift posts to the forge opens (skills/ns-contract, Disclosure).
const DISCLOSED: &str = "🤖 *Posted by nightshift, an AI agent.*\n\n";

/// The `pr edit` and `pr comment` calls on PR 12, and whether a label was created.
fn pr_12_calls(e: &Env) -> (Vec<String>, bool) {
    let calls = e.gh_calls();
    let on_pr = calls
        .lines()
        .filter(|l| l.starts_with("pr edit 12") || l.starts_with("pr comment 12"))
        .map(String::from)
        .collect();
    (on_pr, calls.contains("label create"))
}

#[test]
fn watch_done_with_a_pr_labels_it_for_a_human_merge_and_says_why() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:in-review"]);
    let calls = e.gh_calls();
    assert!(
        calls.contains(&format!(
            "label create {HUMAN_MERGE} --color d73a4a --description nightshift left this PR for a human to merge"
        )),
        "{calls}"
    );
    assert_eq!(
        fs::read_to_string(e.ghd.join("pr-labels-12")).unwrap(),
        format!("{HUMAN_MERGE}\n")
    );
    let comment = calls
        .split("pr comment 12 --body ")
        .nth(1)
        .expect("comment");
    assert!(comment.contains("needs a human merge"), "{comment}");
    assert!(comment.contains("merge.policy"), "{comment}");
    assert!(comment.starts_with(DISCLOSED), "{comment}");
}

#[test]
fn watch_done_needing_a_human_merge_labels_the_pr_with_the_merge_reason() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", ".nightshift/rules-baseline.json\n");
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-human"]);
    let calls = e.gh_calls();
    assert!(
        calls.contains(&format!("pr edit 12 --add-label {HUMAN_MERGE}")),
        "{calls}"
    );
    let comment = calls
        .split("pr comment 12 --body ")
        .nth(1)
        .expect("comment");
    assert!(
        comment.contains(
            "changes files that need human review; needs a human merge (.nightshift/rules-baseline.json)"
        ),
        "{comment}"
    );
    assert!(comment.starts_with(DISCLOSED), "{comment}");
}

#[test]
fn watch_done_without_a_pr_leaves_prs_alone() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(v["units"][0]["pr"], Value::Null, "{v}");
    assert_eq!(pr_12_calls(&e), (vec![], false));
    assert!(!e.gh_calls().contains("pr edit"));
}

#[test]
fn watch_takes_the_human_merge_label_from_the_queue_config() {
    let e = Env::new();
    e.factory("[queue]\nhuman_merge_label = \"merge-me\"\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    let calls = e.gh_calls();
    assert!(calls.contains("label create merge-me "), "{calls}");
    assert!(calls.contains("pr edit 12 --add-label merge-me"), "{calls}");
}

#[test]
fn watch_adds_an_existing_human_merge_label_without_recreating_it() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file("label.exists", "");
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    let calls = e.gh_calls();
    assert!(
        calls.contains(&format!("pr edit 12 --add-label {HUMAN_MERGE}")),
        "{calls}"
    );
    assert!(calls.contains("pr comment 12"), "{calls}");
}

#[test]
fn a_pr_that_cannot_be_labelled_still_gets_its_issue_relabelled_and_comment() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file(
        "label.fail",
        "HTTP 403: Resource not accessible by integration",
    );
    e.queue("build", &["pass:commit"]);
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:in-review"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("PR #12 not labelled {HUMAN_MERGE}"))
            && stderr.contains("HTTP 403"),
        "{stderr}"
    );
    let calls = e.gh_calls();
    assert!(!calls.contains("pr edit 12"), "{calls}");
    assert!(calls.contains("pr comment 12"), "{calls}");
}

#[test]
fn a_pr_comment_that_fails_is_reported_and_the_night_goes_on() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file(
        "hook.sh",
        "case \"$*\" in \"pr comment\"*) echo 'HTTP 502' >&2; exit 1 ;; esac\n",
    );
    e.queue("build", &["pass:commit"]);
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("PR #12 not commented") && stderr.contains("HTTP 502"),
        "{stderr}"
    );
    assert!(e
        .gh_calls()
        .contains(&format!("pr edit 12 --add-label {HUMAN_MERGE}")));
}

#[test]
fn watch_stuck_after_ship_leaves_the_pr_alone() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["blocked:CI never started"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck", "{v}");
    assert_eq!(v["units"][0]["pr"], 12, "{v}");
    assert_eq!(pr_12_calls(&e), (vec![], false));
}

#[test]
fn watch_merged_removes_the_human_merge_label_a_past_night_set() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("pr-labels-12", &format!("type:fix\n{HUMAN_MERGE}\n"));
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(
        pr_12_calls(&e),
        (
            vec![format!("pr edit 12 --remove-label {HUMAN_MERGE}")],
            false
        )
    );
    assert_eq!(
        fs::read_to_string(e.ghd.join("pr-labels-12")).unwrap(),
        "type:fix\n"
    );
}

#[test]
fn watch_merged_without_the_label_edits_no_pr() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(pr_12_calls(&e), (vec![], false));
    assert!(e.gh_calls().contains("pr view 12 --json labels"));
}

#[test]
fn a_merged_pr_whose_label_cannot_be_read_is_reported_and_the_issue_still_closes() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file(
        "hook.sh",
        "case \"$*\" in \"pr view 12 --json labels\") echo 'HTTP 502' >&2; exit 1 ;; esac\n",
    );
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("PR #12 still labelled {HUMAN_MERGE}?")),
        "{stderr}"
    );
    assert!(e.gh_calls().contains("issue close 2"));
}

#[test]
fn watch_paused_at_ship_leaves_the_pr_alone() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.ctl("reset", &(NOW + 36_000).to_string());
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["session"]);
    let v = e.run(&["watch", "--once", "--until", "06:30"], 0);
    assert_eq!(v["units"][0]["outcome"], "paused", "{v}");
    assert_eq!(pr_12_calls(&e), (vec![], false));
}

#[test]
fn watch_dry_run_names_the_human_merge_label() {
    let e = Env::new();
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["human_merge_label"], HUMAN_MERGE, "{v}");
    e.factory("[queue]\nhuman_merge_label = \"merge-me\"\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["human_merge_label"], "merge-me", "{v}");
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
    e.queue(
        "review",
        &["blocked:Open after at least 2 fix attempts: C1, I2."],
    );
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
    e.queue(
        "review",
        &["blocked:Open after at least 2 fix attempts: I2."],
    );
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
fn watch_split_moves_the_parent_to_split_label_without_a_comment() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["split:into #8 and #9."]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "split", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:split"]);
    let calls = e.gh_calls();
    assert!(!calls.contains("issue comment 2"), "{calls}");
    assert!(!calls.contains("status:ready-for-human"), "{calls}");
    assert!(!calls.contains("pr edit"), "{calls}");
    assert!(
        calls.contains("label create status:split --color d73a4a --description Split into child issues; closes when they're done"),
        "{calls}"
    );
}

/// Split parent 5 under `status:split` whose sub-issues are 8 and 9, both closed as completed,
/// and parent 6 under the old split label whose triage notes list open child 10.
fn split_parents(e: &Env) {
    e.open_by(
        5,
        "Parent",
        &["type:feat", "status:split"],
        "",
        Some("MEMBER"),
    );
    e.gh_file("subs-5", "8\n9\n");
    e.issue(8, "Child a", "CLOSED");
    e.issue(9, "Child b", "CLOSED");
    e.open_by(
        6,
        "Old",
        &["type:feat", "status:needs-define"],
        "",
        Some("MEMBER"),
    );
    let notes = serde_json::json!({"number": 1, "body": "Children:\n- #10 c\n", "authorAssociation": "MEMBER"});
    let outside =
        serde_json::json!({"number": 1, "body": "Children:\n- #8\n", "authorAssociation": "NONE"});
    e.gh_file("comments-6.jsonl", &format!("{notes}\n{outside}\n"));
    e.issue(10, "Child c", "OPEN");
}

#[test]
fn watch_closes_split_parents_whose_children_are_done_and_sweeps_once() {
    let e = Env::new();
    split_parents(&e);
    let v = e.run(&["watch", "--once"], 0);
    let calls = e.gh_calls();
    assert!(
        calls.contains("issue close 5 --reason completed"),
        "{calls}"
    );
    assert!(
        calls.contains(&format!("issue comment 5 --body {DISCLOSED}Every child")),
        "{calls}"
    );
    assert_eq!(e.labels(5), ["type:feat"]);
    assert_eq!(e.labels(6), ["type:feat", "status:split"]);
    let actions: Vec<_> = v["split_parents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["number"].as_u64().unwrap(), p["action"].as_str().unwrap()))
        .collect();
    assert_eq!(actions, [(5, "close"), (6, "relabel")], "{v}");

    // The sweep ran: the next night leaves the old label alone.
    e.open_by(11, "Design", &["status:needs-define"], "", Some("MEMBER"));
    e.gh_file("subs-11", "10\n");
    fs::write(e.ghd.join("calls"), "").unwrap();
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(11), ["status:needs-define"]);
    assert!(!e.gh_calls().contains("labels=status:needs-define"));
}

/// Unit 2 ready and set to merge on its own PR 12, as the auto-merge tests do.
fn a_unit_that_merges(e: &Env) {
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file(
        "prs.json",
        r#"[{"number":12,"body":"Closes #2","headRefName":"ns/2-fix-a","isCrossRepository":false}]"#,
    );
}

#[test]
fn a_split_read_that_fails_does_not_hold_up_the_night() {
    let e = Env::new();
    a_unit_that_merges(&e);
    split_parents(&e);
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *labels=status:split*) echo 'HTTP 502' >&2; exit 1 ;; esac\n",
    );
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("split parents not checked") && err.contains("HTTP 502"),
        "{err}"
    );
    let calls = e.gh_calls();
    assert!(calls.contains("pr merge 12"), "{calls}");
    assert!(
        calls.contains("issue close 2 --reason completed"),
        "{calls}"
    );
    assert!(!calls.contains("issue close 5"), "{calls}");
    assert_eq!(e.labels(5), ["type:feat", "status:split"]);
    assert!(!e.root.join(".git/ns/split-swept").exists());
}

#[test]
fn a_dry_run_whose_split_read_fails_says_so_and_exits_ok() {
    let e = Env::new();
    split_parents(&e);
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *labels=status:split*) echo 'HTTP 502' >&2; exit 1 ;; esac\n",
    );
    let out = e.ns().args(["watch", "--dry-run"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("split parents not checked") && err.contains("HTTP 502"),
        "{err}"
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["split_parents"], serde_json::json!([]), "{v}");
}

#[test]
fn a_sweep_marker_that_cannot_be_written_leaves_the_sweep_to_run_again() {
    let e = Env::new();
    split_parents(&e);
    // The marker is a link to a directory that is not there, so the write fails and the
    // clone still looks unswept.
    let marker = e.root.join(".git/ns/split-swept");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(e.base.join("missing/marker"), &marker).unwrap();
    let out = e.ns().args(["watch", "--once"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("the split sweep may run again"), "{err}");
    assert_eq!(e.labels(6), ["type:feat", "status:split"]);
    // The next night sweeps again: an old-label parent that turned up since moves.
    e.open_by(11, "Design", &["status:needs-define"], "", Some("MEMBER"));
    e.gh_file("subs-11", "10\n");
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(11), ["status:split"]);
}

#[test]
fn watch_dry_run_lists_split_parents_and_changes_nothing() {
    let e = Env::new();
    split_parents(&e);
    let v = e.run(&["watch", "--dry-run"], 0);
    let parents = &v["split_parents"];
    assert_eq!(parents[0]["number"], 5, "{v}");
    assert_eq!(parents[0]["action"], "close");
    assert_eq!(parents[0]["children"], serde_json::json!([8, 9]));
    assert_eq!(parents[1]["number"], 6);
    assert_eq!(parents[1]["action"], "relabel");
    assert_eq!(parents[1]["children"], serde_json::json!([10]));
    assert_eq!(parents[1]["open"], 1);
    let calls = e.gh_calls();
    assert!(
        !calls.contains("issue close") && !calls.contains("issue edit"),
        "{calls}"
    );
    // A dry run leaves the sweep to the first real night.
    let again = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(again["split_parents"][1]["action"], "relabel");
}

#[test]
fn watch_dry_run_reports_waiting_and_childless_split_parents() {
    let e = Env::new();
    e.open_by(5, "Waiting", &["status:split"], "", Some("MEMBER"));
    e.gh_file("subs-5", "8\n");
    e.issue(8, "c", "OPEN");
    e.open_by(6, "Bare", &["status:split"], "", Some("MEMBER"));
    let v = e.run(&["watch", "--dry-run"], 0);
    let parents = &v["split_parents"];
    assert_eq!(parents[0]["number"], 5, "{v}");
    assert_eq!(parents[0]["action"], "wait");
    assert_eq!(parents[0]["open"], 1);
    assert_eq!(parents[1]["number"], 6);
    assert_eq!(parents[1]["action"], "no_children");
    assert_eq!(parents[1]["children"], serde_json::json!([]));
}

#[test]
fn watch_leaves_the_sweep_pending_when_a_swept_parent_cannot_be_read() {
    let e = Env::new();
    e.open_by(
        6,
        "Old",
        &["type:feat", "status:needs-define"],
        "",
        Some("MEMBER"),
    );
    e.gh_file("subs-6", "99\n");
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(6), ["type:feat", "status:needs-define"]);

    e.gh_file("subs-6", "10\n");
    e.issue(10, "c", "OPEN");
    e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(6), ["type:feat", "status:split"]);
}

#[test]
fn watch_reports_a_split_parent_with_no_children_once_a_night() {
    let e = Env::new();
    e.open_by(5, "Parent", &["status:split"], "", Some("MEMBER"));
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["split:into #8 and #9."]);
    let out = e
        .ns()
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    let said = |n: u64| {
        err.matches(&format!(
            "#{n} is labelled status:split but has no children"
        ))
        .count()
    };
    // Filled at start and again once unit 2 ended: two passes, one report each.
    assert_eq!((said(5), said(2)), (1, 1), "{err}");
    assert_eq!(e.labels(5), ["status:split"]);
}

#[test]
fn watch_closes_a_split_parent_whose_last_child_closed_during_the_run() {
    let e = Env::new();
    e.open_by(5, "Parent", &["status:split"], "", Some("MEMBER"));
    e.gh_file("subs-5", "8\n");
    e.issue(8, "Child", "OPEN");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("triage", &["split:script"]);
    e.ctl(
        "triage.sh",
        "sed -i 's/\"OPEN\"/\"CLOSED\"/' \"$FAKE_GH_DIR/issue-8.json\"\n",
    );
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "split", "{v}");
    assert!(e.gh_calls().contains("issue close 5 --reason completed"));
    assert_eq!(v["split_parents"][0]["number"], 5, "{v}");
    assert_eq!(v["split_parents"][0]["action"], "close", "{v}");
}

#[test]
fn watch_finds_split_children_by_their_part_of_line() {
    let e = Env::new();
    e.open_by(5, "Parent", &["status:split"], "", Some("MEMBER"));
    let child = |n: u64, body: &str, assoc: &str| serde_json::json!({"number": n, "title": "c", "labels": [], "body": body, "authorAssociation": assoc});
    let found = [
        child(8, "Part of #5\n\nbody", "MEMBER"),
        child(9, "Part of #5", "NONE"),
        child(10, "see #5", "MEMBER"),
    ];
    let lines: String = found.iter().map(|c| format!("{c}\n")).collect();
    e.gh_file("search.jsonl", &lines);
    e.issue(8, "c", "CLOSED");
    for n in [9, 10] {
        e.issue(n, "c", "OPEN");
    }
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(
        v["split_parents"][0]["children"],
        serde_json::json!([8]),
        "{v}"
    );
    let calls = e.gh_calls();
    assert!(
        calls.contains("api search/issues --method GET -F q=repo:{owner}/{repo} is:issue in:body \"Part of #5\""),
        "{calls}"
    );
    assert!(calls.contains("issue close 5"), "{calls}");
}

#[test]
fn watch_sends_a_split_parent_with_a_dropped_child_to_a_human() {
    let e = Env::new();
    e.open_by(5, "Parent", &["status:split"], "", Some("MEMBER"));
    e.gh_file("subs-5", "8\n");
    e.gh_file(
        "issue-8.json",
        r#"{"number":8,"title":"c","url":"u","state":"CLOSED","stateReason":"NOT_PLANNED"}"#,
    );
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(e.labels(5), ["status:ready-for-human"]);
    assert_eq!(v["split_parents"][0]["action"], "ready_for_human", "{v}");
    assert!(!e.gh_calls().contains("issue close 5"));
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

const OVERRIDES: &str = "[forge.github]
token_env = \"DEFAULT_TOKEN\"
[forge.github.owners.\"example-org\"]
token_env = \"OWNER_TOKEN\"
[forge.github.repos.\"example-org/backend\"]
token_env = \"REPO_TOKEN\"
";

const OVERRIDE_WARNING: &str = "GH_TOKEN from the environment overrides the config's choice";

#[test]
fn the_token_comes_from_the_table_origin_matches() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.config(OVERRIDES);
    for (origin, want) in [
        ("git@github-work:example-org/backend.git", "repo-tok"),
        ("https://github.com/Example-Org/frontend.git", "owner-tok"),
        ("ssh://git@github.com/someone/else", "default-tok"),
        ("https://github.com/example-org", "default-tok"),
    ] {
        git(&e.root, &["remote", "set-url", "origin", origin]);
        let _ = fs::remove_file(e.ghd.join("tokens"));
        let _ = fs::remove_file(e.ghd.join("calls"));
        let out = e
            .ns()
            .env("DEFAULT_TOKEN", "default-tok")
            .env("OWNER_TOKEN", "owner-tok")
            .env("REPO_TOKEN", "repo-tok")
            .args(["watch", "--dry-run"])
            .assert()
            .code(0)
            .get_output()
            .clone();
        assert_eq!(e.tokens_seen(), [want], "{origin}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!err.contains(OVERRIDE_WARNING), "{err}");
        let checked = e
            .gh_calls()
            .lines()
            .filter(|l| l.starts_with("api repos/") && l.ends_with(".full_name"))
            .count();
        assert_eq!(checked, 0, "{origin}: a dry run checked: {}", e.gh_calls());
    }
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:example-org/backend.git",
        ],
    );
    let _ = fs::remove_file(e.ghd.join("tokens"));
    let gitlab_default = e.script(
        "gl-default.sh",
        &format!("touch {}/gl-default; echo gl", e.base.display()),
    );
    let gitlab_owner = e.script(
        "gl-owner.sh",
        &format!("touch {}/gl-owner; echo gl", e.base.display()),
    );
    e.config(&format!(
        "{OVERRIDES}[forge.gitlab]\ntoken_command = {:?}\n[forge.gitlab.owners.\"example-org\"]\ntoken_command = {:?}\n",
        gitlab_default.to_str().unwrap(),
        gitlab_owner.to_str().unwrap()
    ));
    e.ns()
        .env_remove("GITLAB_TOKEN")
        .env("DEFAULT_TOKEN", "default-tok")
        .env("OWNER_TOKEN", "owner-tok")
        .env("REPO_TOKEN", "repo-tok")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert_eq!(e.tokens_seen(), ["repo-tok"]);
    assert!(
        e.base.join("gl-default").exists(),
        "the other forge's default table ran no command"
    );
    assert!(
        !e.base.join("gl-owner").exists(),
        "a github origin matched a gitlab owner table"
    );
}

#[test]
fn a_set_gh_token_wins_over_a_matching_table_and_warns_once_a_night() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:example-org/backend.git",
        ],
    );
    e.config(OVERRIDES);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    let out = e
        .ns()
        .env("GH_TOKEN", "from-env")
        .env("REPO_TOKEN", "repo-tok")
        .args(["watch", "--once"])
        .assert()
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.matches(OVERRIDE_WARNING).count(), 1, "{err}");
    assert!(
        err.contains("[forge.github.repos.\"example-org/backend\"]"),
        "{err}"
    );
    assert_eq!(e.tokens_seen(), ["from-env"]);
    assert!(!e.calls().is_empty(), "the unit never ran: {err}");
    let checks = e
        .gh_calls()
        .lines()
        .filter(|l| *l == "api repos/example-org/backend --jq .full_name")
        .count();
    assert_eq!(
        checks,
        1,
        "the unit checked the repo again: {}",
        e.gh_calls()
    );

    for args in [&["run", "--issue", "7"][..], &["watch", "--dry-run"][..]] {
        let out = e
            .ns()
            .env("GH_TOKEN", "from-env")
            .args(args)
            .assert()
            .code(0)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(err.matches(OVERRIDE_WARNING).count(), 1, "{args:?}: {err}");
    }
    e.config("");
    let out = e
        .ns()
        .env("GH_TOKEN", "from-env")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains(OVERRIDE_WARNING),
        "no config, nothing to override: {err}"
    );
}

#[test]
fn a_repo_the_token_cannot_read_stops_run_and_watch_with_exit_2() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/frontend.git",
        ],
    );
    e.config(OVERRIDES);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file(
        "repo.fail",
        "HTTP 404: Not Found (https://api.github.com/repos/example-org/frontend)",
    );
    let table = "[forge.github.owners.\"example-org\"]";
    for login in [Some("work-account"), None] {
        match login {
            Some(l) => e.gh_file("login", &format!("{l}\n")),
            None => fs::remove_file(e.ghd.join("login")).unwrap(),
        }
        for args in [&["run", "--issue", "7"][..], &["watch", "--once"][..]] {
            let out = e
                .ns()
                .env("OWNER_TOKEN", FAKE_TOKEN)
                .args(args)
                .assert()
                .code(2)
                .get_output()
                .clone();
            let err = String::from_utf8_lossy(&out.stderr);
            let want = match login {
                Some(l) => {
                    format!("account {l} cannot read example-org/frontend (token from {table})")
                }
                None => format!("the token from {table} cannot read example-org/frontend"),
            };
            assert!(err.contains(&want), "{args:?}: {err}");
            assert!(err.contains("HTTP 404"), "{err}");
            assert!(err.contains("ns doctor"), "{err}");
            assert!(
                err.contains("[forge.github.repos.\"example-org/frontend\"]"),
                "{err}"
            );
            assert!(!err.contains(FAKE_TOKEN), "{err}");
        }
    }
    assert!(e.calls().is_empty(), "a phase ran: {:?}", e.calls());
    assert_eq!(e.labels(2), ["status:ready-for-agent", "type:fix"]);

    e.config("");
    e.gh_file("repo.fail", "");
    let _ = fs::remove_file(e.ghd.join("calls"));
    e.ns().args(["run", "--issue", "7"]).assert().code(0);
    assert!(
        !e.gh_calls().contains(".full_name"),
        "a forge with no table was checked: {}",
        e.gh_calls()
    );
}

fn unreadable_github_repo(e: &Env) {
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/frontend.git",
        ],
    );
    e.config(OVERRIDES);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.gh_file(
        "repo.fail",
        &format!("HTTP 404: Not Found: token {FAKE_TOKEN}"),
    );
}

#[test]
fn a_stale_or_foreign_checked_mark_never_skips_the_probe() {
    let e = Env::new();
    unreadable_github_repo(&e);
    for mark in [
        "1",
        "github:example-org/other",
        "gitlab:example-org/frontend",
    ] {
        for args in [&["run", "--issue", "7"][..], &["watch", "--once"][..]] {
            let out = e
                .ns()
                .env("OWNER_TOKEN", FAKE_TOKEN)
                .env("NS_FORGE_CHECKED", mark)
                .args(args)
                .assert()
                .code(2)
                .get_output()
                .clone();
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(
                err.contains("cannot read example-org/frontend"),
                "{mark} {args:?}: {err}"
            );
        }
    }
    assert!(e.calls().is_empty(), "a phase ran: {:?}", e.calls());
}

#[test]
fn an_unreadable_repo_names_the_environment_token_that_overrides_the_table() {
    let e = Env::new();
    unreadable_github_repo(&e);
    for args in [&["run", "--issue", "7"][..], &["watch", "--once"][..]] {
        let out = e
            .ns()
            .env("GH_TOKEN", FAKE_TOKEN)
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains(
                "the token from GH_TOKEN from the environment, overriding [forge.github.owners.\"example-org\"]"
            ),
            "{args:?}: {err}"
        );
        assert!(err.contains("[token]"), "{err}");
        assert!(!err.contains(FAKE_TOKEN), "{err}");
    }
}

fn checked_seen(e: &Env) -> Vec<String> {
    fs::read_to_string(e.ctrl.join("checked"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn a_forge_with_no_config_marks_nothing_checked_for_its_children() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/backend.git",
        ],
    );
    e.config("");
    e.ns()
        .env_remove("NS_FORGE_CHECKED")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(!e.calls().is_empty());
    assert!(
        checked_seen(&e).iter().all(|c| c == "unset"),
        "{:?}",
        checked_seen(&e)
    );
}

#[test]
fn a_configured_forge_that_was_checked_marks_its_children() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/backend.git",
        ],
    );
    e.config(OVERRIDES);
    e.ns()
        .env_remove("NS_FORGE_CHECKED")
        .env("DEFAULT_TOKEN", "d")
        .env("OWNER_TOKEN", "o")
        .env("REPO_TOKEN", "r")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let seen = checked_seen(&e);
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|c| c == "github:example-org/backend"),
        "{seen:?}"
    );
}

#[test]
fn a_dry_run_probes_nothing_even_when_the_repo_is_unreadable() {
    let e = Env::new();
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example-org/backend.git",
        ],
    );
    e.config(OVERRIDES);
    e.gh_file("repo.fail", "HTTP 404: Not Found");
    e.ns()
        .env("DEFAULT_TOKEN", "d")
        .env("OWNER_TOKEN", "o")
        .env("REPO_TOKEN", "r")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0);
    assert!(!e.gh_calls().contains(".full_name"), "{}", e.gh_calls());
}

#[test]
fn a_forge_host_keeps_another_hosts_origin_on_the_default_token() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(
        &e.root,
        &["remote", "set-url", "origin", "git@gitlab.com:team/p.git"],
    );
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\nhost = \"gitlab.example.com\"\n[forge.gitlab.owners.\"team\"]\ntoken_env = \"GL_TEAM\"\n",
    );
    e.ns()
        .env("GL_DEFAULT", "default-tok")
        .env("GL_TEAM", "team-tok")
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0);
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(!tokens.is_empty());
    assert!(tokens.lines().all(|t| t == "default-tok"), "{tokens}");
}

fn gitlab_host_env_case(origin: &str, host_var: Option<&str>) -> String {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(&e.root, &["remote", "set-url", "origin", origin]);
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\n[forge.gitlab.owners.\"team\"]\ntoken_env = \"GL_TEAM\"\n",
    );
    let mut ns = e.ns();
    ns.env("GL_DEFAULT", "default-tok")
        .env("GL_TEAM", "team-tok")
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST");
    if let Some(h) = host_var {
        ns.env("GITLAB_HOST", h);
    }
    ns.args(["watch", "--dry-run"]).assert().code(0);
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(!tokens.is_empty());
    let first = tokens.lines().next().unwrap().to_string();
    assert!(tokens.lines().all(|t| t == first), "{tokens}");
    first
}

#[test]
fn a_gitlab_host_variable_scopes_the_override_tables_to_its_host() {
    let origin = "git@gitlab.com:team/p.git";
    assert_eq!(
        gitlab_host_env_case(origin, Some("gitlab.example.com")),
        "default-tok"
    );
    assert_eq!(gitlab_host_env_case(origin, Some("gitlab.com")), "team-tok");
    assert_eq!(gitlab_host_env_case(origin, None), "team-tok");
}

#[test]
fn a_gh_host_variable_scopes_the_override_tables_to_its_host() {
    for (gh_host, want) in [
        ("ghe.example.com", "default-tok"),
        ("github.com", "owner-tok"),
    ] {
        let e = Env::new();
        e.ready(2, "Fix a", &["type:fix"], "");
        e.config(OVERRIDES);
        git(
            &e.root,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/example-org/frontend.git",
            ],
        );
        e.ns()
            .env("GH_HOST", gh_host)
            .env("DEFAULT_TOKEN", "default-tok")
            .env("OWNER_TOKEN", "owner-tok")
            .env("REPO_TOKEN", "repo-tok")
            .args(["watch", "--dry-run"])
            .assert()
            .code(0);
        assert_eq!(e.tokens_seen(), [want], "{gh_host}");
    }
}

const ALIAS_WARNING: &str =
    "origin host gitlab-work is not gitlab.example.com: ignoring [forge.gitlab.owners.\"team\"]";

#[test]
fn an_ssh_alias_origin_under_a_forge_host_warns_that_the_override_tables_are_ignored() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(
        &e.root,
        &["remote", "set-url", "origin", "git@gitlab-work:team/p.git"],
    );
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\nhost = \"gitlab.example.com\"\n[forge.gitlab.owners.\"team\"]\ntoken_env = \"GL_TEAM\"\n",
    );
    let out = e
        .ns()
        .env("GL_DEFAULT", "default-tok")
        .env("GL_TEAM", "team-tok")
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.matches(ALIAS_WARNING).count(), 1, "{err}");
    assert!(err.contains("ssh alias for gitlab.example.com"), "{err}");
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(tokens.lines().all(|t| t == "default-tok"), "{tokens}");

    e.config("[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\nhost = \"gitlab.example.com\"\n");
    let out = e
        .ns()
        .env("GL_DEFAULT", "default-tok")
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_HOST")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("ignoring"), "no table was dropped: {err}");
}

#[test]
fn a_token_ns_exported_is_marked_even_when_the_repo_was_not_probed() {
    let e = Env::new();
    e.config(OVERRIDES);
    e.ns()
        .env_remove("NS_FORGE_EXPORTED")
        .env_remove("NS_FORGE_CHECKED")
        .env("DEFAULT_TOKEN", "default-tok")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let exported = fs::read_to_string(e.ctrl.join("exported")).unwrap();
    assert!(!exported.is_empty());
    assert!(exported.lines().all(|l| l == "github:"), "{exported}");
    assert!(
        checked_seen(&e).iter().all(|c| c == "unset"),
        "{:?}",
        checked_seen(&e)
    );
}

#[test]
fn an_origin_with_no_repo_path_uses_the_default_token_and_checks_nothing() {
    let e = Env::new();
    e.config(OVERRIDES);
    e.gh_file("repo.fail", "HTTP 404: Not Found");
    let local = git(&e.root, &["remote", "get-url", "origin"]);
    assert!(local.trim().starts_with('/'), "{local}");
    for remove in [false, true] {
        if remove {
            git(&e.root, &["remote", "remove", "origin"]);
        }
        let _ = fs::remove_file(e.ghd.join("tokens"));
        let _ = fs::remove_file(e.ghd.join("calls"));
        let _ = fs::remove_file(e.ctrl.join("tokens"));
        e.ns()
            .env("DEFAULT_TOKEN", "default-tok")
            .env("OWNER_TOKEN", "owner-tok")
            .env("REPO_TOKEN", "repo-tok")
            .args(["run", "--issue", "7"])
            .assert()
            .code(0);
        assert_eq!(e.tokens_seen(), ["default-tok"], "origin removed: {remove}");
        assert!(
            !e.gh_calls().contains("api repos/"),
            "origin removed: {remove}: {}",
            e.gh_calls()
        );
    }
}

#[test]
fn an_override_naming_an_unset_variable_stops_run_with_exit_2() {
    for (origin, var) in [
        ("https://github.com/example-org/frontend.git", "OWNER_TOKEN"),
        ("https://github.com/example-org/backend.git", "REPO_TOKEN"),
    ] {
        let e = Env::new();
        git(&e.root, &["remote", "set-url", "origin", origin]);
        e.config(OVERRIDES);
        let out = e
            .ns()
            .env("DEFAULT_TOKEN", FAKE_TOKEN)
            .args(["run", "--issue", "7"])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains(&format!("token_env {var} is unset or empty")),
            "{err}"
        );
        assert!(!err.contains(FAKE_TOKEN), "{err}");
        assert!(e.calls().is_empty(), "a phase ran: {:?}", e.calls());
    }
}

#[test]
fn a_gitlab_repo_table_beats_the_nested_group_owner_table() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.com:team/sub/project.git",
        ],
    );
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\n[forge.gitlab.owners.\"team/sub\"]\ntoken_env = \"GL_SUB\"\n[forge.gitlab.repos.\"team/sub/project\"]\ntoken_env = \"GL_REPO\"\n",
    );
    let ns = || {
        let mut c = e.ns();
        c.env("GL_SUB", "owner-tok")
            .env("GL_REPO", FAKE_TOKEN)
            .env_remove("GITLAB_TOKEN")
            .env_remove("GITLAB_HOST");
        c
    };
    ns().args(["watch", "--dry-run"]).assert().code(0);
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(!tokens.is_empty());
    assert!(tokens.lines().all(|t| t == FAKE_TOKEN), "{tokens}");

    e.gh_file("gl-repo.fail", "404 Project Not Found");
    e.gh_file("gl-login", "gl-work");
    let out = ns()
        .args(["watch", "--once"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "account gl-work cannot read team/sub/project (token from [forge.gitlab.repos.\"team/sub/project\"])"
        ),
        "{err}"
    );
    assert!(!err.contains("token from [forge.gitlab.owners"), "{err}");
}

#[test]
fn a_gitlab_nested_group_picks_its_table_and_an_unreadable_project_exits_2() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.com:team/sub/project.git",
        ],
    );
    e.config(
        "[forge.gitlab]\ntoken_env = \"GL_DEFAULT\"\n[forge.gitlab.owners.\"team/sub\"]\ntoken_env = \"GL_SUB\"\n",
    );
    let ns = || {
        let mut c = e.ns();
        c.env("GL_DEFAULT", "default-tok")
            .env("GL_SUB", FAKE_TOKEN)
            .env_remove("GITLAB_TOKEN")
            .env_remove("GITLAB_HOST");
        c
    };
    let out = ns()
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(queued(&v), [2], "{v}");
    assert!(
        !e.glab_calls().contains("team%2Fsub%2Fproject"),
        "a dry run checked the project: {}",
        e.glab_calls()
    );
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(tokens.lines().all(|t| t == FAKE_TOKEN), "{tokens}");
    e.gh_file(
        "gl-repo.fail",
        &format!("404 Project Not Found: token {FAKE_TOKEN}"),
    );
    e.gh_file("gl-login", "gl-work");
    let out = ns()
        .args(["watch", "--once"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("forge gitlab: account gl-work cannot read team/sub/project"),
        "{err}"
    );
    assert!(err.contains("[forge.gitlab.owners.\"team/sub\"]"), "{err}");
    assert!(err.contains("ns doctor"), "{err}");
    assert!(
        err.contains("[forge.gitlab.repos.\"team/sub/project\"]"),
        "{err}"
    );
    assert!(!err.contains(FAKE_TOKEN), "{err}");
    assert!(err.contains("[token]"), "{err}");
    assert!(
        e.glab_calls().contains("api projects/team%2Fsub%2Fproject"),
        "the project was not read by its encoded path: {}",
        e.glab_calls()
    );

    fs::remove_file(e.ghd.join("gl-repo.fail")).unwrap();
    let _ = fs::remove_file(e.ghd.join("glab-calls"));
    ns().args(["watch", "--once"]).assert().code(0);
    assert!(
        e.glab_calls().contains("api projects/team%2Fsub%2Fproject"),
        "a readable project was not checked: {}",
        e.glab_calls()
    );

    let _ = fs::remove_file(e.ghd.join("glab-tokens"));
    let out = ns()
        .env("GITLAB_TOKEN", "from-env")
        .args(["watch", "--dry-run"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    let warning = "GITLAB_TOKEN from the environment overrides the config's choice";
    assert_eq!(err.matches(warning).count(), 1, "{err}");
    assert!(err.contains("[forge.gitlab.owners.\"team/sub\"]"), "{err}");
    let tokens = fs::read_to_string(e.ghd.join("glab-tokens")).unwrap();
    assert!(!tokens.is_empty());
    assert!(tokens.lines().all(|t| t == "from-env"), "{tokens}");
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
    write_exe(&p, "#!/bin/sh\necho \"$0: not here\" >&2\nexit 1\n");
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

/// Fake `systemd-run` that passes the cgroup probe and runs each command in a "scope" logged
/// to `ctrl/scopes`.
fn fake_systemd_run(e: &Env) {
    let c = e.ctrl.display();
    write_exe(
        &e.bin.join("systemd-run"),
        format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --collect) echo 67108864; exit 0 ;;\n    --unit=*) echo \"${{1#--unit=}}\" >> '{c}/scopes' ;;\n    --) shift; break ;;\n  esac\n  shift\ndone\nexec \"$@\"\n"
        ),
    );
}

/// Fake `systemctl` whose `show` runs the shell `report` for each ask, with the scope's ask
/// count so far in `$n` and its directory of state in `$d`; it prints the `ActiveState` and
/// `Result` lines. `reset-failed` logs the scope, how many asks it had and the state the last
/// one reported to `ctrl/reset`.
fn fake_systemctl(e: &Env, report: &str) {
    let c = e.ctrl.display();
    write_exe(
        &e.bin.join("systemctl"),
        format!(
            "#!/bin/sh\nd='{c}/asks'; mkdir -p \"$d\"\ncase \"$2\" in\n  show) n=$(( $(cat \"$d/$3\" 2>/dev/null || echo 0) + 1 )); echo $n > \"$d/$3\"\n    {report}\n    printf 'ActiveState=%s\\nResult=%s\\n' \"$state\" \"$result\"; echo \"$state\" > \"$d/$3.last\" ;;\n  reset-failed) echo \"$3 $(cat \"$d/$3\" 2>/dev/null || echo 0) $(cat \"$d/$3.last\" 2>/dev/null)\" >> '{c}/reset' ;;\nesac\n"
        ),
    );
}

/// Fake `systemd-run` and `systemctl` where `systemctl show` reports `first`
/// (`ActiveState Result`) for a scope's first `n` asks and `then` after.
fn fake_systemd(e: &Env, n: u32, first: &str, then: &str) {
    fake_systemd_run(e);
    let (fs, fr) = first.split_once(' ').unwrap();
    let (ts, tr) = then.split_once(' ').unwrap();
    fake_systemctl(
        e,
        &format!(
            "if [ $n -le {n} ]; then state={fs}; result={fr}; else state={ts}; result={tr}; fi"
        ),
    );
}

/// Fake `systemd-run` and `systemctl` where a scope reports `active success` until `secs`
/// seconds after the first ask about it, then `failed oom-kill`.
fn fake_systemd_failing_after(e: &Env, secs: u32) {
    fake_systemd_run(e);
    fake_systemctl(
        e,
        &format!(
            "now=$(date +%s); [ -f \"$d/$3.t\" ] || echo $now > \"$d/$3.t\"\n    if [ $(( now - $(cat \"$d/$3.t\") )) -lt {secs} ]; then state=active; result=success; else state=failed; result=oom-kill; fi"
        ),
    );
}

/// Each scope the run started was reset once, after systemd had been asked about it more
/// than `asks` times.
fn assert_every_scope_reset(e: &Env, asks: u32) {
    let scopes = recorded(e, "scopes");
    let reset = fs::read_to_string(e.ctrl.join("reset")).unwrap_or_default();
    assert!(!scopes.is_empty());
    for unit in scopes.lines() {
        let lines: Vec<&str> = reset
            .lines()
            .filter(|l| l.split(' ').next() == Some(unit))
            .collect();
        assert_eq!(lines.len(), 1, "{unit} reset: {reset}");
        let n: u32 = lines[0].split(' ').nth(1).unwrap().parse().unwrap();
        assert!(
            n > asks,
            "{unit} reset after {n} asks, want more than {asks}"
        );
    }
}

#[test]
fn every_capped_phase_and_gate_scope_is_reset_after_it_exits_zero_or_not() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[limits]\nmemory_mb = 64\n[phases.build]\ngate = \"test -f {f} || {{ touch {f}; exit 1; }}\"\n",
        f = flag.display()
    ));
    fake_systemd(&e, 0, "active success", "inactive success");
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(memory_cap(&e)["via"], "systemd-run");
    assert_eq!(gate_events(&e)[0]["green"], false);
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_capped_phase_that_times_out_still_resets_its_scope() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    fake_systemd(&e, 0, "active success", "inactive success");
    e.ctl("build.sh", "true\n");
    e.queue("build", &["pass:script+sleep", "pass:commit"]);
    let v = run_with_phase_timeout(&e, 0);
    assert_eq!(build_event(&e, 1)["reason"], "timed out after 1.5 s", "{v}");
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_capped_gate_killed_by_a_signal_still_resets_its_scope() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n[phases.build]\ngate = \"kill -9 $$\"\n");
    fake_systemd(&e, 0, "active success", "inactive success");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate was killed by SIGKILL"
    );
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_capped_phase_whose_run_process_errors_on_a_stop_still_resets_its_scope() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    fake_systemd(&e, 0, "active success", "inactive success");
    let f = blocking_build(&e);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_capped_gate_whose_run_process_errors_on_a_stop_still_resets_its_scope() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    e.factory(&format!(
        "[limits]\nmemory_mb = 64\n[phases.build]\ngate = {:?}\n",
        blocks(&f)
    ));
    fake_systemd(&e, 0, "active success", "inactive success");
    e.queue("build", &["pass:commit"]);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_scope_that_fails_after_the_settle_deadline_is_not_an_overrun_but_is_still_reset() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    // Active past the 3 s ns waits for the result and failed before the 10 s it waits to
    // release the scope: the late failure is cleared, not reported.
    fake_systemd_failing_after(&e, 6);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_every_scope_reset(&e, 0);
    for line in recorded(&e, "reset").lines() {
        assert!(line.ends_with(" failed"), "reset before it failed: {line}");
    }
}

#[test]
fn an_oom_kill_seen_while_the_scope_deactivates_is_detected_and_reset_once_it_fails() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    fake_systemd(&e, 5, "deactivating oom-kill", "failed oom-kill");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "triage is out of attempts (2): the last attempt exceeded the 64 MB memory limit",
        "{v}"
    );
    assert_every_scope_reset(&e, 5);
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
    write_exe(&p, "#!/bin/sh\necho max\n");
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
    write_exe(
        &p,
        "#!/bin/sh\nfor a; do [ \"$a\" = OOMPolicy=stop ] && { echo 'Unknown assignment' >&2; exit 1; }; done\necho 67108864\n",
    );
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
    write_exe(
        &hook,
        "#!/bin/sh\nwhile read old new ref; do\n  [ \"$ref\" = refs/heads/nightshift/quality ] && { echo declined >&2; exit 1; }\ndone\nexit 0\n",
    );
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
fn a_merge_never_checks_out_main_in_the_unit_worktree() {
    let e = auto_merge_env();
    // The main checkout is off main, so nothing stops a checkout of main in the unit worktree.
    git(&e.root, &["checkout", "-qb", "elsewhere"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
    assert!(!e.worktree(UNIT).exists());
    let calls = e.gh_calls();
    assert!(
        calls.contains(&format!(
            "api repos/{{owner}}/{{repo}}/git/refs/heads/ns/{UNIT} -X DELETE"
        )),
        "{calls}"
    );
    git(&e.root, &["checkout", "-q", "main"]);
}

#[test]
fn a_remote_branch_github_will_not_delete_is_logged_and_the_merge_stands() {
    let e = auto_merge_env();
    e.gh_file("branch-delete.fail", "HTTP 422: Reference does not exist");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let ev = &run_events(&e)[event_at(&e, |v| v["event"] == "branch_not_deleted")];
    assert_eq!(ev["pr"], 12);
    assert_eq!(ev["branch"], format!("ns/{UNIT}"));
    assert!(
        ev["error"]
            .as_str()
            .unwrap()
            .contains("Reference does not exist"),
        "{ev}"
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
fn clean_removes_the_tmpdirs_a_merged_unit_kept() {
    let e = Env::new();
    unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    unit_with_pr(&e, 4, "4-open", 40, "OPEN", "MERGED");
    for unit in ["3-done", "4-open"] {
        fs::create_dir_all(unit_tmp(&e, unit).join("build-1")).unwrap();
        fs::write(unit_tmp(&e, unit).join("build-1/scratch"), "x\n").unwrap();
    }
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert!(!unit_tmp(&e, "3-done").exists());
    assert!(unit_tmp(&e, "4-open").join("build-1/scratch").is_file());
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

/// A merged unit's worktree left on main, as `gh pr merge --delete-branch` left it: the main
/// checkout is on another branch and the unit's branch is gone.
fn unit_left_on_main(e: &Env, n: u64, unit: &str, pr: u64) -> PathBuf {
    unit_left_on_main_as(e, n, unit, pr, "CLOSED", "MERGED")
}

fn unit_left_on_main_as(
    e: &Env,
    n: u64,
    unit: &str,
    pr: u64,
    issue_state: &str,
    pr_state: &str,
) -> PathBuf {
    let wt = unit_with_pr(e, n, unit, pr, issue_state, pr_state);
    e.gh_file(&format!("pr-{pr}.ref"), &format!("ns/{unit}"));
    git(&e.root, &["checkout", "-qb", "elsewhere"]);
    git(&wt, &["checkout", "-q", "main"]);
    git(&wt, &["branch", "-q", "-D", &format!("ns/{unit}")]);
    wt
}

#[test]
fn clean_detaches_and_removes_a_merged_worktree_left_on_main() {
    let e = Env::new();
    let wt = unit_left_on_main(&e, 3, "3-done", 30);
    let v = e.run(&["clean", "--dry-run"], 0);
    assert_eq!(units_in(&v["planned"]), ["3-done"], "{v}");
    assert_eq!(v["planned"][0]["detached"], "main", "{v}");
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert!(!wt.exists());
    let ev = &cleanup_events(&e)[0];
    assert_eq!(
        (ev["removed"].as_bool(), ev["detached"].as_str()),
        (Some(true), Some("main")),
        "{ev}"
    );
    assert_eq!(quality_records(&e)[0]["unit"], "3-done");
    git(&e.root, &["checkout", "-q", "main"]);
}

#[test]
fn clean_removes_the_tmpdirs_of_a_merged_worktree_left_on_main() {
    let e = Env::new();
    unit_left_on_main(&e, 3, "3-done", 30);
    fs::create_dir_all(unit_tmp(&e, "3-done").join("build-1")).unwrap();
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert!(!unit_tmp(&e, "3-done").exists());
    git(&e.root, &["checkout", "-q", "main"]);
}

#[test]
fn clean_keeps_a_worktree_on_main_whose_unit_is_not_proven_merged() {
    // Only one worktree can hold main, so each case gets its own repo.
    let cases = [
        ("3-open-issue", "OPEN", "MERGED", None, "issue #3 is open"),
        ("3-open-pr", "CLOSED", "OPEN", None, "PR #30 is OPEN"),
        (
            "3-elsewhere",
            "CLOSED",
            "MERGED",
            Some("someone/branch"),
            "PR #30 is from branch \"someone/branch\", not ns/3-elsewhere",
        ),
    ];
    for (unit, issue_state, pr_state, head_ref, reason) in cases {
        let e = Env::new();
        let wt = unit_left_on_main_as(&e, 3, unit, 30, issue_state, pr_state);
        if let Some(r) = head_ref {
            e.gh_file("pr-30.ref", r);
        }
        let head = git(&e.root, &["rev-parse", "HEAD"]);
        let v = e.run(&["clean"], 0);
        assert_eq!(reasons(&v)[unit], reason, "{v}");
        assert_eq!(units_in(&v["removed"]), Vec::<String>::new(), "{v}");
        assert!(wt.is_dir(), "{unit}");
        assert_eq!(git(&wt, &["symbolic-ref", "HEAD"]), "refs/heads/main");
        assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), head);
    }
}

#[test]
fn clean_keeps_a_worktree_on_main_with_uncommitted_tracked_changes() {
    let e = Env::new();
    let wt = unit_left_on_main(&e, 3, "3-done", 30);
    fs::write(wt.join("README"), "edited\n").unwrap();
    let v = e.run(&["clean"], 0);
    assert_eq!(
        reasons(&v)["3-done"],
        "uncommitted changes to tracked files (1 paths)",
        "{v}"
    );
    assert_eq!(git(&wt, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(fs::read_to_string(wt.join("README")).unwrap(), "edited\n");
}

#[test]
fn clean_leaves_a_worktree_on_main_outside_the_unit_worktree_dir_alone() {
    let e = Env::new();
    git(&e.root, &["checkout", "-qb", "elsewhere"]);
    let other = e.base.join("3-done");
    git(
        &e.root,
        &["worktree", "add", "-q", other.to_str().unwrap(), "main"],
    );
    e.issue(3, "3-done", "CLOSED");
    let v = e.run(&["clean"], 0);
    assert_eq!(v["status"], "nothing-to-clean", "{v}");
    assert_eq!(v["kept"], serde_json::json!([]), "{v}");
    assert!(other.is_dir());
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
    fs::create_dir_all(unit_tmp(&e, "3-done").join("build-1")).unwrap();
    let v = e.run(&["clean"], 1);
    let reason = &reasons(&v)["3-done"];
    assert!(
        reason.starts_with("worktree removed, branch ns/3-done kept: "),
        "{v}"
    );
    assert!(!done.exists());
    assert!(branch_exists(&e, "3-done"));
    assert!(
        !unit_tmp(&e, "3-done").exists(),
        "a failed branch deletion kept it"
    );
}

#[test]
fn clean_reports_a_tmpdir_it_could_not_remove() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let tmp = unit_tmp(&e, "3-done");
    fs::write(&tmp, "a file, not a directory\n").unwrap();
    let v = e.run(&["clean"], 1);
    let reason = &reasons(&v)["3-done"];
    let kept = format!("worktree removed, {} kept: ", tmp.display());
    assert!(reason.starts_with(&kept), "{v}");
    assert!(!done.exists());
    assert!(!branch_exists(&e, "3-done"));
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

/// Unit 2 is ready and #5 sits in the backlog, untriaged: the night's first fill starts unit 2
/// and leaves #5, since the queue had a ready issue. A slot left free is filled every
/// `refill_ms`.
fn a_long_unit_beside_the_backlog(e: &Env, refill_ms: u64, args: &[&str]) -> Watching {
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    Watching::start(
        e.ns()
            .env("NS_REFILL_MS", refill_ms.to_string())
            .arg("watch")
            .args(args),
    )
}

/// The `gh` calls watch made while it sat for `wait` with only a held unit running.
fn gh_calls_during(e: &Env, wait: Duration) -> String {
    let before = e.gh_calls().len();
    thread::sleep(wait);
    e.gh_calls()[before..].to_string()
}

#[test]
fn an_idle_slot_triages_the_backlog_and_starts_it_while_a_unit_runs() {
    let e = Env::new();
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "build");
    let w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    // Unit 5's build begins while unit 2's is still held.
    w.entered(&in_e);
    release(&go_a);
    release(&go_e);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 5], "done"), "{v}");
    assert_eq!(e.triage_only_runs(), 1);
    let started_5 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 5);
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    assert!(started_5 < ended_2, "{:?}", run_events(&e));
}

/// Poll until a refill has listed issues, then return the `gh` calls made from there for `wait`.
fn refill_calls_during(e: &Env, wait: Duration) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    let before = e.gh_calls().len();
    while !e.gh_calls()[before..].contains("issues?state=open") {
        assert!(Instant::now() < deadline, "no refill listed issues");
        thread::sleep(Duration::from_millis(20));
    }
    let start = e.gh_calls().len();
    thread::sleep(wait);
    e.gh_calls()[start..].to_string()
}

#[test]
fn an_idle_slot_with_nothing_to_start_does_not_spin() {
    let e = Env::new();
    // #5's triage leaves it untriaged: nothing comes out ready.
    e.queue("triage.5-fix-e", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = a_long_unit_beside_the_backlog(&e, 500, &["--parallel", "2"]);
    w.entered(&in_a);
    w.line("ns watch: triage #5");
    let wait = Duration::from_millis(1500);
    let calls = refill_calls_during(&e, wait);
    let listings = calls.matches("issues?state=open").count();
    // At most one fill each 500 ms, two listings each, and one fill under way at either end.
    let most = 2 * (wait.as_millis() as usize / 500 + 2);
    assert!(
        listings <= most,
        "{listings} listings in {wait:?}:\n{calls}"
    );
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
    assert_eq!(e.triage_only_runs(), 1);
}

#[test]
fn an_idle_refill_skips_the_worktree_cleanup() {
    let e = Env::new();
    e.queue("triage.5-fix-e", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    w.line("ns watch: triage #5");
    let calls = refill_calls_during(&e, Duration::from_millis(1000));
    for view in ["issue view", "pr view"] {
        assert!(!calls.contains(view), "{view} on a refill:\n{calls}");
    }
    release(&go_a);
    let (code, _, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
}

#[test]
fn a_failed_listing_on_a_refill_does_not_fail_the_night() {
    let e = Env::new();
    e.queue("triage.5-fix-e", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    w.line("ns watch: triage #5");
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *issues?state=open*)\n\
           if [ -f \"$d/boom\" ]; then echo 'HTTP 502' >&2; touch \"$d/failed\"; exit 1; fi ;;\n\
         esac\n",
    );
    e.gh_file("boom", "");
    w.line("refill failed");
    assert!(e.ghd.join("failed").exists());
    fs::remove_file(e.ghd.join("boom")).unwrap();
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
}

#[test]
fn a_claim_that_fails_on_a_refill_does_not_spend_a_max_units_claim() {
    let e = Env::new();
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *\"issue edit 5 \"*\"--add-label status:in-progress\"*)\n\
           if [ -f \"$d/boom\" ]; then echo 'HTTP 502' >&2; echo x >> \"$d/failed\"; exit 1; fi ;;\n\
         esac\n",
    );
    e.gh_file("boom", "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "build");
    let w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2", "--max-units", "2"]);
    w.entered(&in_a);
    let deadline = Instant::now() + Duration::from_secs(60);
    while fs::read_to_string(e.ghd.join("failed")).map_or(0, |s| s.lines().count()) < 3 {
        assert!(Instant::now() < deadline, "the claim never failed 3 times");
        thread::sleep(Duration::from_millis(20));
    }
    fs::remove_file(e.ghd.join("boom")).unwrap();
    w.entered(&in_e);
    release(&go_a);
    release(&go_e);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 5], "done"), "{v}");
}

#[test]
fn a_claim_whose_verifying_read_fails_on_a_refill_is_put_back_to_ready() {
    let e = Env::new();
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    e.gh_file(
        "hook.sh",
        "case \"$*\" in\n\
         *\"issue edit 5 \"*\"--add-label status:in-progress\"*) touch \"$d/edited\" ;;\n\
         *\"issue view 5 --json labels\"*)\n\
           if [ -f \"$d/edited\" ] && [ ! -f \"$d/failed\" ]; then echo 'HTTP 502' >&2; touch \"$d/failed\"; exit 1; fi ;;\n\
         esac\n",
    );
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "build");
    let w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    let put_back =
        "issue edit 5 --remove-label status:in-progress --add-label status:ready-for-agent";
    let deadline = Instant::now() + Duration::from_secs(60);
    while !e.gh_calls().contains(put_back) {
        assert!(
            Instant::now() < deadline,
            "#5 was never put back:\n{}",
            e.gh_calls()
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(e.ghd.join("failed").exists());
    w.entered(&in_e);
    release(&go_a);
    release(&go_e);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 5], "done"), "{v}");
}

#[test]
fn a_claim_whose_unit_cannot_start_on_a_refill_does_not_spend_a_max_units_claim() {
    let e = Env::new();
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_t, go_t) = hold_phase(&e, "5-fix-e", "triage");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "build");
    let mut w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2", "--max-units", "2"]);
    w.entered(&in_a);
    w.entered(&in_t);
    let night = fs::read_dir(e.root.join(".git/ns"))
        .unwrap()
        .map(|d| d.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("watch-")
        })
        .unwrap();
    // A directory where unit 5's result file goes: spawning its `ns run` fails.
    let blocks: Vec<_> = (2..60)
        .map(|n| night.join(format!("result-5-{n}.json")))
        .collect();
    for b in &blocks {
        fs::create_dir(b).unwrap();
    }
    release(&go_t);
    w.line("refill failed");
    w.line("refill failed");
    for b in &blocks {
        let _ = fs::remove_dir(b);
    }
    w.entered(&in_e);
    release(&go_a);
    release(&go_e);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 5], "done"), "{v}");
}

#[test]
fn a_hold_that_cannot_be_written_on_a_refill_fails_the_night() {
    let e = Env::new();
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("triage.5-fix-e", &["limit"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "triage");
    let w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    w.entered(&in_e);
    let night = fs::read_dir(e.root.join(".git/ns"))
        .unwrap()
        .map(|d| d.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("watch-")
        })
        .unwrap();
    // A directory where the hold's temp file goes: the hold write on the refill fails.
    fs::create_dir(night.join("hold.tmp")).unwrap();
    release(&go_e);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_ne!(code, Some(0), "{v}\n{err}");
    assert!(err.contains("cannot write"), "{err}");
    assert!(!err.contains("refill failed"), "{err}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
}

#[test]
fn no_refill_while_every_slot_is_busy_or_no_claim_may_start() {
    for (args, done) in [
        (&["--parallel", "1"][..], &[2, 3][..]),
        (&["--parallel", "2", "--max-units", "1"], &[2]),
    ] {
        let e = Env::new();
        // A second ready issue: the night's first fill meets `--max-units`, before unit 2 builds.
        e.ready(3, "Fix b", &["type:fix"], "");
        let (in_a, go_a) = hold_phase(&e, A, "build");
        let w = a_long_unit_beside_the_backlog(&e, 100, args);
        w.entered(&in_a);
        let calls = gh_calls_during(&e, Duration::from_millis(1000));
        assert_eq!(calls, "", "{args:?}");
        release(&go_a);
        let (code, v, err) = w.finish();
        assert_eq!(code, Some(0), "{err}");
        assert_eq!(by_issue(&units(&v)), each(done, "done"), "{v}");
    }
}

#[test]
fn no_refill_while_a_usage_limit_pause_holds_a_free_slot() {
    let e = Env::new();
    e.ready(3, "Fix b", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue(&format!("build.{A}"), &["limit", "pass:commit"]);
    e.queue("triage.5-fix-e", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "3", "--until", "06:30"]);
    w.entered(&in_b);
    w.entered(&in_a);
    release(&go_a);
    w.line("ns watch: #2 paused on a usage limit until");
    // Unit 3's build stays held, so the pause lasts; a slot is free and #5 is untriaged.
    let triaged = e.triage_only_runs();
    let calls = gh_calls_during(&e, Duration::from_millis(1000));
    assert_eq!(calls, "");
    assert_eq!(e.triage_only_runs(), triaged, "{:?}", e.calls());
    release(&go_b);
    let (code, _, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
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

/// A build that records its CPU variables to `seen` and commits, and a build gate that records
/// them too. Returns `seen`, one `<who>:<NS_CPUS>:<RUST_TEST_THREADS>:<CARGO_BUILD_JOBS>` line
/// per build and gate.
fn cpus_seen(e: &Env) -> PathBuf {
    let seen = e.ctrl.join("cpus-seen");
    let dump = |who: &str| {
        format!(
            "echo \"{who}:${{NS_CPUS-unset}}:${{RUST_TEST_THREADS-unset}}:${{CARGO_BUILD_JOBS-unset}}\" >> {}\n",
            seen.display()
        )
    };
    build_only(e, "");
    e.ctl("ci.sh", &dump("gate"));
    e.ctl(
        "build.sh",
        &format!(
            "{}date >> built.txt && git add built.txt && git -c user.name=f -c user.email=f@f commit -qm built",
            dump("build")
        ),
    );
    e.queue("build", &["pass:script"]);
    seen
}

#[test]
fn run_cpus_sets_all_three_variables_for_phases_and_gates_and_logs_them() {
    let e = Env::new();
    let seen = cpus_seen(&e);
    let v = e.run(&["run", "--issue", "7", "--cpus", "3"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        fs::read_to_string(&seen).unwrap(),
        "build:3:3:3\ngate:3:3:3\n"
    );
    let start = &events_named(&e, "start")[0];
    assert_eq!(
        start["cpus"],
        serde_json::json!({
            "NS_CPUS": {"value": "3", "source": "computed"},
            "RUST_TEST_THREADS": {"value": "3", "source": "computed"},
            "CARGO_BUILD_JOBS": {"value": "3", "source": "computed"},
        }),
        "{start}"
    );
}

#[test]
fn run_without_cpus_sets_none_of_the_variables() {
    let e = Env::new();
    let seen = cpus_seen(&e);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        fs::read_to_string(&seen).unwrap(),
        "build:unset:unset:unset\ngate:unset:unset:unset\n"
    );
    assert!(events_named(&e, "start")[0]["cpus"].is_null());
}

#[test]
fn run_cpus_passes_a_variable_already_set_through_unchanged() {
    let vars = ["NS_CPUS", "RUST_TEST_THREADS", "CARGO_BUILD_JOBS"];
    for (i, set) in vars.iter().enumerate() {
        let e = Env::new();
        let seen = cpus_seen(&e);
        let out = e
            .ns()
            .args(["run", "--issue", "7", "--cpus", "2"])
            .env(set, "16")
            .output();
        assert!(out.status.success(), "{set}: {out:?}");
        let mut values = ["2", "2", "2"];
        values[i] = "16";
        let line = |who: &str| format!("{who}:{}:{}:{}\n", values[0], values[1], values[2]);
        assert_eq!(
            fs::read_to_string(&seen).unwrap(),
            format!("{}{}", line("build"), line("gate")),
            "{set}"
        );
        let cpus = &events_named(&e, "start")[0]["cpus"];
        for (j, var) in vars.iter().enumerate() {
            let (value, source) = if i == j {
                ("16", "inherited")
            } else {
                ("2", "computed")
            };
            assert_eq!(
                cpus[var],
                serde_json::json!({"value": value, "source": source}),
                "{set}: {var}"
            );
        }
    }
}

#[test]
fn run_cpus_reaches_the_worktree_setup_commands() {
    let e = Env::new();
    let seen = e.ctrl.join("setup-seen");
    let cmd = format!(
        "echo \"${{NS_CPUS-unset}}:${{RUST_TEST_THREADS-unset}}:${{CARGO_BUILD_JOBS-unset}}\" >> {}",
        seen.display()
    );
    e.factory(&format!("[worktree]\nsetup = {:?}\n", [cmd]));
    let v = e.run(&["run", "--issue", "7", "--cpus", "3"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(fs::read_to_string(&seen).unwrap(), "3:3:3\n");
}

#[test]
fn run_cpus_must_be_a_number_of_at_least_one() {
    let e = Env::new();
    for bad in ["0", "x"] {
        e.ns()
            .args(["run", "--issue", "7", "--cpus", bad])
            .assert()
            .code(2)
            .stderr(predicates::str::contains("--cpus"));
    }
    assert!(e.calls().is_empty(), "{:?}", e.calls());
}

#[test]
fn watch_gives_each_unit_its_share_of_the_cores() {
    let cores = std::thread::available_parallelism().unwrap().get();
    for parallel in [1, 2, 3, cores + 1] {
        let e = Env::new();
        two_ready(&e);
        let seen = e.ctrl.join("cpus-seen");
        e.ctl(
            "build.sh",
            &format!(
                "echo \"$NS_CPUS:$CARGO_BUILD_JOBS\" >> {}\n",
                seen.display()
            ),
        );
        e.queue("build", &["pass:script", "pass:script"]);
        let p = parallel.to_string();
        let v = e.run(&["watch", "--parallel", &p], 0);
        assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
        let share = (cores / parallel).max(1);
        assert_eq!(
            fs::read_to_string(&seen).unwrap(),
            format!("{share}:{share}\n{share}:{share}\n"),
            "--parallel {parallel} on {cores} cores"
        );
    }
}

#[test]
fn watch_parallel_settles_the_other_unit_when_one_ends_without_a_result() {
    let e = Env::new();
    two_ready(&e);
    // A file where unit 2's worktree goes makes its ns run fail.
    fs::create_dir(e.base.join("myrepo.worktrees")).unwrap();
    fs::write(e.base.join("myrepo.worktrees/2-fix-a"), "").unwrap();
    let out = e.ns().args(["watch", "--parallel", "2"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(
        err.contains("is not the worktree for branch ns/2-fix-a"),
        "{err}"
    );
    assert!(err.contains("ns run --issue 2 ended with exit 1"), "{err}");
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
            .contains("ns run --issue 2 ended with exit 1"),
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

/// Make the repo the nightshift source, `cli/Cargo.toml` naming the package `nightshift`, put the
/// fake cargo on PATH, and return the commit pushed, which the `ns` under test counts as its
/// build commit.
fn nightshift_repo(e: &Env) -> String {
    copy_exe(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/factory/cargo"),
        &e.bin.join("cargo"),
    );
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
    // The check ran the new binary's dry run with the hand-off's own arguments, on a copy of
    // that state.
    let dry = fs::read_to_string(e.ctrl.join("dry-run-args")).unwrap();
    let dry = dry.trim();
    assert!(
        dry.starts_with("watch --until 06:30 --max-units 2 --resume-night ")
            && dry.ends_with(" --dry-run"),
        "{dry}"
    );
    let probe = dry.rsplit(' ').nth(1).unwrap();
    assert!(probe != state, "{dry}");
    // The build went to the staging directory, locked, and the exec'd binary is the staged copy
    // named for the commit, not the running ns.
    let stage = fs::canonicalize(e.root.join(".git/ns/self-update")).unwrap();
    let calls = fs::read_to_string(e.ctrl.join("cargo-calls")).unwrap();
    assert!(calls.starts_with("build --release --locked "), "{calls}");
    assert!(
        calls
            .trim()
            .ends_with(&format!("--target-dir {}", stage.join("target").display())),
        "{calls} vs {}",
        stage.display()
    );
    let staged = PathBuf::from(fs::read_to_string(e.ctrl.join("staged")).unwrap().trim());
    assert_eq!(staged, stage.join(format!("ns-{to}")));
    assert_ne!(staged, common::ns_path());
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
fn the_carried_deadline_beats_the_new_binarys_own() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let w = Watching::start(watch_from(&e, &from).args(["watch", "--until", "06:30"]));
    w.entered(&in_a);
    land(&e, "cli/x", "new\n");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    // The stub drops --until when it execs, so only the carried deadline can be 06:30.
    let args = fs::read_to_string(e.ctrl.join("handed-off")).unwrap();
    assert!(args.contains("--until 06:30"), "{args}");
    assert_eq!(v["until"], "2026-10-09T06:30:00+00:00", "{v}");
}

/// A carried night with nothing in it, for a `--resume-night` test.
fn empty_night(e: &Env) -> PathBuf {
    let path = e.root.join("night.json");
    fs::write(&path, r#"{"version": 1}"#).unwrap();
    path
}

#[test]
fn a_resumed_night_does_not_say_the_main_ci_notice_again() {
    let e = Env::new();
    let path = empty_night(&e);
    let out = e
        .ns()
        .args(["watch", "--once", "--resume-night"])
        .arg(&path)
        .output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("fix_main_first"), "{err}");
}

#[test]
fn a_stop_that_landed_during_the_hand_off_still_stops_the_new_binary() {
    let e = Env::new();
    two_ready(&e);
    let path = empty_night(&e);
    let out = e
        .ns()
        .args(["watch", "--resume-night"])
        .arg(&path)
        .with_pending_sigterm()
        .output();
    let err = String::from_utf8_lossy(&out.stderr);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    assert_eq!(out.status.code(), Some(143), "{err}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert!(units(&v).is_empty(), "{v}");
}

#[test]
fn a_dry_run_reads_the_carried_state_and_fails_on_what_it_cannot_read() {
    for (state, ok) in [
        (r#"{"version": 1}"#, true),
        (r#"{"version": 99}"#, false),
        ("not json", false),
    ] {
        let e = Env::new();
        two_ready(&e);
        let path = e.root.join("night.json");
        fs::write(&path, state).unwrap();
        let out = e
            .ns()
            .args(["watch", "--dry-run", "--resume-night"])
            .arg(&path)
            .output();
        assert_eq!(out.status.success(), ok, "{state}: {out:?}");
    }
}

#[test]
fn a_dry_run_that_resumes_a_night_asks_gh_nothing() {
    let e = Env::new();
    two_ready(&e);
    let path = empty_night(&e);
    let out = e
        .ns()
        .args(["watch", "--dry-run", "--resume-night"])
        .arg(&path)
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["dry_run"], true, "{v}");
    let calls = fs::read_to_string(e.ghd.join("calls")).unwrap_or_default();
    assert_eq!(calls, "", "a hand-off check must not depend on the network");
}

#[test]
fn state_the_new_binary_cannot_read_starts_a_fresh_night() {
    for (state, why) in [
        ("{\"version\": 99}", "version 99"),
        ("not json", "is not a night"),
    ] {
        let e = Env::new();
        two_ready(&e);
        let path = e.root.join("night.json");
        fs::write(&path, state).unwrap();
        let out = e
            .ns()
            .args(["watch", "--until", "06:30", "--resume-night"])
            .arg(&path)
            .assert()
            .code(0)
            .get_output()
            .clone();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("starting a fresh night") && err.contains(why),
            "{err}"
        );
        assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
        assert_eq!(v["until"], "2026-10-09T06:30:00+00:00", "{v}");
    }
    let e = Env::new();
    two_ready(&e);
    let err = e
        .ns()
        .args(["watch", "--resume-night"])
        .arg(e.root.join("missing.json"))
        .assert()
        .code(0)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&err).contains("starting a fresh night"));
}

/// A night held in a build, with an update on origin, whose fake cargo runs in `mode`.
fn watching_an_update(mode: &str) -> (Env, Watching) {
    let e = Env::new();
    let from = nightshift_repo(&e);
    two_ready(&e);
    e.ctl("cargo-mode", mode);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = Watching::start(watch_from(&e, &from).args(["watch"]));
    w.entered(&in_a);
    land(&e, "cli/x", "new\n");
    release(&go_a);
    w.line("self-update: building");
    (e, w)
}

fn assert_stopped_without_a_hand_off(e: &Env, v: &Value, code: Option<i32>, err: &str) {
    assert_eq!(code, Some(143), "{err}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert!(!e.ctrl.join("handed-off").exists(), "{err}");
    assert!(events_named(e, "self_update").is_empty());
    assert!(events_named(e, "self_update_failed").is_empty(), "{err}");
    assert!(err.contains("abandoned: stopping"), "{err}");
}

#[test]
fn a_stop_during_the_build_kills_it_and_ends_the_night() {
    let (e, w) = watching_an_update("slow");
    let give_up = Instant::now() + Duration::from_secs(60);
    while !e.ctrl.join("cargo-started").exists() {
        assert!(
            Instant::now() < give_up,
            "the build never started, so there was nothing to stop"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let start = Instant::now();
    w.signal(libc::SIGTERM, false);
    let (code, v, err) = w.finish();
    assert_stopped_without_a_hand_off(&e, &v, code, &err);
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "build not killed"
    );
}

#[test]
fn a_stop_that_arrives_as_the_build_ends_is_not_lost_at_the_hand_off() {
    let (e, w) = watching_an_update("stopped");
    let (code, v, err) = w.finish();
    assert_stopped_without_a_hand_off(&e, &v, code, &err);
}

#[test]
fn a_build_past_its_ceiling_is_killed_and_the_night_goes_on() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    let to = land(&e, "cli/x", "new\n");
    e.ctl("cargo-mode", "slow");
    two_ready(&e);
    let out = watch_from(&e, &from)
        .env("NS_SELF_UPDATE_TIMEOUT_MS", "300")
        .args(["watch"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_kept_the_old_binary(&e, &to, &v, &err, "timed out");
}

#[test]
fn draining_runs_no_triage_to_fill_an_idle_slot() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    two_ready(&e);
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(watch_from(&e, &from).env("NS_REFILL_MS", "100").args([
        "watch",
        "--parallel",
        "2",
    ]));
    w.entered(&in_a);
    w.entered(&in_b);
    land(&e, "cli/x", "new\n");
    release(&go_b);
    w.line("draining");
    // Many refill intervals pass with a slot free and #5 untriaged; draining fills nothing.
    let calls = gh_calls_during(&e, Duration::from_millis(1000));
    assert_eq!(calls, "");
    assert_eq!(e.triage_only_runs(), 0, "{:?}", e.calls());
    release(&go_a);
    let (code, _, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    let updated = event_at(&e, |ev| ev["event"] == "self_update");
    assert!(ended_2 < updated, "{:?}", run_events(&e));
}

#[test]
fn a_refill_does_not_check_for_an_update_so_a_new_ready_issue_still_starts() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    e.ready(2, "Fix a", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_c, go_c) = hold_phase(&e, "4-fix-c", "build");
    let w = Watching::start(watch_from(&e, &from).env("NS_REFILL_MS", "100").args([
        "watch",
        "--parallel",
        "2",
    ]));
    w.entered(&in_a);
    land(&e, "cli/x", "new\n");
    // Several refill intervals pass with a slot free and a new commit on origin.
    thread::sleep(Duration::from_millis(1000));
    e.ready(4, "Fix c", &["type:fix"], "");
    w.entered(&in_c);
    release(&go_c);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 4], "done"), "{v}");
    let started_4 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 4);
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    assert!(started_4 < ended_2, "{:?}", run_events(&e));
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

/// A night with two units held in their builds, drained for a `cli/` change that lands once
/// unit 3 ends; `between` runs while unit 2 is still building. The night's stderr and the commit
/// the drain began for.
fn drained_while_the_base_moves(e: &Env, from: &str, between: impl FnOnce()) -> (String, String) {
    two_ready(e);
    let (in_a, go_a) = hold_phase(e, A, "build");
    let (in_b, go_b) = hold_phase(e, B, "build");
    let mut w = Watching::start(watch_from(e, from).args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    let noted = land(e, "cli/x", "new\n");
    release(&go_b);
    w.line("draining");
    between();
    release(&go_a);
    let (code, _, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    (err, noted)
}

fn assert_built(e: &Env, from: &str, to: &str, text: &str, err: &str) {
    assert_eq!(cargo_calls(e), 1, "{err}");
    assert_eq!(
        fs::read_to_string(e.ctrl.join("cargo-built")).unwrap(),
        text
    );
    let ev = events_named(e, "self_update");
    assert_eq!(ev.len(), 1, "{err}");
    assert_eq!((&ev[0]["from"], &ev[0]["to"]), (&json!(from), &json!(to)));
    assert!(
        err.contains(&format!("self-update: building {}", &to[..12])),
        "{err}"
    );
    assert!(
        err.contains(&format!("self-update {} -> {}", &from[..12], &to[..12])),
        "{err}"
    );
}

#[test]
fn a_drain_hands_off_to_the_base_at_its_end_not_its_start() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    let mut newer = String::new();
    let (err, _) = drained_while_the_base_moves(&e, &from, || {
        newer = land(&e, "cli/x", "newer\n");
    });
    assert_built(&e, &from, &newer, "newer\n", &err);
}

#[test]
fn a_failed_fetch_at_the_drain_end_hands_off_to_the_noted_commit() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    failing_fetch(&e);
    let (err, noted) = drained_while_the_base_moves(&e, &from, || {
        land(&e, "cli/x", "newer\n");
        e.ctl("fetch-fails", "");
    });
    assert_built(&e, &from, &noted, "new\n", &err);
}

/// A `git` on PATH whose `fetch` fails once `$FAKE_CTRL/fetch-fails` exists.
fn failing_fetch(e: &Env) {
    write_exe(
        &e.bin.join("git"),
        format!(
            "#!/bin/sh\ncase \" $* \" in *\" fetch \"*) [ -e \"$FAKE_CTRL/fetch-fails\" ] && {{ echo 'fatal: unreachable' >&2; exit 128; }} ;; esac\nexec {} \"$@\"\n",
            real_git()
        ),
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
    assert!(events_named(e, "self_update").is_empty());
    assert_kept_after_an_attempt(e, to, v, err, why);
}

fn assert_kept_after_an_attempt(e: &Env, to: &str, v: &Value, err: &str, why: &str) {
    assert_eq!(by_issue(&units(v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(cargo_calls(e), 1, "{err}");
    assert!(!e.ctrl.join("handed-off").exists());
    assert_no_night_state(e);
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

fn assert_no_night_state(e: &Env) {
    let dir = e.root.join(".git/ns/self-update");
    let left: Vec<_> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("night-"))
        .collect();
    assert!(left.is_empty(), "state left behind: {left:?}");
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

/// The path of the real `git`, for a fake one to exec.
fn real_git() -> String {
    let out = StdCommand::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().into()
}

fn counting_git(e: &Env) {
    let p = e.bin.join("git");
    write_exe(
        &p,
        format!(
            "#!/bin/sh\ncase \" $* \" in *\" fetch \"*) echo >> \"$FAKE_CTRL/git-fetches\" ;; esac\nexec {} \"$@\"\n",
            real_git()
        ),
    );
}

fn fetches(e: &Env) -> usize {
    fs::read_to_string(e.ctrl.join("git-fetches"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn a_failed_exec_keeps_the_old_binary_and_is_not_retried() {
    let (e, to, v, err) = night_with_a_failed_update("unexecutable");
    assert_kept_after_an_attempt(&e, &to, &v, &err, "cannot exec");
    assert_eq!(events_named(&e, "self_update").len(), 1, "{err}");
}

/// A night with a `cli/` change on origin the build commit lacks, which never updates; its
/// stderr.
fn assert_never_updates(e: &Env, ns: &mut Ns) -> String {
    two_ready(e);
    counting_git(e);
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
    let err = assert_never_updates(&e, watch_from(&e, &from).args(["watch"]));
    assert_eq!(fetches(&e), 2, "only each unit's own fetch: {err}");
}

#[test]
fn no_self_update_turns_it_off() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    land(&e, "cli/x", "new\n");
    let err = assert_never_updates(
        &e,
        watch_from(&e, &from).args(["watch", "--no-self-update"]),
    );
    assert_eq!(fetches(&e), 2, "only each unit's own fetch: {err}");
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
fn the_budget_stop_while_draining_ends_the_night_without_an_update() {
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
fn a_signal_while_draining_ends_the_night_without_an_update() {
    for sig in [libc::SIGTERM, libc::SIGINT] {
        let e = Env::new();
        let from = nightshift_repo(&e);
        two_ready(&e);
        let (in_a, go_a) = hold_phase(&e, A, "build");
        let (in_b, go_b) = hold_phase(&e, B, "build");
        let mut w = Watching::start(watch_from(&e, &from).args(["watch", "--parallel", "2"]));
        w.entered(&in_a);
        w.entered(&in_b);
        land(&e, "cli/x", "new\n");
        release(&go_b);
        w.line("draining");
        w.signal(sig, false);
        release(&go_a);
        let (code, v, err) = w.finish();
        let name = if sig == libc::SIGTERM {
            "SIGTERM"
        } else {
            "SIGINT"
        };
        assert_eq!(code, Some(128 + sig), "{err}");
        assert_eq!(v["stopped"], name, "{v}");
        assert_eq!(cargo_calls(&e), 0, "{err}");
        assert!(events_named(&e, "self_update").is_empty(), "{err}");
        assert!(!e.ctrl.join("handed-off").exists(), "{err}");
    }
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

fn watch_refuses_source(e: &Env, says: &str) {
    e.ns()
        .args(["watch", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("[queue] source"))
        .stderr(predicates::str::contains(says))
        .stderr(predicates::str::contains("source = \"github\""));
    assert!(e.gh_calls().is_empty(), "{}", e.gh_calls());
}

#[test]
fn watch_reads_github_when_source_says_so_or_origin_is_not_gitlab() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.factory("[queue]\nsource = \"github\"\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["queue"][0]["number"], 2, "{v}");
    e.factory("");
    git(
        &e.root,
        &["remote", "set-url", "origin", "git@github.com:o/r.git"],
    );
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["queue"][0]["number"], 2, "{v}");
}

#[test]
fn watch_refuses_an_unknown_source() {
    let e = Env::new();
    e.factory("[queue]\nsource = \"bitbucket\"\n");
    watch_refuses_source(&e, "\"bitbucket\"");
}

/// A GitLab project: member ids 1-6 at Developer, Maintainer, Owner, Reporter, Planner and Guest,
/// and 7 a Developer whose invitation is still awaiting.
fn gitlab_env() -> Env {
    let e = Env::new();
    e.factory("[queue]\nsource = \"gitlab\"\n");
    let awaiting =
        serde_json::json!({"id": 7, "username": "u7", "access_level": 30, "state": "awaiting"});
    let extra = [
        awaiting,
        serde_json::json!({"id": 8, "username": "u8", "access_level": 30}),
        serde_json::json!({"id": 9, "username": "u9", "access_level": 30, "state": "blocked"}),
        serde_json::json!({"id": 11, "username": "u11", "access_level": null}),
    ];
    e.gl_members(
        &[(1, 30), (2, 40), (3, 50), (4, 20), (5, 15), (6, 10)],
        &extra,
    );
    e
}

fn queued(v: &Value) -> Vec<u64> {
    v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect()
}

fn skipped(v: &Value) -> Vec<(u64, String)> {
    v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["number"].as_u64().unwrap(),
                i["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn watch_on_gitlab_queues_developers_and_above_and_skips_the_rest() {
    let e = gitlab_env();
    for (n, author) in [
        (2, 1),
        (3, 2),
        (4, 3),
        (5, 4),
        (6, 5),
        (7, 6),
        (8, 99),
        (10, 7),
        (11, 8),
        (12, 9),
        (13, 11),
    ] {
        e.gl_issue(n, &[READY, "type:fix"], "", author);
    }
    e.gl_issue(9, &["type:fix"], "", 1);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [2, 3, 4, 11], "{v}");
    let outside = "author outside the team".to_string();
    assert_eq!(
        skipped(&v),
        [5, 6, 7, 8, 10, 12, 13].map(|n| (n, outside.clone())),
        "{v}"
    );
    assert!(e.gh_calls().is_empty(), "{}", e.gh_calls());
    assert!(
        e.glab_calls()
            .contains("api --paginate projects/:id/members/all"),
        "{}",
        e.glab_calls()
    );
}

#[test]
fn watch_on_gitlab_closes_a_split_parent_from_its_team_notes() {
    let e = gitlab_env();
    e.gl_issue(5, &["status:split"], "", 1);
    let notes = serde_json::json!([
        {"body": "Children:\n- #8\n", "author": {"id": 1}, "system": false},
        {"body": "Children:\n- #9\n", "author": {"id": 4}, "system": false},
        {"body": "Children:\n- #9\n", "author": {"id": 1}, "system": true},
    ]);
    e.gh_file("gl-notes-5.json", &notes.to_string());
    e.gh_file("gl-state-8", "closed");
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(
        v["split_parents"][0]["children"],
        serde_json::json!([8]),
        "{v}"
    );
    let calls = e.glab_calls();
    assert!(calls.contains("issue close 5"), "{calls}");
    assert!(
        calls.contains(&format!("issue note 5 --message {DISCLOSED}Every child")),
        "{calls}"
    );
    assert!(
        calls.contains("api --paginate projects/:id/issues/5/notes?sort=asc&per_page=100"),
        "{calls}"
    );
    assert!(e.labels(5).is_empty());
}

/// GitLab split parent 5 with no notes, and issues 8 (a developer, closed), 9 (a reporter) and 10
/// (a developer, open or closed per `ten`) saying `Part of #5`, as a search lists them.
fn gl_part_of(e: &Env, ten: &str) {
    e.gl_issue(5, &["status:split"], "", 1);
    let child = |n: u64, author: u64| serde_json::json!({"iid": n, "title": "c", "labels": [], "description": "Part of #5", "author": {"id": author, "username": "u"}});
    let found = serde_json::json!([child(8, 1), child(9, 4), child(10, 2)]);
    e.gh_file("gl-search.json", &found.to_string());
    e.gh_file("gl-state-8", "closed");
    e.gh_file("gl-state-9", "opened");
    e.gh_file("gl-state-10", ten);
}

#[test]
fn watch_on_gitlab_finds_split_children_by_their_part_of_line() {
    let e = gitlab_env();
    gl_part_of(&e, "closed");
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(
        v["split_parents"][0]["children"],
        serde_json::json!([8, 10]),
        "{v}"
    );
    assert_eq!(v["split_parents"][0]["action"], "close", "{v}");
    let calls = e.glab_calls();
    assert!(
        calls.contains("in=description&search=Part%20of%20%235"),
        "{calls}"
    );
    assert!(calls.contains("issue close 5"), "{calls}");
}

#[test]
fn watch_on_gitlab_waits_for_an_open_part_of_child() {
    let e = gitlab_env();
    gl_part_of(&e, "opened");
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["split_parents"], serde_json::json!([]), "{v}");
    assert!(!e.glab_calls().contains("issue close"));
    let d = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(d["split_parents"][0]["action"], "wait", "{d}");
    assert_eq!(d["split_parents"][0]["open"], 1, "{d}");
}

#[test]
fn watch_on_gitlab_percent_encodes_the_ready_label_in_the_issue_query() {
    let e = Env::new();
    e.factory("[queue]\nsource = \"gitlab\"\nready_label = \"needs work,a&b\"\n");
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &["needs work,a&b"], "", 1);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [2], "{v}");
    assert!(
        e.glab_calls().contains("&labels=needs%20work%2Ca%26b&"),
        "{}",
        e.glab_calls()
    );
}

#[test]
fn watch_on_gitlab_passes_over_issues_an_open_or_merged_mr_closes() {
    let e = gitlab_env();
    for n in [2, 3, 4, 5, 6, 7] {
        e.gl_issue(n, &[READY], "", 1);
    }
    // 5's open MR is from its own unit branch, so that unit resumes. 6's is a fork's, and 7's
    // names no project, so it may be one too.
    let mr = |iid: u64, closes: u64, branch: &str| {
        serde_json::json!({
            "iid": iid,
            "description": format!("Closes #{closes}"),
            "source_branch": branch,
        })
    };
    let from = |mut v: Value, project: u64| {
        v["source_project_id"] = project.into();
        v["target_project_id"] = 1.into();
        v
    };
    let open = [
        from(mr(12, 3, "x"), 1),
        from(mr(14, 5, "ns/5-issue-5"), 1),
        from(mr(15, 6, "ns/6-issue-6"), 2),
        mr(16, 7, "ns/7-issue-7"),
    ];
    e.gh_file("gl-mrs.json", &Value::from(open.to_vec()).to_string());
    let merged = vec![from(mr(13, 4, "y"), 1)];
    e.gh_file("gl-mrs-merged.json", &Value::from(merged).to_string());
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [2, 5], "{v}");
    assert_eq!(
        skipped(&v),
        [
            (3, "open PR #12 closes it".to_string()),
            (4, "merged PR #13 closes it".to_string()),
            (6, "open PR #15 closes it".to_string()),
            (7, "open PR #16 closes it".to_string()),
        ],
        "{v}"
    );
}

#[test]
fn watch_on_gitlab_reads_blockers_state() {
    let e = gitlab_env();
    e.gl_issue(2, &[READY], "Blocked by: #20", 1);
    e.gl_issue(3, &[READY], "Blocked by: #21", 1);
    // An unreadable blocker counts as open.
    e.gl_issue(4, &[READY], "Blocked by: #22", 1);
    e.gh_file("gl-state-20", "closed\n");
    e.gh_file("gl-issue-22.raw", "not json\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [2], "{v}");
    assert_eq!(
        skipped(&v),
        [
            (3, "blocked by open #21".to_string()),
            (4, "blocked by open #22".to_string()),
        ],
        "{v}"
    );
}

#[test]
fn watch_on_gitlab_swaps_labels_and_comments_through_glab() {
    let e = gitlab_env();
    e.gl_issue(2, &["type:fix", READY], "", 1);
    e.queue("triage", &["none"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "stuck", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-human"]);
    let gh = e.gh_calls();
    for verb in ["issue list", "issue edit", "issue comment", "issue close"] {
        assert!(!gh.contains(verb), "{verb}: {gh}");
    }
    let calls = e.glab_calls();
    assert!(
        calls
            .contains("issue update 2 --unlabel status:ready-for-agent --label status:in-progress"),
        "{calls}"
    );
    let note = calls.split("issue note 2 --message").nth(1).expect("note");
    assert!(note.contains("triage wrote no brief"), "{note}");
    assert!(note.trim_start().starts_with(DISCLOSED), "{note}");
}

#[test]
fn watch_on_gitlab_closes_a_merged_issue_and_drops_its_status_label() {
    let e = gitlab_env();
    e.factory(&format!("[queue]\nsource = \"gitlab\"\n{AUTO}"));
    e.gl_issue(2, &["type:fix", READY], "", 1);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(e.labels(2), ["type:fix"]);
    let gh = e.gh_calls();
    for verb in ["issue list", "issue edit", "issue comment", "issue close"] {
        assert!(!gh.contains(verb), "{verb}: {gh}");
    }
    let calls = e.glab_calls();
    assert!(!calls.contains("issue note"), "{calls}");
    assert!(calls.contains("issue close 2"), "{calls}");
    assert!(
        calls.contains("issue update 2 --unlabel status:in-progress"),
        "{calls}"
    );
}

#[test]
fn watch_on_gitlab_reads_main_ci_from_the_head_commit_statuses() {
    let e = gitlab_env();
    e.gl_issue(2, &[READY], "", 1);
    let (v, _) = dry_run(&e);
    assert_eq!(v["main_ci"]["state"], "none", "{v}");
    let head = git(&e.root, &["rev-parse", "HEAD"]);
    e.gh_file(
        "gl-statuses.json",
        &serde_json::json!([
            {"name": "test", "status": "failed", "allow_failure": false},
            {"name": "flaky", "status": "failed", "allow_failure": true},
            {"name": "lint", "status": "success", "allow_failure": false},
        ])
        .to_string(),
    );
    let (v, _) = dry_run(&e);
    assert_eq!(
        v["main_ci"],
        serde_json::json!({"state": "red", "sha": head, "failing": ["test"]})
    );
    assert_eq!(v["queue_held"], true);
    let calls = e.glab_calls();
    assert!(
        calls.contains(&format!(
            "projects/:id/repository/commits/{head}/statuses?ref=main"
        )),
        "{calls}"
    );
    assert!(e.gh_calls().is_empty(), "{}", e.gh_calls());
}

#[test]
fn watch_on_gitlab_files_the_fix_issue_through_glab_and_starts_only_it() {
    let e = gitlab_env();
    e.gl_issue(2, &["type:fix", READY], "", 1);
    e.gh_file(
        "gl-statuses.json",
        &serde_json::json!([{"id": 51, "name": "test", "status": "failed", "allow_failure": false}])
            .to_string(),
    );
    let v = e.run(&["watch", "--max-units", "3"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[100], "done"), "{v}");
    let calls = e.glab_calls();
    assert_eq!(
        calls
            .matches("api --method POST projects/:id/issues")
            .count(),
        1,
        "{calls}"
    );
    assert!(
        calls.contains("--raw-field labels=type:fix,priority:critical,status:ready-for-agent"),
        "{calls}"
    );
    assert!(calls.contains("projects/:id/jobs/51/trace"), "{calls}");
    assert!(
        calls.contains("projects/:id/pipelines?ref=main&status=success&per_page=30"),
        "{calls}"
    );
    let body = fs::read_to_string(e.ghd.join("created-100.body")).unwrap();
    assert!(body.starts_with(DISCLOSED), "{body}");
    assert!(body.contains("ERROR: Job failed"), "{body}");
    assert!(!e.gh_calls().contains("issue create"), "{}", e.gh_calls());
}

#[test]
fn watch_on_gitlab_fails_a_dry_run_when_main_has_no_branch_or_head() {
    let e = gitlab_env();
    e.gl_issue(2, &[READY], "", 1);
    for (file, want) in [
        ("gl-project.json", "GitLab names no default branch"),
        ("gl-branch.json", "GitLab names no head for main"),
    ] {
        e.gh_file(file, "{}");
        let out = e.ns().args(["watch", "--dry-run"]).output();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{err}");
        assert!(err.contains(want), "{err}");
        fs::remove_file(e.ghd.join(file)).unwrap();
    }
}

#[test]
fn watch_on_gitlab_keeps_the_token_out_of_its_errors() {
    let e = gitlab_env();
    e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\n");
    e.gl_issue(2, &[READY], "", 1);
    e.gh_file("glab.fail", "");
    let out = e
        .ns()
        .env("MY_GL_TOKEN", FAKE_TOKEN)
        .env_remove("GITLAB_TOKEN")
        .args(["watch", "--dry-run"])
        .output();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("glab api"), "{err}");
    assert!(err.contains("401 Unauthorized"), "{err}");
    assert!(!err.contains(FAKE_TOKEN), "{err}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(FAKE_TOKEN));
}

#[test]
fn watch_detects_a_gitlab_origin_when_source_is_unset() {
    let e = Env::new();
    e.gl_members(&[(1, 30)], &[]);
    e.gl_issue(2, &[READY], "", 1);
    e.ready(3, "Fix b", &["type:fix"], "");
    for origin in ["https://gitlab.com/o/r.git", "git@code.example.com:o/r.git"] {
        git(&e.root, &["remote", "set-url", "origin", origin]);
        e.config("[forge.gitlab]\ntoken_env = \"MY_GL_TOKEN\"\nhost = \"code.example.com\"\n");
        let out = e
            .ns()
            .env("MY_GL_TOKEN", FAKE_TOKEN)
            .env_remove("GITLAB_TOKEN")
            .env_remove("GITLAB_HOST")
            .args(["watch", "--dry-run"])
            .assert()
            .code(0)
            .get_output()
            .clone();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(queued(&v), [2], "{origin}: {v}");
    }
    assert!(e.gh_calls().is_empty(), "{}", e.gh_calls());
    e.config("");
    git(
        &e.root,
        &["remote", "set-url", "origin", "https://github.com/o/r"],
    );
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [3], "{v}");
    git(
        &e.root,
        &["remote", "set-url", "origin", "https://gitlab.com/o/r.git"],
    );
    e.factory("[queue]\nsource = \"github\"\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(queued(&v), [3], "{v}");
}

/// `skills/ns-a` committed in the main checkout and pushed, then `skills/ns-b` pushed to
/// origin from another clone, with `ns-a` linked into each `$HOME`-relative dir in `dirs`.
fn skills_behind_origin(e: &Env, dirs: &[&str]) {
    let skill = |repo: &Path, name: &str| {
        fs::create_dir_all(repo.join("skills").join(name)).unwrap();
        fs::write(repo.join("skills").join(name).join("SKILL.md"), "s\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", name]);
        git(repo, &["push", "-q", "origin", "main"]);
    };
    skill(&e.root, "ns-a");
    let other = e.base.join("other");
    git(
        &e.base,
        &["clone", "-q", "remote.git", other.to_str().unwrap()],
    );
    skill(&other, "ns-b");
    for d in dirs {
        let dir = e.home.join(d);
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(e.root.join("skills/ns-a"), dir.join("ns-a")).unwrap();
    }
}

/// Runs one unit of `ns watch` and checks the main checkout reached origin's `main`.
fn watch_syncs_skills(e: &Env) {
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.run(&["watch", "--once"], 0);
    let remote = git(&e.base.join("remote.git"), &["rev-parse", "main"]);
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), remote);
    assert_eq!(events_named(e, "skills_synced").len(), 1);
}

#[test]
fn watch_syncs_skills_installed_only_for_cursor() {
    let e = Env::new();
    skills_behind_origin(&e, &[".cursor/skills"]);
    watch_syncs_skills(&e);
    assert_eq!(
        fs::read_link(e.home.join(".cursor/skills/ns-b")).unwrap(),
        e.root.join("skills/ns-b")
    );
    assert!(!e.home.join(".claude/skills").exists());
    assert!(!e.home.join(".agents/skills").exists());
}

#[test]
fn watch_syncs_skills_installed_into_the_default_dirs() {
    let e = Env::new();
    skills_behind_origin(&e, &[".agents/skills", ".claude/skills"]);
    watch_syncs_skills(&e);
    for d in [".agents/skills", ".claude/skills"] {
        assert_eq!(
            fs::read_link(e.home.join(d).join("ns-b")).unwrap(),
            e.root.join("skills/ns-b")
        );
    }
    assert!(!e.home.join(".cursor/skills").exists());
}

/// Every phase on a `codex` harness, the fake harness under that name.
fn codex_night(e: &Env) {
    let codex = e.bin.join("codex");
    copy_exe(&e.bin.join("claude"), &codex);
    e.config(&format!(
        "[harness.codex]\ncommand = [{c:?}]\ncommand_write = [{c:?}]\n",
        c = codex.to_str().unwrap()
    ));
    e.factory("[defaults]\nharness = \"codex\"\n");
}

/// `skills/ns-a` committed in the main checkout, pushed, and linked into `~/.agents/skills`, so
/// the checkout is current.
fn skills_installed(e: &Env) {
    land(e, "skills/ns-a/SKILL.md", "a\n");
    let dir = e.home.join(".agents/skills");
    fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(e.root.join("skills/ns-a"), dir.join("ns-a")).unwrap();
}

/// Push a commit adding `path` to origin from another clone, so the main checkout lacks it; the
/// commit.
fn push_from_elsewhere(e: &Env, path: &str) -> String {
    let other = e.base.join("other");
    if !other.exists() {
        git(
            &e.base,
            &["clone", "-q", "remote.git", other.to_str().unwrap()],
        );
    }
    git(&other, &["pull", "-q", "origin", "main"]);
    let p = other.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, "new\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", path]);
    git(&other, &["push", "-q", "origin", "main"]);
    git(&other, &["rev-parse", "HEAD"])
}

#[test]
fn a_codex_night_drains_before_it_syncs_a_skill_change() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    let from = git(&e.root, &["rev-parse", "HEAD"]);
    two_ready(&e);
    e.ready(4, "Fix c", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w =
        Watching::start(
            e.ns()
                .env("NS_REFILL_MS", "100")
                .args(["watch", "--parallel", "2"]),
        );
    w.entered(&in_a);
    w.entered(&in_b);
    let to = push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    release(&go_b);
    w.line("#3 done");
    w.line("draining");
    // Unit 2 is still in its build: the checkout stays put and unit 4 waits for the sync.
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), from);
    assert!(events_named(&e, "skills_synced").is_empty());
    // The drain claims nothing and runs no triage pass for the untriaged #5.
    assert_eq!(e.triage_only_runs(), 0, "{:?}", e.calls());
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3, 4], "done"), "{v}");
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), to);
    assert_eq!(err.matches("draining").count(), 1, "{err}");
    let short = |c: &str| c[..12].to_string();
    assert!(
        err.contains(&format!("({} -> {})", short(&from), short(&to))),
        "{err}"
    );
    let drain = events_named(&e, "skills_drain");
    assert_eq!(drain.len(), 1, "{drain:?}");
    assert_eq!(
        (drain[0]["from"].as_str(), drain[0]["to"].as_str()),
        (Some(&*from), Some(&*to))
    );
    assert_eq!(events_named(&e, "skills_synced").len(), 1);
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    let synced = event_at(&e, |ev| ev["event"] == "skills_synced");
    let started_4 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 4);
    assert!(
        ended_2 < synced && synced < started_4,
        "{:?}",
        run_events(&e)
    );
}

#[test]
fn a_night_with_one_codex_phase_drains_before_it_syncs_a_skill_change() {
    let e = Env::new();
    let codex = e.bin.join("codex");
    copy_exe(&e.bin.join("claude"), &codex);
    e.config(&format!(
        "[harness.codex]\ncommand = [{c:?}]\ncommand_write = [{c:?}]\n",
        c = codex.to_str().unwrap()
    ));
    e.factory("[phases.verify]\nharness = \"codex\"\n");
    skills_installed(&e);
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    release(&go_b);
    w.line("draining");
    assert!(events_named(&e, "skills_synced").is_empty());
    release(&go_a);
    let (code, _, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(events_named(&e, "skills_drain").len(), 1, "{err}");
    assert_eq!(events_named(&e, "skills_synced").len(), 1, "{err}");
}

#[test]
fn a_codex_night_hands_off_to_a_new_build_before_it_syncs_skills() {
    let e = Env::new();
    codex_night(&e);
    let from = nightshift_repo(&e);
    skills_installed(&e);
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(watch_from(&e, &from).args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    land(&e, "cli/x", "new\n");
    push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    release(&go_b);
    w.line("draining");
    release(&go_a);
    let (code, _, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    let updated = event_at(&e, |ev| ev["event"] == "self_update");
    let synced = event_at(&e, |ev| ev["event"] == "skills_synced");
    assert!(updated < synced, "{:?}", run_events(&e));
}

#[test]
fn a_codex_unit_started_by_a_refill_does_not_sync_under_a_running_one() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    let from = git(&e.root, &["rev-parse", "HEAD"]);
    e.ready(2, "Fix a", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_c, go_c) = hold_phase(&e, "4-fix-c", "build");
    let w = Watching::start(
        e.ns()
            .env("NS_REFILL_MS", "100")
            .args(["watch", "--parallel", "2"]),
    );
    w.entered(&in_a);
    let to = push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    e.ready(4, "Fix c", &["type:fix"], "");
    w.entered(&in_c);
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), from);
    release(&go_c);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 4], "done"), "{v}");
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), to);
}

#[test]
fn a_codex_night_starts_on_current_skills() {
    let e = Env::new();
    codex_night(&e);
    skills_behind_origin(&e, &[".agents/skills"]);
    watch_syncs_skills(&e);
    assert_eq!(events_named(&e, "skills_drain").len(), 1);
}

#[test]
fn a_codex_night_does_not_drain_for_a_change_outside_the_skills() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    push_from_elsewhere(&e, "src/x");
    e.ready(2, "Fix a", &["type:fix"], "");
    let out = e.ns().args(["watch", "--once"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("draining"), "{err}");
    assert!(events_named(&e, "skills_drain").is_empty());
}

#[test]
fn a_codex_night_whose_checkout_cannot_move_warns_once_and_runs_its_units() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    fs::write(e.root.join("skills/ns-a/SKILL.md"), "edited\n").unwrap();
    push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    two_ready(&e);
    let out = e.ns().args(["watch"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(err.matches("draining").count(), 1, "{err}");
    assert_eq!(events_named(&e, "skills_stale").len(), 1);
    assert!(events_named(&e, "skills_synced").is_empty());
}

#[test]
fn a_claude_night_syncs_before_every_unit_without_draining() {
    let e = Env::new();
    skills_installed(&e);
    two_ready(&e);
    e.ready(4, "Fix c", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    let to = push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    let (in_c, go_c) = hold_phase(&e, "4-fix-c", "build");
    release(&go_b);
    w.entered(&in_c);
    // Unit 4 started beside unit 2 on the synced checkout.
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), to);
    release(&go_c);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3, 4], "done"), "{v}");
    assert!(!err.contains("draining"), "{err}");
    assert!(events_named(&e, "skills_drain").is_empty());
}

#[test]
fn a_claude_night_on_nightshift_does_not_drain_for_skills() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    skills_installed(&e);
    push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    e.ready(2, "Fix a", &["type:fix"], "");
    let out = watch_from(&e, &from).args(["watch", "--once"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("draining"), "{err}");
    assert_eq!(events_named(&e, "skills_synced").len(), 1, "{err}");
}

#[test]
fn a_signal_while_draining_for_skills_ends_the_night_without_a_sync() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    let from = git(&e.root, &["rev-parse", "HEAD"]);
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    release(&go_b);
    w.line("draining");
    w.signal(libc::SIGTERM, false);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(128 + libc::SIGTERM), "{err}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), from);
    assert!(events_named(&e, "skills_synced").is_empty(), "{err}");
}

/// `(phase, green)` of each `gate` event in the run log.
fn gate_verdicts(e: &Env) -> Vec<(String, bool)> {
    gate_events(e)
        .iter()
        .map(|g| {
            (
                g["phase"].as_str().unwrap().to_string(),
                g["green"].as_bool().unwrap(),
            )
        })
        .collect()
}

fn verdicts(v: &[(&str, bool)]) -> Vec<(String, bool)> {
    v.iter().map(|(p, g)| (p.to_string(), *g)).collect()
}

const COMMIT: &str = "git -c user.name=f -c user.email=f@f commit -qm";

#[test]
fn a_red_gate_after_a_verify_commit_goes_back_to_build_before_review() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"test ! -f broken\"\n");
    e.ctl(
        "verify.sh",
        &format!("touch broken && git add broken && {COMMIT} break"),
    );
    e.ctl("build.sh", &format!("git rm -q broken && {COMMIT} fix"));
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("verify", &["pass:script", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(4, "build").contains("test ! -f broken"));
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("verify", false), ("build", true)])
    );
}

#[test]
fn a_ship_base_merge_that_breaks_the_gate_goes_back_to_build_before_the_merge_step() {
    let e = Env::new();
    e.factory(&format!(
        "{AUTO}[phases.build]\ngate = \"test ! -f broken\"\n"
    ));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("ship", &["pass:script", "pass:script"]);
    e.ctl(
        "ship.sh",
        r#"set -e
c="git -c user.name=f -c user.email=f@f"
o="$FAKE_CTRL/other"
if [ ! -d "$o" ]; then
  git clone -q "$(git remote get-url origin)" "$o"
  (cd "$o" && touch broken && git add broken && $c commit -qm 'Break b (#3)' && git push -q origin main &&
    printf '%s' '[{"number":3,"merged_at":"2026-10-09T00:00:00Z","head":{"ref":"ns/3-fix-b"}}]' > "$FAKE_GH_DIR/pulls-$(git rev-parse HEAD).json")
  git fetch -q origin main
  $c merge -q --no-edit FETCH_HEAD
fi
git push -q -f origin HEAD"#,
    );
    e.ctl("build.sh", &format!("git rm -q broken && {COMMIT} fix"));
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(6, "build").contains("test ! -f broken"));
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("ship", false), ("build", true)])
    );
}

/// Ship pushes the branch; GitHub reports the PR `BEHIND`, and its update-branch lands `updated`,
/// a commit on top of the branch that adds `broken`, unless the PR is already up to date. The
/// factory turns the local merge queue off, so these tests pin today's merge path, and adds
/// `build` to `[phases.build]`.
fn behind_with_a_broken_update(e: &Env, build: &str) {
    e.factory(&format!("{AUTO}{LEGACY}[phases.build]\n{build}"));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("ship", &["pass:script", "pass:script"]);
    e.ctl(
        "ship.sh",
        r#"set -e
c="git -c user.name=f -c user.email=f@f"
git push -q -f origin HEAD
o="$FAKE_CTRL/other"
if [ ! -d "$o" ]; then
  b=$(git symbolic-ref --short HEAD)
  git clone -q -b "$b" "$(git remote get-url origin)" "$o"
  (cd "$o" && touch broken && git add broken && $c commit -qm 'merge main' && git push -q origin "$b")
  git -C "$o" rev-parse HEAD > "$FAKE_GH_DIR/updated-12.head"
  echo BEHIND > "$FAKE_GH_DIR/pr-12.merge"
else
  rm -f "$FAKE_GH_DIR/pr-12.head"
fi"#,
    );
    e.ctl("build.sh", &format!("git rm -q broken && {COMMIT} fix"));
    e.gh_file("checks-12.json", GREEN);
}

#[test]
fn an_update_branch_that_breaks_the_gate_goes_back_to_build_before_merging() {
    let e = Env::new();
    behind_with_a_broken_update(&e, "gate = \"test ! -f broken\"\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "build", "verify", "review", "ship"]
    );
    let p = e.prompt(6, "build");
    assert!(p.contains("test ! -f broken"), "{p}");
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", false), ("build", true)])
    );
    let calls = e.gh_calls();
    assert_eq!(calls.matches("pr merge 12").count(), 1, "{calls}");
    let red = &gate_events(&e)[1];
    let other = git(&e.ctrl.join("other"), &["rev-parse", "--short", "HEAD"]);
    assert_eq!(red["sha"].as_str(), Some(other.as_str()), "{red}");
    assert_eq!(red["attempt"], 1, "{red}");
}

#[test]
fn a_second_update_branch_gates_the_merge_head_again_at_the_next_attempt() {
    let e = Env::new();
    e.factory(&format!(
        "{AUTO}{LEGACY}[phases.build]\nmax_attempts = 3\ngate = \"test ! -f broken\"\n"
    ));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("ship", &["pass:script", "pass:script", "pass:script"]);
    e.ctl(
        "ship.sh",
        r#"set -e
c="git -c user.name=f -c user.email=f@f"
rm -f "$FAKE_GH_DIR/pr-12.head"
git push -q -f origin HEAD
n=$(cat "$FAKE_CTRL/ships" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "$FAKE_CTRL/ships"
if [ "$n" -le 2 ]; then
  b=$(git symbolic-ref --short HEAD)
  o="$FAKE_CTRL/other$n"
  git clone -q -b "$b" "$(git remote get-url origin)" "$o"
  f=broken
  [ "$n" -eq 2 ] && f=fine
  (cd "$o" && touch "$f" && git add "$f" && $c commit -qm "merge main $n" && git push -q origin "$b")
  git -C "$o" rev-parse HEAD > "$FAKE_GH_DIR/updated-12.head"
  echo BEHIND > "$FAKE_GH_DIR/pr-12.merge"
else
  rm -f "$FAKE_GH_DIR/pr-12.head"
fi"#,
    );
    e.ctl("build.sh", &format!("git rm -q broken && {COMMIT} fix"));
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[
            ("build", true),
            ("merge", false),
            ("build", true),
            ("merge", true)
        ])
    );
    let merges: Vec<_> = gate_events(&e)
        .into_iter()
        .filter(|g| g["phase"] == "merge")
        .map(|g| g["attempt"].clone())
        .collect();
    assert_eq!(merges, [1, 2]);
}

#[test]
fn an_update_branch_head_that_cannot_be_checked_out_needs_a_human_merge() {
    let e = Env::new();
    e.factory(&format!("{AUTO}{LEGACY}[phases.build]\ngate = \"true\"\n"));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file(
        "updated-12.head",
        "0123456789abcdef0123456789abcdef01234567",
    );
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.contains("cannot check out PR #12 head 0123456789abcdef0123456789abcdef01234567"),
        "{reason}"
    );
    assert!(
        reason.contains("git fetch origin ns/7-fix-the-thing failed"),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
}

#[test]
fn a_green_update_branch_head_is_gated_and_merged_at_that_head() {
    let e = Env::new();
    behind_with_a_broken_update(&e, "gate = \"true\"\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", true)])
    );
    let updated = git(&e.ctrl.join("other"), &["rev-parse", "HEAD"]);
    let merge = &gate_events(&e)[1];
    assert!(
        updated.starts_with(merge["sha"].as_str().unwrap()),
        "{merge}"
    );
    let want = format!("--match-head-commit {updated}");
    assert!(e.gh_calls().contains(&want), "{}", e.gh_calls());
}

#[test]
fn a_rebased_pr_head_is_gated_at_that_head_and_merged() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.factory(&format!("{AUTO}[phases.build]\ngate = \"true\"\n"));
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let old = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    let old = old.trim();
    let merge = gate_events(&e)
        .into_iter()
        .find(|g| g["phase"] == "merge")
        .expect("merge gate");
    assert_eq!(merge["green"], true, "{merge}");
    assert!(old.starts_with(merge["sha"].as_str().unwrap()), "{merge}");
    let want = format!("--match-head-commit {old}");
    assert!(e.gh_calls().contains(&want), "{}", e.gh_calls());
}

#[test]
fn an_update_branch_head_missing_after_a_good_fetch_needs_a_human_merge() {
    let e = Env::new();
    e.factory(&format!("{AUTO}{LEGACY}[phases.build]\ngate = \"true\"\n"));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:script"]);
    e.ctl("ship.sh", "git push -q -f origin HEAD");
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file(
        "updated-12.head",
        "0123456789abcdef0123456789abcdef01234567",
    );
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("cannot check out PR #12 head"), "{reason}");
    assert!(!reason.contains("git fetch"), "{reason}");
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
}

#[test]
fn an_unmoved_head_does_not_rerun_the_gate_through_ship_and_the_merge_step() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    e.factory(&format!(
        "{AUTO}[phases.build]\ngate = \"echo ran >> {}\"\n",
        log.display()
    ));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);
}

#[test]
fn a_gate_killed_at_the_merge_step_says_so_when_build_runs_out() {
    let e = Env::new();
    behind_with_a_broken_update(
        &e,
        "max_attempts = 1\ngate = \"test ! -f broken || kill -9 $$\"\n",
    );
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"], "build is out of attempts (1): the CI gate was killed by SIGKILL",
        "{v}"
    );
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", false)])
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_stop_during_the_merge_gate_merges_nothing() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    let gate = format!("test ! -f broken || {{ {}; }}", blocks(&f));
    behind_with_a_broken_update(&e, &format!("gate = {gate:?}\n"));
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "gate"
        && ev["phase"] == "merge"
        && ev["attempt"] == 1
        && ev["interrupted"] == "SIGTERM"));
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_failed_build_that_commits_does_not_run_the_gate() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    e.factory(&format!(
        "[phases.build]\ngate = \"echo ran >> {}\"\n",
        log.display()
    ));
    e.queue("build", &["fail:commit", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "verify", "review", "ship"]
    );
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
    assert_eq!(gate_events(&e)[0]["attempt"], 2);
}

#[test]
fn a_timed_out_verify_that_moved_head_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["blocked:commit+sleep", "pass"]);
    let v = run_with_timeouts(&e, "verify=1500", 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("verify", true)])
    );
}

/// Land a commit on `origin/main` that adds `file` holding `text`, from a clone under
/// `FAKE_CTRL`: the main a unit's PR merges into has moved on since the unit branched.
fn main_moves(e: &Env, file: &str, text: &str) -> String {
    let o = e.ctrl.join(format!("main-{file}"));
    let remote = e.base.join("remote.git");
    git(
        &e.base,
        &["clone", "-q", remote.to_str().unwrap(), o.to_str().unwrap()],
    );
    fs::write(o.join(file), text).unwrap();
    git(&o, &["add", file]);
    git(&o, &["commit", "-qm", &format!("add {file} on main")]);
    git(&o, &["push", "-q", "origin", "main"]);
    git(&o, &["rev-parse", "HEAD"])
}

/// The scratch worktrees of the local merge queue, as `git worktree list` and the disk see them.
fn merge_queue_leftovers(e: &Env) -> Vec<String> {
    let queue = e.root.join(".git/ns/merge-queue");
    let mut left: Vec<String> = git(&e.root, &["worktree", "list", "--porcelain"])
        .lines()
        .filter(|l| l.contains(&*queue.to_string_lossy()))
        .map(String::from)
        .collect();
    if let Ok(dir) = fs::read_dir(&queue) {
        left.extend(dir.map(|d| d.unwrap().path().display().to_string()));
    }
    left
}

fn merge_gate_failures(e: &Env) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|ev| ev["event"] == "merge_gate_failed")
        .collect()
}

/// `ns run` with the local queue on (the default) and `gate` as the build gate.
fn local_queue(e: &Env, build: &str) {
    e.factory(&format!("{AUTO}[phases.build]\n{build}"));
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
}

/// A gate that appends `merged` to `log` when the tree holds main's `from-main`, else `branch`.
fn where_gate(log: &Path) -> String {
    format!(
        "gate = \"if test -f from-main; then echo merged >> {0}; else echo branch >> {0}; fi\"\n",
        log.display()
    )
}

#[test]
fn the_local_queue_gates_the_merge_with_main_and_merges_without_update_branch() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit"]);
    let base = main_moves(&e, "from-main", "x\n");
    e.gh_file("pr-12.merge", "BEHIND");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\nmerged\n");
    let calls = e.gh_calls();
    assert!(!calls.contains("pr update-branch"), "{calls}");
    let merge = gate_events(&e)
        .into_iter()
        .find(|g| g["phase"] == "merge")
        .expect("merge gate");
    assert_eq!(merge["merge_result"], true, "{merge}");
    assert_eq!(merge["base"].as_str(), Some(base.as_str()), "{merge}");
    assert!(merge["wall_s"].is_number(), "{merge}");
    let head = merge["sha"].as_str().unwrap();
    assert!(
        calls.contains(&format!("--match-head-commit {head}")),
        "{calls}"
    );
    assert!(merge_gate_failures(&e).is_empty());
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn every_step_keeps_the_local_queue_and_sends_a_conflict_back_to_build() {
    let every_step = |e: &Env, log: &Path| {
        e.factory(&format!(
            "{AUTO}[gate]\nlocal = \"every-step\"\n[phases.build]\n{}",
            where_gate(log)
        ));
        e.ctl("pr", "12");
        e.gh_file("checks-12.json", GREEN);
    };
    let e = Env::new();
    let log = e.ctrl.join("gates");
    every_step(&e, &log);
    e.queue("build", &["pass:commit"]);
    main_moves(&e, "from-main", "x\n");
    e.gh_file("pr-12.merge", "BEHIND");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\nmerged\n");
    assert!(!e.gh_calls().contains("pr update-branch"));

    let e = Env::new();
    every_step(&e, &e.ctrl.join("gates"));
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("pr-12.merge", "DIRTY");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    // The first conflict's rebuild is free; the next, with main unmoved, uses build's second
    // attempt.
    assert_eq!(
        e.calls(),
        [
            "triage", "build", "verify", "review", "ship", "build", "verify", "review", "ship",
            "build"
        ]
    );
    assert!(e.prompt(6, "build").contains("PR #12 conflicts with main"));
    assert_eq!(rebase_events(&e).len(), 1);
}

#[test]
fn a_main_the_pr_head_already_holds_is_gated_as_the_head_without_a_scratch_worktree() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\n");
    let gates = gate_events(&e);
    assert!(
        gates.iter().all(|g| g.get("merge_result").is_none()),
        "{gates:?}"
    );
    assert!(e.gh_calls().contains("pr merge"));
    assert!(merge_gate_failures(&e).is_empty());
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_main_that_cannot_merge_with_the_pr_head_needs_a_human_merge() {
    let e = Env::new();
    local_queue(&e, "gate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    let o = e.ctrl.join("orphan");
    let remote = e.base.join("remote.git");
    git(
        &e.base,
        &["clone", "-q", remote.to_str().unwrap(), o.to_str().unwrap()],
    );
    git(&o, &["checkout", "-q", "--orphan", "other"]);
    fs::write(o.join("unrelated"), "x\n").unwrap();
    git(&o, &["add", "unrelated"]);
    git(&o, &["commit", "-qm", "unrelated root"]);
    git(&o, &["push", "-qf", "origin", "other:main"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("cannot merge PR #12 into main for the local merge queue: git merge"),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_red_merge_result_goes_back_to_build_and_merges_nothing() {
    let e = Env::new();
    local_queue(&e, "gate = \"test ! -f broken\"\n");
    e.queue("build", &["pass:commit", "fail"]);
    let base = main_moves(&e, "broken", "x\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "build"]
    );
    let p = e.prompt(6, "build");
    assert!(p.contains("test ! -f broken"), "{p}");
    assert!(p.contains("combining it with main"), "{p}");
    assert_eq!(
        v["phases"][5]["decision"], "the CI gate failed on the merge with main",
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", false)])
    );
    // The failed rebuild leaves the PR head and main where they were, so the merge step that
    // follows reuses the red verdict.
    let failed = merge_gate_failures(&e);
    let reused: Vec<_> = failed.iter().map(|f| f["reused"].clone()).collect();
    assert_eq!(reused, [false, true], "{failed:?}");
    assert_eq!(failed[0]["pr"], 12);
    assert_eq!(failed[0]["base"].as_str(), Some(base.as_str()));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_merge_result_that_conflicts_goes_back_to_build_to_rebase() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit", "fail"]);
    main_moves(&e, "work.txt", "main's own line\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let p = e.prompt(6, "build");
    assert!(p.contains("PR #12 conflicts with main"), "{p}");
    assert!(p.contains("rebase onto main"), "{p}");
    assert!(p.contains("rebase onto the newest origin/main"), "{p}");
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "conflict"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

/// CI fails on the first merge step and build passes without a commit, so the second merge
/// step meets the same PR head. `build.sh` runs at that rebuild.
fn ci_red_then_green_at_one_head(e: &Env, log: &Path) {
    local_queue(e, &where_gate(log));
    e.queue("build", &["pass:commit", "pass:script"]);
    e.gh_file("checks-12-1.json", RED);
    e.gh_file("checks-12-2.json", GREEN);
}

fn merge_gate_bases(e: &Env) -> Vec<String> {
    gate_events(e)
        .into_iter()
        .filter(|g| g["phase"] == "merge")
        .map(|g| g["base"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn an_unmoved_pr_head_and_base_reuse_the_merge_result_verdict() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    ci_red_then_green_at_one_head(&e, &log);
    e.ctl("build.sh", "true");
    let base = main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(merge_gate_bases(&e), [base]);
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\nmerged\n");
}

#[test]
fn a_moved_base_runs_the_merge_result_gate_again() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    ci_red_then_green_at_one_head(&e, &log);
    let first = main_moves(&e, "from-main", "x\n");
    let o = e.ctrl.join("main-from-main");
    e.ctl(
        "build.sh",
        &format!(
            "cd '{}' && echo y > later && git add later && {COMMIT} later && git push -q origin main",
            o.display()
        ),
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let second = git(&o, &["rev-parse", "HEAD"]);
    assert_ne!(first, second);
    assert_eq!(merge_gate_bases(&e), [first, second]);
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_stop_during_the_merge_result_gate_merges_nothing_and_removes_the_scratch_worktree() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    let gate = format!("case $PWD in */ns/merge-queue/*) {};; esac", blocks(&f));
    local_queue(&e, &format!("gate = {gate:?}\n"));
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:script"]);
    e.ctl(
        "ship.sh",
        &format!(
            "set -e\no=\"$FAKE_CTRL/main\"\ngit clone -q \"$(git remote get-url origin)\" \"$o\"\ncd \"$o\" && touch from-main && git add from-main && {COMMIT} main && git push -q origin main"
        ),
    );
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "gate"
        && ev["phase"] == "merge"
        && ev["interrupted"] == "SIGTERM"));
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_failed_fetch_of_main_for_the_local_queue_needs_a_human_merge() {
    let e = Env::new();
    local_queue(&e, "gate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:script"]);
    e.ctl(
        "ship.sh",
        "git remote set-url origin \"$FAKE_CTRL/gone.git\"",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with(
            "cannot fetch main for the local merge queue: git fetch origin main failed"
        ),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_scratch_worktree_that_cannot_be_added_needs_a_human_merge() {
    let e = Env::new();
    local_queue(&e, "gate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    main_moves(&e, "from-main", "x\n");
    fs::create_dir_all(e.root.join(".git/ns")).unwrap();
    fs::write(e.root.join(".git/ns/merge-queue"), "in the way").unwrap();
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("cannot add a worktree for the local merge queue: cannot create"),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
}

#[test]
fn a_ratchet_over_the_line_on_the_merge_result_is_the_stuck_reason() {
    let e = Env::new();
    local_queue(
        &e,
        "\n[ratchets.flash]\ncommand = \"case $PWD in */ns/merge-queue/*) echo 200;; *) echo 100;; esac\"\nbetter = \"lower\"\n",
    );
    fs::write(
        e.root.join(".nightshift/ratchets.json"),
        r#"{"version": 1, "values": {"flash": 100}}"#,
    )
    .unwrap();
    e.queue("build", &["pass:commit", "fail"]);
    main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate failed on ratchet flash is worse: measured 200, recorded 100, tolerance 0, limit 100, lower is better",
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_gate_killed_on_the_merge_result_says_so_when_build_runs_out() {
    let e = Env::new();
    local_queue(
        &e,
        "gate = \"case $PWD in */ns/merge-queue/*) kill -9 $$;; esac\"\n",
    );
    e.queue("build", &["pass:commit", "fail"]);
    main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["phases"][5]["decision"], "the CI gate was killed by SIGKILL on the merge with main",
        "{v}"
    );
    assert_eq!(
        v["reason"], "build is out of attempts (2): the CI gate was killed by SIGKILL",
        "{v}"
    );
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", false), ("merge", false)])
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_moved_pr_head_runs_the_merge_result_gate_again() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12-1.json", RED);
    e.gh_file("checks-12-2.json", GREEN);
    let base = main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let merges: Vec<_> = gate_events(&e)
        .into_iter()
        .filter(|g| g["phase"] == "merge")
        .collect();
    assert_eq!(merges.len(), 2, "{merges:?}");
    assert_ne!(merges[0]["sha"], merges[1]["sha"]);
    assert_eq!(merge_gate_bases(&e), [base.clone(), base]);
}

#[test]
fn the_queue_leaves_the_worktree_on_the_pr_head_when_a_rebase_moved_head() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.factory(&format!("{AUTO}[phases.build]\ngate = \"true\"\n"));
        main_moves(e, "from-main", "x\n");
        e.gh_file(
            "hook.sh",
            "case \"$1 $2\" in \"pr merge\") git rev-parse HEAD > \"$FAKE_GH_DIR/head-at-merge\";; esac\n",
        );
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let old = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    let at_merge = fs::read_to_string(e.ghd.join("head-at-merge")).unwrap();
    assert_eq!(at_merge.trim(), old.trim());
    assert!(gate_events(&e).iter().any(|g| g["merge_result"] == true));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_moved_main_with_the_queue_off_gates_the_pr_head_only() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    e.factory(&format!(
        "{AUTO}{LEGACY}[phases.build]\n{}",
        where_gate(&log)
    ));
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:commit"]);
    main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\n");
    let gates = gate_events(&e);
    assert!(
        gates.iter().all(|g| g.get("merge_result").is_none()),
        "{gates:?}"
    );
    assert!(merge_gate_failures(&e).is_empty());
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_dirty_pr_under_the_queue_takes_the_conflict_path_without_a_merge_gate() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit", "fail"]);
    main_moves(&e, "from-main", "x\n");
    e.gh_file("pr-12.merge", "DIRTY");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let p = e.prompt(6, "build");
    assert!(p.contains("PR #12 conflicts with main"), "{p}");
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "conflict"));
    assert!(gate_events(&e).iter().all(|g| g["phase"] != "merge"));
    assert!(merge_gate_failures(&e).is_empty());
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_pr_head_that_cannot_be_checked_out_under_the_queue_needs_a_human_merge() {
    let lock_the_index = "touch \"$(git rev-parse --git-dir)/index.lock\"\n";
    let (e, v, _) = main_moves_after_review(
        &["pass:script"],
        PRINT_HEAD_SHA_CMD,
        &format!(
            "git update-ref refs/heads/main upstream\n\
             git -c user.name=f -c user.email=f@f rebase -q main\n{lock_the_index}"
        ),
        |e| {
            e.factory(&format!("{AUTO}[phases.build]\ngate = \"true\"\n"));
        },
    );
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("cannot check out PR #12 head"),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_killed_merge_gate_runs_again_on_an_unchanged_head_and_base() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    let marker = e.ctrl.join("killed-once");
    local_queue(
        &e,
        &format!(
            "max_attempts = 3\ngate = \"case $PWD in */ns/merge-queue/*) echo ran >> {}; test -f {m} || {{ touch {m}; kill -9 $$; }};; esac\"\n",
            log.display(),
            m = marker.display()
        ),
    );
    e.queue("build", &["pass:commit", "pass"]);
    main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "ran\nran\n");
    assert!(merge_gate_failures(&e).iter().all(|f| f["reused"] == false));
}

/// A merge-result gate that fails once the way `fail_once` says, then passes, logging each run.
fn flaky_merge_gate(e: &Env, limits: &str, fail_once: &str) -> PathBuf {
    let log = e.ctrl.join("gates");
    let marker = e.ctrl.join("failed-once");
    e.factory(&format!(
        "{AUTO}{limits}[phases.build]\nmax_attempts = 3\ngate = \"case $PWD in */ns/merge-queue/*) echo ran >> {}; test -f {m} || {{ touch {m}; {fail_once}; }};; esac\"\n",
        log.display(),
        m = marker.display()
    ));
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:commit", "pass"]);
    main_moves(e, "from-main", "x\n");
    log
}

#[test]
fn a_timed_out_merge_gate_runs_again_on_an_unchanged_head_and_base() {
    let e = Env::new();
    let log = flaky_merge_gate(&e, "", "sleep 30");
    let out = e
        .ns()
        .env("NS_GATE_TIMEOUT_MS", "1500")
        .args(["run", "--issue", "7"])
        .output();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "ran\nran\n");
    assert!(merge_gate_failures(&e).iter().all(|f| f["reused"] == false));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_merge_gate_past_the_memory_cap_runs_again_on_an_unchanged_head_and_base() {
    let e = Env::new();
    let log = flaky_merge_gate(
        &e,
        "[limits]\nmemory_mb = 64\n",
        "python3 -c 'b = bytes(range(256)) * (1 << 20)'",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "ran\nran\n");
    assert!(merge_gate_failures(&e).iter().all(|f| f["reused"] == false));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_red_gate_on_an_unmoved_main_logs_merge_gate_failed() {
    // Ship rebased the branch, so the PR head is the older commit, which the gate fails.
    let gate = "gate = \"test -f UPSTREAM || test ! -f $FAKE_GH_DIR/pr-12.shipped\"\n";
    let (e, v, _) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.factory(&format!("{AUTO}[phases.build]\n{gate}"));
        e.queue("build", &["pass:commit", "fail"]);
    });
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert!(!e.gh_calls().contains("pr merge"));
    let failed = merge_gate_failures(&e);
    assert!(!failed.is_empty(), "{failed:?}");
    assert!(failed.iter().all(|f| f["reused"] == false), "{failed:?}");
    assert_eq!(failed[0]["pr"], 12);
    assert!(failed[0]["sha"].is_string(), "{failed:?}");
    assert!(failed[0]["base"].is_string(), "{failed:?}");
    let p = e.prompt(6, "build");
    assert!(!p.contains("combining it with main"), "{p}");
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}
