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
    if sha.is_empty() || sha.starts_with('-') || base.starts_with('-') {
        return None;
    }
    let mb = run(cwd, &["merge-base", base, sha]).ok()?;
    let diff = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["diff-tree", "-p", "--binary", "--no-color", &mb, sha])
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

    /// The repo's default branch: origin/HEAD (local branch if present), else the checked-out
    /// branch, else the detached HEAD's sha. Never the literal `HEAD`, which moves with the worktree.
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
        if let Ok(sha) = run(&self.root, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
            return Ok(
                run(&self.root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or(sha),
            );
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
    use crate::testutil::{commit_file, g, rebased_unit, write_commit};

    #[test]
    fn parses_porcelain() {
        let text = "worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /w/x\nHEAD def\ndetached\n\nworktree /b\nbare\n";
        let e = parse_worktree_list(text);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(e[1].branch, None);
        assert!(e[2].bare);
    }

    #[test]
    fn a_rebase_keeps_the_diff_id_and_a_content_change_does_not() {
        let r = rebased_unit();
        let d = &r.dir;
        assert_ne!(r.reviewed, r.rebased);
        assert!(same_diff(d, "main", &r.reviewed, &r.rebased));
        assert!(!same_diff(d, "main", &r.reviewed, "0000000"));
        let changed = commit_file(d, "work.txt", "two\n");
        assert!(!same_diff(d, "main", &r.reviewed, &changed));
        assert!(!same_diff(d, "main", "0000000", &r.rebased));
        assert!(!same_diff(d, "main", "", &r.rebased));
    }

    #[test]
    fn an_empty_diff_is_never_the_same_diff() {
        let r = rebased_unit();
        assert!(!same_diff(&r.dir, "main", "main", "main"));
        assert!(same_diff(&r.dir, "main", &r.rebased, &r.rebased));
    }

    #[test]
    fn option_looking_shas_and_bases_have_no_diff_id() {
        let r = rebased_unit();
        assert_eq!(diff_id(&r.dir, "main", "--help"), None);
        assert_eq!(diff_id(&r.dir, "--all", &r.rebased), None);
        assert_eq!(diff_id(&r.dir, "main", "-p"), None);
    }

    #[test]
    fn a_binary_change_after_review_is_not_the_same_diff_but_a_rebase_is() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        g(d, &["init", "-q", "-b", "main"]);
        commit_file(d, "README", "hi\n");
        g(d, &["checkout", "-qb", "unit"]);
        write_commit(d, "blob.bin", &[0, 1, 2, 0, 3]);
        let reviewed = g(d, &["rev-parse", "HEAD"]);
        write_commit(d, "blob.bin", &[0, 9, 9, 0, 9]);
        let changed = g(d, &["rev-parse", "HEAD"]);
        g(d, &["reset", "-q", "--hard", &reviewed]);
        g(d, &["checkout", "-q", "main"]);
        commit_file(d, "UPSTREAM", "x\n");
        g(d, &["checkout", "-q", "unit"]);
        g(d, &["rebase", "-q", "main"]);
        let rebased = g(d, &["rev-parse", "HEAD"]);
        assert!(same_diff(d, "main", &reviewed, &rebased));
        assert!(!same_diff(d, "main", &changed, &reviewed));
    }

    #[test]
    fn the_default_base_is_never_the_literal_head() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        g(&dir, &["init", "-q", "-b", "dev"]);
        assert!(Repo::discover(&dir).unwrap().default_base().is_err());
        let sha = commit_file(&dir, "README", "hi\n");
        let repo = Repo::discover(&dir).unwrap();
        assert_eq!(repo.default_base().unwrap(), "dev");
        g(&dir, &["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
        g(
            &dir,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
        );
        assert_eq!(repo.default_base().unwrap(), "origin/trunk");
        g(
            &dir,
            &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
        );
        g(&dir, &["checkout", "-q", "--detach"]);
        assert_eq!(repo.default_base().unwrap(), sha);
    }
}
