mod common;

use std::fs;
use std::path::{Path, PathBuf};

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
    common::command("git")
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
fn ask_inside_a_phase_gives_claude_the_phase_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    fs::write(&cfg, "[roles.default]\nharness = \"claude\"\n").unwrap();
    let wt = tmp.path().join("wt");
    let settings = tmp.path().join("common/ns/units/u1/phase-settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let ask_cmd = |unit: Option<&str>, worktree: bool| {
        let mut c = ns();
        c.env("NS_CONFIG", &cfg)
            .env("NS_PHASE_SETTINGS", &settings)
            .args(["ask", "--role", "build", "--dry-run"]);
        if worktree {
            c.env("NS_WORKTREE", &wt);
        }
        if let Some(u) = unit {
            c.env("NS_UNIT", u);
        }
        c
    };
    let ask = |unit: Option<&str>| {
        let v = json(&ask_cmd(unit, true).assert().success().get_output().stdout);
        v["command"].as_array().unwrap().clone()
    };
    let tail = serde_json::json!([
        "--setting-sources",
        "project,local",
        "--settings",
        settings.to_str().unwrap(),
        "--add-dir",
        wt.join(".ns/u1/skills").to_str().unwrap()
    ]);
    let tail = tail.as_array().unwrap();
    // A phase whose settings file is gone is an error, not a claude with the user's settings.
    let err = String::from_utf8(
        ask_cmd(Some("u1"), true)
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone(),
    )
    .unwrap();
    assert!(err.contains("NS_PHASE_SETTINGS"), "{err}");
    // So is a variable that is unset, or names a directory.
    let missing_variable = |c: &mut Ns| {
        let out = c.assert().failure().code(2).get_output().stderr.clone();
        String::from_utf8(out).unwrap()
    };
    let err = missing_variable(ask_cmd(Some("u1"), true).env_remove("NS_PHASE_SETTINGS"));
    assert!(err.contains("NS_PHASE_SETTINGS"), "{err}");
    let dir = tmp.path().join("a-directory");
    fs::create_dir(&dir).unwrap();
    let err = missing_variable(ask_cmd(Some("u1"), true).env("NS_PHASE_SETTINGS", &dir));
    assert!(err.contains("NS_PHASE_SETTINGS"), "{err}");
    fs::write(&settings, "{}").unwrap();
    assert!(ask(Some("u1")).ends_with(tail), "{:?}", ask(Some("u1")));
    assert!(ask(None).iter().all(|a| a != "--settings"));
    // A unit with no worktree variable is not in a phase: the command stays the user's own.
    let v = json(
        &ask_cmd(Some("u1"), false)
            .assert()
            .success()
            .get_output()
            .stdout,
    );
    assert!(v["command"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a != "--settings"));
    // A unit id that is not one is refused.
    ask_cmd(Some("../x"), true).assert().failure();
    fs::write(&cfg, "[roles.default]\nharness = \"codex\"\n").unwrap();
    assert!(ask(Some("u1")).iter().all(|a| a != "--settings"));
}

#[test]
fn ask_inside_a_phase_refuses_a_claude_command_that_carries_a_settings_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    fs::write(
        &cfg,
        "[harness.claude]\ncommand = [\"claude\", \"-p\", \"--settings=x.json\"]\n[roles.default]\nharness = \"claude\"\n",
    )
    .unwrap();
    let wt = tmp.path().join("wt");
    let settings = tmp.path().join("phase-settings.json");
    fs::write(&settings, "{}").unwrap();
    let err = ns()
        .env("NS_CONFIG", &cfg)
        .env("NS_PHASE_SETTINGS", &settings)
        .env("NS_WORKTREE", &wt)
        .env("NS_UNIT", "u1")
        .args(["ask", "--role", "build", "--dry-run"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8(err).unwrap().contains("--settings"));
    // Outside a phase the command is the user's own.
    ns().env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "build", "--dry-run"])
        .assert()
        .success();
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
    ns().current_dir(tmp.path())
        .env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review"])
        .write_stdin("hello harness\n")
        .assert()
        .success()
        .stdout("hello harness\n");

    let prompt = tmp.path().join("p.md");
    fs::write(&prompt, "from file\n").unwrap();
    ns().current_dir(tmp.path())
        .env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review.spec", "--prompt-file"])
        .arg(&prompt)
        .assert()
        .success()
        .stdout("from file\n");

    ns().current_dir(tmp.path())
        .env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review.flaky"])
        .write_stdin("x")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("exited with 1"));

    ns().current_dir(tmp.path())
        .env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "verify"])
        .write_stdin("x")
        .assert()
        .code(4)
        .stderr(predicate::str::contains("not on PATH"));
}

#[test]
fn ask_passes_the_harness_stderr_through_and_keeps_it_out_of_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    fs::write(
        &cfg,
        "[harness.h]\ncommand = [\"sh\", \"-c\", \"cat; echo progress >&2\"]\n[roles.review]\nharness = \"h\"\n",
    )
    .unwrap();
    ns().current_dir(tmp.path())
        .env("NS_CONFIG", &cfg)
        .args(["ask", "--role", "review"])
        .write_stdin("answer\n")
        .assert()
        .success()
        .stdout("answer\n")
        .stderr(predicate::str::contains("progress\n"));
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
    common::write_exe(&fake, "#!/bin/sh\ncat >/dev/null\nenv\n");
    let cfg = repo.parent().unwrap().join("config.toml");
    fs::write(
        &cfg,
        format!(
            "[harness.fake]\ncommand = [{:?}]\n[roles.default]\nharness = \"fake\"\n",
            fake.to_str().unwrap()
        ),
    )
    .unwrap();
    let home = repo.parent().unwrap().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(
        home.join(".claude/.credentials.json"),
        common::CLAUDE_CREDENTIALS,
    )
    .unwrap();
    let mut c = ns();
    c.env("NS_CONFIG", &cfg)
        .env("HOME", &home)
        .current_dir(start);
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

/// `ns ask` sets claude up as a phase does: under subscription billing it needs a login and
/// drops `--bare`; auto-memory is off and background tasks run to the end either way.
#[test]
fn ask_sets_claude_up_as_a_phase_does() {
    let (_tmp, root) = repo();
    let base = root.parent().unwrap();
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("claude");
    common::write_exe(&fake, "#!/bin/sh\ncat >/dev/null\necho \"args=$*\"\nenv\n");
    let cfg = base.join("config.toml");
    fs::write(
        &cfg,
        format!(
            "[harness.claude]\ncommand = [{:?}, \"--bare\", \"-p\"]\n[roles.default]\nharness = \"claude\"\n",
            fake.to_str().unwrap()
        ),
    )
    .unwrap();
    let home = base.join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    let ask = || {
        let mut c = ns();
        c.env("NS_CONFIG", &cfg)
            .env("HOME", &home)
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .env_remove("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS")
            .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "0")
            .current_dir(&root)
            .args(["ask", "--role", "review"])
            .write_stdin("p\n");
        c
    };
    ask()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("missing"))
        .stderr(predicate::str::contains("claude /login"));

    // A credentials file with no OAuth token is no login.
    let creds = home.join(".claude/.credentials.json");
    for bad in ["", "{}", r#"{"claudeAiOauth":{"accessToken":""}}"#] {
        fs::write(&creds, bad).unwrap();
        ask()
            .assert()
            .code(2)
            .stderr(predicate::str::contains("empty or invalid"))
            .stderr(predicate::str::contains("claude /login"));
        // An empty token in the environment falls through to the file.
        ask()
            .env("CLAUDE_CODE_OAUTH_TOKEN", "")
            .assert()
            .code(2)
            .stderr(predicate::str::contains("claude /login"));
        // A token in the environment is a login whatever the file holds.
        ask().env("CLAUDE_CODE_OAUTH_TOKEN", "t").assert().success();
    }

    fs::write(&creds, common::CLAUDE_CREDENTIALS).unwrap();
    let out = ask().assert().success().get_output().stdout.clone();
    let out = String::from_utf8(out).unwrap();
    assert!(out.lines().any(|l| l == "args=-p"), "{out}");
    assert!(
        out.lines()
            .any(|l| l == "CLAUDE_CODE_DISABLE_AUTO_MEMORY=1"),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l == "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0"),
        "{out}"
    );

    // API billing needs no login and keeps --bare.
    fs::remove_file(home.join(".claude/.credentials.json")).unwrap();
    write_factory(&root, "[defaults]\nbilling = \"api\"\n");
    let out = ask().assert().success().get_output().stdout.clone();
    let out = String::from_utf8(out).unwrap();
    assert!(out.lines().any(|l| l == "args=--bare -p"), "{out}");
    assert!(
        out.lines()
            .any(|l| l == "CLAUDE_CODE_DISABLE_AUTO_MEMORY=1"),
        "{out}"
    );
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

/// The JSON of a finished `ns doctor`, after checking it exits 0 when `ok` is true and 1 otherwise.
fn doctor_report(out: &std::process::Output) -> Value {
    let v = json(&out.stdout);
    let ok = v["ok"].as_bool().unwrap_or_else(|| panic!("no ok: {v}"));
    assert_eq!(out.status.code(), Some(if ok { 0 } else { 1 }), "{v}");
    v
}

#[test]
fn doctor_exits_1_when_a_check_fails_and_0_when_all_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    let doctor = |toml: &str| {
        fs::write(&cfg, toml).unwrap();
        ns().current_dir(tmp.path())
            .env("NS_CONFIG", &cfg)
            .env("GIT_CEILING_DIRECTORIES", tmp.path())
            .env_remove("GH_TOKEN")
            .env_remove("GITLAB_TOKEN")
            .arg("doctor")
            .assert()
            .get_output()
            .clone()
    };

    let out = doctor("[harness.echo]\ncommand = [\"cat\"]\n[roles.default]\nharness = \"echo\"\n");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(json(&out.stdout)["ok"], true);

    for (toml, problem) in [
        (
            "[harness.echo]\ncommand = [\"cat\"]\n[roles.review]\nharness = \"echo\"\n",
            "no [roles.default]",
        ),
        ("[roles.default\n", ""),
    ] {
        let out = doctor(toml);
        assert_eq!(out.status.code(), Some(1), "{toml}");
        let v = json(&out.stdout);
        assert_eq!(v["ok"], false, "{toml}");
        assert!(v["problems"].to_string().contains(problem), "{v}");
    }

    ns().args(["doctor", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Exit codes: 0 every check passed (ok is true), 1 a check failed",
        ));
}

#[test]
fn doctor_reports_config() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    let v = doctor_report(
        ns().current_dir(tmp.path())
            .env("NS_CONFIG", &cfg)
            .arg("doctor")
            .assert()
            .get_output(),
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
    assert_eq!(v["harnesses"]["claude"]["pinning"], "snapshot");
    assert_eq!(v["harnesses"]["codex"]["pinning"], "partial");
    for h in ["cursor-agent", "gemini", "opencode"] {
        assert_eq!(v["harnesses"][h]["pinning"], "none", "{h}");
    }
    assert_eq!(v["claude_phases"]["auto_memory"], false);
    assert_eq!(
        v["claude_phases"]["env"],
        "CLAUDE_CODE_DISABLE_AUTO_MEMORY=1"
    );
}

#[test]
fn doctor_reports_the_memory_cap_the_p90_peak_and_the_units_that_fit() {
    let (tmp, root) = repo();
    let meminfo = tmp.path().join("meminfo");
    fs::write(
        &meminfo,
        "MemTotal: 25165824 kB\nMemAvailable: 20480000 kB\n",
    )
    .unwrap();
    fs::create_dir(root.join(".nightshift")).unwrap();
    fs::write(
        root.join(".nightshift/nightshift.toml"),
        "[limits]\ntotal_memory_mb = 9000\n",
    )
    .unwrap();
    let ends: String = [1000, 2000, 3000, 2500, 1500]
        .iter()
        .map(|mb| format!("{{\"event\":\"end\",\"outcome\":\"done\",\"memory_peak_mb\":{mb}}}\n"))
        .collect();
    fs::create_dir_all(root.join(".git/ns")).unwrap();
    fs::write(root.join(".git/ns/runs.jsonl"), ends).unwrap();
    let cfg = config(tmp.path());
    let out = ns()
        .current_dir(&root)
        .env("NS_CONFIG", &cfg)
        .env("NS_MEMINFO", &meminfo)
        .arg("doctor")
        .assert()
        .get_output()
        .clone();
    let m = &json(&out.stdout)["memory"];
    assert_eq!(m["total_mb"], 24576, "{m}");
    assert_eq!(m["cap_mb"], 9000, "{m}");
    assert_eq!(m["p90_peak_mb"], 3000, "{m}");
    assert_eq!(m["peaks"], 5, "{m}");
    assert_eq!(m["units_fit"], 3, "{m}");
}

#[test]
fn doctor_reports_where_pinning_looks_for_installed_skills_per_harness() {
    let home = tempfile::tempdir().unwrap();
    let v = doctor_report(
        ns().current_dir(home.path())
            .env("HOME", home.path())
            .arg("doctor")
            .assert()
            .get_output(),
    );
    let dir = |d: &str| home.path().join(d).to_string_lossy().into_owned();
    let (claude, agents) = (dir(".claude/skills"), dir(".agents/skills"));
    assert_eq!(
        v["harnesses"]["claude"]["skills_from"],
        serde_json::json!([claude])
    );
    assert_eq!(
        v["harnesses"]["codex"]["skills_from"],
        serde_json::json!([agents, claude])
    );
    for h in ["cursor-agent", "gemini", "opencode"] {
        assert_eq!(
            v["harnesses"][h]["skills_from"],
            serde_json::json!([claude]),
            "{h}"
        );
    }
}

#[test]
fn doctor_reports_whether_phases_get_a_private_tmp() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let doctor = |bwrap: &str| {
        common::write_exe(&bin.join("bwrap"), format!("#!/bin/sh\n{bwrap}\n"));
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let v = doctor_report(
            ns().current_dir(tmp.path())
                .env("PATH", path)
                .arg("doctor")
                .assert()
                .get_output(),
        );
        for h in ["claude", "codex", "cursor-agent", "gemini", "opencode"] {
            assert_eq!(
                v["harnesses"][h]["private_tmp"], v["private_tmp"]["isolated"],
                "{h}"
            );
        }
        v["private_tmp"].clone()
    };
    assert_eq!(
        doctor("exit 0"),
        serde_json::json!({"isolated": true, "via": "bwrap", "reason": null})
    );
    let v = doctor("exit 1");
    assert_eq!(v["isolated"], false, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.starts_with("bwrap failed"), "{reason}");
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

    let v = doctor_report(
        ns().current_dir(home.path())
            .env("HOME", home.path())
            .arg("doctor")
            .assert()
            .get_output(),
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
fn doctor_reports_free_disk_space_against_the_factory_limits() {
    let (tmp, root) = repo();
    fs::create_dir(root.join(".nightshift")).unwrap();
    fs::write(
        root.join(".nightshift/nightshift.toml"),
        "[limits]\ndisk_wait_gb = 30\ndisk_stop_gb = 12\n",
    )
    .unwrap();
    let free = tmp.path().join("free-gib");
    let doctor = |dir: &Path, gib: &str| {
        fs::write(&free, gib).unwrap();
        doctor_report(
            ns().current_dir(dir)
                .env("NS_CONFIG", tmp.path().join("missing.toml"))
                .env_remove("GH_TOKEN")
                .env_remove("GITLAB_TOKEN")
                .env("NS_FREE_GIB_FILE", &free)
                .env("GIT_CEILING_DIRECTORIES", tmp.path())
                .arg("doctor")
                .assert()
                .get_output(),
        )
    };
    // The worktrees directory doesn't exist yet: its parent's filesystem is the one read.
    let worktrees = root.parent().unwrap().to_string_lossy().into_owned();
    let v = doctor(&root, "100");
    assert_eq!(v["disk"]["wait_gb"], 30, "{v}");
    assert_eq!(v["disk"]["stop_gb"], 12, "{v}");
    let first = &v["disk"]["filesystems"][0];
    assert_eq!(first["path"], worktrees.as_str(), "{v}");
    assert_eq!(first["free_gb"], 100, "{v}");
    assert_eq!(first["state"], "ok", "{v}");
    assert!(!v["warnings"].to_string().contains("disk_"), "{v}");

    let v = doctor(&root, "20");
    assert_eq!(v["disk"]["filesystems"][0]["state"], "wait", "{v}");
    let want = format!("{worktrees} has 20 GiB free, under [limits] disk_wait_gb = 30");
    assert!(v["warnings"].to_string().contains(&want), "{v}");

    let v = doctor(&root, "11");
    assert_eq!(v["disk"]["filesystems"][0]["state"], "stop", "{v}");
    let want = format!("{worktrees} has 11 GiB free, under [limits] disk_stop_gb = 12");
    assert!(v["warnings"].to_string().contains(&want), "{v}");

    let v = doctor(&root, "lots");
    let err = v["disk"]["filesystems"][0]["error"].as_str().unwrap();
    assert!(
        err.ends_with("\"lots\" is not a whole number of GiB"),
        "{v}"
    );
    assert!(v["warnings"].to_string().contains("can't be read"), "{v}");

    // A factory file that doesn't load gives the default limits.
    fs::write(root.join(".nightshift/nightshift.toml"), "[limits\n").unwrap();
    let v = doctor(&root, "100");
    assert_eq!(v["disk"]["wait_gb"], 20, "{v}");

    // Outside a repo: the current directory's filesystem, under the default limits.
    let v = doctor(tmp.path(), "100");
    assert_eq!(v["disk"]["wait_gb"], 20, "{v}");
    assert_eq!(v["disk"]["stop_gb"], 10, "{v}");
    let here = tmp.path().to_string_lossy().into_owned();
    assert_eq!(v["disk"]["filesystems"][0]["path"], here.as_str(), "{v}");
}

#[test]
fn doctor_reports_whether_core_hooks_path_points_at_githooks() {
    let (_tmp, root) = repo();
    fs::create_dir(root.join(".githooks")).unwrap();
    let doctor = || {
        doctor_report(
            ns().current_dir(&root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .arg("doctor")
                .assert()
                .get_output(),
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
fn doctor_warns_when_the_default_branch_is_not_in_the_main_checkout() {
    let (tmp, root) = repo();
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        &root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    let doctor = || {
        let out = ns()
            .current_dir(&root)
            .arg("doctor")
            .assert()
            .get_output()
            .clone();
        let w: Vec<String> = doctor_report(&out)["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_string())
            .filter(|w| w.contains("main checkout"))
            .collect();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(w.iter().all(|w| stderr.contains(w.as_str())), "{stderr}");
        w
    };

    let w = doctor();
    assert!(w.is_empty(), "{w:?}");

    git(&root, &["checkout", "-qb", "other"]);
    let w = doctor();
    assert_eq!(
        w,
        ["the main checkout is on other, not main; fix: git checkout main"]
    );

    let wt = tmp
        .path()
        .canonicalize()
        .unwrap()
        .join("myrepo.worktrees/3-done");
    git(
        &root,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "main"],
    );
    let w = doctor();
    let held = format!(
        "main is checked out in worktree {}, so the main checkout cannot switch to it; fix: ns clean, or git -C {} checkout --detach",
        wt.display(),
        wt.display()
    );
    assert_eq!(
        w,
        [
            "the main checkout is on other, not main; fix: git checkout main".to_string(),
            held
        ]
    );

    git(&wt, &["checkout", "-q", "--detach"]);
    git(&root, &["checkout", "-q", "--detach"]);
    let w = doctor();
    assert_eq!(
        w,
        ["the main checkout is detached, not on main; fix: git checkout main"]
    );

    // A default branch with no local branch has nothing to check out.
    git(&root, &["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
    git(
        &root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    assert!(doctor().is_empty());
}

#[test]
fn doctor_warns_when_a_gh_or_glab_tracker_doc_has_no_operations_section() {
    let (tmp, root) = repo();
    let doc = root.join("docs/agents/issue-tracker.md");
    let doctor_in = |dir: &Path| {
        let out = ns()
            .current_dir(dir)
            .env("GIT_CEILING_DIRECTORIES", tmp.path())
            .arg("doctor")
            .assert()
            .get_output()
            .clone();
        let report = doctor_report(&out);
        let w: Vec<String> = report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_string())
            .filter(|w| w.contains("issue-tracker.md"))
            .collect();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            w.iter()
                .all(|w| stderr.contains(&format!("ns: warning: {w}"))),
            "{stderr}"
        );
        (report["ok"].clone(), w)
    };
    let doctor = || doctor_in(&root);
    let warning = format!(
        "{} names gh or glab but has no ## Operations section, so skills find no tracker operation; fix: load the ns-setup skill to add it",
        doc.display()
    );

    let (ok, w) = doctor();
    assert!(w.is_empty(), "{w:?}");

    fs::create_dir_all(doc.parent().unwrap()).unwrap();
    for stale in [
        "# Issue tracker: GitHub\n\nUse the `gh` CLI for all operations.\n",
        "# Issue tracker: GitLab\n\n- **Read an issue**: `glab issue view <n>`\n",
        "Use the `gh` CLI.\n\n```\n## Operations\n```\n",
    ] {
        fs::write(&doc, stale).unwrap();
        assert_eq!(doctor(), (ok.clone(), vec![warning.clone()]), "{stale}");
    }

    for fine in [
        "Use the `gh` CLI.\n\n## Operations\n\n### Read an issue\n\n`gh issue view <n>`\n",
        "# Issue tracker: Local\n\nIssues live as files under `.scratch/`.\n",
    ] {
        fs::write(&doc, fine).unwrap();
        assert_eq!(doctor(), (ok.clone(), vec![]), "{fine}");
    }

    // An unreadable file, and a stale file outside any repo, give no warning.
    fs::remove_file(&doc).unwrap();
    fs::create_dir(&doc).unwrap();
    assert_eq!(doctor().1, Vec::<String>::new());
    let bare = tmp.path().canonicalize().unwrap().join("bare");
    fs::create_dir_all(bare.join("docs/agents")).unwrap();
    fs::write(
        bare.join("docs/agents/issue-tracker.md"),
        "Use the `gh` CLI.\n",
    )
    .unwrap();
    assert_eq!(doctor_in(&bare).1, Vec::<String>::new());
}

#[test]
fn doctor_reports_a_tracked_skill_dir_that_ns_run_cannot_pin() {
    let (tmp, root) = repo();
    let cfg = tmp.path().join("config.toml");
    fs::write(
        &cfg,
        "[harness.echo]\ncommand = [\"cat\"]\n[roles.default]\nharness = \"echo\"\n",
    )
    .unwrap();
    let doctor = |dir: &Path| {
        let out = ns()
            .current_dir(dir)
            .env("NS_CONFIG", &cfg)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GH_TOKEN")
            .env_remove("GITLAB_TOKEN")
            .arg("doctor")
            .assert()
            .get_output()
            .clone();
        let v = doctor_report(&out);
        let mentions = |key: &str| -> Vec<String> {
            v[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w.as_str().unwrap().to_string())
                .filter(|w| w.contains("/skills is tracked"))
                .collect()
        };
        let (problems, warnings) = (mentions("problems"), mentions("warnings"));
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(warnings.iter().all(|w| stderr.contains(w)), "{stderr}");
        (v["ok"].clone(), problems, warnings)
    };

    // An untracked .claude/skills, or ns run's link to a unit's snapshot, is no problem.
    fs::create_dir_all(root.join(".claude")).unwrap();
    std::os::unix::fs::symlink("../.ns/u/skills", root.join(".claude/skills")).unwrap();
    assert_eq!(doctor(&root), (Value::Bool(true), vec![], vec![]));
    fs::remove_file(root.join(".claude/skills")).unwrap();
    fs::create_dir_all(root.join(".claude/skills/mine")).unwrap();
    fs::write(root.join(".claude/skills/mine/SKILL.md"), "x\n").unwrap();
    assert_eq!(doctor(&root), (Value::Bool(true), vec![], vec![]));
    // A tracked sibling that only shares the prefix is not the skill dir.
    fs::create_dir_all(root.join(".claude/skills-old")).unwrap();
    fs::write(root.join(".claude/skills-old/x"), "x\n").unwrap();
    git(&root, &["add", ".claude/skills-old"]);
    assert_eq!(doctor(&root), (Value::Bool(true), vec![], vec![]));

    git(&root, &["add", ".claude/skills"]);
    let (ok, problems, warnings) = doctor(&root.join(".claude"));
    assert_eq!(ok, false);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with(".claude/skills is tracked"),
        "{problems:?}"
    );
    assert!(problems[0].contains("fix: move its skills to .agents/skills"));
    assert!(problems[0].contains("git rm -r --cached .claude/skills"));
    assert!(warnings.is_empty(), "{warnings:?}");

    git(&root, &["rm", "-rq", "--cached", ".claude/skills"]);
    fs::create_dir_all(root.join(".agents/skills/mine")).unwrap();
    fs::write(root.join(".agents/skills/mine/SKILL.md"), "x\n").unwrap();
    git(&root, &["add", ".agents/skills"]);
    let (ok, problems, warnings) = doctor(&root);
    assert_eq!(ok, true, "{problems:?}");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].starts_with(".agents/skills is tracked"),
        "{warnings:?}"
    );
    assert!(warnings[0].contains("git rm -r --cached .agents/skills"));

    // A tracked symlink is tracked too: pin() rejects one that points elsewhere.
    fs::remove_dir_all(root.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink("../elsewhere", root.join(".claude/skills")).unwrap();
    git(&root, &["add", ".claude/skills"]);
    let (ok, problems, warnings) = doctor(&root);
    assert_eq!(ok, false);
    assert_eq!(
        (problems.len(), warnings.len()),
        (1, 1),
        "{problems:?} {warnings:?}"
    );

    // When git ls-files fails, the check reports nothing.
    fs::write(root.join(".git/index"), "not an index").unwrap();
    let (_, problems, warnings) = doctor(&root);
    assert_eq!((problems, warnings), (vec![], vec![]));
}

#[test]
fn doctor_reports_no_tracked_skill_dir_outside_a_repo() {
    let outside = tempfile::tempdir().unwrap();
    let v = doctor_report(
        ns().current_dir(outside.path())
            .env("GIT_CEILING_DIRECTORIES", outside.path())
            .arg("doctor")
            .assert()
            .get_output(),
    );
    assert!(!v.to_string().contains("/skills is tracked"), "{v}");
}

#[test]
fn doctor_checks_hooks_path_from_a_subdirectory() {
    let (_tmp, root) = repo();
    fs::create_dir(root.join(".githooks")).unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    let v = doctor_report(
        ns().current_dir(root.join("sub"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("doctor")
            .assert()
            .get_output(),
    );
    assert_eq!(v["hooks"]["ok"], false);
    assert_eq!(v["hooks"]["path"], Value::Null);
}

#[test]
fn doctor_skips_the_hooks_check_without_a_githooks_dir() {
    let (_tmp, root) = repo();
    let outside = tempfile::tempdir().unwrap();
    for dir in [root.as_path(), outside.path()] {
        let v = doctor_report(ns().current_dir(dir).arg("doctor").assert().get_output());
        assert_eq!(v["hooks"]["ok"], true, "{}", dir.display());
        assert_eq!(v["hooks"]["fix"], Value::Null);
        assert!(!v["problems"].to_string().contains("hooksPath"));
    }
}

#[test]
fn doctor_shows_the_quality_remote() {
    let (_tmp, root) = repo();
    let outside = tempfile::tempdir().unwrap();
    let quality = |dir: &Path| {
        doctor_report(
            ns().current_dir(dir)
                .env("GIT_CEILING_DIRECTORIES", outside.path())
                .arg("doctor")
                .assert()
                .get_output(),
        )["quality"]
            .clone()
    };
    assert_eq!(quality(&root), serde_json::json!({"remote": "origin"}));
    fs::create_dir(root.join(".nightshift")).unwrap();
    fs::write(
        root.join(".nightshift/nightshift.toml"),
        "[quality]\nremote = \"none\"\n",
    )
    .unwrap();
    assert_eq!(quality(&root), serde_json::json!({"remote": "none"}));
    assert_eq!(quality(outside.path()), Value::Null);
}

fn executable(path: &Path, body: &str) {
    common::write_exe(path, format!("#!/bin/sh\n{body}\n"));
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
        doctor_report(&out)
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
    let v = doctor_report(&out);
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
    factory: Option<&'a str>,
    glab_user: &'a str,
    ssh_greeting: Option<&'a str>,
    /// The `serveraliveinterval` the fake `ssh -G` resolves.
    ssh_alive: u32,
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
            factory: None,
            glab_user: "",
            ssh_greeting: None,
            ssh_alive: 30,
        }
    }
}

impl Doctor<'_> {
    /// The JSON, stderr, and the arguments ssh got (empty when it never ran).
    fn run(&self) -> (Value, String, String) {
        let (tmp, root) = repo();
        git(&root, &["remote", "add", "origin", self.origin]);
        if let Some(f) = self.factory {
            fs::create_dir_all(root.join(".nightshift")).unwrap();
            fs::write(root.join(".nightshift/nightshift.toml"), f).unwrap();
        }
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
        let glab_user = self.glab_user;
        executable(
            &bin.join("glab"),
            &format!(
                r#"case "$*" in
  "api user") printf '%s\n' '{glab_user}' ;;
  "api projects/team%2Fsub%2Fproject") [ {unreadable} = false ] && echo '{{"path_with_namespace":"team/sub/project"}}' || {{ echo "404 Project Not Found, token $GITLAB_TOKEN" >&2; exit 1; }} ;;
  *) exit 1 ;;
esac"#
            ),
        );
        let ssh_args = tmp.path().join("ssh-args");
        let greeting = self.ssh_greeting.map_or_else(
            || {
                format!(
                    "Hi {}! You've successfully authenticated, but GitHub does not provide shell access.",
                    self.ssh_account
                )
            },
            String::from,
        );
        executable(
            &bin.join("ssh"),
            &format!(
                "[ \"$1\" != -G ] || {{ echo 'serveraliveinterval {}'; exit 0; }}\necho \"$*\" > {}\necho \"{greeting}\" >&2\nexit 1",
                self.ssh_alive,
                ssh_args.display(),
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
            .env_remove("NS_FORGE_EXPORTED")
            .env_remove("GH_HOST")
            .env_remove("GITLAB_HOST")
            .env_remove("PERSONAL")
            .env_remove("WORK")
            .env_remove("GIT_SSH_COMMAND")
            .env("GIT_CONFIG_GLOBAL", tmp.path().join("gitconfig"));
        for (k, v) in self.env {
            c.env(k, v);
        }
        let out = c.arg("doctor").assert().get_output().clone();
        let args = fs::read_to_string(&ssh_args).unwrap_or_default();
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        (doctor_report(&out), err, args)
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
        "-T -o BatchMode=yes -o ConnectTimeout=10 -- git@github-work"
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
    assert!(ssh_args.contains("-p 2222 -- git@github.com"), "{ssh_args}");
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
fn doctor_warns_when_an_ssh_origin_has_no_keepalive() {
    let (v, err, _) = Doctor {
        ssh_alive: 0,
        ..Doctor::default()
    }
    .run();
    assert_eq!(v["ssh_keepalive"]["ok"], false, "{v}");
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("origin is SSH (git@github-work) and nothing sets ServerAliveInterval"),
        "{warnings}"
    );
    assert!(
        warnings.contains("git config core.sshCommand 'ssh -o ServerAliveInterval=30"),
        "{warnings}"
    );
    assert!(err.contains("ns: warning: origin is SSH"), "{err}");
    assert!(!v["problems"].to_string().contains("ServerAlive"));

    let (v, _, _) = Doctor::default().run();
    assert_eq!(v["ssh_keepalive"]["source"], "ssh config", "{v}");

    let (v, _, _) = Doctor {
        ssh_alive: 0,
        env: &[
            ("GIT_SSH_COMMAND", "ssh -o ServerAliveInterval=15"),
            ("PERSONAL", "personal-tok"),
            ("WORK", "work-tok"),
        ],
        ..Doctor::default()
    }
    .run();
    assert_eq!(v["ssh_keepalive"]["source"], "GIT_SSH_COMMAND", "{v}");

    let (v, _, _) = Doctor {
        ssh_alive: 0,
        env: &[
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "core.sshCommand"),
            ("GIT_CONFIG_VALUE_0", "ssh -o ServerAliveInterval=15"),
            ("PERSONAL", "personal-tok"),
            ("WORK", "work-tok"),
        ],
        ..Doctor::default()
    }
    .run();
    assert_eq!(v["ssh_keepalive"]["source"], "core.sshCommand", "{v}");

    let (v, _, _) = Doctor {
        origin: "https://github.com/example-org/backend.git",
        ssh_alive: 0,
        ..Doctor::default()
    }
    .run();
    assert_eq!(v["ssh_keepalive"]["ok"], true, "{v}");
    assert!(!v["warnings"].to_string().contains("ServerAlive"));
}

const GITLAB_CONFIG: &str = "[forge.gitlab]
token_env = \"PERSONAL\"
[forge.gitlab.owners.\"team/sub\"]
token_env = \"WORK\"
";

fn gitlab_doctor<'a>() -> Doctor<'a> {
    Doctor {
        origin: "git@gitlab.com:team/sub/project.git",
        config: GITLAB_CONFIG,
        glab_user: r#"{"username":"gl-work"}"#,
        ssh_greeting: Some("Welcome to GitLab, @gl-work!"),
        ..Doctor::default()
    }
}

#[test]
fn doctor_reports_a_gitlab_nested_group_repo_and_its_ssh_account() {
    let (v, err, ssh_args) = gitlab_doctor().run();
    assert_eq!(
        v["forge"]["gitlab"],
        serde_json::json!({"configured": true, "token_resolved": true, "account": "gl-work",
            "table": "[forge.gitlab.owners.\"team/sub\"]", "repo": "team/sub/project",
            "repo_readable": true, "ssh": {"dest": "git@gitlab.com", "account": "gl-work"}})
    );
    assert_eq!(v["forge"]["github"]["repo"], Value::Null);
    assert_eq!(v["warnings"], serde_json::json!([]), "{err}");
    assert!(ssh_args.ends_with("-- git@gitlab.com\n"), "{ssh_args}");

    let (v, _, _) = Doctor {
        unreadable: true,
        ..gitlab_doctor()
    }
    .run();
    assert_eq!(v["forge"]["gitlab"]["repo_readable"], false);
    let problems = v["problems"].to_string();
    assert!(
        problems.contains("account gl-work cannot read team/sub/project"),
        "{problems}"
    );
    assert!(!problems.contains("work-tok"), "{problems}");
}

#[test]
fn doctor_reports_the_default_table_when_the_forge_host_is_not_the_origins() {
    let (v, err, _) = Doctor {
        config: "[forge.gitlab]\ntoken_env = \"PERSONAL\"\nhost = \"gitlab.example.com\"\n[forge.gitlab.owners.\"team/sub\"]\ntoken_env = \"WORK\"\n",
        ..gitlab_doctor()
    }
    .run();
    assert_eq!(v["forge"]["gitlab"]["table"], "[forge.gitlab]", "{err}");
    assert_eq!(v["forge"]["gitlab"]["repo"], "team/sub/project");
}

#[test]
fn doctor_warns_when_ssh_greets_another_gitlab_account() {
    let (v, err, _) = Doctor {
        ssh_greeting: Some("Welcome to GitLab, @gl-personal!"),
        ..gitlab_doctor()
    }
    .run();
    assert_eq!(v["forge"]["gitlab"]["ssh"]["account"], "gl-personal");
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("ssh to git@gitlab.com authenticates as gl-personal"),
        "{warnings}"
    );
    assert!(warnings.contains("account gl-work"), "{warnings}");
    assert!(err.contains("authenticates as gl-personal"), "{err}");
}

#[test]
fn doctor_shows_no_gitlab_account_for_malformed_or_empty_user_json() {
    for user in ["not json", "", "{}", r#"{"username":""}"#] {
        let (v, _, _) = Doctor {
            glab_user: user,
            ..gitlab_doctor()
        }
        .run();
        assert_eq!(v["forge"]["gitlab"]["account"], Value::Null, "{user:?}");
        assert_eq!(v["forge"]["gitlab"]["token_resolved"], true, "{user:?}");
    }
}

#[test]
fn doctor_classifies_an_aliased_origin_by_the_factory_queue_source() {
    let aliased = Doctor {
        origin: "git@work-alias:example-org/backend.git",
        ..Doctor::default()
    };
    let (v, _, _) = aliased.run();
    assert_eq!(v["forge"]["github"]["repo"], "example-org/backend");
    assert_eq!(v["forge"]["gitlab"]["repo"], Value::Null);

    let (v, _, _) = Doctor {
        factory: Some("[queue]\nsource = \"gitlab\"\n"),
        ..aliased
    }
    .run();
    assert_eq!(v["forge"]["gitlab"]["repo"], "example-org/backend");
    assert_eq!(v["forge"]["github"]["repo"], Value::Null);
}

#[test]
fn doctor_compares_the_ssh_and_token_accounts_without_regard_to_case() {
    let (v, err, _) = Doctor {
        ssh_account: "Work-Account",
        ..Doctor::default()
    }
    .run();
    assert_eq!(v["warnings"], serde_json::json!([]), "{err}");
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
    assert!(!problems.contains("cannot read"), "{problems}");

    let (v, _, _) = Doctor {
        origin: "https://github.com/example-org/backend",
        config: "[forge.github]\ntoken_env = \"PERSONAL\"\n",
        env: &[("GH_TOKEN", "env-tok"), ("PERSONAL", "personal-tok")],
        unreadable: true,
        ..Doctor::default()
    }
    .run();
    let problems = v["problems"].to_string();
    assert!(
        problems.contains("token from GH_TOKEN from the environment, overriding [forge.github]"),
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
    for stale in [
        "1",
        "github:example-org/other",
        "gitlab:example-org/backend",
    ] {
        let (v, _, _) = Doctor {
            env: &[("GH_TOKEN", "env-tok"), ("NS_FORGE_EXPORTED", stale)],
            ..Doctor::default()
        }
        .run();
        assert!(
            v["warnings"].to_string().contains("GH_TOKEN"),
            "{stale}: {}",
            v["warnings"]
        );
    }
    let (v, _, _) = Doctor {
        env: &[
            ("GH_TOKEN", "env-tok"),
            ("NS_FORGE_EXPORTED", "github:example-org/backend"),
        ],
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
fn doctor_still_warns_when_a_parent_only_checked_the_repo() {
    let (v, _, _) = Doctor {
        env: &[
            ("GH_TOKEN", "env-tok"),
            ("NS_FORGE_CHECKED", "github:example-org/backend"),
        ],
        ..Doctor::default()
    }
    .run();
    assert!(
        v["warnings"].to_string().contains("GH_TOKEN"),
        "{}",
        v["warnings"]
    );
}

#[test]
fn doctor_warns_when_an_ssh_alias_origin_drops_the_override_tables_of_a_forge_host() {
    let (v, err, _) = Doctor {
        origin: "git@gitlab-work:team/sub/project.git",
        config: "[forge.gitlab]\ntoken_env = \"PERSONAL\"\nhost = \"gitlab.example.com\"\n[forge.gitlab.owners.\"team/sub\"]\ntoken_env = \"WORK\"\n",
        ..gitlab_doctor()
    }
    .run();
    let w = v["warnings"].to_string();
    assert!(
        w.contains("origin host gitlab-work is not gitlab.example.com: ignoring [forge.gitlab.owners.\\\"team/sub\\\"]"),
        "{w}"
    );
    assert!(err.contains("ignoring"), "{err}");
    assert_eq!(v["forge"]["gitlab"]["table"], "[forge.gitlab]");
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
        vec!["artifact", "--help"],
        vec!["artifact", "check", "--help"],
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
fn help_lists_public_subcommands_and_hides_guard_hooks() {
    let out = ns().arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    for cmd in [
        "worktree",
        "ask",
        "lint",
        "check-markers",
        "artifact",
        "install",
        "eval",
        "factory",
        "score",
        "run",
        "watch",
        "clean",
        "log",
        "status",
        "quality",
        "ratchet",
        "rules",
        "transcripts",
        "upstream",
        "doctor",
    ] {
        assert!(
            stdout
                .lines()
                .any(|l| l.split_whitespace().next() == Some(cmd)),
            "ns --help does not list {cmd}:\n{stdout}"
        );
    }
    assert!(!stdout.contains("guard-hooks"), "{stdout}");
}

#[test]
fn help_of_run_and_watch_hides_internal_flags() {
    for (args, shown, hidden) in [
        (
            ["run", "--help"],
            &[
                "--issue",
                "--from",
                "--gates",
                "--base",
                "--dry-run",
                "--cpus",
            ][..],
            "--night",
        ),
        (
            ["watch", "--help"],
            &[
                "--once",
                "--until",
                "--parallel",
                "--dry-run",
                "--no-self-update",
            ][..],
            "--resume-night",
        ),
    ] {
        let out = ns().args(args).assert().success();
        let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
        for flag in shown {
            assert!(stdout.contains(flag), "ns {args:?} lacks {flag}:\n{stdout}");
        }
        assert!(
            !stdout.contains(hidden),
            "ns {args:?} shows {hidden}:\n{stdout}"
        );
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

/// A setup command that writes its shell's pid and its `sleep`'s pid to the fifo `pids`, then
/// waits on the sleep.
fn slow_setup(pids: &Path) -> String {
    format!(
        "name = \"demo\"\n\n[worktree]\nsetup = [\"sleep 600 & echo $$ $! > {}; wait\"]\n",
        pids.display()
    )
}

/// Signal an `ns worktree` run whose setup command sleeps, and check that the shell and the
/// `sleep` it started both end, and that ns reports the stop as a failed setup command.
fn a_signal_ends_the_setup_command(args: &[&str], sig: libc::c_int, prepare: impl Fn(&Path)) {
    let (_tmp, root) = repo();
    prepare(&root);
    let fifo = root.parent().unwrap().join("pids");
    common::mkfifo(&fifo);
    factory_def(&root, &slow_setup(&fifo));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(fs::read_to_string(fifo).unwrap()));
    let mut ns = ns();
    let mut g = ns.current_dir(&root).args(args).start();
    let line = g.recv(&rx);
    let pids: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(pids.len(), 2, "{line:?}");
    // SAFETY: kill(2) on the ns process this test started.
    assert_eq!(unsafe { libc::kill(g.id() as libc::pid_t, sig) }, 0);
    let status = g.wait();
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut g.take_stderr(), &mut stderr).unwrap();
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("exited with a signal") && stderr.contains("ns worktree setup u1"),
        "{stderr}"
    );
    for pid in &pids {
        assert!(common::exits(pid), "pid {pid} outlived ns");
    }
}

#[test]
fn a_sigint_to_worktree_new_ends_the_setup_command() {
    a_signal_ends_the_setup_command(&["worktree", "new", "u1"], libc::SIGINT, |_| ());
}

#[test]
fn a_sigterm_to_worktree_new_ends_the_setup_command() {
    a_signal_ends_the_setup_command(&["worktree", "new", "u1"], libc::SIGTERM, |_| ());
}

#[test]
fn a_sigint_or_sigterm_to_worktree_setup_ends_the_setup_command() {
    for sig in [libc::SIGINT, libc::SIGTERM] {
        a_signal_ends_the_setup_command(&["worktree", "setup", "u1"], sig, |root| {
            ns().current_dir(root)
                .args(["worktree", "new", "u1"])
                .assert()
                .success();
        });
    }
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
fn own_factory_definition_turns_on_hooks_that_run_fast_ci_on_commit_and_push() {
    let (tmp, root) = repo();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    factory_def(
        &root,
        &fs::read_to_string(src.join(".nightshift/nightshift.toml")).unwrap(),
    );
    fs::create_dir_all(root.join(".githooks")).unwrap();
    for hook in ["pre-commit", "pre-push"] {
        common::copy_exe(
            &src.join(".githooks").join(hook),
            &root.join(".githooks").join(hook),
        );
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
    assert!(String::from_utf8_lossy(&out.stderr).contains("ci-local ran with [--fast]\n"));

    // The failed commit left the edit behind; pre-push checks the commit, not the tree.
    let out = git_output(&path, &["push", "-q", "origin", "HEAD:refs/heads/u5"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ci-local ran with [--fast]\n"));
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
/// dir holds `red`, and a bare remote. When the common git dir holds `nested`, a run first runs
/// the pre-push hook itself, once, on its HEAD, as CI's own tests do. When the common git dir holds `build`, each run also
/// stands in for cargo: it marks the target dir busy, builds its HEAD into it, writes its HEAD
/// to `started`, waits for `release`, then tests what the target dir holds. A run that found
/// the target dir busy, or tested another commit's build, appends to `clashes`. Returns the repo, the remote and the log of runs. Tests
/// commit with `--no-verify` so the log holds only pre-push runs.
fn hooked_repo() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let (tmp, root) = repo();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::create_dir_all(root.join(".githooks")).unwrap();
    for hook in ["pre-commit", "pre-push"] {
        common::copy_exe(
            &src.join(".githooks").join(hook),
            &root.join(".githooks").join(hook),
        );
    }
    fs::create_dir_all(root.join("scripts")).unwrap();
    let log = root.join(".git/ci-runs");
    executable(
        &root.join("scripts/ci-local.sh"),
        &format!(
            r#"cat >/dev/null
common=$(git rev-parse --git-common-dir)
echo "$(git rev-parse HEAD) $PWD $CARGO_TARGET_DIR" >> '{}'
[ ! -e "$common/slow" ] || {{ echo up > "$common/started"; exec sleep 30; }}
if [ -e "$common/nested" ] && [ -z "${{NESTED-}}" ]; then
  echo "HEAD $(git rev-parse HEAD) refs/heads/nested 0000000000000000000000000000000000000000" |
    NESTED=1 .githooks/pre-push origin nested || exit 1
fi
if [ -e "$common/build" ]; then
  head=$(git rev-parse HEAD)
  mkdir -p "$CARGO_TARGET_DIR"
  [ ! -e "$CARGO_TARGET_DIR/busy" ] || echo "busy $head" >> "$common/clashes"
  touch "$CARGO_TARGET_DIR/busy"
  echo "$head" > "$CARGO_TARGET_DIR/built"
  echo "$head" > "$common/started"
  i=0
  while [ ! -e "$common/release" ] && [ $i -lt 600 ]; do sleep 0.1; i=$((i + 1)); done
  [ "$(cat "$CARGO_TARGET_DIR/built")" = "$head" ] || echo "clobbered $head" >> "$common/clashes"
  rm -f "$CARGO_TARGET_DIR/busy"
fi
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
    common::command("git")
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("CARGO_TARGET_DIR")
        .args(["push", "-q", "origin"])
        .args(specs)
        .output()
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
fn pre_push_runs_fast_ci_when_the_commit_pushed_sets_gate_local_build_only() {
    let (_tmp, root, _remote, _log) = hooked_repo();
    let args = root.join(".git/ci-args");
    executable(
        &root.join("scripts/ci-local.sh"),
        &format!("echo \"[$*]\" >> '{}'", args.display()),
    );
    let mut pushed = Vec::new();
    for (n, toml) in [
        None,
        Some("[gate]\nlocal = \"build-only\"   # one full run, after build\n"),
        Some("[merge]\nlocal = \"build-only\"\n[gate]\nlocal = \"every-step\"\n"),
        Some("[ gate ]\n  local=\"build-only\"\n[merge]\npolicy = \"auto\"\n"),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(t) = toml {
            factory_def(&root, t);
        }
        fs::write(root.join("README"), format!("{n}\n")).unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "--no-verify", "-m", "next"]);
        let out = push(&root, &[&format!("HEAD:refs/heads/b{n}")]);
        assert!(out.status.success(), "{out:?}");
        pushed.push(
            fs::read_to_string(&args)
                .unwrap()
                .lines()
                .last()
                .unwrap()
                .to_owned(),
        );
    }
    // Only the commit pushed counts, not the working tree.
    factory_def(&root, "[gate]\nlocal = \"every-step\"\n");
    let out = push(&root, &["HEAD:refs/heads/b4"]);
    assert!(out.status.success(), "{out:?}");
    pushed.push(
        fs::read_to_string(&args)
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_owned(),
    );
    assert_eq!(pushed, ["[]", "[--fast]", "[]", "[--fast]", "[--fast]"]);
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

/// Make `path` a FIFO and return the lines written to it. The test holds a write end too, so a
/// writer never blocks and the reader never sees EOF; a hook that dies before writing leaves
/// the receiver empty, for `Group::recv` to fail at its hang guard.
fn events(path: &Path) -> std::sync::mpsc::Receiver<String> {
    common::mkfifo(path);
    let fifo = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(fifo).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// Start `git push -q origin <spec>` from `dir` in the background, stderr piped.
fn start_push(dir: &Path, spec: &str) -> common::Group {
    common::command("git")
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("CARGO_TARGET_DIR")
        .args(["push", "-q", "origin", spec])
        .start()
}

#[test]
fn concurrent_pre_push_runs_never_share_the_target_dir_and_each_tests_its_own_build() {
    let (tmp, root, _remote, log) = hooked_repo();
    let linked = tmp.path().join("linked");
    let linked_arg = linked.to_str().unwrap();
    git(&root, &["worktree", "add", "-q", "-b", "side", linked_arg]);
    fs::write(linked.join("side.txt"), "side\n").unwrap();
    git(&linked, &["add", "side.txt"]);
    git(&linked, &["commit", "-q", "--no-verify", "-m", "side"]);
    let (a, b) = (
        git(&root, &["rev-parse", "HEAD"]),
        git(&linked, &["rev-parse", "HEAD"]),
    );
    let common = root.join(".git");
    fs::write(common.join("build"), "").unwrap();
    let started = events(&common.join("started"));

    let mut first = start_push(&root, "HEAD:refs/heads/a");
    assert_eq!(
        first.recv(&started),
        a,
        "the first push built another commit"
    );
    let mut second = start_push(&linked, "HEAD:refs/heads/side");
    let (tx, rx) = std::sync::mpsc::channel();
    let err = std::io::BufReader::new(second.take_stderr());
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in err.lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    loop {
        let line = second.recv(&rx);
        if line.contains("pre-push: waiting for another push's checks") {
            break;
        }
    }
    assert_eq!(
        started.try_recv().ok(),
        None,
        "the second push built while the first held the target dir"
    );
    fs::write(common.join("release"), "").unwrap();
    assert!(first.wait().success());
    assert!(second.wait().success());
    assert_eq!(second.recv(&started), b);

    let clashes = fs::read_to_string(common.join("clashes")).unwrap_or_default();
    assert_eq!(clashes, "", "the runs shared the target dir");
    let commits: Vec<_> = runs(&log).into_iter().map(|r| r.0).collect();
    assert_eq!(commits, [a, b]);
}

/// CI's own tests run this hook inside a push's checks, sharing its target dir.
#[test]
fn pre_push_run_inside_a_pre_push_run_does_not_wait_on_its_parents_lock() {
    let (_tmp, root, _remote, log) = hooked_repo();
    let head = git(&root, &["rev-parse", "HEAD"]);
    fs::write(root.join(".git/nested"), "").unwrap();
    let mut push = start_push(&root, "HEAD:refs/heads/a");
    assert!(push.wait().success());
    let runs = runs(&log);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(runs.iter().all(|r| r.0 == head), "{runs:?}");
    assert_runs_left_no_worktree(&root, &log);
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
    let out = common::command("git")
        .current_dir(&root)
        .env_remove("CARGO_TARGET_DIR")
        .env("GIT_DIR", root.join(".git"))
        .env("GIT_WORK_TREE", &root)
        .env("GIT_INDEX_FILE", &index)
        .args(["push", "-q", "origin", &format!("{old}:refs/heads/a")])
        .output();
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
    let out = common::command("git")
        .current_dir(&root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("CARGO_TARGET_DIR", &target)
        .args(["push", "-q", "origin", "HEAD:refs/heads/a"])
        .output();
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
        let started = events(&root.join(".git/started"));
        let mut hook = hook.start();
        hook.recv(&started);
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
        let out = common::command("sh")
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
            .output();
        assert!(!out.status.success(), "{hook} passed with skip variables");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("red"),
            "{hook} did not run ci-local.sh"
        );
    }
}

#[test]
fn guard_hooks_denies_a_write_through_a_skills_link_by_relative_and_absolute_path() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    for link in [".claude/skills", ".agents/skills"] {
        let path = dir.join(link);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../.ns/u1/skills", &path).unwrap();
    }
    let guard = |call: serde_json::Value| {
        let out = ns()
            .current_dir(&dir)
            .arg("guard-hooks")
            .write_stdin(call.to_string())
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap().contains("\"deny\"")
    };
    let bash = |c: String| serde_json::json!({"tool_name": "Bash", "tool_input": {"command": c}});
    for link in [".claude/skills", ".agents/skills"] {
        assert!(
            guard(bash(format!("rm -rf {link}/ns-x"))),
            "relative {link}"
        );
        assert!(
            guard(bash(format!("rm -rf {}/ns-x", dir.join(link).display()))),
            "absolute {link}"
        );
        assert!(
            guard(
                serde_json::json!({"tool_name": "Edit", "tool_input": {"file_path": format!("{link}/ns-x/SKILL.md")}})
            ),
            "edit {link}"
        );
    }
    assert!(!guard(bash("rm -rf build".into())));
}

#[test]
fn doctor_reports_no_account_when_gh_api_user_fails_or_prints_nothing() {
    for login in ["half-a-login; exit 1", "''"] {
        let (v, _, _) = Doctor {
            login,
            ..Doctor::default()
        }
        .run();
        assert_eq!(v["forge"]["github"]["account"], Value::Null, "{login}");
        assert_eq!(v["forge"]["github"]["token_resolved"], true, "{login}");
    }
}

const VALID_EVIDENCE: &str = "---
schema: ns-artifact/2
unit: 142-uart-timeout
phase: verify
status: fail
cause: criteria_failed
next: build
reason: \"AC2 fails: the timeout is still 5s\"
sha: 0e05787
updated: 2026-10-08T21:14:00Z
---
# Evidence: 142-uart-timeout
";

fn artifact_check(text: &str, extra: &[&str]) -> (Option<i32>, serde_json::Value) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("evidence.md");
    fs::write(&file, text).unwrap();
    let out = ns()
        .args(["artifact", "check", file.to_str().unwrap()])
        .args(extra)
        .output();
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

#[test]
fn artifact_check_prints_the_routing_fields_of_a_valid_artifact() {
    let (code, v) = artifact_check(VALID_EVIDENCE, &["--phase", "verify"]);
    assert_eq!(code, Some(0));
    assert_eq!(v["ok"], true);
    assert_eq!(v["schema"], "ns-artifact/2");
    assert_eq!(v["phase"], "verify");
    assert_eq!(v["status"], "fail");
    assert_eq!(v["cause"], "criteria_failed");
    assert_eq!(v["next"], "build");
    assert_eq!(v["errors"], serde_json::json!([]));
}

#[test]
fn artifact_check_exits_1_with_each_error_and_the_values_as_written() {
    let text = VALID_EVIDENCE.replace("cause: criteria_failed", "cause: flaky");
    let (code, v) = artifact_check(&text, &["--phase", "review"]);
    assert_eq!(code, Some(1));
    assert_eq!(v["ok"], false);
    assert_eq!(v["cause"], "flaky");
    assert_eq!(v["phase"], "verify");
    assert_eq!(
        v["errors"],
        serde_json::json!([
            "`phase: verify` but this artifact is checked as `review`",
            "`cause: flaky` is not one of none, findings_open, criteria_failed, gate_red, base_moved, denied, needs_human, out_of_scope",
        ])
    );
}

#[test]
fn artifact_check_rejects_a_v1_artifact() {
    let (code, v) = artifact_check("---\nstatus: pass\nsha: abc1234\n---\nbody\n", &[]);
    assert_eq!(code, Some(1));
    assert_eq!(v["schema"], serde_json::Value::Null);
    assert_eq!(
        v["errors"][0],
        "`schema` is missing: add `schema: ns-artifact/2` (an artifact with no schema reads as ns-artifact/1)"
    );
}

#[test]
fn artifact_check_usage_errors_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.md");
    ns().args(["artifact", "check", missing.to_str().unwrap()])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("cannot read"))
        .stderr(predicate::str::contains("ns artifact check"));
    let file = dir.path().join("evidence.md");
    fs::write(&file, VALID_EVIDENCE).unwrap();
    ns().args(["artifact", "check", file.to_str().unwrap(), "--phase", "qa"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "possible values: triage, troubleshoot, build, verify, review, review-fix, ship",
        ));
}

/// A repo on branch `ns/u`, one commit past `main` that changes `src/a.py`, with
/// `.ns/u/brief.md` naming `base: main` and `.ns/u/build.md` holding `risk` under Open risks.
fn build_artifact_in_unit(risk: &str) -> (TempDir, PathBuf) {
    let (tmp, root) = repo();
    git(&root, &["checkout", "-q", "-b", "ns/u"]);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "x = 1\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "feat: a"]);
    let dir = root.join(".ns/u");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("brief.md"), "---\nbase: main\n---\nbrief\n").unwrap();
    let build = dir.join("build.md");
    fs::write(
        &build,
        format!(
            "---\nschema: ns-artifact/2\nunit: u\nphase: build\nstatus: pass\ncause: none\nnext: verify\nsha: abc1234\nupdated: 2026-10-08T21:14:00Z\n---\n## Open risks\n{risk}\n"
        ),
    )
    .unwrap();
    (tmp, build)
}

#[test]
fn artifact_check_build_rejects_an_untagged_file_line_on_a_touched_file() {
    let check = |risk: &str| {
        let (_tmp, build) = build_artifact_in_unit(risk);
        let out = ns()
            .args([
                "artifact",
                "check",
                build.to_str().unwrap(),
                "--phase",
                "build",
            ])
            .output();
        (out.status.code(), json(&out.stdout))
    };
    let (code, v) = check("- file: off by one: src/a.py:1, x is 1");
    assert_eq!(code, Some(1));
    assert_eq!(
        v["errors"],
        serde_json::json!([
            "a `file:` line names `src/a.py`, which this unit's diff touches: fix it in the unit, or tag the line `unrelated: <reason>` or `too-big: <lines>`"
        ])
    );
    // A path relative to the artifact's own directory finds brief.md and the repo there.
    let (_tmp, build) = build_artifact_in_unit("- file: off by one: src/a.py:1");
    ns().current_dir(build.parent().unwrap())
        .args(["artifact", "check", "build.md", "--phase", "build"])
        .assert()
        .code(1);
    for ok in [
        "- file: off by one: src/a.py:1, unrelated: the parser, not this feature",
        "- file: off by one: src/a.py:1, too-big: 240",
        "- file: stale intro: README:1",
        "- note: src/a.py has an untested path",
    ] {
        let (code, v) = check(ok);
        assert_eq!(code, Some(0), "{ok}: {v}");
    }
}

#[test]
fn artifact_check_build_finds_the_base_and_keeps_the_other_errors() {
    let misfiled = "a `file:` line names `src/a.py`, which this unit's diff touches: fix it in the unit, or tag the line `unrelated: <reason>` or `too-big: <lines>`";
    let (_tmp, build) = build_artifact_in_unit("- file: off by one: src/a.py:1");
    // With no brief.md the base is the repo's default branch, origin's HEAD, and the written
    // phase is enough.
    fs::remove_file(build.with_file_name("brief.md")).unwrap();
    let root = build.parent().unwrap().parent().unwrap().parent().unwrap();
    git(root, &["update-ref", "refs/remotes/origin/main", "main"]);
    git(
        root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    let text = fs::read_to_string(&build).unwrap();
    fs::write(&build, text.replace("cause: none", "cause: flaky")).unwrap();
    let out = ns()
        .args(["artifact", "check", build.to_str().unwrap()])
        .output();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        json(&out.stdout)["errors"],
        serde_json::json!([
            "`cause: flaky` is not one of none, findings_open, criteria_failed, gate_red, base_moved, denied, needs_human, out_of_scope",
            misfiled,
        ])
    );
}

fn check_build(build: &Path) -> (Option<i32>, Value) {
    let out = ns()
        .args([
            "artifact",
            "check",
            build.to_str().unwrap(),
            "--phase",
            "build",
        ])
        .output();
    (out.status.code(), json(&out.stdout))
}

#[test]
fn artifact_check_build_fails_when_it_cannot_find_the_diff_a_file_line_needs() {
    let (_tmp, build) = build_artifact_in_unit("- file: off by one: src/a.py:1");
    let brief = build.with_file_name("brief.md");
    let cannot = |v: &Value| {
        let e = v["errors"][0]
            .as_str()
            .unwrap_or_else(|| panic!("{v}"))
            .to_string();
        assert_eq!(v["errors"].as_array().unwrap().len(), 1, "{v}");
        assert!(
            e.contains("cannot tell which files this unit's diff touches"),
            "{e}"
        );
        assert_eq!(v["ok"], false);
    };
    // A base that is not a commit.
    fs::write(&brief, "---\nbase: no-such-branch\n---\nbrief\n").unwrap();
    let (code, v) = check_build(&build);
    assert_eq!(code, Some(1));
    cannot(&v);
    // A base that looks like an option.
    fs::write(&brief, "---\nbase: --x\n---\nbrief\n").unwrap();
    cannot(&check_build(&build).1);
    // Outside a git repo.
    let dir = tempfile::tempdir().unwrap();
    let alone = dir.path().join("build.md");
    fs::copy(&build, &alone).unwrap();
    let (code, v) = check_build(&alone);
    assert_eq!(code, Some(1));
    cannot(&v);
    // An artifact with no `file:` line needs no diff, so it passes where the diff is unreadable.
    fs::write(
        &alone,
        fs::read_to_string(&build)
            .unwrap()
            .replace("- file:", "- note:"),
    )
    .unwrap();
    assert_eq!(check_build(&alone).0, Some(0));
}

#[test]
fn artifact_check_only_holds_a_build_artifact_to_the_touched_files() {
    let (_tmp, build) = build_artifact_in_unit("- file: off by one: src/a.py:1");
    let text = fs::read_to_string(&build).unwrap();
    for phase in ["review", "verify", "ship"] {
        let other = build.with_file_name(format!("{phase}.md"));
        fs::write(
            &other,
            text.replace("phase: build", &format!("phase: {phase}")),
        )
        .unwrap();
        let out = ns()
            .args([
                "artifact",
                "check",
                other.to_str().unwrap(),
                "--phase",
                phase,
            ])
            .output();
        let v = json(&out.stdout);
        let errors = v["errors"].to_string();
        assert!(!errors.contains("`file:` line names"), "{phase}: {errors}");
        assert!(
            !errors.contains("cannot tell which files"),
            "{phase}: {errors}"
        );
    }
}
