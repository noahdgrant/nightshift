use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// Every label `ns` owns with the description this version gives it, as literals.
#[allow(dead_code)]
pub const LABEL_DESCRIPTIONS: [(&str, &str); 25] = [
    ("status:needs-triage", "Nobody has looked at this yet"),
    (
        "status:needs-define",
        "Too vague to build; waits for a spec",
    ),
    ("status:needs-repro", "A bug nobody has reproduced yet"),
    ("status:needs-info", "Waits for an answer from the author"),
    ("status:ready-for-agent", "Ready for the agent to build"),
    ("status:in-progress", "ns is working on this"),
    (
        "status:stuck",
        "A state failed every attempt or could not be proven; a person looks first",
    ),
    (
        "status:waiting-for-human",
        "Waiting for a person: a merge, hardware, credentials or a decision",
    ),
    ("status:wontfix", "Will not be done"),
    ("type:feat", "A new capability"),
    ("type:fix", "A bug fix"),
    ("type:docs", "Documentation only"),
    ("type:refactor", "A change that neither fixes nor adds"),
    ("type:test", "Tests only"),
    ("type:perf", "A faster way to do the same"),
    ("type:ci", "Continuous integration"),
    ("type:build", "The build or its dependencies"),
    ("type:chore", "Upkeep that is none of the above"),
    ("type:style", "Formatting only"),
    ("type:revert", "Undoes an earlier change"),
    ("priority:critical", "Drop everything"),
    ("priority:high", "Before new features"),
    ("priority:medium", "Next in line"),
    ("priority:low", "When there is room"),
    (
        "needs-human-merge",
        "A person merges this: it touches files the repository wants reviewed",
    ),
];

/// The JSON `gh label list` prints for `labels`, each with this version's description.
#[allow(dead_code)]
pub fn labels_json(labels: &[&str]) -> String {
    let items: Vec<String> = labels
        .iter()
        .map(|n| {
            let d = LABEL_DESCRIPTIONS
                .iter()
                .find(|(name, _)| name == n)
                .map_or("", |(_, d)| *d);
            format!(r#"{{"name":"{n}","description":"{d}"}}"#)
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// A git repository with one commit, in a temp dir that also holds its `.workspaces` sibling.
pub struct Repo {
    _tmp: TempDir,
    pub path: PathBuf,
}

impl Repo {
    pub fn new() -> Repo {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo");
        std::fs::create_dir(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        git(&path, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let origin = tmp.path().join("origin.git");
        git(
            tmp.path(),
            &["init", "-q", "--bare", "-b", "main", "origin.git"],
        );
        git(
            &path,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        Repo { _tmp: tmp, path }
    }

    pub fn workspace(&self, unit: &str) -> PathBuf {
        self.path
            .parent()
            .unwrap()
            .join("repo.workspaces")
            .join(unit)
    }

    /// Commits the command that runs the repository's checks. Units started after this use it.
    pub fn set_check(&self, command: &str) {
        std::fs::create_dir_all(self.path.join(".nightshift")).unwrap();
        std::fs::write(self.path.join(".nightshift/check"), command).unwrap();
        git(&self.path, &["add", ".nightshift/check"]);
        git(&self.path, &["commit", "-q", "-m", "set the checks"]);
    }

    /// Commits the coding standards `ns sync` keeps in a repository, as a pinned one has them, so
    /// that a sync leaves the working tree as it was.
    pub fn standards_commit(&self) {
        let shipped =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../standards/ns-coding-standards.md");
        std::fs::create_dir_all(self.path.join(".nightshift")).unwrap();
        std::fs::copy(shipped, self.path.join(".nightshift/coding-standards.md")).unwrap();
        git(&self.path, &["add", ".nightshift/coding-standards.md"]);
        git(&self.path, &["commit", "-q", "-m", "commit the standards"]);
    }

    pub fn branches(&self) -> Vec<String> {
        git(&self.path, &["branch", "--format=%(refname:short)"])
            .lines()
            .map(String::from)
            .collect()
    }
}

/// Puts a script named `name` in `bin`.
pub fn script(bin: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
