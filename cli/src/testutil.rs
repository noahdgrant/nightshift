use std::fs;
use std::path::{Path, PathBuf};

use crate::git;

/// Panic, naming each one set, when the test process holds a git location variable: a git
/// child would act on the repo it names instead of the test's own (#120).
pub fn assert_no_git_location() {
    let set: Vec<_> = git::LOCATION_VARS
        .iter()
        .filter(|v| std::env::var_os(v).is_some())
        .collect();
    assert!(
        set.is_empty(),
        "the test process has git location variables set: {set:?}; unset them before running the tests"
    );
}

/// `git` in `dir` as user `t`. Asserts [`assert_no_git_location`].
pub fn g(dir: &Path, args: &[&str]) -> String {
    assert_no_git_location();
    let mut a = vec!["-c", "user.name=t", "-c", "user.email=t@t"];
    a.extend_from_slice(args);
    git_any(dir, &a)
}

/// Run git in `dir` through [`git::run_net`] or [`git::run`], whichever `args` needs, and panic
/// on failure.
pub fn git_any(dir: &Path, args: &[&str]) -> String {
    if git::network_subcommand(args).is_some() {
        git::run_net(dir, args).unwrap()
    } else {
        git::run(dir, args).unwrap()
    }
}

pub fn write_commit(dir: &Path, file: &str, bytes: &[u8]) {
    fs::write(dir.join(file), bytes).unwrap();
    g(dir, &["add", file]);
    g(dir, &["commit", "-qm", file]);
}

pub fn commit_file(dir: &Path, file: &str, text: &str) -> String {
    write_commit(dir, file, text.as_bytes());
    g(dir, &["rev-parse", "HEAD"])
}

pub struct Rebased {
    pub _tmp: tempfile::TempDir,
    pub dir: PathBuf,
    pub reviewed: String,
    pub rebased: String,
}

/// A repo whose `unit` branch was reviewed at `reviewed`, then rebased onto a newer `main`.
pub fn rebased_unit() -> Rebased {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    g(&dir, &["init", "-q", "-b", "main"]);
    commit_file(&dir, "README", "hi\n");
    g(&dir, &["checkout", "-qb", "unit"]);
    let reviewed = commit_file(&dir, "work.txt", "one\n");
    g(&dir, &["checkout", "-q", "main"]);
    commit_file(&dir, "UPSTREAM", "x\n");
    g(&dir, &["checkout", "-q", "unit"]);
    g(&dir, &["rebase", "-q", "main"]);
    let rebased = g(&dir, &["rev-parse", "HEAD"]);
    Rebased {
        _tmp: tmp,
        dir,
        reviewed,
        rebased,
    }
}

/// Write `contents` to `path` as an executable (mode 755), so that it can be exec'd while other
/// test threads start processes.
///
/// A file this process writes itself can't be: a thread that forks while the write handle is
/// open gives its child a copy, held until that child execs, and an exec of the file meanwhile
/// fails with ETXTBSY ("Text file busy"). So `sh`, a process no test thread forks from, writes
/// it under a temp name, which is renamed into place. `tests/common` has the same helper for
/// the integration tests.
pub fn write_exe(path: &Path, contents: impl AsRef<[u8]>) {
    let tmp = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name().unwrap().to_string_lossy()
    ));
    let out = crate::proc::Proc::tool("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(&tmp)
        .stdin(contents.as_ref().to_vec())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sh could not write {}: {}",
        tmp.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    fs::rename(&tmp, path).unwrap();
}

/// A command that re-runs the test `test` (a full path such as `stop::tests::probe`) alone in a
/// process of its own, with `mark` set to `value`, so a probe that records a stop or replaces
/// itself cannot disturb the other tests. The probe returns at once unless `mark` is set.
#[allow(
    clippy::disallowed_methods,
    reason = "a probe test spawns, signals and kills this process itself, which proc does not offer"
)]
pub fn probe_command(test: &str, mark: &str, value: &str) -> std::process::Command {
    let mut probe = std::process::Command::new(std::env::current_exe().unwrap());
    probe
        .args(["--exact", test, "--test-threads=1", "--nocapture"])
        .env(mark, value);
    probe
}

/// Run `probe` and return its output, after asserting that it succeeded and ran the test.
pub fn run_probe(mut probe: std::process::Command) -> std::process::Output {
    let out = probe.output().unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains("1 passed"),
        "the probe did not run: {stdout}"
    );
    out
}

/// [`probe_command`] run through [`run_probe`].
pub fn probe(test: &str, mark: &str, value: &str) -> std::process::Output {
    run_probe(probe_command(test, mark, value))
}

/// Put an executable `name` running `body` (a shell script's text) first on this process's
/// PATH; the returned directory holds it.
pub fn fake_on_path(name: &str, body: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_exe(&dir.path().join(name), format!("#!/bin/sh\n{body}\n"));
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![dir.path().to_path_buf()];
    dirs.extend(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
    dir
}

/// A temp dir that gives its owner write on every directory in it before it is deleted, so a
/// test that leaves a read-only directory, or panics before putting it back, leaks nothing.
pub struct TempTree(tempfile::TempDir);

impl TempTree {
    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        crate::tree::make_removable(self.0.path());
    }
}

pub fn temp_tree() -> TempTree {
    TempTree(tempfile::tempdir().unwrap())
}

pub fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Whether this process reads past a directory's mode (it runs as root).
pub fn reads_any_mode(dir: &Path) -> bool {
    let probe = dir.join("probe");
    fs::create_dir(&probe).unwrap();
    chmod(&probe, 0);
    let any = fs::read_dir(&probe).is_ok();
    chmod(&probe, 0o750);
    fs::remove_dir(&probe).unwrap();
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARK: &str = "NS_TEST_GIT_LOCATION_PROBE";

    #[test]
    fn g_probe() {
        if std::env::var_os(MARK).is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        g(tmp.path(), &["init", "-q"]);
    }

    #[test]
    fn g_panics_naming_each_git_location_variable_the_test_process_has() {
        let mut probe = probe_command("testutil::tests::g_probe", MARK, "1");
        probe.env("GIT_DIR", "/nonexistent").env("GIT_PREFIX", "");
        let out = probe.output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "{text}");
        assert!(
            text.contains(r#"git location variables set: ["GIT_DIR", "GIT_PREFIX"]"#),
            "{text}"
        );
    }
}
