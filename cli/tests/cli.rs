use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn ns() -> Command {
    let mut c = Command::cargo_bin("ns").unwrap();
    c.env_remove("NS_CONFIG").env_remove("XDG_CONFIG_HOME");
    c
}

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

/// A temp dir holding `myrepo/` with one commit on `main`.
fn repo() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap().join("myrepo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(root.join("README"), "hi\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "init"]);
    (tmp, root)
}

fn json(out: &[u8]) -> Value {
    serde_json::from_slice(out).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(out)))
}

#[test]
fn worktree_new_is_idempotent() {
    let (tmp, root) = repo();
    let first = ns()
        .current_dir(&root)
        .args(["worktree", "new", "142-uart-timeout"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v = json(&first);
    let expected = tmp
        .path()
        .canonicalize()
        .unwrap()
        .join("myrepo.worktrees/142-uart-timeout");
    assert_eq!(v["unit"], "142-uart-timeout");
    assert_eq!(v["branch"], "ns/142-uart-timeout");
    assert_eq!(v["path"], expected.to_str().unwrap());
    assert!(expected.join(".ns/142-uart-timeout").is_dir());
    assert_eq!(
        git(&expected, &["branch", "--show-current"]),
        "ns/142-uart-timeout"
    );

    // Run again, this time from inside the new worktree: same JSON, one exclude line.
    let second = ns()
        .current_dir(&expected)
        .args(["worktree", "new", "142-uart-timeout"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(first, second);
    let exclude = fs::read_to_string(root.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.lines().filter(|l| *l == ".ns/").count(), 1);
    assert_eq!(git(&expected, &["status", "--porcelain"]), "");
}

#[test]
fn worktree_new_with_base_and_repo_flag() {
    let (_tmp, root) = repo();
    git(&root, &["branch", "develop"]);
    let out = ns()
        .args(["worktree", "new", "x1", "--base", "develop", "--repo"])
        .arg(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json(&out)["branch"], "ns/x1");

    ns().current_dir(&root)
        .args(["worktree", "new", "x2", "--base", "nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ns worktree new x2 --base main"));
}

#[test]
fn worktree_new_rejects_bad_unit_id() {
    let (_tmp, root) = repo();
    ns().current_dir(&root)
        .args(["worktree", "new", "Bad_Id"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ns worktree new 142-uart-timeout"));
}

#[test]
fn worktree_list_and_remove() {
    let (_tmp, root) = repo();
    let out = ns()
        .current_dir(&root)
        .args(["worktree", "new", "7-thing"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let wt = PathBuf::from(json(&out)["path"].as_str().unwrap());
    fs::write(
        wt.join(".ns/7-thing/build.md"),
        "---\nunit: 7-thing\nphase: build\nstatus: pass\nupdated: 2026-10-08T21:14:00Z\n---\n",
    )
    .unwrap();

    let list = json(
        &ns()
            .current_dir(&root)
            .args(["worktree", "list"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1, "main worktree must not be listed");
    assert_eq!(arr[0]["unit"], "7-thing");
    assert_eq!(arr[0]["status"]["phase"], "build");
    assert_eq!(arr[0]["status"]["status"], "pass");

    // Dirty: refused without --force.
    fs::write(wt.join("new.txt"), "x").unwrap();
    ns().current_dir(&root)
        .args(["worktree", "remove", "7-thing"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "ns worktree remove 7-thing --force",
        ));

    // Dry run leaves it in place.
    let dry = json(
        &ns()
            .current_dir(&root)
            .args(["worktree", "remove", "7-thing", "--dry-run", "--force"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(dry["dry_run"], true);
    assert!(wt.exists());

    // Clean (artifacts are ignored): removes without --force, keeps the branch.
    fs::remove_file(wt.join("new.txt")).unwrap();
    ns().current_dir(&root)
        .args(["worktree", "remove", "7-thing"])
        .assert()
        .success();
    assert!(!wt.exists());
    assert!(git(&root, &["branch", "--list", "ns/7-thing"]).contains("ns/7-thing"));

    // Idempotent: second remove is a no-op.
    let again = json(
        &ns()
            .current_dir(&root)
            .args(["worktree", "remove", "7-thing"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(again["action"], "none");

    // new again reuses the kept branch.
    ns().current_dir(&root)
        .args(["worktree", "new", "7-thing"])
        .assert()
        .success();
    assert!(wt.join(".ns/7-thing").is_dir());
}

#[test]
fn worktree_outside_repo_fails_with_hint() {
    let tmp = tempfile::tempdir().unwrap();
    ns().current_dir(tmp.path())
        .args(["worktree", "list"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--repo"));
}

fn write_skill(root: &Path, dir: &str, text: &str) {
    fs::create_dir_all(root.join(dir)).unwrap();
    fs::write(root.join(dir).join("SKILL.md"), text).unwrap();
}

fn fixture_skills() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let s = tmp.path();
    write_skill(
        s,
        "ns-good",
        "---\nname: ns-good\ndescription: A good skill.\nmetadata:\n  upstream:\n    - mattpocock/skills@b0618bc436ad:skills/x\n    - cursor/plugins@ccb5507cec15:y\n---\nSee [ref](./references/r.md), [other](../ns-other/SKILL.md#x), [web](https://x.y).\n",
    );
    fs::create_dir_all(s.join("ns-good/references")).unwrap();
    fs::write(
        s.join("ns-good/references/r.md"),
        "back to [skill](../SKILL.md)\n",
    )
    .unwrap();
    write_skill(
        s,
        "ns-other",
        "---\nname: ns-other\ndescription: Other.\n---\n",
    );
    write_skill(
        s,
        "ns-bad",
        "---\nname: wrong\ndescription: \"\"\nmetadata:\n  upstream: mattpocock/skills@main:x\n---\n[gone](./missing.md)\n",
    );
    write_skill(s, "ns-broken-yaml", "---\nname: [oops\n---\n");
    fs::create_dir_all(s.join("not-a-skill")).unwrap();
    tmp
}

#[test]
fn lint_reports_good_and_bad_skills() {
    let tmp = fixture_skills();
    let out = ns()
        .arg("lint")
        .arg(tmp.path())
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v = json(&out);
    assert_eq!(v["ok"], false);
    assert_eq!(v["skills"], 4);
    let errors = v["errors"].as_array().unwrap();
    let for_skill = |s: &str| {
        errors
            .iter()
            .filter(|e| e["skill"] == s)
            .map(|e| e["message"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert!(
        for_skill("ns-good").is_empty(),
        "{:?}",
        for_skill("ns-good")
    );
    assert!(for_skill("ns-other").is_empty());
    let bad = for_skill("ns-bad");
    assert!(
        bad.iter().any(|m| m.contains("does not match directory")),
        "{bad:?}"
    );
    assert!(bad.iter().any(|m| m.contains("must start with")), "{bad:?}");
    assert!(
        bad.iter().any(|m| m.contains("description is empty")),
        "{bad:?}"
    );
    assert!(
        bad.iter().any(|m| m.contains("metadata.upstream")),
        "{bad:?}"
    );
    assert!(
        bad.iter().any(|m| m.contains("broken link: ./missing.md")),
        "{bad:?}"
    );
    assert!(for_skill("ns-broken-yaml")[0].contains("YAML"));

    ns().arg("lint")
        .arg(tmp.path())
        .arg("--human")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("4 skills checked"));
}

#[test]
fn lint_passes_clean_dir_and_rejects_missing_dir() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "ns-a",
        "---\nname: ns-a\ndescription: A.\n---\n",
    );
    let v = json(
        &ns()
            .arg("lint")
            .arg(tmp.path())
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["ok"], true);
    assert_eq!(v["skills"], 1);

    ns().arg("lint")
        .arg(tmp.path().join("nope"))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ns lint skills"));
}

#[test]
fn install_dry_run_touches_nothing() {
    let skills = fixture_skills();
    let home = tempfile::tempdir().unwrap();
    let out = ns()
        .env("HOME", home.path())
        .args(["install", "--dry-run", "--source"])
        .arg(skills.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v = json(&out);
    assert_eq!(v["dry_run"], true);
    let targets = v["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    assert!(targets[0].as_str().unwrap().ends_with(".agents/skills"));
    assert!(targets[1].as_str().unwrap().ends_with(".claude/skills"));
    assert!(v["actions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["action"] == "link"));
    assert_eq!(v["actions"].as_array().unwrap().len(), 8);
    assert!(!home.path().join(".agents").exists());
    assert!(!home.path().join(".claude").exists());
}

#[test]
fn install_links_relinks_and_reports_conflicts() {
    let skills = fixture_skills();
    let target = tempfile::tempdir().unwrap();
    let t = target.path();
    // A stale link we own, and a real directory we must not clobber.
    std::os::unix::fs::symlink("/old/checkout/ns-good", t.join("ns-good")).unwrap();
    fs::create_dir(t.join("ns-other")).unwrap();

    let run = || {
        ns().args(["install", "--source"])
            .arg(skills.path())
            .arg("--target")
            .arg(t)
            .assert()
    };
    let v = json(&run().code(1).get_output().stdout);
    let action = |skill: &str| {
        v["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["skill"] == skill)
            .unwrap()["action"]
            .clone()
    };
    assert_eq!(action("ns-good"), "relink");
    assert_eq!(action("ns-other"), "conflict");
    assert_eq!(action("ns-bad"), "link");
    assert_eq!(v["conflicts"], 1);
    assert_eq!(
        fs::read_link(t.join("ns-good")).unwrap(),
        skills.path().canonicalize().unwrap().join("ns-good")
    );

    fs::remove_dir(t.join("ns-other")).unwrap();
    let v = json(&run().success().get_output().stdout);
    let counts = |name: &str| {
        v["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["action"] == name)
            .count()
    };
    assert_eq!(counts("unchanged"), 3);
    assert_eq!(counts("link"), 1);
}

fn config(dir: &Path) -> PathBuf {
    let p = dir.join("config.toml");
    fs::write(
        &p,
        r#"
[harness.echo]
command = ["cat"]
command_write = ["sh", "-c", "pwd; cat"]
[harness.fails]
command = ["false"]
[harness.ghost]
command = ["ns-test-no-such-binary"]

[roles.default]
harness = "codex"
model = "gpt-test"
[roles.review]
harness = "echo"
[roles."review.flaky"]
harness = "fails"
[roles.verify]
harness = "ghost"
"#,
    )
    .unwrap();
    p
}

#[test]
fn ask_dry_run_resolves_role_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    let v = json(
        &ns()
            .env("NS_CONFIG", &cfg)
            .args(["ask", "--role", "review.security", "--dry-run"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["matched_role"], "review");
    assert_eq!(v["command"], serde_json::json!(["cat"]));

    let v = json(
        &ns()
            .env("NS_CONFIG", &cfg)
            .args(["ask", "--role", "build", "--dry-run"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["matched_role"], "default");
    assert_eq!(
        v["command"],
        serde_json::json!(["codex", "exec", "--sandbox", "read-only", "-m", "gpt-test"])
    );
}

#[test]
fn ask_write_mode_and_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    let work = tmp.path().canonicalize().unwrap().join("work");
    fs::create_dir(&work).unwrap();

    let v = json(
        &ns()
            .env("NS_CONFIG", &cfg)
            .args(["ask", "--role", "build", "--write", "--dry-run", "--cwd"])
            .arg(&work)
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["mode"], "write");
    assert_eq!(v["cwd"], work.to_str().unwrap());
    assert_eq!(
        v["command"],
        serde_json::json!([
            "codex",
            "exec",
            "--sandbox",
            "workspace-write",
            "-m",
            "gpt-test"
        ])
    );

    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review", "--write", "--cwd"])
        .arg(&work)
        .write_stdin("p\n")
        .assert()
        .success()
        .stdout(format!("{}\np\n", work.display()));

    // Harness with no command_write: exit 5 with a snippet.
    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review.flaky", "--write"])
        .write_stdin("x")
        .assert()
        .code(5)
        .stderr(predicate::str::contains("command_write = ["));

    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review", "--cwd"])
        .arg(tmp.path().join("nope"))
        .write_stdin("x")
        .assert()
        .code(2);
}

#[test]
fn ask_xdg_config_home() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("nightshift")).unwrap();
    fs::rename(
        config(tmp.path()),
        tmp.path().join("nightshift/config.toml"),
    )
    .unwrap();
    ns().env("XDG_CONFIG_HOME", tmp.path())
        .args(["ask", "--role", "review", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"matched_role\": \"review\""));
}

#[test]
fn ask_runs_harness_and_maps_exit_codes() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review"])
        .write_stdin("hello harness\n")
        .assert()
        .success()
        .stdout("hello harness\n");

    let prompt = tmp.path().join("p.md");
    fs::write(&prompt, "from file\n").unwrap();
    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review.spec", "--prompt-file"])
        .arg(&prompt)
        .assert()
        .success()
        .stdout("from file\n");

    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review.flaky"])
        .write_stdin("x")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("exited with 1"));

    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "verify"])
        .write_stdin("x")
        .assert()
        .code(4)
        .stderr(predicate::str::contains("not on PATH"));
}

#[test]
fn ask_unconfigured_exits_3_with_example() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("c.toml");
    fs::write(&cfg, "[roles.review]\nharness = \"claude\"\n").unwrap();
    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "verify", "--dry-run"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("[roles.default]"));

    ns().env("NS_CONFIG", tmp.path().join("missing.toml"))
        .args(["ask", "--role", "verify"])
        .write_stdin("x")
        .assert()
        .code(3);
}

#[test]
fn doctor_reports_config() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    let v = json(
        &ns()
            .env("NS_CONFIG", &cfg)
            .arg("doctor")
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["config"]["exists"], true);
    assert_eq!(v["config"]["parses"], true);
    let roles = v["roles"].as_array().unwrap();
    assert_eq!(roles.len(), 4);
    let default = roles.iter().find(|r| r["name"] == "default").unwrap();
    assert_eq!(default["harness"], "codex");
    assert_eq!(default["model"], "gpt-test");
    for h in ["claude", "codex", "cursor-agent", "gemini", "opencode"] {
        assert!(v["harnesses"][h]["on_path"].is_boolean());
    }
}

#[test]
fn every_subcommand_help_has_examples() {
    for args in [
        vec!["--help"],
        vec!["worktree", "--help"],
        vec!["worktree", "new", "--help"],
        vec!["worktree", "list", "--help"],
        vec!["worktree", "remove", "--help"],
        vec!["worktree", "setup", "--help"],
        vec!["eval", "--help"],
        vec!["ask", "--help"],
        vec!["lint", "--help"],
        vec!["install", "--help"],
        vec!["doctor", "--help"],
    ] {
        ns().args(&args)
            .assert()
            .success()
            .stdout(predicate::str::contains("Examples:"));
    }
}

fn factory_def(root: &Path, text: &str) {
    fs::create_dir_all(root.join(".nightshift")).unwrap();
    fs::write(root.join(".nightshift/nightshift.toml"), text).unwrap();
}

#[test]
fn worktree_new_runs_setup_once() {
    let (_tmp, root) = repo();
    factory_def(
        &root,
        "name = \"demo\"\n\n[worktree]\nsetup = [\"echo \\\"$NS_UNIT|$NS_WORKTREE|$NS_MAIN_ROOT\\\" > setup.out\", \"echo noisy\"]\n",
    );
    let out = ns()
        .current_dir(&root)
        .args(["worktree", "new", "u1"])
        .assert()
        .success()
        .stderr(predicate::str::contains("noisy"))
        .get_output()
        .stdout
        .clone();
    let v = json(&out);
    let path = PathBuf::from(v["path"].as_str().unwrap());
    assert_eq!(v["setup"].as_array().unwrap().len(), 2);
    assert_eq!(v["setup"][0]["exit"], 0);
    assert_eq!(v["setup"][1]["run"], "echo noisy");
    assert_eq!(
        fs::read_to_string(path.join("setup.out")).unwrap().trim(),
        format!("u1|{}|{}", path.display(), root.display())
    );

    // Idempotent repeat: no setup.
    fs::remove_file(path.join("setup.out")).unwrap();
    let v = json(
        &ns()
            .current_dir(&root)
            .args(["worktree", "new", "u1"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["setup"], serde_json::json!([]));
    assert!(!path.join("setup.out").exists());

    // --no-setup on a fresh unit.
    let v = json(
        &ns()
            .current_dir(&root)
            .args(["worktree", "new", "u2", "--no-setup"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["setup"], serde_json::json!([]));
    assert!(!PathBuf::from(v["path"].as_str().unwrap())
        .join("setup.out")
        .exists());
}

#[test]
fn worktree_setup_failure_keeps_worktree_and_retries() {
    let (_tmp, root) = repo();
    factory_def(
        &root,
        "[worktree]\nsetup = [\"test -f ready\", \"touch done\"]\n",
    );
    let out = ns()
        .current_dir(&root)
        .args(["worktree", "new", "u3"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("ns worktree setup u3"))
        .get_output()
        .stdout
        .clone();
    let v = json(&out);
    let path = PathBuf::from(v["path"].as_str().unwrap());
    assert!(path.is_dir());
    assert_eq!(v["setup"].as_array().unwrap().len(), 1);
    assert_eq!(v["setup"][0]["exit"], 1);
    assert!(!path.join("done").exists());

    fs::write(path.join("ready"), "").unwrap();
    let v = json(
        &ns()
            .current_dir(&root)
            .args(["worktree", "setup", "u3"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["setup"][1]["exit"], 0);
    assert!(path.join("done").exists());

    ns().current_dir(&root)
        .args(["worktree", "setup", "nope"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("ns worktree new nope"));

    factory_def(&root, "[worktree]\nsetpu = []\n");
    ns().current_dir(&root)
        .args(["worktree", "new", "u4"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot parse"));
    assert!(!root.parent().unwrap().join("myrepo.worktrees/u4").exists());
}
