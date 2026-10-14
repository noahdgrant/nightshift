//! `scripts/ci-local.sh`, run in a temp repo against stub `cargo`, `ns`, `python3`, `python`
//! and the tools `scripts/ci-tools.d/` pins, checked by the real `scripts/ci-tools.sh`.

#[path = "ci_local/affected_rules.rs"]
mod affected_rules;
#[path = "ci_local/cache.rs"]
mod cache;
mod common;

use std::fs;

use std::path::{Path, PathBuf};
use std::process::Output;

use common::write_exe;

/// The full run's steps before the rule packs'.
const BEFORE_PACKS: &[&str] = &[
    "pinned tools (scripts/ci-tools.d)",
    "cargo fmt --check",
    "cargo clippy",
    "cargo test",
    "cargo test under a decoy GIT_DIR",
    "build ns",
    "ns lint",
    "ns lint .agents/skills",
    "verify-nightshift Launch and Doctor recipes",
    "ns check-markers",
    "ns rules check",
    "ns ratchet check",
    "ns eval --dry-run",
    "py-inventory fixture tests",
];

/// The full run's steps after the rule packs'.
const AFTER_PACKS: &[&str] = &["ns-troubleshoot brief checker tests", "private guard"];

/// This checkout's rule packs, as `scripts/rule-packs.sh` finds them: each `rules/<pack>/` with
/// a `tests/run.sh`, with the name in its `NAME`, in order of `<pack>`.
fn packs() -> Vec<(String, String)> {
    let rules = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rules");
    let mut packs: Vec<_> = fs::read_dir(rules)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("tests/run.sh").is_file())
        .map(|p| {
            let pack = p.file_name().unwrap().to_string_lossy().into_owned();
            let name = fs::read_to_string(p.join("NAME")).unwrap();
            (pack, name.trim_end().to_owned())
        })
        .collect();
    packs.sort();
    packs
}

/// Every step a full run takes, in order: one per rule pack among the fixed ones.
fn all_steps() -> Vec<String> {
    let packs = packs()
        .into_iter()
        .map(|(_, name)| format!("{name} rule pack tests"));
    BEFORE_PACKS
        .iter()
        .map(|s| s.to_string())
        .chain(packs)
        .chain(AFTER_PACKS.iter().map(|s| s.to_string()))
        .collect()
}

/// The steps `--fast` runs, the pre-commit hook's subset of `all_steps`, in its order.
const FAST_STEPS: &[&str] = &[
    "pinned tools (scripts/ci-tools.d)",
    "cargo fmt --check",
    "cargo clippy",
    "build ns",
    "ns lint",
    "ns lint .agents/skills",
    "ns check-markers",
    "ns rules check --only ast-grep",
    "private guard",
];

/// Fails when its command line contains `$FAIL_ON`.
const CARGO: &str = r#"#!/bin/sh
case "$FAIL_ON" in ?*) case "cargo $*" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac
"#;

/// A stub script that fails when its path contains `$FAIL_ON`.
const SCRIPT: &str = r#"#!/bin/sh
case "$FAIL_ON" in ?*) case "$0" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac
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

/// `scripts/ci-tools.d/`, where each pinned tool has a `<tool>.toml`.
fn pins() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/ci-tools.d")
}

/// Each pinned tool's name, command (the first word of its `check`), version and `check`.
fn pinned_tools() -> Vec<(String, String, String, String)> {
    fs::read_dir(pins())
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let t: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let check = t["check"].as_str().unwrap();
            let command = check.split_whitespace().next().unwrap().to_owned();
            (
                name,
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
        for script in [
            "ci-local.sh",
            "ci-tools.sh",
            "ci-affected.sh",
            "rule-packs.sh",
        ] {
            write_exe(
                &root.join("scripts").join(script),
                fs::read_to_string(scripts.join(script)).unwrap(),
            );
        }
        fs::create_dir_all(root.join("scripts/ci-tools.d")).unwrap();
        for pin in fs::read_dir(pins()).unwrap() {
            let pin = pin.unwrap();
            fs::copy(
                pin.path(),
                root.join("scripts/ci-tools.d").join(pin.file_name()),
            )
            .unwrap();
        }
        for (_, command, version, _) in pinned_tools() {
            write_exe(
                &root.join("bin").join(&command),
                tool_stub(&command, &version),
            );
        }
        write_exe(&root.join("scripts/check-private.sh"), "#!/bin/sh\n");
        write_exe(&root.join("scripts/check-git-dir-decoy.sh"), SCRIPT);
        write_exe(&root.join("scripts/check-verify-recipes.sh"), SCRIPT);
        write_exe(&root.join("cli/target/debug/ns"), NS);
        write_exe(&root.join("bin/cargo"), CARGO);
        let python = python_stub(&real_python3());
        write_exe(&root.join("bin/python3"), &python);
        write_exe(&root.join("bin/python"), &python);
        fs::create_dir_all(root.join("evals/fixtures/py-inventory")).unwrap();
        for (pack, name) in packs() {
            add_pack(root, &pack, &name);
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

/// Adds a rule pack named `name` in `rules/<pack>/`, whose tests are `rule_pack_tests(pack)`.
fn add_pack(root: &Path, pack: &str, name: &str) {
    let dir = root.join("rules").join(pack);
    write_exe(&dir.join("tests/run.sh"), rule_pack_tests(pack));
    fs::write(dir.join("NAME"), format!("{name}\n")).unwrap();
    fs::write(dir.join("TOOLS"), "ruff\n").unwrap();
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
    assert_eq!(steps(&out), all_steps());
    assert!(!stderr(&out).contains("SKIPPED"));
    assert!(!stderr(&out).contains("ci-tools:"));
    assert!(stderr(&out).contains("pytest ran in py-inventory: -q"));
    for case in ["py-hold-expires-early", "py-lost-hold-needs-redesign"] {
        assert!(stderr(&out).contains(&format!(
            "python ran in {case}: -m unittest -q test_check_brief"
        )));
    }
    assert!(!stderr(&out).contains("python ran in py-no-checker"));
    assert!(packs().len() >= 2, "{:?}", packs());
    for (pack, _) in packs() {
        assert!(stderr(&out).contains(&format!("{pack} rule pack tests ran")));
    }
    assert!(stderr(&out).contains("ci-local: all checks passed"));
}

#[test]
fn a_failing_step_stops_the_run() {
    let fixed = [
        ("cargo clippy", "cargo clippy"),
        ("decoy", "cargo test under a decoy GIT_DIR"),
        ("pytest ran", "py-inventory fixture tests"),
        ("lint .agents/skills", "ns lint .agents/skills"),
        ("recipes", "verify-nightshift Launch and Doctor recipes"),
        ("ns check-markers", "ns check-markers"),
        ("ns rules", "ns rules check"),
        ("ns ratchet", "ns ratchet check"),
        ("ns eval", "ns eval --dry-run"),
        (
            "python ran in py-hold-expires-early",
            "ns-troubleshoot brief checker tests",
        ),
    ]
    .map(|(f, s)| (f.to_owned(), s.to_owned()));
    let packs = packs().into_iter().map(|(pack, name)| {
        (
            format!("{pack} rule pack tests ran"),
            format!("{name} rule pack tests"),
        )
    });
    let all = all_steps();
    for (fail_on, step) in fixed.into_iter().chain(packs) {
        let out = Repo::new().run(&[], &[("FAIL_ON", &fail_on)]);
        assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
        let at = all.iter().position(|s| *s == step).unwrap();
        assert_eq!(steps(&out), all[..=at]);
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
        assert_eq!(steps(&out), all_steps()[..1]);
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
        assert_eq!(steps(&out), all_steps()[..1]);
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
    assert_eq!(steps(&out), all_steps()[..1]);
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
    assert_eq!(steps(&out), all_steps());

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
    assert_eq!(steps(&out), all_steps());
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
        ("ns rules check --only", "ns rules check --only ast-grep"),
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

/// The steps `scripts/ci-affected.sh` runs on any change, before the tests under changed paths.
const AFFECTED_ALWAYS: &[&str] = &["cargo test", "ns rules check --only ast-grep"];

/// `scripts/ci-affected.sh` with `NS_GATE_FILES` set to `files`, or unset.
fn affected(repo: &Repo, files: Option<&str>, env: &[(&str, &str)]) -> Output {
    let mut cmd = common::command("bash");
    cmd.arg(repo.dir.path().join("scripts/ci-affected.sh"));
    repo.stub_env(&mut cmd, env);
    match files {
        Some(f) => cmd.env("NS_GATE_FILES", f),
        None => cmd.env_remove("NS_GATE_FILES"),
    };
    cmd.output()
}

#[test]
fn ci_affected_runs_cargo_test_and_the_tests_under_each_changed_path() {
    let repo = Repo::new();
    let packs = packs();
    let [(a, a_name), (b, b_name), ..] = &packs[..] else {
        panic!("this checkout needs two rule packs: {packs:?}");
    };
    let (a_step, b_step) = (
        format!("{a_name} rule pack tests"),
        format!("{b_name} rule pack tests"),
    );
    let always: Vec<String> = AFFECTED_ALWAYS.iter().map(|s| s.to_string()).collect();
    let mut all = always.clone();
    all.extend(packs.iter().map(|(_, n)| format!("{n} rule pack tests")));
    all.push("py-inventory fixture tests".to_owned());
    for (files, want) in [
        (None, all.clone()),
        (Some(String::new()), vec![]),
        (Some("skills/ns-build/SKILL.md".to_owned()), always.clone()),
        (Some(format!("rules/{a}x/y\nrules/{a}")), always.clone()),
        (
            Some(format!(
                "rules/{b}/src/a.ts\nrules/{a}/tests/test_x.py\ndocs/FACTORY.md"
            )),
            [always.clone(), vec![a_step.clone(), b_step.clone()]].concat(),
        ),
        (
            Some(format!(
                "evals/fixtures/py-inventory/x.py\nrules/{b}/a.java"
            )),
            [
                always.clone(),
                vec![b_step, "py-inventory fixture tests".to_owned()],
            ]
            .concat(),
        ),
    ] {
        let out = affected(&repo, files.as_deref(), &[]);
        assert!(out.status.success(), "{files:?}: {}", stderr(&out));
        assert_eq!(steps(&out), want, "{files:?}");
    }
}

#[test]
fn ci_affected_stops_at_a_failing_step() {
    let repo = Repo::new();
    let packs = packs();
    let [(a, a_name), (b, _), ..] = &packs[..] else {
        panic!("this checkout needs two rule packs: {packs:?}");
    };
    let out = affected(
        &repo,
        Some(&format!("rules/{a}/a.py\nrules/{b}/a.ts")),
        &[("FAIL_ON", &format!("{a} rule pack"))],
    );
    assert_eq!(out.status.code(), Some(1));
    let step = format!("{a_name} rule pack tests");
    assert_eq!(steps(&out), [AFFECTED_ALWAYS, &[&step]].concat());
    assert!(stderr(&out).contains(&format!("ci-affected: FAILED at step: {step}")));
}

/// The rule packs `scripts/rule-packs.sh` lists in `root`.
fn listed_packs(root: &Path) -> Output {
    let mut cmd = common::command("bash");
    cmd.arg(root.join("scripts/rule-packs.sh"));
    cmd.output()
}

#[test]
fn a_new_pack_needs_only_files_under_its_own_directory() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    add_pack(root, "zz", "Zed");
    git(root, &["add", "-A"]);
    let changed = git(root, &["diff", "--cached", "--name-only"]);
    assert_eq!(
        changed,
        "rules/zz/NAME\nrules/zz/TOOLS\nrules/zz/tests/run.sh"
    );

    let out = listed_packs(root);
    assert!(out.status.success(), "{}", stderr(&out));
    let list = String::from_utf8_lossy(&out.stdout);
    assert!(list.ends_with("zz\tZed\n"), "{list}");

    let out = repo.run(&[], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let mut want = all_steps();
    let at = want.len() - AFTER_PACKS.len();
    want.insert(at, "Zed rule pack tests".to_owned());
    assert_eq!(steps(&out), want);
    assert!(stderr(&out).contains("zz rule pack tests ran"));

    let out = affected(&repo, Some("rules/zz/tests/run.sh"), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        steps(&out),
        [AFFECTED_ALWAYS, &["Zed rule pack tests"]].concat()
    );
    assert!(stderr(&out).contains("zz rule pack tests ran"));
}

#[test]
fn a_pack_without_a_one_line_name_fails_the_run_naming_its_name_file() {
    for name in [None, Some(""), Some("One\nTwo\n"), Some("One\tTwo\n")] {
        let repo = Repo::new();
        let root = repo.dir.path();
        add_pack(root, "zz", "Zed");
        match name {
            Some(n) => fs::write(root.join("rules/zz/NAME"), n).unwrap(),
            None => fs::remove_file(root.join("rules/zz/NAME")).unwrap(),
        }
        let want = "rule-packs: rules/zz/NAME must hold the pack's name on one line\n";
        let out = listed_packs(root);
        assert_eq!(out.status.code(), Some(1), "{name:?}");
        assert_eq!(stderr(&out), want, "{name:?}");

        for args in [&[][..], &["--fast"]] {
            let out = repo.run(args, &[]);
            assert_eq!(out.status.code(), Some(1), "{name:?}: {}", stderr(&out));
            assert!(steps(&out).is_empty(), "{name:?}: {}", stderr(&out));
            assert!(stderr(&out).contains(want), "{name:?}: {}", stderr(&out));
        }

        let out = affected(&repo, Some("rules/zz/x"), &[]);
        assert_eq!(out.status.code(), Some(1), "{name:?}: {}", stderr(&out));
        assert!(stderr(&out).contains(want), "{name:?}: {}", stderr(&out));
    }
}

#[test]
fn a_pack_without_tool_names_fails_the_run_naming_its_tools_file() {
    for tools in [None, Some(""), Some("\n"), Some(" \t\n")] {
        let repo = Repo::new();
        let root = repo.dir.path();
        add_pack(root, "zz", "Zed");
        match tools {
            Some(t) => fs::write(root.join("rules/zz/TOOLS"), t).unwrap(),
            None => fs::remove_file(root.join("rules/zz/TOOLS")).unwrap(),
        }
        let want = "rule-packs: rules/zz/TOOLS must name the pack's pinned tools\n";
        let out = listed_packs(root);
        assert_eq!(out.status.code(), Some(1), "{tools:?}");
        assert_eq!(stderr(&out), want, "{tools:?}");
    }
}

#[test]
fn a_rules_directory_without_tests_run_sh_is_no_pack() {
    let repo = Repo::new();
    let root = repo.dir.path();
    fs::create_dir_all(root.join("rules/nopack/tests")).unwrap();
    fs::write(root.join("rules/nopack/NAME"), "Nope\n").unwrap();
    let out = repo.run(&[], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), all_steps());

    fs::remove_dir_all(root.join("rules")).unwrap();
    let out = listed_packs(root);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let out = repo.run(&[], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let fixed: Vec<_> = BEFORE_PACKS.iter().chain(AFTER_PACKS).copied().collect();
    assert_eq!(steps(&out), fixed);
    let out = affected(&repo, Some("rules/x/y"), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), AFFECTED_ALWAYS);
    let out = affected(&repo, None, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        steps(&out),
        [AFFECTED_ALWAYS, &["py-inventory fixture tests"]].concat()
    );
}
