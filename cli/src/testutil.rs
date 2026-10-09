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
