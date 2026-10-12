use std::fs;
use std::path::{Path, PathBuf};

use crate::git;

pub fn g(dir: &Path, args: &[&str]) -> String {
    let mut a = vec!["-c", "user.name=t", "-c", "user.email=t@t"];
    a.extend_from_slice(args);
    git::run(dir, &a).unwrap()
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
    use std::io::Write;
    use std::process::{Command, Stdio};
    let tmp = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name().unwrap().to_string_lossy()
    ));
    let mut sh = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(&tmp)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    sh.stdin
        .take()
        .unwrap()
        .write_all(contents.as_ref())
        .unwrap();
    let status = sh.wait().unwrap();
    assert!(
        status.success(),
        "sh could not write {}: {status}",
        tmp.display()
    );
    fs::rename(&tmp, path).unwrap();
}
