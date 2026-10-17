use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A git repository with one commit, in a temp dir that also holds its `.workspaces` sibling.
pub struct Repo {
    _tmp: TempDir,
    pub path: PathBuf,
    /// The directory `ns` reads its user-level skills from, as `NS_CONFIG_DIR`.
    pub config: PathBuf,
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
        let config = tmp.path().join("config");
        let repo = Repo {
            _tmp: tmp,
            path,
            config,
        };
        repo.install_skills(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills"));
        repo
    }

    /// Runs `ns install` for the skills in `source`, as a user would.
    pub fn install_skills(&self, source: &Path) {
        assert_cmd::Command::cargo_bin("ns")
            .unwrap()
            .args(["install", "--source"])
            .arg(source)
            .env("NS_CONFIG_DIR", &self.config)
            .assert()
            .success();
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

    pub fn branches(&self) -> Vec<String> {
        git(&self.path, &["branch", "--format=%(refname:short)"])
            .lines()
            .map(String::from)
            .collect()
    }
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
