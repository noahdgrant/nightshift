//! `scripts/ci-local.sh`, run in a temp repo against stub `cargo`, `ns`, `python3`, `python`
//! and the tools `scripts/ci-tools.toml` pins, checked by the real `scripts/ci-tools.sh`.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

const ALL_STEPS: [&str; 18] = [
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
    "ns-troubleshoot brief checker tests",
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

/// Logs that it ran, and fails when that log line contains `$FAIL_ON`.
const RULE_PACK_TESTS: &str = r#"#!/bin/sh
line="rule pack tests ran"
echo "$line" >&2
case "$FAIL_ON" in ?*) case "$line" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac
"#;

const JS_RULE_PACK_TESTS: &str = r#"#!/bin/sh
line="js rule pack tests ran"
echo "$line" >&2
case "$FAIL_ON" in ?*) case "$line" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac
"#;

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

/// A pinned tool whose `--version` prints `version`. Any other run logs its directory and
/// fails when that log line contains `$FAIL_ON`.
fn tool_stub(tool: &str, version: &str) -> String {
    format!(
        r#"#!/bin/sh
case "$*" in
  --version) echo '{tool} {version}' ;;
  *) line="{tool} ran in ${{PWD##*/}}: $*"
     echo "$line" >&2
     case "$FAIL_ON" in ?*) case "$line" in *"$FAIL_ON"*) exit 1 ;; esac ;; esac ;;
esac
"#
    )
}

/// Each pinned tool's name, command (the first word of its `check`) and version.
fn pinned_tools() -> Vec<(String, String, String)> {
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

fn write_exe(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
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
                &fs::read_to_string(scripts.join(script)).unwrap(),
            );
        }
        fs::copy(
            scripts.join("ci-tools.toml"),
            root.join("scripts/ci-tools.toml"),
        )
        .unwrap();
        for (_, command, version) in pinned_tools() {
            write_exe(
                &root.join("bin").join(&command),
                &tool_stub(&command, &version),
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
        write_exe(&root.join("rules/python/tests/run.sh"), RULE_PACK_TESTS);
        write_exe(&root.join("rules/js/tests/run.sh"), JS_RULE_PACK_TESTS);
        let cases = root.join("skills/ns-troubleshoot/evals/cases");
        for case in ["py-hold-expires-early", "py-lost-hold-needs-redesign"] {
            write_exe(&cases.join(case).join("test_check_brief.py"), "");
        }
        fs::create_dir_all(cases.join("py-no-checker")).unwrap();
        Repo { dir }
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let root = self.dir.path();
        let mut cmd = common::command("bash");
        cmd.arg(root.join("scripts/ci-local.sh"))
            .args(args)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", root.join("bin").display()),
            )
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("FAIL_ON")
            .env_remove("PLAN");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output()
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
    assert!(stderr(&out).contains("rule pack tests ran"));
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
        ("rule pack tests ran", "Python rule pack tests"),
        (
            "js rule pack tests ran",
            "JavaScript/TypeScript rule pack tests",
        ),
        ("discover -s rules/c/tests", "C rule pack tests"),
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
    for (name, command, _) in pinned_tools() {
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
    for (name, command, version) in pinned_tools() {
        let repo = Repo::new();
        write_exe(
            &repo.dir.path().join("bin").join(&command),
            &tool_stub(&command, "0.0.0"),
        );
        let out = repo.run(&[], &[]);
        assert_eq!(out.status.code(), Some(1), "{name}: {}", stderr(&out));
        assert_eq!(steps(&out), ALL_STEPS[..1]);
        let err = stderr(&out);
        assert!(
            err.contains(&format!(
                "ci-tools: {name} is not version {version} (`{command} --version` printed: \
                 {command} 0.0.0); install it with: "
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
fn any_argument_prints_usage_and_exits_2() {
    for arg in ["--bogus", "--fast"] {
        let out = Repo::new().run(&[arg], &[]);
        assert_eq!(out.status.code(), Some(2), "{arg}");
        assert_eq!(stderr(&out), "usage: scripts/ci-local.sh\n");
    }
}
