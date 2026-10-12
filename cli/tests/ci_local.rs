//! `scripts/ci-local.sh`, run in a temp repo against stub `cargo`, `ns`, `python3`, `python`
//! and the tools `scripts/ci-tools.toml` pins, checked by the real `scripts/ci-tools.sh`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::write_exe;

const ALL_STEPS: [&str; 19] = [
    "pinned tools (scripts/ci-tools.toml)",
    "cargo fmt --check",
    "cargo clippy",
    "cargo test",
    "cargo test under a decoy GIT_DIR",
    "build ns",
    "ns lint",
    "ns lint .agents/skills",
    "ns check-markers",
    "ns rules check",
    "ns ratchet check",
    "ns eval --dry-run",
    "py-inventory fixture tests",
    "Python rule pack tests",
    "JavaScript/TypeScript rule pack tests",
    "C rule pack tests",
    "Java rule pack tests",
    "ns-troubleshoot brief checker tests",
    "private guard",
];

/// The steps `--fast` runs, the pre-commit hook's subset of `ALL_STEPS`, in its order.
const FAST_STEPS: [&str; 8] = [
    "pinned tools (scripts/ci-tools.toml)",
    "cargo fmt --check",
    "cargo clippy",
    "build ns",
    "ns lint",
    "ns lint .agents/skills",
    "ns check-markers",
    "private guard",
];

/// Fails when its command line contains `$FAIL_ON`.
const CARGO: &str = r#"#!/bin/sh
case "$FAIL_ON" in ?*) case "cargo $*" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac
"#;

/// Fails when `$FAIL_ON` is `decoy`.
const DECOY: &str = r#"#!/bin/sh
[ "$FAIL_ON" != decoy ]
"#;

/// Prints `$PLAN` for `eval --dry-run`, even when it then fails because its command line
/// contains `$FAIL_ON`.
const NS: &str = r#"#!/bin/sh
[ -n "$PLAN" ] || PLAN='{"skipped":[]}'
[ "$1" = eval ] && printf '%s\n' "$PLAN"
case "$FAIL_ON" in ?*) case "ns $*" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac
exit 0
"#;

/// A pack's `tests/run.sh`: logs that `pack`'s tests ran, and fails when that log line contains
/// `$FAIL_ON`.
fn rule_pack_tests(pack: &str) -> String {
    format!(
        r#"#!/bin/sh
line="{pack} rule pack tests ran"
echo "$line" >&2
case "$FAIL_ON" in ?*) case "$line" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac
"#
    )
}

/// Hands `-I -` runs (the eval plan filter, and `ci-tools.sh` reading its list) to the real
/// python3 so the script itself is what runs. Any other run logs its directory and fails when
/// that log line contains `$FAIL_ON`.
fn python_stub(real: &Path) -> String {
    format!(
        r#"#!/bin/sh
case "$*" in
  "-I - "*) exec {real:?} "$@" ;;
  *) line="python ran in ${{PWD##*/}}: $*"
     echo "$line" >&2
     case "$FAIL_ON" in ?*) case "$line" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac ;;
esac
"#
    )
}

/// A pinned tool whose `--version` or `-version` prints `version`. Any other run logs its directory and
/// fails when that log line contains `$FAIL_ON`.
fn tool_stub(tool: &str, version: &str) -> String {
    format!(
        r#"#!/bin/sh
case "$*" in
  --version | -version) echo '{tool} {version}' ;;
  *) line="{tool} ran in ${{PWD##*/}}: $*"
     echo "$line" >&2
     case "$FAIL_ON" in ?*) case "$line" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac ;;
esac
"#
    )
}

/// Each pinned tool's name, command (the first word of its `check`), version and `check`.
fn pinned_tools() -> Vec<(String, String, String, String)> {
    let list = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/ci-tools.toml");
    let list: toml::Table = toml::from_str(&fs::read_to_string(list).unwrap()).unwrap();
    list.iter()
        .map(|(name, t)| {
            let check = t["check"].as_str().unwrap();
            let command = check.split_whitespace().next().unwrap().to_owned();
            (
                name.clone(),
                command,
                t["version"].as_str().unwrap().to_owned(),
                check.to_owned(),
            )
        })
        .collect()
}

fn real_python3() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("python3"))
        .find(|p| p.is_file())
        .expect("python3 is required to run the eval plan filter")
}

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts");
        for script in ["ci-local.sh", "ci-tools.sh"] {
            write_exe(
                &root.join("scripts").join(script),
                fs::read_to_string(scripts.join(script)).unwrap(),
            );
        }
        fs::copy(
            scripts.join("ci-tools.toml"),
            root.join("scripts/ci-tools.toml"),
        )
        .unwrap();
        for (_, command, version, _) in pinned_tools() {
            write_exe(
                &root.join("bin").join(&command),
                tool_stub(&command, &version),
            );
        }
        write_exe(&root.join("scripts/check-private.sh"), "#!/bin/sh\n");
        write_exe(&root.join("scripts/check-git-dir-decoy.sh"), DECOY);
        write_exe(&root.join("cli/target/debug/ns"), NS);
        write_exe(&root.join("bin/cargo"), CARGO);
        let python = python_stub(&real_python3());
        write_exe(&root.join("bin/python3"), &python);
        write_exe(&root.join("bin/python"), &python);
        fs::create_dir_all(root.join("evals/fixtures/py-inventory")).unwrap();
        for pack in ["python", "js", "java"] {
            write_exe(
                &root.join("rules").join(pack).join("tests/run.sh"),
                rule_pack_tests(pack),
            );
        }
        let cases = root.join("skills/ns-troubleshoot/evals/cases");
        for case in ["py-hold-expires-early", "py-lost-hold-needs-redesign"] {
            write_exe(&cases.join(case).join("test_check_brief.py"), "");
        }
        fs::create_dir_all(cases.join("py-no-checker")).unwrap();
        Repo { dir }
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.run_in(self.dir.path(), args, env)
    }

    /// Runs the `ci-local.sh` in `checkout`, a checkout of this repo, with this repo's stubs.
    fn run_in(&self, checkout: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = common::command("bash");
        cmd.arg(checkout.join("scripts/ci-local.sh")).args(args);
        self.stub_env(&mut cmd, env);
        cmd.output()
    }

    fn stub_env(&self, cmd: &mut common::Ns, env: &[(&str, &str)]) {
        cmd.env(
            "PATH",
            format!("{}:/usr/bin:/bin", self.dir.path().join("bin").display()),
        )
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CI_LOCAL_CACHE_MAX")
        .env_remove("FAIL_ON")
        .env_remove("PLAN");
        for (k, v) in env {
            cmd.env(k, v);
        }
    }

    /// Makes the repo a git repo on `main` with everything in one commit.
    fn commit_all(&self) {
        let root = self.dir.path();
        fs::write(root.join("README"), "repo\n").unwrap();
        fs::write(root.join("evals/fixtures/py-inventory/.keep"), "").unwrap();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "init"]);
    }

    /// The pass cache's entries.
    fn cached(&self) -> Vec<String> {
        let dir = self.dir.path().join(".git/ns/ci-passed");
        let mut names: Vec<_> = fs::read_dir(dir)
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Runs with only the repo's `bin` on PATH, so a stub removed from it stays missing even
    /// where the real tool is installed under /usr/bin. The few system tools the script needs
    /// are linked in.
    fn run_on_bin_only(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let bin = self.dir.path().join("bin");
        for tool in SYSTEM_TOOLS {
            let real = ["/usr/bin", "/bin"]
                .iter()
                .map(|d| Path::new(d).join(tool))
                .find(|p| p.is_file())
                .unwrap_or_else(|| panic!("{tool} is required to run ci-local"));
            std::os::unix::fs::symlink(real, bin.join(tool)).unwrap();
        }
        let path = bin.display().to_string();
        let mut env = env.to_vec();
        env.push(("PATH", &path));
        self.run(args, &env)
    }
}

/// `bash` and the system tools `scripts/ci-local.sh` runs outside its stubs.
const SYSTEM_TOOLS: [&str; 4] = ["bash", "dirname", "mktemp", "rm"];

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn steps(out: &Output) -> Vec<String> {
    stderr(out)
        .lines()
        .filter_map(|l| l.strip_prefix("==> ").map(str::to_owned))
        .collect()
}

#[test]
fn every_step_runs_in_order_and_the_run_passes() {
    let out = Repo::new().run(&[], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), ALL_STEPS);
    assert!(!stderr(&out).contains("SKIPPED"));
    assert!(!stderr(&out).contains("ci-tools:"));
    assert!(stderr(&out).contains("pytest ran in py-inventory: -q"));
    for case in ["py-hold-expires-early", "py-lost-hold-needs-redesign"] {
        assert!(stderr(&out).contains(&format!(
            "python ran in {case}: -m unittest -q test_check_brief"
        )));
    }
    assert!(!stderr(&out).contains("python ran in py-no-checker"));
    assert!(stderr(&out).contains("python rule pack tests ran"));
    assert!(stderr(&out).contains("java rule pack tests ran"));
    assert!(stderr(&out).contains(": -I -m unittest discover -s rules/c/tests\n"));
    assert!(stderr(&out).contains("ci-local: all checks passed"));
}

#[test]
fn a_failing_step_stops_the_run() {
    for (fail_on, step) in [
        ("cargo clippy", "cargo clippy"),
        ("decoy", "cargo test under a decoy GIT_DIR"),
        ("pytest ran", "py-inventory fixture tests"),
        ("lint .agents/skills", "ns lint .agents/skills"),
        ("ns check-markers", "ns check-markers"),
        ("ns rules", "ns rules check"),
        ("ns ratchet", "ns ratchet check"),
        ("ns eval", "ns eval --dry-run"),
        ("python rule pack tests ran", "Python rule pack tests"),
        (
            "js rule pack tests ran",
            "JavaScript/TypeScript rule pack tests",
        ),
        ("discover -s rules/c/tests", "C rule pack tests"),
        ("java rule pack tests ran", "Java rule pack tests"),
        (
            "python ran in py-hold-expires-early",
            "ns-troubleshoot brief checker tests",
        ),
    ] {
        let out = Repo::new().run(&[], &[("FAIL_ON", fail_on)]);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        let at = ALL_STEPS.iter().position(|s| *s == step).unwrap();
        assert_eq!(steps(&out), ALL_STEPS[..=at]);
        assert!(stderr(&out).contains(&format!("ci-local: FAILED at step: {step}\n")));
        assert!(!stderr(&out).contains("all checks passed"));
    }
}

#[test]
fn a_missing_pinned_tool_fails_the_run_first_naming_its_install_command() {
    for (name, command, _, _) in pinned_tools() {
        let repo = Repo::new();
        fs::remove_file(repo.dir.path().join("bin").join(&command)).unwrap();
        let out = repo.run_on_bin_only(&[], &[]);
        assert_eq!(out.status.code(), Some(1), "{name}: {}", stderr(&out));
        assert_eq!(steps(&out), ALL_STEPS[..1]);
        let err = stderr(&out);
        assert!(
            err.contains(&format!(
                "ci-tools: {name} is not on PATH; install it with: "
            )),
            "{err}"
        );
        assert!(
            err.contains("ci-local: FAILED at step: pinned tools"),
            "{err}"
        );
    }
}

#[test]
fn a_wrong_version_pinned_tool_fails_the_run_first() {
    for (name, command, version, check) in pinned_tools() {
        let repo = Repo::new();
        write_exe(
            &repo.dir.path().join("bin").join(&command),
            tool_stub(&command, "0.0.0"),
        );
        let out = repo.run(&[], &[]);
        assert_eq!(out.status.code(), Some(1), "{name}: {}", stderr(&out));
        assert_eq!(steps(&out), ALL_STEPS[..1]);
        let err = stderr(&out);
        assert!(
            err.contains(&format!(
                "ci-tools: {name} is not version {version} (`{check}` printed: {command} 0.0.0); \
                 install it with: "
            )),
            "{err}"
        );
    }
}

#[test]
fn without_python3_the_run_fails_at_the_first_step() {
    let repo = Repo::new();
    fs::remove_file(repo.dir.path().join("bin/python3")).unwrap();
    let out = repo.run_on_bin_only(&[], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(steps(&out), ALL_STEPS[..1]);
    assert!(stderr(&out).contains("ci-local: FAILED at step: pinned tools"));
}

fn plan(reasons: &[&str]) -> String {
    let skipped: Vec<_> = reasons
        .iter()
        .map(|r| format!(r#"{{"skill":"ns-x","case":"c","reason":"{r}"}}"#))
        .collect();
    format!(r#"{{"skipped":[{}]}}"#, skipped.join(","))
}

#[test]
fn the_eval_filter_allows_only_zephyr_skips() {
    let zephyr = plan(&[
        "missing capability: zephyr",
        "missing capability: zephyr (west)",
    ]);
    let out = Repo::new().run(&[], &[("PLAN", &zephyr)]);
    assert!(out.status.success(), "{}", stderr(&out));

    let other = plan(&["missing capability: zephyr", "missing capability: python"]);
    let out = Repo::new().run(&[], &[("PLAN", &other)]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("skipped: ns-x/c: missing capability: python"));
    assert!(!stderr(&out).contains("skipped: ns-x/c: missing capability: zephyr"));
    assert!(stderr(&out).contains("ci-local: FAILED at step: ns eval --dry-run"));
}

#[test]
fn the_ns_run_is_the_one_under_cargo_target_dir_when_that_is_set() {
    let repo = Repo::new();
    let root = repo.dir.path();
    let shared = root.join("shared");
    fs::create_dir_all(shared.join("debug")).unwrap();
    fs::rename(root.join("cli/target/debug/ns"), shared.join("debug/ns")).unwrap();
    let out = repo.run(&[], &[("CARGO_TARGET_DIR", shared.to_str().unwrap())]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), ALL_STEPS);

    let out = repo.run(&[], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("ci-local: FAILED at step: ns lint\n"));
}

#[test]
fn a_relative_cargo_target_dir_is_relative_to_the_repo_root() {
    let repo = Repo::new();
    let root = repo.dir.path();
    fs::create_dir_all(root.join("shared/debug")).unwrap();
    fs::rename(
        root.join("cli/target/debug/ns"),
        root.join("shared/debug/ns"),
    )
    .unwrap();
    let out = repo.run(&[], &[("CARGO_TARGET_DIR", "shared")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), ALL_STEPS);
}

#[test]
fn fast_runs_only_the_quick_steps_in_order_and_passes() {
    let out = Repo::new().run(&["--fast"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), FAST_STEPS);
    let err = stderr(&out);
    assert!(!err.contains("pytest ran"), "{err}");
    assert!(!err.contains("rule pack tests ran"), "{err}");
    assert!(!err.contains("python ran in"), "{err}");
    assert!(err.contains("ci-local: all checks passed"), "{err}");
}

#[test]
fn a_failing_step_stops_the_fast_run() {
    for (fail_on, step) in [
        ("cargo fmt", "cargo fmt --check"),
        ("cargo clippy", "cargo clippy"),
        ("cargo build", "build ns"),
        ("ns lint skills", "ns lint"),
        ("lint .agents/skills", "ns lint .agents/skills"),
        ("ns check-markers", "ns check-markers"),
    ] {
        let out = Repo::new().run(&["--fast"], &[("FAIL_ON", fail_on)]);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        let at = FAST_STEPS.iter().position(|s| *s == step).unwrap();
        assert_eq!(steps(&out), FAST_STEPS[..=at]);
        assert!(stderr(&out).contains(&format!("ci-local: FAILED at step: {step}\n")));
        assert!(!stderr(&out).contains("all checks passed"));
    }
}

#[test]
fn the_fast_run_fails_on_the_private_guard() {
    let repo = Repo::new();
    write_exe(
        &repo.dir.path().join("scripts/check-private.sh"),
        "#!/bin/sh\nexit 1\n",
    );
    let out = repo.run(&["--fast"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(steps(&out), FAST_STEPS);
    assert!(stderr(&out).contains("ci-local: FAILED at step: private guard\n"));
}

#[test]
fn an_unknown_or_extra_argument_prints_usage_and_exits_2() {
    for args in [
        &["--bogus"][..],
        &["--full"],
        &["--fast", "--fast"],
        &["--fast", "x"],
    ] {
        let out = Repo::new().run(args, &[]);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(
            stderr(&out),
            "usage: scripts/ci-local.sh [--fast] [--no-cache]\n"
        );
    }
}

/// `git -C dir args`, on `dir` whatever location the test inherits, with no user config.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn commit(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    let mut body = fs::read_to_string(&path).unwrap_or_default();
    body.push_str(text);
    fs::write(&path, body).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", file]);
}

/// The line a run that skips on a cached pass prints, up to the `where` it names.
fn already_passed(dir: &Path) -> String {
    let tree = git(dir, &["write-tree"]);
    format!("ci-local: tree {tree} already passed (")
}

fn ran_everything(out: &Output) {
    assert!(out.status.success(), "{}", stderr(out));
    assert_eq!(steps(out), ALL_STEPS);
    assert!(!stderr(out).contains("already passed"), "{}", stderr(out));
}

fn skipped(out: &Output, dir: &Path) {
    assert!(out.status.success(), "{}", stderr(out));
    assert!(steps(out).is_empty(), "{}", stderr(out));
    assert!(
        stderr(out).contains(&already_passed(dir)),
        "{}",
        stderr(out)
    );
}

#[test]
fn a_second_full_run_on_the_same_tree_skips_naming_when_and_where_it_passed() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    ran_everything(&repo.run(&[], &[]));
    assert_eq!(repo.cached().len(), 1);

    let out = repo.run(&[], &[]);
    skipped(&out, root);
    let head = git(root, &["rev-parse", "--short", "HEAD"]);
    let err = stderr(&out);
    let line = err.lines().find(|l| l.contains("already passed")).unwrap();
    let rest = line.strip_prefix(&already_passed(root)).unwrap();
    let (when, place) = rest.strip_suffix(')').unwrap().split_once(", ").unwrap();
    assert_eq!(when.len(), "2026-10-12T00:00:00Z".len(), "{line}");
    assert!(when.ends_with('Z'), "{line}");
    let root = root.canonicalize().unwrap();
    assert_eq!(place, format!("{} at {head}", root.display()));
    assert_eq!(repo.cached().len(), 1);
}

#[test]
fn a_changed_file_ci_tools_toml_or_ci_local_sh_runs_again() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    ran_everything(&repo.run(&[], &[]));
    for (file, text) in [
        ("README", "changed\n"),
        ("scripts/ci-tools.toml", "\n# changed\n"),
        ("scripts/ci-local.sh", "\n# changed\n"),
    ] {
        commit(root, file, text);
        ran_everything(&repo.run(&[], &[]));
        skipped(&repo.run(&[], &[]), root);
    }
    assert_eq!(repo.cached().len(), 4);
}

#[test]
fn a_failure_is_never_cached() {
    let repo = Repo::new();
    repo.commit_all();
    let out = repo.run(&[], &[("FAIL_ON", "ns ratchet")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(repo.cached().is_empty());
    let out = repo.run(&[], &[("FAIL_ON", "ns ratchet")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    ran_everything(&repo.run(&[], &[]));
}

#[test]
fn no_cache_runs_on_a_tree_that_passed() {
    let repo = Repo::new();
    repo.commit_all();
    ran_everything(&repo.run(&[], &[]));
    ran_everything(&repo.run(&["--no-cache"], &[]));
    let out = repo.run(&["--no-cache"], &[("FAIL_ON", "ns ratchet")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    for args in [&["--fast", "--no-cache"][..], &["--no-cache", "--fast"]] {
        let out = repo.run(args, &[]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(steps(&out), FAST_STEPS);
    }
    let out = repo.run(&["--no-cache", "--no-cache"], &[]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn fast_neither_uses_nor_fills_the_cache() {
    let repo = Repo::new();
    repo.commit_all();
    let out = repo.run(&["--fast"], &[]);
    assert_eq!(steps(&out), FAST_STEPS);
    assert!(repo.cached().is_empty());
    ran_everything(&repo.run(&[], &[]));
    let out = repo.run(&["--fast"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), FAST_STEPS);
}

#[test]
fn a_dirty_working_tree_never_uses_or_fills_the_cache() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    ran_everything(&repo.run(&[], &[]));

    fs::write(root.join("README"), "unstaged\n").unwrap();
    let out = repo.run(&[], &[]);
    ran_everything(&out);
    assert!(stderr(&out).contains("ci-local: the working tree differs from the index"));
    assert_eq!(repo.cached().len(), 1);
    git(root, &["checkout", "README"]);

    fs::write(root.join("scratch"), "untracked\n").unwrap();
    let out = repo.run(&[], &[]);
    ran_everything(&out);
    assert!(stderr(&out).contains("ci-local: the working tree has untracked files"));
    assert_eq!(repo.cached().len(), 1);
    fs::remove_file(root.join("scratch")).unwrap();

    skipped(&repo.run(&[], &[]), root);
}

#[test]
fn a_run_that_changes_the_tree_it_tests_is_not_recorded() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    write_exe(
        &root.join("scripts/check-private.sh"),
        "#!/bin/sh\necho edited >> README\n",
    );
    git(root, &["commit", "-qam", "edits"]);
    let out = repo.run(&[], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("ci-local: the tree changed during the run"));
    assert!(repo.cached().is_empty());
}

/// The pre-push hook records the pushed commit's tree; the gate in another worktree, and the
/// merge queue's gate on a merge result staged in a scratch worktree, skip on it.
#[test]
fn pre_push_the_gate_and_the_merge_queue_share_the_cache() {
    let repo = Repo::new();
    let root = repo.dir.path();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.githooks/pre-push");
    write_exe(
        &root.join(".githooks/pre-push"),
        fs::read_to_string(src).unwrap(),
    );
    repo.commit_all();
    write_exe(&root.join(".git/pre-push-target/debug/ns"), NS);
    git(root, &["checkout", "-q", "-b", "pr"]);
    commit(root, "pr.txt", "pr\n");
    let pr = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "-q", "main"]);
    commit(root, "main.txt", "main\n");
    git(root, &["checkout", "-q", "pr"]);
    git(root, &["merge", "-q", "--no-edit", "main"]);
    let merged = git(root, &["rev-parse", "HEAD"]);

    let mut hook = common::command("sh");
    hook.arg(root.join(".githooks/pre-push"))
        .current_dir(root)
        .env("GIT_DIR", root.join(".git"))
        .env_remove("GIT_WORK_TREE")
        .write_stdin(format!(
            "refs/heads/pr {merged} refs/heads/pr {}\n",
            "0".repeat(40)
        ));
    repo.stub_env(&mut hook, &[]);
    let out = hook.output();
    ran_everything(&out);
    assert_eq!(repo.cached().len(), 1);

    let worktrees = tempfile::tempdir().unwrap();
    let gate = worktrees.path().join("gate");
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            gate.to_str().unwrap(),
            &merged,
        ],
    );
    skipped(&repo.run_in(&gate, &[], &[]), &gate);

    let scratch = worktrees.path().join("merge-queue");
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            scratch.to_str().unwrap(),
            &pr,
        ],
    );
    git(&scratch, &["merge", "-q", "--no-commit", "--no-ff", "main"]);
    skipped(&repo.run_in(&scratch, &[], &[]), &scratch);
    assert_eq!(repo.cached().len(), 1);
}

#[test]
fn the_cache_keeps_the_entries_used_last() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    let max = [("CI_LOCAL_CACHE_MAX", "2")];
    ran_everything(&repo.run(&[], &max));
    let first = git(root, &["rev-parse", "HEAD"]);
    commit(root, "README", "2\n");
    let second = git(root, &["rev-parse", "HEAD"]);
    ran_everything(&repo.run(&[], &max));
    git(root, &["checkout", "-q", &first]);
    skipped(&repo.run(&[], &max), root);
    git(root, &["checkout", "-q", "main"]);
    commit(root, "README", "3\n");
    ran_everything(&repo.run(&[], &max));
    assert_eq!(repo.cached().len(), 2);
    git(root, &["checkout", "-q", &first]);
    skipped(&repo.run(&[], &max), root);
    git(root, &["checkout", "-q", &second]);
    ran_everything(&repo.run(&[], &max));
}
