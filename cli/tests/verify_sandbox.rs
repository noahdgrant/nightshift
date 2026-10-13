//! verify-nightshift's sandbox script (`.agents/skills/verify-nightshift/scripts/sandbox.sh`):
//! every recipe runs inside it, so it alone keeps a recipe off the real forge, `HOME` and model.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap()
}

fn script() -> PathBuf {
    repo().join(".agents/skills/verify-nightshift/scripts/sandbox.sh")
}

/// The target directory `ns` was built into, as `CARGO_TARGET_DIR` names it.
fn target_dir() -> PathBuf {
    common::ns_path()
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn entries(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// Every variable the sandbox unsets: the caller's forge, model and git settings.
const UNSET: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GH_HOST",
    "GH_CONFIG_DIR",
    "GH_REPO",
    "GITLAB_TOKEN",
    "GITLAB_HOST",
    "GLAB_TOKEN",
    "GLAB_CONFIG_DIR",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "XDG_CONFIG_HOME",
    "SSH_AUTH_SOCK",
    "SSH_ASKPASS",
    "GIT_ASKPASS",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_KEY_0",
    "GIT_CONFIG_KEY_7",
    "GIT_CONFIG_VALUE_0",
    "GIT_CONFIG_VALUE_7",
    "NS_UNIT",
    "NS_WORKTREE",
    "NS_PHASE_SETTINGS",
    "NS_PHASE",
    "NS_RUN_PID",
    "NS_WATCH_PID",
    "NS_ATTEMPT",
    "NS_CPUS",
    "NS_FORGE_EXPORTED",
    "NS_FORGE_CHECKED",
];

/// Runs `program args` with a fresh `TMPDIR` and `OUT`, `CARGO_TARGET_DIR` at the crate's build,
/// and every variable in `UNSET` set to a decoy.
fn run(tmp: &Path, program: &Path, args: &[&str]) -> Output {
    run_with_path(tmp, program, args, None)
}

/// As `run`, with `path_tail` put first on the caller's `PATH`.
fn run_with_path(tmp: &Path, program: &Path, args: &[&str], path_tail: Option<&Path>) -> Output {
    let mut c = common::command(program);
    c.args(args)
        .env("TMPDIR", tmp.join("t"))
        .env("OUT", tmp.join("out"))
        .env("CARGO_TARGET_DIR", target_dir())
        .env_remove("V");
    if let Some(tail) = path_tail {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = std::env::split_paths(&path).collect::<Vec<_>>();
        dirs.insert(0, tail.to_path_buf());
        c.env("PATH", std::env::join_paths(dirs).unwrap());
    }
    for v in UNSET {
        c.env(v, tmp.join("leaked"));
    }
    c.output()
}

struct Sandbox {
    tmp: tempfile::TempDir,
    v: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("t")).unwrap();
        let out = run(tmp.path(), &script(), &["--new"]);
        assert!(out.status.success(), "--new failed: {}", stderr(&out));
        let v = PathBuf::from(stdout(&out));
        assert!(v.starts_with(tmp.path().join("t")), "{v:?}");
        Sandbox { tmp, v }
    }

    fn run(&self, args: &[&str]) -> Output {
        run(self.tmp.path(), &self.v.join("sandbox"), args)
    }

    fn run_with_path(&self, args: &[&str], path_tail: &Path) -> Output {
        run_with_path(
            self.tmp.path(),
            &self.v.join("sandbox"),
            args,
            Some(path_tail),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        stdout(&out)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.v);
    }
}

#[test]
fn no_command_exits_2_with_usage_and_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("t")).unwrap();
    let out = run(tmp.path(), &script(), &[]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("usage:"), "{}", stderr(&out));
    assert!(entries(&tmp.path().join("t")).is_empty());
    assert!(!tmp.path().join("out").exists());
}

#[test]
fn a_command_outside_a_sandbox_exits_2_and_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("t")).unwrap();
    let out = run(tmp.path(), &script(), &["cap", "x", "true"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--new"), "{}", stderr(&out));
    assert!(entries(&tmp.path().join("t")).is_empty());
}

#[test]
fn a_missing_ns_binary_is_named_and_nothing_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("t")).unwrap();
    let out = common::command(script())
        .arg("--new")
        .env("TMPDIR", tmp.path().join("t"))
        .env("OUT", tmp.path().join("out"))
        .env("CARGO_TARGET_DIR", tmp.path().join("no-target"))
        .output();
    assert!(!out.status.success());
    let want = tmp.path().join("no-target/debug/ns");
    assert!(
        stderr(&out).contains(&want.display().to_string()),
        "{}",
        stderr(&out)
    );
    assert!(entries(&tmp.path().join("t")).is_empty());
    assert!(!tmp.path().join("out").exists());
}

#[test]
fn a_missing_fake_is_named_and_nothing_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("t")).unwrap();
    let copy = tmp
        .path()
        .join("src/.agents/skills/verify-nightshift/scripts/sandbox.sh");
    common::copy_exe(&script(), &copy);
    let out = run(tmp.path(), &copy, &["--new"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cli/tests/fixtures/factory/claude"),
        "{}",
        stderr(&out)
    );
    assert!(entries(&tmp.path().join("t")).is_empty());
    assert!(!tmp.path().join("out").exists());
}

#[test]
fn ssh_fails_inside_the_sandbox() {
    let s = Sandbox::new();
    for args in [
        vec!["ssh", "-T", "git@github.com"],
        vec!["git", "ls-remote", "git@github.com:o/r.git"],
        vec!["git", "ls-remote", "ssh://git@github.com/o/r.git"],
    ] {
        let out = s.run(&args);
        assert!(!out.status.success(), "{args:?} succeeded: {out:?}");
    }
    let out = s.run(&["ssh", "-T", "git@github.com"]);
    assert!(
        stderr(&out).contains("sandbox: ssh is cut off"),
        "{}",
        stderr(&out)
    );
    assert_eq!(s.ok(&["sh", "-c", "echo $GIT_SSH_COMMAND"]), "false");
    let out = s.run(&["git", "ls-remote", "ssh://git@github.com/o/r.git"]);
    assert!(stderr(&out).contains("fatal:"), "{}", stderr(&out));
}

#[test]
fn a_real_gh_on_the_callers_path_finds_no_account_inside_the_sandbox() {
    let s = Sandbox::new();
    let bin = s.tmp.path().join("real-bin");
    fs::create_dir(&bin).unwrap();
    let stub = "#!/bin/sh\nif [ -n \"${GH_TOKEN-}${GITHUB_TOKEN-}${GH_CONFIG_DIR-}\" ] || [ -e \"$HOME/.config/gh/hosts.yml\" ]; then\n  echo account\n  exit 0\nfi\necho 'stub gh: no account' >&2\nexit 1\n";
    common::write_exe(&bin.join("gh"), stub);
    let out = s.run_with_path(
        &[
            "sh",
            "-c",
            r#"PATH=${PATH#"$V/bin:"}; test "$(command -v gh)" = "$1"; gh auth token"#,
            "_",
            &bin.join("gh").display().to_string(),
        ],
        &bin,
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr(&out).contains("stub gh: no account"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn the_fakes_and_the_built_ns_come_first_on_path() {
    let s = Sandbox::new();
    for b in ["claude", "gh", "glab", "ssh", "ns"] {
        let found = s.ok(&["sh", "-c", "command -v \"$1\"", "_", b]);
        assert_eq!(found, s.v.join("bin").join(b).display().to_string());
    }
    let version = s.ok(&["ns", "--version"]);
    assert!(version.starts_with("ns "), "{version}");
}

#[test]
fn the_callers_tokens_and_git_locations_are_unset() {
    let s = Sandbox::new();
    let seen = s.ok(&[
        "sh",
        "-c",
        "for v in $1; do eval \"printf '%s=%s\\n' $v \\\"\\${$v-unset}\\\"\"; done",
        "_",
        &UNSET.join(" "),
    ]);
    for v in UNSET {
        assert!(
            seen.lines().any(|l| l == format!("{v}=unset")),
            "{v}: {seen}"
        );
    }
    let tz = s.ok(&["sh", "-c", "echo $TZ $GIT_CONFIG_NOSYSTEM $GIT_SSH_COMMAND"]);
    assert_eq!(tz, "UTC 1 false");
    let env = s.ok(&["sh", "-c", "echo $HOME $GIT_SSH_COMMAND $NS_CONFIG $TMPDIR"]);
    let v = s.v.display();
    assert_eq!(env, format!("{v}/home false {v}/config.toml {v}/tmp"));
    let made = s.ok(&["mktemp", "-d"]);
    assert!(made.starts_with(&format!("{v}/tmp/")), "{made}");
}

#[test]
fn a_command_runs_in_the_repo_and_src_runs_in_the_checkout() {
    let s = Sandbox::new();
    assert_eq!(s.ok(&["pwd"]), s.v.join("repo").display().to_string());
    assert_eq!(s.ok(&["--src", "pwd"]), repo().display().to_string());
    let log = s.ok(&["git", "log", "--format=%s"]);
    assert_eq!(log, "init");
    let origin = s.ok(&["git", "remote", "get-url", "origin"]);
    assert_eq!(origin, s.v.join("remote.git").display().to_string());
}

#[test]
fn new_installs_the_ns_skills_into_the_sandbox_home() {
    let s = Sandbox::new();
    let link = s.v.join("home/.claude/skills/ns-tdd");
    assert_eq!(
        fs::read_link(&link).unwrap(),
        repo().join("skills/ns-tdd"),
        "{link:?}"
    );
}

#[test]
fn cap_saves_the_command_output_and_exit_code() {
    let s = Sandbox::new();
    let printed = s.ok(&["cap", "x", "sh", "-c", "echo out; echo err >&2; exit 3"]);
    assert_eq!(printed, "3");
    let out = s.tmp.path().join("out");
    let read = |ext: &str| fs::read_to_string(out.join(format!("x.{ext}"))).unwrap();
    assert_eq!(read("out"), "out\n");
    assert_eq!(read("err"), "err\n");
    assert_eq!(read("exit"), "3\n");
    assert!(read("cmd").starts_with("$ sh -c"), "{}", read("cmd"));
}

#[test]
fn a_command_keeps_its_exit_code() {
    let s = Sandbox::new();
    let out = s.run(&["sh", "-c", "exit 4"]);
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn two_sandboxes_get_their_own_directories() {
    let a = Sandbox::new();
    let b = Sandbox::new();
    assert_ne!(a.v, b.v);
    assert_eq!(a.ok(&["sh", "-c", "echo $V"]), a.v.display().to_string());
}

#[test]
fn a_command_line_missing_its_command_exits_2_with_usage() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("t")).unwrap();
    let out = run(tmp.path(), &script(), &["--new", "extra"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(entries(&tmp.path().join("t")).is_empty());
    let s = Sandbox::new();
    for args in [vec!["--src"], vec!["cap", "x"], vec!["--src", "cap"]] {
        let out = s.run(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains("usage:"), "{}", stderr(&out));
    }
}

#[test]
fn a_relative_cargo_target_dir_is_read_from_the_checkout() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("t")).unwrap();
    let out = common::command(script())
        .arg("--new")
        .env("TMPDIR", tmp.path().join("t"))
        .env("OUT", tmp.path().join("out"))
        .env("CARGO_TARGET_DIR", "no-such-target")
        .output();
    assert!(!out.status.success());
    let want = repo().join("no-such-target/debug/ns");
    assert!(
        stderr(&out).contains(&want.display().to_string()),
        "{}",
        stderr(&out)
    );
}

/// A copy of the script in a checkout of its own under `tmp/src`, with the fakes and one skill,
/// so `--new` can write its default evidence directory there.
fn checkout_copy(tmp: &Path) -> PathBuf {
    let src = tmp.join("src");
    let copy = src.join(".agents/skills/verify-nightshift/scripts/sandbox.sh");
    common::copy_exe(&script(), &copy);
    for f in ["claude", "gh", "glab"] {
        let rel = format!("cli/tests/fixtures/factory/{f}");
        common::copy_exe(&repo().join(&rel), &src.join(rel));
    }
    fs::create_dir_all(src.join("skills/ns-x")).unwrap();
    fs::write(
        src.join("skills/ns-x/SKILL.md"),
        "---\nname: ns-x\ndescription: x\n---\n",
    )
    .unwrap();
    copy
}

#[test]
fn evidence_defaults_to_a_run_directory_in_the_checkout() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("t")).unwrap();
    let copy = checkout_copy(tmp.path());
    let out = common::command(&copy)
        .arg("--new")
        .env("TMPDIR", tmp.path().join("t"))
        .env("CARGO_TARGET_DIR", target_dir())
        .env_remove("OUT")
        .output();
    assert!(out.status.success(), "{}", stderr(&out));
    let v = PathBuf::from(stdout(&out));
    let evidence = common::command(v.join("sandbox"))
        .args(["sh", "-c", "echo $OUT"])
        .output();
    let evidence = PathBuf::from(stdout(&evidence));
    assert_eq!(
        evidence.parent().unwrap(),
        tmp.path().join("src/.ns/verify")
    );
    assert!(evidence.is_dir());
    let suffix = v.extension().unwrap().to_string_lossy().into_owned();
    assert!(
        evidence
            .display()
            .to_string()
            .ends_with(&format!("Z-{suffix}")),
        "{evidence:?} {v:?}"
    );
}

#[test]
fn a_failed_setup_removes_the_half_made_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("t")).unwrap();
    let copy = checkout_copy(tmp.path());
    fs::remove_dir_all(tmp.path().join("src/skills")).unwrap();
    let out = run(tmp.path(), &copy, &["--new"]);
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(entries(&tmp.path().join("t")).is_empty());
}

#[test]
fn a_failed_early_setup_step_fails_the_setup_and_removes_the_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("t")).unwrap();
    let decoy = tmp.path().join("decoy");
    fs::create_dir(&decoy).unwrap();
    let real = String::from_utf8(
        common::command("sh")
            .args(["-c", "command -v git"])
            .output()
            .stdout,
    )
    .unwrap();
    common::write_exe(
        &decoy.join("git"),
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = push ] && {{ echo 'decoy: push refused' >&2; exit 1; }}; done\nexec {} \"$@\"\n",
            real.trim()
        ),
    );
    let out = run_with_path(tmp.path(), &script(), &["--new"], Some(&decoy));
    assert!(!out.status.success(), "setup passed: {}", stdout(&out));
    assert!(stderr(&out).contains("push refused"), "{}", stderr(&out));
    assert!(entries(&tmp.path().join("t")).is_empty());
}
