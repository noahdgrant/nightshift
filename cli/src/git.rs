//! Thin wrapper around the `git` binary.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::error::SfError;

/// Run git in `cwd` and return trimmed stdout. Fails with git's stderr on non-zero exit.
pub fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .context("failed to run git; is it installed and on PATH?")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(
            SfError::general(format!("git {} failed: {}", args.join(" "), stderr.trim())).into(),
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Run git and report only whether it succeeded.
pub fn ok(cwd: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The `git patch-id` of `sha`'s changes since its merge base with `base`. `None` when either
/// is not a commit here, or the diff is empty.
pub fn diff_id(cwd: &Path, base: &str, sha: &str) -> Option<String> {
    if sha.is_empty() {
        return None;
    }
    let mb = run(cwd, &["merge-base", base, sha]).ok()?;
    let diff = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["diff-tree", "-p", "--no-color", &mb, sha])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(&diff.stdout).ok()?;
    let out = child.wait_with_output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(String::from)
}

/// `a` and `b` carry the same change against `base`, as after a rebase.
pub fn same_diff(cwd: &Path, base: &str, a: &str, b: &str) -> bool {
    diff_id(cwd, base, a).is_some_and(|x| diff_id(cwd, base, b) == Some(x))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// Full ref, e.g. `refs/heads/ns/142-uart-timeout`. `None` when detached.
    pub branch: Option<String>,
    pub bare: bool,
}

/// Parse `git worktree list --porcelain`.
pub fn parse_worktree_list(text: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(e) = current.take() {
                entries.push(e);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(path),
                branch: None,
                bare: false,
            });
        } else if let Some(e) = current.as_mut() {
            if let Some(b) = line.strip_prefix("branch ") {
                e.branch = Some(b.to_string());
            } else if line == "bare" {
                e.bare = true;
            }
        }
    }
    if let Some(e) = current {
        entries.push(e);
    }
    entries
}

pub fn worktrees(cwd: &Path) -> Result<Vec<WorktreeEntry>> {
    let text = run(cwd, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_list(&text))
}

/// Context for a repository: the main worktree root and the common git dir.
pub struct Repo {
    pub root: PathBuf,
    pub common_dir: PathBuf,
}

impl Repo {
    /// Resolve the main worktree from any directory inside the repo (or a linked worktree).
    pub fn discover(start: &Path) -> Result<Repo> {
        if !ok(start, &["rev-parse", "--git-dir"]) {
            return Err(SfError::usage(
                format!("{} is not inside a git repository", start.display()),
                "run from inside a repo, or pass --repo:\n  ns worktree new 142-uart-timeout --repo ~/src/myrepo",
            )
            .into());
        }
        let common = run(
            start,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let list = worktrees(start)?;
        let main = list
            .first()
            .ok_or_else(|| SfError::general("git worktree list returned nothing"))?;
        if main.bare {
            return Err(SfError::general(
                "bare repositories are not supported; run from a repo with a main worktree",
            )
            .into());
        }
        Ok(Repo {
            root: main.path.clone(),
            common_dir: PathBuf::from(common),
        })
    }

    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string())
    }

    /// The repo's default branch: origin/HEAD (local branch if present), else current HEAD.
    pub fn default_base(&self) -> Result<String> {
        if let Ok(r) = run(
            &self.root,
            &[
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
        ) {
            if let Some(local) = r.strip_prefix("origin/") {
                if ok(
                    &self.root,
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        &format!("refs/heads/{local}"),
                    ],
                ) {
                    return Ok(local.to_string());
                }
            }
            return Ok(r);
        }
        if ok(&self.root, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
            return Ok("HEAD".to_string());
        }
        Err(
            SfError::general("repository has no commits yet; make a first commit, or pass --base")
                .hint("ns worktree new 142-uart-timeout --base main")
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain() {
        let text = "worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /w/x\nHEAD def\ndetached\n\nworktree /b\nbare\n";
        let e = parse_worktree_list(text);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(e[1].branch, None);
        assert!(e[2].bare);
    }

    fn commit(dir: &Path, file: &str, text: &str) -> String {
        std::fs::write(dir.join(file), text).unwrap();
        run(dir, &["add", file]).unwrap();
        run(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                file,
            ],
        )
        .unwrap();
        run(dir, &["rev-parse", "HEAD"]).unwrap()
    }

    #[test]
    fn a_rebase_keeps_the_diff_id_and_a_content_change_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        run(d, &["init", "-q", "-b", "main"]).unwrap();
        commit(d, "README", "hi\n");
        run(d, &["checkout", "-qb", "unit"]).unwrap();
        let reviewed = commit(d, "work.txt", "one\n");
        run(d, &["checkout", "-q", "main"]).unwrap();
        commit(d, "UPSTREAM", "x\n");
        run(d, &["checkout", "-q", "unit"]).unwrap();
        run(
            d,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "rebase",
                "-q",
                "main",
            ],
        )
        .unwrap();
        let rebased = run(d, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(reviewed, rebased);
        assert!(same_diff(d, "main", &reviewed, &rebased));
        let changed = commit(d, "work.txt", "two\n");
        assert!(!same_diff(d, "main", &reviewed, &changed));
        assert!(!same_diff(d, "main", "0000000", &rebased));
        assert!(!same_diff(d, "main", "", &rebased));
    }
}
