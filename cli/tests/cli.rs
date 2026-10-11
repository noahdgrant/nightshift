mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use common::Ns;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn ns() -> Ns {
    let mut c = common::ns();
    c.env_remove("NS_CONFIG").env_remove("XDG_CONFIG_HOME");
    c
}

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    StdCommand::new("git")
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
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_output(dir, args);
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
fn lint_passes_this_repos_local_skills() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let v = json(
        &ns()
            .arg("lint")
            .arg(src.join(".agents/skills"))
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["skills"].as_u64().unwrap() > 0, "{v}");
}

#[test]
fn lint_fails_misordered_feature_h2s_naming_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join(".agents/skills");
    write_skill(
        &local,
        "verify-x",
        "---\nname: verify-x\ndescription: X.\n---\n",
    );
    let features = local.join("verify-x/features");
    fs::create_dir_all(&features).unwrap();
    fs::write(
        features.join("x.md"),
        "# F\n\n## Gotchas\n\n## Sub-features\n\n## How to get to it (user POV)\n\n## Driving it with t\n",
    )
    .unwrap();

    let v = json(
        &ns()
            .arg("lint")
            .arg(&local)
            .assert()
            .code(1)
            .get_output()
            .stdout,
    );
    assert_eq!(v["ok"], false, "{v}");
    let errors = v["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{v}");
    assert!(
        errors[0]["file"]
            .as_str()
            .unwrap()
            .ends_with("features/x.md"),
        "{v}"
    );
}

#[test]
fn lint_human_names_the_bad_feature_file_and_the_rule() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join(".agents/skills");
    write_skill(
        &local,
        "verify-x",
        "---\nname: verify-x\ndescription: X.\n---\n",
    );
    let features = local.join("verify-x/features");
    fs::create_dir_all(&features).unwrap();
    fs::write(features.join("x.md"), "# F\n\n## Nope\n").unwrap();

    ns().arg("lint")
        .arg(&local)
        .arg("--human")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("features/x.md"))
        .stdout(predicate::str::contains("H2 headings must be"));
}

#[test]
fn lint_passes_relative_repo_local_skills_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join(".agents/skills");
    write_skill(
        &local,
        "verify-x",
        "---\nname: verify-x\ndescription: X.\n---\n",
    );

    let v = json(
        &ns()
            .current_dir(tmp.path())
            .args(["lint", ".agents/skills"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["skills"].as_u64().unwrap() > 0, "{v}");

    let v = json(
        &ns()
            .current_dir(&local)
            .args(["lint", "."])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["skills"].as_u64().unwrap() > 0, "{v}");
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

fn install_targets(home: &Path, args: &[&str]) -> Vec<String> {
    let skills = fixture_skills();
    let v = json(
        &ns()
            .env("HOME", home)
            .args(["install", "--dry-run", "--source"])
            .arg(skills.path())
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    v["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn install_harness_picks_each_harness_skill_dir() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path().to_string_lossy();
    let dir = |d: &str| format!("{h}/{d}");
    assert_eq!(
        install_targets(home.path(), &["--harness", "cursor"]),
        [dir(".cursor/skills")]
    );
    assert_eq!(
        install_targets(home.path(), &["--harness", "claude"]),
        [dir(".claude/skills")]
    );
    // Codex reads the shared ~/.agents/skills, so it and `agents` name one target.
    assert_eq!(
        install_targets(
            home.path(),
            &[
                "--harness",
                "codex",
                "--harness",
                "agents",
                "--harness",
                "cursor"
            ]
        ),
        [dir(".agents/skills"), dir(".cursor/skills")]
    );
    // --harness and --target add up.
    assert_eq!(
        install_targets(
            home.path(),
            &["--harness", "claude", "--target", "/x/skills"]
        ),
        [dir(".claude/skills"), "/x/skills".to_string()]
    );
    assert_eq!(
        install_targets(home.path(), &["--target", "/x/skills"]),
        ["/x/skills"]
    );
    assert!(!home.path().join(".cursor").exists());
}

#[test]
fn install_harness_links_into_a_temp_home() {
    let skills = fixture_skills();
    let home = tempfile::tempdir().unwrap();
    ns().env("HOME", home.path())
        .args(["install", "--harness", "cursor", "--source"])
        .arg(skills.path())
        .assert()
        .success();
    assert_eq!(
        fs::read_link(home.path().join(".cursor/skills/ns-good")).unwrap(),
        skills.path().canonicalize().unwrap().join("ns-good")
    );
    assert!(!home.path().join(".claude").exists());

    ns().env("HOME", home.path())
        .args(["install", "--harness", "copilot"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("claude, codex, cursor, agents"));
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

const ASK_API_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "CODEX_API_KEY",
];

/// Run `ns ask` in `repo` with every API variable set, through a fake harness named `name`
/// that prints its environment, and return the variable names that reached it.
fn ask_api_env_seen(repo: &Path, name: &str) -> Vec<String> {
    ask_api_env_seen_from(repo, repo, name)
}

/// As [`ask_api_env_seen`], but `ns` starts in `start` and gets `--cwd <repo>` when they differ.
fn ask_api_env_seen_from(start: &Path, repo: &Path, name: &str) -> Vec<String> {
    let bin = repo.parent().unwrap().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fake = bin.join(name);
    fs::write(&fake, "#!/bin/sh\ncat >/dev/null\nenv\n").unwrap();
    let mut perms = fs::metadata(&fake).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&fake, perms).unwrap();
    let cfg = repo.parent().unwrap().join("config.toml");
    fs::write(
        &cfg,
        format!(
            "[harness.fake]\ncommand = [{:?}]\n[roles.default]\nharness = \"fake\"\n",
            fake.to_str().unwrap()
        ),
    )
    .unwrap();
    let mut c = ns();
    c.env("NS_CONFIG", &cfg).current_dir(start);
    for k in ASK_API_ENV {
        c.env(k, "x");
    }
    c.args(["ask", "--role", "review"]);
    if start != repo {
        c.arg("--cwd").arg(repo);
    }
    let out = c
        .write_stdin("p\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    ASK_API_ENV
        .iter()
        .filter(|k| out.lines().any(|l| l.starts_with(&format!("{k}="))))
        .map(|k| k.to_string())
        .collect()
}

fn write_factory(repo: &Path, toml: &str) {
    fs::create_dir_all(repo.join(".nightshift")).unwrap();
    fs::write(repo.join(".nightshift/nightshift.toml"), toml).unwrap();
}

#[test]
fn ask_strips_api_env_under_subscription_billing() {
    let (_tmp, root) = repo();
    write_factory(&root, "[defaults]\nbilling = \"subscription\"\n");
    assert_eq!(
        ask_api_env_seen(&root, "claude"),
        ["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"]
    );
    assert_eq!(ask_api_env_seen(&root, "codex"), &ASK_API_ENV[..6]);
    // Another harness is left alone: only claude and codex have known API variables.
    assert_eq!(ask_api_env_seen(&root, "other"), ASK_API_ENV);
}

#[test]
fn ask_strips_api_env_without_a_factory_file() {
    let (_tmp, root) = repo();
    assert_eq!(
        ask_api_env_seen(&root, "claude"),
        ["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"]
    );
    assert_eq!(ask_api_env_seen(&root, "codex"), &ASK_API_ENV[..6]);
}

#[test]
fn ask_strips_api_env_outside_a_git_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap().join("plain");
    fs::create_dir(&dir).unwrap();
    assert_eq!(ask_api_env_seen(&dir, "codex"), &ASK_API_ENV[..6]);
}

#[test]
fn ask_strips_api_env_inside_a_bare_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().canonicalize().unwrap().join("bare.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare"]);
    assert_eq!(ask_api_env_seen(&bare, "codex"), &ASK_API_ENV[..6]);
}

#[test]
fn ask_dry_run_ignores_a_malformed_factory_file() {
    let (_tmp, root) = repo();
    write_factory(&root, "[defaults\n");
    let cfg = config(root.parent().unwrap());
    ns().env("NS_CONFIG", &cfg)
        .current_dir(&root)
        .args(["ask", "--role", "review", "--dry-run"])
        .assert()
        .success();
}

#[test]
fn ask_keeps_api_env_under_api_billing() {
    let (_tmp, root) = repo();
    write_factory(&root, "[defaults]\nbilling = \"api\"\n");
    assert_eq!(ask_api_env_seen(&root, "claude"), ASK_API_ENV);
    assert_eq!(ask_api_env_seen(&root, "codex"), ASK_API_ENV);
}

#[test]
fn ask_reads_billing_from_the_main_checkout_of_cwd() {
    let (_tmp, root) = repo();
    write_factory(&root, "[defaults]\nbilling = \"api\"\n");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "factory"]);
    let wt = root.parent().unwrap().join("wt");
    git(&root, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    fs::remove_dir_all(wt.join(".nightshift")).unwrap();
    assert_eq!(ask_api_env_seen(&wt, "claude"), ASK_API_ENV);

    // --cwd, not the directory ns starts in, picks the repo.
    let plain = root.parent().unwrap().join("plain");
    fs::create_dir(&plain).unwrap();
    assert_eq!(ask_api_env_seen_from(&plain, &wt, "claude"), ASK_API_ENV);
    assert_eq!(
        ask_api_env_seen_from(&wt, &plain, "codex"),
        &ASK_API_ENV[..6]
    );
}

#[test]
fn ask_fails_on_a_malformed_factory_file() {
    let (_tmp, root) = repo();
    write_factory(&root, "[defaults\n");
    let cfg = config(root.parent().unwrap());
    ns().env("NS_CONFIG", &cfg)
        .current_dir(&root)
        .args(["ask", "--role", "review"])
        .write_stdin("x")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("nightshift.toml"));
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
            .current_dir(tmp.path())
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
    assert_eq!(v["claude_phases"]["auto_memory"], false);
    assert_eq!(
        v["claude_phases"]["env"],
        "CLAUDE_CODE_DISABLE_AUTO_MEMORY=1"
    );
}

#[test]
fn doctor_reports_which_harness_skill_dirs_hold_the_ns_skills() {
    let skills = fixture_skills();
    let home = tempfile::tempdir().unwrap();
    ns().env("HOME", home.path())
        .args(["install", "--harness", "cursor", "--source"])
        .arg(skills.path())
        .assert()
        .success();
    let cursor = home.path().join(".cursor/skills");
    // Neither a dangling ns-* link nor a non-ns skill counts.
    std::os::unix::fs::symlink("/gone/ns-old", cursor.join("ns-old")).unwrap();
    fs::create_dir(cursor.join("other")).unwrap();
    fs::write(cursor.join("other/SKILL.md"), "x").unwrap();

    let v = json(
        &ns()
            .current_dir(home.path())
            .env("HOME", home.path())
            .arg("doctor")
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    let dirs = &v["skill_dirs"];
    assert_eq!(dirs["cursor"]["path"], cursor.to_string_lossy().as_ref());
    assert_eq!(dirs["cursor"]["ns_skills"], 4);
    for h in ["claude", "codex", "agents"] {
        assert_eq!(dirs[h]["ns_skills"], 0, "{h}");
    }
    assert!(dirs["codex"]["path"]
        .as_str()
        .unwrap()
        .ends_with(".agents/skills"));
}

#[test]
fn doctor_reports_whether_core_hooks_path_points_at_githooks() {
    let (_tmp, root) = repo();
    fs::create_dir(root.join(".githooks")).unwrap();
    let doctor = || {
        json(
            &ns()
                .current_dir(&root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .arg("doctor")
                .assert()
                .success()
                .get_output()
                .stdout,
        )
    };
    let fix = "git config core.hooksPath .githooks";

    let v = doctor();
    assert_eq!(v["hooks"]["ok"], false);
    assert_eq!(v["hooks"]["path"], Value::Null);
    assert_eq!(v["hooks"]["fix"], fix);
    assert_eq!(v["ok"], false);
    assert!(v["problems"].to_string().contains(fix), "{}", v["problems"]);

    git(&root, &["config", "core.hooksPath", "elsewhere"]);
    let v = doctor();
    assert_eq!(v["hooks"]["ok"], false);
    assert_eq!(v["hooks"]["path"], "elsewhere");
    assert!(v["problems"].to_string().contains(fix), "{}", v["problems"]);

    fs::create_dir(root.join("other")).unwrap();
    git(&root, &["config", "core.hooksPath", "other"]);
    let v = doctor();
    assert_eq!(v["hooks"]["ok"], false);
    assert_eq!(v["hooks"]["path"], "other");
    assert_eq!(v["hooks"]["fix"], fix);
    assert!(v["problems"].to_string().contains(fix), "{}", v["problems"]);

    for set in [
        ".githooks",
        "./.githooks/",
        root.join(".githooks").to_str().unwrap(),
    ] {
        git(&root, &["config", "core.hooksPath", set]);
        let v = doctor();
        assert_eq!(v["hooks"]["ok"], true, "{set}");
        assert_eq!(v["hooks"]["path"], set);
        assert_eq!(v["hooks"]["fix"], Value::Null);
        assert!(!v["problems"].to_string().contains("hooksPath"), "{set}");
    }
}

#[test]
fn doctor_checks_hooks_path_from_a_subdirectory() {
    let (_tmp, root) = repo();
    fs::create_dir(root.join(".githooks")).unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    let v = json(
        &ns()
            .current_dir(root.join("sub"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("doctor")
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["hooks"]["ok"], false);
    assert_eq!(v["hooks"]["path"], Value::Null);
}

#[test]
fn doctor_skips_the_hooks_check_without_a_githooks_dir() {
    let (_tmp, root) = repo();
    let outside = tempfile::tempdir().unwrap();
    for dir in [root.as_path(), outside.path()] {
        let v = json(
            &ns()
                .current_dir(dir)
                .arg("doctor")
                .assert()
                .success()
                .get_output()
                .stdout,
        );
        assert_eq!(v["hooks"]["ok"], true, "{}", dir.display());
        assert_eq!(v["hooks"]["fix"], Value::Null);
        assert!(!v["problems"].to_string().contains("hooksPath"));
    }
}

fn executable(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    StdCommand::new("chmod")
        .arg("+x")
        .arg(path)
        .status()
        .unwrap();
}

#[test]
fn doctor_reports_forge_account_but_never_the_token() {
    const TOKEN: &str = "fake-token-77d1e0";
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("gh"),
        &format!(
            r#"[ "$*" = "api user --jq .login" ] && [ "$GH_TOKEN" = "{TOKEN}" ] && echo octocat"#
        ),
    );
    let good = tmp.path().join("token.sh");
    executable(&good, &format!("echo {TOKEN}"));
    let bad = tmp.path().join("bad.sh");
    executable(&bad, &format!("echo {TOKEN}; exit 1"));
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let doctor = |cfg: &str| {
        let p = tmp.path().join("config.toml");
        fs::write(&p, cfg).unwrap();
        let out = ns()
            .current_dir(tmp.path())
            .env("NS_CONFIG", &p)
            .env("PATH", &path)
            .env_remove("GH_TOKEN")
            .env_remove("GITLAB_TOKEN")
            .arg("doctor")
            .assert()
            .get_output()
            .clone();
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!all.contains(TOKEN), "{all}");
        json(&out.stdout)
    };

    let v = doctor(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        good.to_str().unwrap()
    ));
    assert_eq!(
        v["forge"]["github"],
        serde_json::json!({"configured": true, "token_resolved": true, "account": "octocat",
            "table": "[forge.github]", "repo": null, "repo_readable": null, "ssh": null})
    );
    assert_eq!(
        v["forge"]["gitlab"],
        serde_json::json!({"configured": false, "token_resolved": false, "account": null,
            "table": null, "repo": null, "repo_readable": null, "ssh": null})
    );
    assert_eq!(v["warnings"], serde_json::json!([]));

    let v = doctor(&format!(
        "[forge.github]\ntoken_command = {:?}\n",
        bad.to_str().unwrap()
    ));
    assert_eq!(v["ok"], false);
    assert_eq!(v["forge"]["github"]["token_resolved"], false);
    assert_eq!(v["forge"]["github"]["account"], Value::Null);
    let problems = v["problems"].to_string();
    assert!(problems.contains("forge github"), "{problems}");
}

#[test]
fn doctor_with_a_set_gh_token_runs_no_token_command() {
    const TOKEN: &str = "fake-token-5be2c9";
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("gh"),
        &format!(
            r#"[ "$*" = "api user --jq .login" ] && [ "$GH_TOKEN" = "{TOKEN}" ] && echo octocat"#
        ),
    );
    let marker = tmp.path().join("ran");
    let cmd = tmp.path().join("token.sh");
    executable(&cmd, &format!("touch {}; exit 1", marker.display()));
    let cfg = tmp.path().join("config.toml");
    fs::write(
        &cfg,
        format!(
            "[forge.github]\ntoken_command = {:?}\n",
            cmd.to_str().unwrap()
        ),
    )
    .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = ns()
        .current_dir(tmp.path())
        .env("NS_CONFIG", &cfg)
        .env("PATH", &path)
        .env("GH_TOKEN", TOKEN)
        .env_remove("GITLAB_TOKEN")
        .arg("doctor")
        .assert()
        .get_output()
        .clone();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!all.contains(TOKEN), "{all}");
    assert!(
        !marker.exists(),
        "token_command ran though GH_TOKEN was set"
    );
    let v = json(&out.stdout);
    assert_eq!(
        v["forge"]["github"],
        serde_json::json!({"configured": true, "token_resolved": true, "account": "octocat",
            "table": "[forge.github]", "repo": null, "repo_readable": null, "ssh": null})
    );
}

const WORK_CONFIG: &str = "[forge.github]
token_env = \"PERSONAL\"
[forge.github.owners.\"example-org\"]
token_env = \"WORK\"
";

/// One `ns doctor` run in a repo whose origin is `origin`, with a fake `gh` whose token is account
/// `login` and can read `example-org/backend` unless `unreadable` (its 404 echoes the token, as
/// a leaky server might), and a fake `ssh` that greets `ssh_account`.
struct Doctor<'a> {
    origin: &'a str,
    config: &'a str,
    env: &'a [(&'a str, &'a str)],
    login: &'a str,
    unreadable: bool,
    ssh_account: &'a str,
}

impl Default for Doctor<'_> {
    fn default() -> Self {
        Doctor {
            origin: "git@github-work:example-org/backend.git",
            config: WORK_CONFIG,
            env: &[("PERSONAL", "personal-tok"), ("WORK", "work-tok")],
            login: "work-account",
            unreadable: false,
            ssh_account: "work-account",
        }
    }
}

impl Doctor<'_> {
    /// The JSON, stderr, and the arguments ssh got (empty when it never ran).
    fn run(&self) -> (Value, String, String) {
        let (tmp, root) = repo();
        git(&root, &["remote", "add", "origin", self.origin]);
        let bin = tmp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let (login, unreadable) = (self.login, self.unreadable);
        executable(
            &bin.join("gh"),
            &format!(
                r#"case "$*" in
  "api user --jq .login") echo {login} ;;
  "api repos/example-org/backend --jq .full_name") [ {unreadable} = false ] && echo example-org/backend || {{ echo "HTTP 404: Not Found, token $GH_TOKEN" >&2; exit 1; }} ;;
  *) exit 1 ;;
esac"#
            ),
        );
        let ssh_args = tmp.path().join("ssh-args");
        executable(
            &bin.join("ssh"),
            &format!(
                "echo \"$*\" > {}\necho \"Hi {}! You've successfully authenticated, but GitHub does not provide shell access.\" >&2\nexit 1",
                ssh_args.display(),
                self.ssh_account
            ),
        );
        let cfg = tmp.path().join("config.toml");
        fs::write(&cfg, self.config).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let mut c = ns();
        c.current_dir(&root)
            .env("NS_CONFIG", &cfg)
            .env("PATH", &path)
            .env_remove("GH_TOKEN")
            .env_remove("GITLAB_TOKEN")
            .env_remove("NS_FORGE_CHECKED")
            .env_remove("PERSONAL")
            .env_remove("WORK");
        for (k, v) in self.env {
            c.env(k, v);
        }
        let out = c.arg("doctor").assert().get_output().clone();
        let args = fs::read_to_string(&ssh_args).unwrap_or_default();
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        (json(&out.stdout), err, args)
    }
}

#[test]
fn doctor_reports_the_table_repo_and_ssh_account_for_origin() {
    let (v, err, ssh_args) = Doctor::default().run();
    assert_eq!(
        v["forge"]["github"],
        serde_json::json!({"configured": true, "token_resolved": true, "account": "work-account",
            "table": "[forge.github.owners.\"example-org\"]", "repo": "example-org/backend",
            "repo_readable": true, "ssh": {"dest": "git@github-work", "account": "work-account"}})
    );
    assert_eq!(v["warnings"], serde_json::json!([]), "{err}");
    assert_eq!(
        ssh_args.trim_end(),
        "-T -o BatchMode=yes -o ConnectTimeout=10 git@github-work"
    );
    assert!(
        !v["problems"].to_string().contains("forge"),
        "{}",
        v["problems"]
    );
}

#[test]
fn doctor_warns_when_ssh_greets_another_account_or_none() {
    let (v, err, ssh_args) = Doctor {
        origin: "ssh://git@github.com:2222/example-org/backend",
        ssh_account: "personal-account",
        ..Doctor::default()
    }
    .run();
    assert!(ssh_args.contains("-p 2222 git@github.com"), "{ssh_args}");
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("authenticates as personal-account"),
        "{warnings}"
    );
    assert!(warnings.contains("account work-account"), "{warnings}");
    assert!(
        err.contains("ns: warning: ssh to git@github.com authenticates as personal-account"),
        "{err}"
    );
    assert!(
        !v["problems"].to_string().contains("ssh"),
        "a mismatch warns, never fails"
    );

    let (v, _, _) = Doctor {
        ssh_account: "",
        ..Doctor::default()
    }
    .run();
    assert_eq!(v["forge"]["github"]["ssh"]["account"], Value::Null);
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("ssh -T git@github-work greeted no account"),
        "{warnings}"
    );
}

#[test]
fn doctor_reports_a_repo_the_token_cannot_read_as_a_problem() {
    let (v, _, ssh_args) = Doctor {
        origin: "https://github.com/example-org/backend",
        unreadable: true,
        ..Doctor::default()
    }
    .run();
    assert_eq!(v["forge"]["github"]["repo_readable"], false);
    assert_eq!(v["forge"]["github"]["ssh"], Value::Null);
    assert!(
        ssh_args.is_empty(),
        "ssh ran for an https origin: {ssh_args}"
    );
    assert_eq!(v["ok"], false);
    let problems = v["problems"].to_string();
    assert!(
        problems.contains("account work-account cannot read example-org/backend"),
        "{problems}"
    );
    assert!(problems.contains("HTTP 404"), "{problems}");
    assert!(!problems.contains("work-tok"), "{problems}");

    let (v, _, _) = Doctor {
        origin: "https://github.com/example-org/backend",
        config: "",
        env: &[("GH_TOKEN", "env-tok")],
        unreadable: true,
        ..Doctor::default()
    }
    .run();
    assert_eq!(v["forge"]["github"]["configured"], false);
    assert_eq!(v["forge"]["github"]["repo_readable"], false);
    let problems = v["problems"].to_string();
    assert!(
        problems.contains("token from GH_TOKEN from the environment"),
        "{problems}"
    );
    assert!(!problems.contains("env-tok"), "{problems}");
}

#[test]
fn doctor_skips_the_repo_check_when_the_token_does_not_resolve() {
    let (v, _, _) = Doctor {
        env: &[("PERSONAL", "personal-tok")],
        ..Doctor::default()
    }
    .run();
    let gh = &v["forge"]["github"];
    assert_eq!(gh["token_resolved"], false);
    assert_eq!(gh["repo_readable"], Value::Null);
    assert!(
        v["problems"].to_string().contains("WORK"),
        "{}",
        v["problems"]
    );
}

#[test]
fn doctor_warns_about_a_set_gh_token_unless_an_ns_parent_set_it() {
    let (v, _, _) = Doctor {
        env: &[("GH_TOKEN", "env-tok")],
        ..Doctor::default()
    }
    .run();
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("GH_TOKEN from the environment overrides the config's choice ([forge.github.owners.\\\"example-org\\\"])"),
        "{warnings}"
    );
    let (v, _, _) = Doctor {
        env: &[("GH_TOKEN", "env-tok"), ("NS_FORGE_CHECKED", "1")],
        ..Doctor::default()
    }
    .run();
    assert!(
        !v["warnings"].to_string().contains("GH_TOKEN"),
        "{}",
        v["warnings"]
    );
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
        vec!["check-markers", "--help"],
        vec!["install", "--help"],
        vec!["doctor", "--help"],
        vec!["quality", "--help"],
        vec!["log", "--help"],
    ] {
        ns().args(&args)
            .assert()
            .success()
            .stdout(predicate::str::contains("Examples:"));
    }
}

#[test]
fn check_markers_passes_a_clean_tree_and_names_bad_markers() {
    let (_tmp, root) = repo();
    // Spelled out at run time so this file holds no real markers.
    let (start, end) = ("ns:human-review start", "ns:human-review end");
    fs::write(
        root.join("brake.c"),
        format!("/* {start}: torque */\nint max = 5;\n/* {end} */\n"),
    )
    .unwrap();
    git(&root, &["add", "."]);
    let out = ns()
        .args(["check-markers"])
        .current_dir(&root)
        .assert()
        .success();
    let v = json(&out.get_output().stdout);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["errors"].as_array().unwrap().len(), 0);

    fs::write(
        root.join("pump.py"),
        format!("# {start}\n# {start}\nx = 1\n# {end}\n# {end}\n"),
    )
    .unwrap();
    fs::write(root.join("untracked.py"), format!("# {end}\n")).unwrap();
    git(&root, &["add", "pump.py"]);
    let out = ns()
        .args(["check-markers", root.to_str().unwrap()])
        .assert()
        .code(1);
    let v = json(&out.get_output().stdout);
    assert_eq!(v["ok"], false);
    let errs: Vec<(String, u64)> = v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["file"].as_str().unwrap().to_string(),
                e["line"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        errs,
        [("pump.py".to_string(), 2), ("pump.py".to_string(), 5)]
    );

    ns().args(["check-markers", "--human"])
        .current_dir(&root)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("pump.py:2: "));
    let outside = tempfile::tempdir().unwrap();
    ns().args(["check-markers", outside.path().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ns check-markers"));
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
    let path = PathBuf::from(v["path"].as_str().unwrap());
    assert!(!path.join("setup.out").exists());

    // A worktree made without setup has none on record, so the next `new` runs it.
    let v = json(
        &ns()
            .current_dir(&root)
            .args(["worktree", "new", "u2"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert_eq!(v["setup"].as_array().unwrap().len(), 2);
    assert!(path.join("setup.out").exists());
    // The record lives outside the worktree's files.
    assert_eq!(git(&path, &["status", "--porcelain"]), "?? setup.out");
}

#[test]
fn own_factory_definition_turns_on_hooks_that_run_full_ci_on_commit_and_push() {
    let (tmp, root) = repo();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    factory_def(
        &root,
        &fs::read_to_string(src.join(".nightshift/nightshift.toml")).unwrap(),
    );
    fs::create_dir_all(root.join(".githooks")).unwrap();
    for hook in ["pre-commit", "pre-push"] {
        fs::copy(
            src.join(".githooks").join(hook),
            root.join(".githooks").join(hook),
        )
        .unwrap();
    }
    fs::create_dir_all(root.join("scripts")).unwrap();
    executable(
        &root.join("scripts/ci-local.sh"),
        "echo \"ci-local ran with [$*]\" >&2\nexit 1",
    );
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "--no-verify", "-m", "hooks"]);
    let remote = tmp.path().join("remote.git");
    git(
        tmp.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );

    ns().current_dir(&root)
        .args(["factory", "validate"])
        .assert()
        .success();
    let v = json(
        &ns()
            .current_dir(&root)
            .args(["worktree", "new", "u5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    let path = PathBuf::from(v["path"].as_str().unwrap());
    assert_eq!(git(&path, &["config", "core.hooksPath"]), ".githooks");

    fs::write(path.join("README"), "changed\n").unwrap();
    let out = git_output(&path, &["commit", "-qam", "change"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ci-local ran with []"));

    // The failed commit left the edit behind; pre-push checks the commit, not the tree.
    let out = git_output(&path, &["push", "-q", "origin", "HEAD:refs/heads/u5"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ci-local ran with []"));
    assert_eq!(git(&remote, &["for-each-ref"]), "");
}

#[test]
fn a_main_checkout_on_another_unit_runs_that_units_setup() {
    let (_tmp, root) = repo();
    factory_def(
        &root,
        "[worktree]\nsetup = [\"echo $NS_UNIT >> ../setup.log\"]\n",
    );
    let new = |unit: &str| {
        ns().current_dir(&root)
            .args(["worktree", "new", unit])
            .assert()
            .success();
    };
    git(&root, &["checkout", "-q", "-b", "ns/m1"]);
    new("m1");
    new("m1");
    git(&root, &["checkout", "-q", "-b", "ns/m2"]);
    new("m2");
    assert_eq!(
        fs::read_to_string(root.parent().unwrap().join("setup.log")).unwrap(),
        "m1\nm2\n"
    );
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

    // A failed setup isn't on record, so the next `new` runs it again.
    let new_u3 = || {
        let out = ns()
            .current_dir(&root)
            .args(["worktree", "new", "u3"])
            .output();
        let setup = json(&out.stdout)["setup"].clone();
        let failed = setup.as_array().unwrap().iter().any(|r| r["exit"] != 0);
        assert_eq!(out.status.code(), Some(i32::from(failed)), "{setup}");
        setup
    };
    assert_eq!(new_u3()[0]["exit"], 1);

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
    assert_eq!(new_u3(), serde_json::json!([]));

    // A rerun that fails takes the earlier success off the record.
    fs::remove_file(path.join("ready")).unwrap();
    ns().current_dir(&root)
        .args(["worktree", "setup", "u3"])
        .assert()
        .code(1);
    assert_eq!(new_u3()[0]["exit"], 1);
    fs::write(path.join("ready"), "").unwrap();
    assert_eq!(new_u3()[1]["exit"], 0);
    assert_eq!(new_u3(), serde_json::json!([]));

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

#[test]
fn an_inherited_git_dir_never_reaches_another_repo() {
    // A parent git (a hook, `git rebase --exec`) exports GIT_DIR. Every git that ns spawns
    // must act on the repo it names, never on the one GIT_DIR points at.
    let (_t, root) = repo();
    let (_d, decoy) = repo();
    let decoy_git = decoy.join(".git");
    let config_before = fs::read_to_string(decoy_git.join("config")).unwrap();
    let branches_before = git(&decoy, &["branch", "--list"]);

    ns().current_dir(&root)
        .env("GIT_DIR", &decoy_git)
        .env("GIT_WORK_TREE", &decoy)
        .args(["worktree", "new", "7-thing"])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(decoy_git.join("config")).unwrap(),
        config_before
    );
    assert_eq!(git(&decoy, &["branch", "--list"]), branches_before);
    assert!(git(&root, &["branch", "--list", "ns/7-thing"]).contains("ns/7-thing"));
    assert_eq!(git(&decoy, &["rev-parse", "--is-bare-repository"]), "false");
}

#[test]
fn version_names_the_commit_ns_was_built_from() {
    let head = git(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &["rev-parse", "HEAD"],
    );
    ns().arg("--version")
        .assert()
        .success()
        .stdout(format!("ns {} ({head})\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn log_prints_the_run_log_from_any_worktree() {
    let (_tmp, root) = repo();
    let log = |args: &[&str]| ns().current_dir(&root).arg("log").args(args).output();
    let out = log(&[]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no run log at"));

    git(
        &root,
        &["remote", "add", "origin", "git@github.com:o/r.git"],
    );
    let dir = root.join(".git/ns");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("runs.jsonl"),
        concat!(
            r#"{"event":"phase","unit":"142-uart-timeout","phase":"build","attempt":1,"status":"pass","wall_s":3.5,"prompt":"secret","ts":"2026-10-09T10:00:00Z"}"#,
            "\n",
            r#"{"event":"end","unit":"143-other","outcome":"stuck","pr":9,"reason":"ci failed","ts":"2026-10-10T10:00:00Z"}"#,
            "\nnot json\n",
        ),
    )
    .unwrap();
    let out = log(&[]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "2026-10-09T10:00:00Z phase #142 build attempt 1 status pass 3.5s\n\
         2026-10-10T10:00:00Z end #143 outcome stuck https://github.com/o/r/pull/9 reason: ci failed\n"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("skipped 1 line(s)"));

    let out = log(&["--json", "--since", "2026-10-10"]);
    assert_eq!(
        json(&out.stdout),
        serde_json::json!({"ts":"2026-10-10T10:00:00Z","event":"end","unit":"143-other","issue":143,
            "outcome":"stuck","pr":9,"pr_url":"https://github.com/o/r/pull/9","reason":"ci failed"})
    );
    let out = log(&["--unit", "142-uart-timeout", "--json"]);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("secret"));
    assert_eq!(json(&out.stdout)["phase"], "build");

    let wt = root.with_file_name("wt");
    git(
        &root,
        &["worktree", "add", "-q", "-b", "x", wt.to_str().unwrap()],
    );
    let out = ns()
        .current_dir(&wt)
        .args(["log", "--unit", "143-other"])
        .output();
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("2026-10-10T10:00:00Z end #143"));

    let out = log(&["--since", "yesterday"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("ns log --since 2026-10-01T00:00:00Z"));
}

#[test]
fn log_phase_output_prints_the_newest_transcript_redacted() {
    let (_tmp, root) = repo();
    let log = |args: &[&str]| {
        ns().current_dir(&root)
            .args(["log", "--phase-output"])
            .args(args)
            .output()
    };
    let out = log(&["--unit", "7-a"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no phase transcript for 7-a"));

    let dir = root.join(".git/ns/transcripts/7-a");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("build-1.jsonl"),
        concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Building."}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"GH_TOKEN=ghp_0123456789abcdefghijABCDEFGHIJ gh pr list"}}]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let out = log(&[]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "assistant: Building.\ntool: Bash {\"command\":\"GH_TOKEN=[redacted]\"}\n"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("build-1.jsonl"));
    let out = log(&["--unit", "7-a", "--json"]);
    let first = out.stdout.split(|b| *b == b'\n').next().map(json);
    assert_eq!(
        first,
        Some(serde_json::json!({"kind":"assistant","text":"Building."}))
    );

    assert_eq!(log(&["--since", "2026-10-01"]).status.code(), Some(2));
    assert_eq!(log(&["--unit", "../x"]).status.code(), Some(2));
}

#[test]
fn log_follow_prints_the_log_so_far_and_keeps_running() {
    use std::io::BufRead;
    let (_tmp, root) = repo();
    let dir = root.join(".git/ns");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("runs.jsonl"),
        "{\"event\":\"start\",\"unit\":\"9-a\",\"ts\":\"2026-10-09T10:00:00Z\"}\n",
    )
    .unwrap();
    let mut child = ns().current_dir(&root).args(["log", "-f"]).start_piped();
    let stdout = child.take_stdout();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).map(|n| (n, line));
            let done = !matches!(read, Ok((n, _)) if n > 0);
            if tx.send(read).is_err() || done {
                break;
            }
        }
    });
    let first = child.recv(&rx).unwrap().1;
    assert_eq!(first, "2026-10-09T10:00:00Z start #9\n");
    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(dir.join("runs.jsonl"))
        .unwrap();
    std::io::Write::write_all(
        &mut log,
        b"{\"event\":\"end\",\"unit\":\"9-a\",\"outcome\":\"merged\",\"ts\":\"2026-10-09T11:00:00Z\"}\n",
    )
    .unwrap();
    let second = child.recv(&rx).unwrap().1;
    assert_eq!(second, "2026-10-09T11:00:00Z end #9 outcome merged\n");
    assert!(child.is_running());
    child.kill();
}

#[test]
fn log_exits_1_when_the_log_cannot_be_read_and_2_outside_a_repo() {
    let (_tmp, root) = repo();
    fs::create_dir_all(root.join(".git/ns/runs.jsonl")).unwrap();
    let out = ns().current_dir(&root).arg("log").output();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("runs.jsonl"));

    let outside = tempfile::tempdir().unwrap();
    let out = ns()
        .current_dir(outside.path())
        .env("GIT_CEILING_DIRECTORIES", outside.path().parent().unwrap())
        .arg("log")
        .output();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn status_summarises_the_log_and_reports_an_unreadable_queue() {
    let (tmp, root) = repo();
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    executable(&bin.join("gh"), "echo 'gh: not logged in' >&2; exit 1");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let status = |args: &[&str]| {
        ns().current_dir(&root)
            .env("PATH", &path)
            .env("NS_NOW", "1791622800")
            .arg("status")
            .args(args)
            .output()
    };
    let out = status(&[]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no run log at"));

    let dir = root.join(".git/ns");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("runs.jsonl"),
        concat!(
            "not json\n",
            r#"{"event":"end","unit":"141-old","outcome":"merged","ts":"2026-10-08T09:00:00Z"}"#,
            "\n",
            r#"{"event":"end","unit":"142-uart","outcome":"stuck","reason":"ci failed","prompt":"secret","ts":"2026-10-10T01:00:00Z"}"#,
            "\n",
        ),
    )
    .unwrap();
    let out = status(&[]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "since 2026-10-09T09:00:00Z\nrunning (0)\nmerged (0)\nPR for a human (0)\n\
         stuck (1)\n  2026-10-10T01:00:00Z #142 outcome stuck reason: ci failed\n\
         paused (0)\nsplit (0)\nqueue unavailable\n"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("queue unavailable: "), "{err}");
    assert!(err.contains("skipped 1 line(s)"), "{err}");

    let out = status(&["--json", "--since", "2026-10-08"]);
    let v = json(&out.stdout);
    assert_eq!(v["since"], "2026-10-08T00:00:00Z");
    assert_eq!(v["ended"]["merged"][0]["unit"], "141-old");
    assert_eq!(v["ended"]["stuck"][0]["reason"], "ci failed");
    assert!(v["next"].is_null());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("secret"));

    let out = status(&["--since", "last night"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("ns status --since 2026-10-01"));

    fs::remove_file(dir.join("runs.jsonl")).unwrap();
    fs::create_dir(dir.join("runs.jsonl")).unwrap();
    let out = status(&[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("runs.jsonl"));
}

#[test]
fn status_shows_a_unit_whose_run_lock_is_held_as_running_with_its_phase() {
    let (tmp, root) = repo();
    let dir = root.join(".git/ns");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("runs.jsonl"),
        concat!(
            r#"{"event":"end","unit":"143-live","outcome":"paused","ts":"2026-10-10T01:00:00Z"}"#,
            "\n",
            r#"{"event":"phase","unit":"143-live","phase":"build","attempt":1,"status":"pass","ts":"2026-10-10T02:00:00Z"}"#,
            "\n",
        ),
    )
    .unwrap();
    let held = fs::File::create(root.join(".git/ns-run-143-live.lock")).unwrap();
    held.lock().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    executable(&bin.join("gh"), "echo 'gh: not logged in' >&2; exit 1");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = ns()
        .current_dir(&root)
        .env("PATH", &path)
        .env("NS_NOW", "1791622800")
        .arg("status")
        .output();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("running (1)\n  2026-10-10T02:00:00Z #143 build attempt 1 status pass\n"),
        "{text}"
    );
    assert!(text.contains("paused (0)\n"), "{text}");
}

#[test]
fn status_reports_an_unreadable_factory_file_as_an_unavailable_queue() {
    let (_tmp, root) = repo();
    fs::create_dir_all(root.join(".nightshift")).unwrap();
    fs::write(root.join(".nightshift/nightshift.toml"), "queue = [\n").unwrap();
    let out = ns()
        .current_dir(&root)
        .env("NS_NOW", "1791622800")
        .arg("status")
        .output();
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).ends_with("queue unavailable\n"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("queue unavailable: "), "{err}");
    assert!(err.contains("nightshift.toml"), "{err}");
}

#[test]
fn status_lists_the_ready_issues_the_queue_returns() {
    let (tmp, root) = repo();
    git(
        &root,
        &["remote", "add", "origin", "git@github.com:o/r.git"],
    );
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("gh"),
        r#"case "$1" in
api) echo '{"number":7,"title":"Fix the thing","labels":[{"name":"status:ready-for-agent"}],"body":"","authorAssociation":"OWNER"}' ;;
pr) echo '[]' ;;
esac"#,
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let status = |args: &[&str]| {
        ns().current_dir(&root)
            .env("PATH", &path)
            .env("NS_NOW", "1791622800")
            .arg("status")
            .args(args)
            .output()
    };
    let out = status(&[]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.ends_with("next (1)\n  #7 Fix the thing\n"), "{text}");

    let out = status(&["--json"]);
    assert_eq!(
        json(&out.stdout)["next"],
        serde_json::json!([{"number": 7, "title": "Fix the thing"}])
    );
}

/// A repo with this repo's hooks on, a committed `ci-local.sh` that logs each run as
/// `<HEAD> <dir> <CARGO_TARGET_DIR>` and fails when the checkout holds `red` or the common git
/// dir holds `red`, and a bare remote. Returns the repo, the remote and the log of runs. Tests
/// commit with `--no-verify` so the log holds only pre-push runs.
fn hooked_repo() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let (tmp, root) = repo();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::create_dir_all(root.join(".githooks")).unwrap();
    for hook in ["pre-commit", "pre-push"] {
        fs::copy(
            src.join(".githooks").join(hook),
            root.join(".githooks").join(hook),
        )
        .unwrap();
    }
    fs::create_dir_all(root.join("scripts")).unwrap();
    let log = root.join(".git/ci-runs");
    executable(
        &root.join("scripts/ci-local.sh"),
        &format!(
            r#"cat >/dev/null
common=$(git rev-parse --git-common-dir)
echo "$(git rev-parse HEAD) $PWD $CARGO_TARGET_DIR" >> '{}'
[ ! -e "$common/slow" ] || {{ touch "$common/started"; sleep 30; }}
if [ -e red ] || [ -e "$common/red" ]; then echo red >&2; exit 1; fi"#,
            log.display()
        ),
    );
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "hooks"]);
    let remote = tmp.path().join("remote.git");
    git(
        tmp.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&root, &["config", "core.hooksPath", ".githooks"]);
    (tmp, root, remote, log)
}

/// `git push -q origin <specs>` with no inherited cargo target directory.
fn push(root: &Path, specs: &[&str]) -> std::process::Output {
    StdCommand::new("git")
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("CARGO_TARGET_DIR")
        .args(["push", "-q", "origin"])
        .args(specs)
        .output()
        .unwrap()
}

/// Each run logged, as `(commit, dir, cargo target dir)`.
fn runs(log: &Path) -> Vec<(String, PathBuf, String)> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let f: Vec<_> = l.split(' ').collect();
            (f[0].to_owned(), PathBuf::from(f[1]), f[2].to_owned())
        })
        .collect()
}

/// Each run happened outside `root`, and left no worktree or directory behind.
fn assert_runs_left_no_worktree(root: &Path, log: &Path) {
    let list = git(root, &["worktree", "list", "--porcelain"]);
    assert_eq!(list.matches("worktree ").count(), 1, "{list}");
    for (_, dir, _) in runs(log) {
        assert!(
            !dir.starts_with(root),
            "{} ran in the checkout",
            dir.display()
        );
        assert!(!dir.exists(), "{} was left behind", dir.display());
    }
}

#[test]
fn pre_push_checks_the_commit_pushed_whatever_the_tree_holds() {
    let (_tmp, root, remote, log) = hooked_repo();
    let head = git(&root, &["rev-parse", "HEAD"]);
    fs::write(root.join("README"), "uncommitted\n").unwrap();
    fs::write(root.join("red"), "untracked\n").unwrap();
    let out = push(&root, &["HEAD:refs/heads/a"]);
    assert!(out.status.success(), "{out:?}");
    let target = root.join(".git/pre-push-target").display().to_string();
    assert_eq!(runs(&log).len(), 1);
    assert_eq!(runs(&log)[0].0, head);
    assert_eq!(runs(&log)[0].2, target);

    git(&root, &["add", "red"]);
    git(&root, &["commit", "-q", "--no-verify", "-m", "red"]);
    git(&root, &["rm", "-q", "red"]);
    let out = push(&root, &["HEAD:refs/heads/b"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("pre-push: ci-local.sh failed for HEAD at"),
        "{err}"
    );
    assert_eq!(runs(&log).len(), 2);
    assert_eq!(runs(&log)[1].0, git(&root, &["rev-parse", "HEAD"]));
    assert!(!git(&remote, &["for-each-ref"]).contains("refs/heads/b"));
    assert_runs_left_no_worktree(&root, &log);
}

#[test]
fn pre_push_from_a_linked_worktree_shares_the_main_repos_target_dir() {
    let (tmp, root, _remote, log) = hooked_repo();
    let linked = tmp.path().join("linked");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );
    fs::write(linked.join("side.txt"), "side\n").unwrap();
    git(&linked, &["add", "side.txt"]);
    git(&linked, &["commit", "-q", "--no-verify", "-m", "side"]);
    let head = git(&linked, &["rev-parse", "HEAD"]);
    let out = push(&linked, &["HEAD:refs/heads/side"]);
    assert!(out.status.success(), "{out:?}");
    let runs = runs(&log);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].0, head);
    let target = root.join(".git/pre-push-target").display().to_string();
    assert_eq!(runs[0].2, target);
    let list = git(&root, &["worktree", "list", "--porcelain"]);
    assert_eq!(list.matches("worktree ").count(), 2, "{list}");
    assert!(
        !runs[0].1.exists(),
        "{} was left behind",
        runs[0].1.display()
    );
}

#[test]
fn pre_push_checks_every_ref_in_one_push_and_each_commit_once() {
    let (_tmp, root, remote, log) = hooked_repo();
    let good = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "-qb", "bad"]);
    fs::write(root.join("red"), "").unwrap();
    git(&root, &["add", "red"]);
    git(&root, &["commit", "-q", "--no-verify", "-m", "red"]);
    let bad = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "-q", "main"]);

    let out = push(&root, &["main:refs/heads/good", "bad:refs/heads/bad"]);
    assert!(!out.status.success());
    let commits: Vec<_> = runs(&log).into_iter().map(|r| r.0).collect();
    assert_eq!(commits, [good.clone(), bad]);
    assert_eq!(git(&remote, &["for-each-ref"]), "");

    let out = push(&root, &["main:refs/heads/one", "main:refs/heads/two"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(runs(&log).len(), 3);
    assert_eq!(runs(&log)[2].0, good);
    assert_runs_left_no_worktree(&root, &log);
}

#[test]
fn pre_push_peels_an_annotated_tag_and_refuses_one_on_a_tree() {
    let (_tmp, root, remote, log) = hooked_repo();
    let old = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["tag", "-a", "-m", "old", "v-old"]);
    fs::write(root.join("README"), "next\n").unwrap();
    git(&root, &["commit", "-q", "--no-verify", "-am", "next"]);
    let out = push(&root, &["refs/tags/v-old"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(runs(&log).len(), 1);
    assert_eq!(runs(&log)[0].0, old);
    assert!(git(&remote, &["for-each-ref"]).contains("refs/tags/v-old"));

    git(&root, &["tag", "-a", "-m", "t", "v-tree", "HEAD^{tree}"]);
    let out = push(&root, &["refs/tags/v-tree"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("is not a commit"));
    assert_eq!(runs(&log).len(), 1);
}

#[test]
fn pre_push_with_follow_tags_pushes_the_branch_beside_its_annotated_tag() {
    let (_tmp, root, _remote, log) = hooked_repo();
    git(&root, &["config", "push.followTags", "true"]);
    git(&root, &["tag", "-a", "-m", "release", "v1"]);
    let out = push(&root, &["HEAD:refs/heads/a"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(runs(&log).len(), 1);
}

#[test]
fn pre_push_skips_a_deleted_ref() {
    let (_tmp, root, remote, log) = hooked_repo();
    let out = push(&root, &["HEAD:refs/heads/a"]);
    assert!(out.status.success(), "{out:?}");
    fs::write(root.join(".git/red"), "").unwrap();
    let out = push(&root, &[":refs/heads/a"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(runs(&log).len(), 1);
    assert_eq!(git(&remote, &["for-each-ref"]), "");
}

#[test]
fn pre_push_checks_the_commit_pushed_when_git_location_variables_are_set() {
    let (tmp, root, _remote, log) = hooked_repo();
    let index = tmp.path().join("index");
    let old = git(&root, &["rev-parse", "HEAD"]);
    fs::write(root.join("README"), "next\n").unwrap();
    git(&root, &["commit", "-q", "--no-verify", "-am", "next"]);
    let out = StdCommand::new("git")
        .current_dir(&root)
        .env_remove("CARGO_TARGET_DIR")
        .env("GIT_DIR", root.join(".git"))
        .env("GIT_WORK_TREE", &root)
        .env("GIT_INDEX_FILE", &index)
        .args(["push", "-q", "origin", &format!("{old}:refs/heads/a")])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(runs(&log).len(), 1);
    assert_eq!(runs(&log)[0].0, old);
    assert!(
        !index.exists(),
        "the temporary worktree wrote the caller's index"
    );
    assert_runs_left_no_worktree(&root, &log);
}

#[test]
fn pre_push_keeps_a_cargo_target_dir_already_set() {
    let (tmp, root, _remote, log) = hooked_repo();
    let target = tmp.path().join("target");
    let out = StdCommand::new("git")
        .current_dir(&root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("CARGO_TARGET_DIR", &target)
        .args(["push", "-q", "origin", "HEAD:refs/heads/a"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(runs(&log)[0].2, target.display().to_string());
}

#[test]
fn pre_push_removes_its_worktree_when_interrupted() {
    for (signal, code) in [
        (libc::SIGINT, 130),
        (libc::SIGTERM, 143),
        (libc::SIGHUP, 129),
    ] {
        let (_tmp, root, _remote, log) = hooked_repo();
        let head = git(&root, &["rev-parse", "HEAD"]);
        fs::write(root.join(".git/slow"), "").unwrap();
        fs::write(
            root.join(".git/refs-pushed"),
            format!("HEAD {head} refs/heads/a {}\n", "0".repeat(40)),
        )
        .unwrap();
        let mut hook = common::command("sh");
        hook.arg("-c")
            .arg("exec .githooks/pre-push < .git/refs-pushed")
            .current_dir(&root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("CARGO_TARGET_DIR");
        let mut hook = hook.start();
        let started = root.join(".git/started");
        let deadline = std::time::Instant::now() + common::TIMEOUT;
        while !started.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "ci-local.sh never ran"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // SAFETY: kill(2) on the hook's own process group, as a terminal's Ctrl-C would.
        unsafe {
            libc::kill(-(hook.id() as libc::pid_t), signal);
        }
        assert_eq!(hook.wait().code(), Some(code), "signal {signal}");
        assert_eq!(runs(&log).len(), 1);
        assert_runs_left_no_worktree(&root, &log);
    }
}

/// A hook that exits 0 on a skip variable passes a commit the checks would fail.
#[test]
fn neither_hook_has_an_environment_variable_that_skips_it() {
    let (_tmp, root, _remote, _log) = hooked_repo();
    fs::write(root.join(".git/red"), "").unwrap();
    let head = git(&root, &["rev-parse", "HEAD"]);
    for hook in ["pre-commit", "pre-push"] {
        let out = StdCommand::new("sh")
            .arg("-c")
            .arg(format!(
                "echo 'refs/heads/main {head} refs/heads/main {head}' | .githooks/{hook}"
            ))
            .current_dir(&root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .envs(
                [
                    "SKIP",
                    "SKIP_CI",
                    "CI_SKIP",
                    "SKIP_HOOKS",
                    "NO_VERIFY",
                    "HUSKY",
                    "CI",
                ]
                .map(|k| (k, "1")),
            )
            .env("HUSKY", "0")
            .output()
            .unwrap();
        assert!(!out.status.success(), "{hook} passed with skip variables");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("red"),
            "{hook} did not run ci-local.sh"
        );
    }
}
