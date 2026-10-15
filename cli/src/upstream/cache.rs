//! The cache of upstream repos that `ns upstream` compares against: bare clones fetched
//! shallow and deepened only as far as a recorded sha needs.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use crate::error::SfError;
use crate::{git, lint};

/// Where `owner/repo` is fetched from: `<base>/owner/repo`. git's `url.<base>.insteadOf`
/// redirects it, to a mirror or, in tests, to a local bare repo.
pub(super) const GITHUB: &str = "https://github.com";

pub(super) const HEAD_REF: &str = "refs/ns/upstream-head";

pub(super) const FIRST_DEEPEN: u32 = 64;

const MAX_DEEPEN: u32 = 4096;

/// Bare clones of upstream repos under `<git-common-dir>/ns/upstream/<owner>/<repo>`, each
/// fetched at most once per invocation.
pub(super) struct Cache {
    root: PathBuf,
    base: String,
    pub(super) max_deepen: u32,
    heads: HashMap<String, Result<String, String>>,
    unreachable: HashMap<(String, String), String>,
}

impl Cache {
    pub(super) fn new(root: PathBuf, base: &str) -> Cache {
        Cache {
            root,
            base: base.to_string(),
            max_deepen: MAX_DEEPEN,
            heads: HashMap::new(),
            unreachable: HashMap::new(),
        }
    }

    pub(super) fn dir(&self, repo: &str) -> PathBuf {
        self.root.join(repo)
    }

    /// `git fetch` of upstream HEAD into [`HEAD_REF`], with `extra` options. The fetch's auto
    /// maintenance runs before it returns: detached, its repack rewrites the `shallow` file while
    /// the next `--deepen` fetch reads it, and that fetch dies with "shallow file has changed"
    /// (#710). Older git detaches `gc --auto`, newer git `maintenance run --auto`.
    fn fetch(&self, repo: &str, extra: &[&str]) -> Result<()> {
        let url = format!("{}/{repo}", self.base);
        let refspec = format!("+HEAD:{HEAD_REF}");
        let mut args = vec![
            "-c",
            "gc.autoDetach=false",
            "-c",
            "maintenance.autoDetach=false",
            "fetch",
            "--quiet",
        ];
        args.extend_from_slice(extra);
        args.extend([url.as_str(), refspec.as_str()]);
        let out = git::net(&self.dir(repo), &args).output()?;
        if !out.status.success() {
            return Err(SfError::general(format!(
                "git fetch {url} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
            .into());
        }
        Ok(())
    }

    /// Upstream HEAD's full sha. The first fetch into a new cache is shallow (`--depth=1`);
    /// later ones fetch only what is new.
    fn head(&mut self, repo: &str) -> Result<String> {
        if !self.heads.contains_key(repo) {
            let fetched = self.fetch_head(repo).map_err(|e| format!("{e:#}"));
            self.heads.insert(repo.to_string(), fetched);
        }
        self.heads[repo]
            .clone()
            .map_err(|e| SfError::general(e).into())
    }

    fn fetch_head(&self, repo: &str) -> Result<String> {
        let dir = self.dir(repo);
        if !dir.join("HEAD").is_file() {
            fs::create_dir_all(&dir)?;
            git::run(&dir, &["init", "--bare", "--quiet"])?;
        }
        let cached = git::ok(&dir, &["rev-parse", "--verify", "--quiet", HEAD_REF]);
        self.fetch(repo, if cached { &[] } else { &["--depth=1"] })?;
        git::run(&dir, &["rev-parse", HEAD_REF])
    }

    /// The full sha of `sha`, deepening the shallow cache until it holds the commit, the whole
    /// history, or `max_deepen` more commits. A failure is remembered per `(repo, sha)`.
    fn resolve(&mut self, repo: &str, sha: &str) -> Result<String> {
        let key = (repo.to_string(), sha.to_string());
        if let Some(e) = self.unreachable.get(&key) {
            return Err(SfError::general(e.clone()).into());
        }
        let resolved = self.deepen_to(repo, sha);
        if let Err(e) = &resolved {
            self.unreachable.insert(key, format!("{e:#}"));
        }
        resolved
    }

    fn deepen_to(&self, repo: &str, sha: &str) -> Result<String> {
        let dir = self.dir(repo);
        let commit = format!("{sha}^{{commit}}");
        let mut step = FIRST_DEEPEN;
        let mut deepened = 0;
        loop {
            if let Ok(full) = git::run(&dir, &["rev-parse", "--verify", "--quiet", &commit]) {
                return Ok(full);
            }
            if git::run(&dir, &["rev-parse", "--is-shallow-repository"])? != "true" {
                return Err(SfError::general(format!(
                    "recorded sha {sha} is not in {repo}'s history from HEAD"
                ))
                .into());
            }
            if deepened >= self.max_deepen {
                return Err(SfError::general(format!(
                    "recorded sha {sha} is not in {repo}'s history within {deepened} commits of HEAD"
                ))
                .into());
            }
            step = step.min(self.max_deepen - deepened);
            if step == 0 {
                return Err(SfError::general(format!(
                    "cannot deepen {repo}: step is zero (FIRST_DEEPEN {FIRST_DEEPEN})"
                ))
                .into());
            }
            self.fetch(repo, &[&format!("--deepen={step}")])?;
            deepened += step;
            step = step.saturating_mul(2);
        }
    }

    fn range(&mut self, up: &lint::Upstream) -> Result<(String, String)> {
        let head = self.head(up.repo)?;
        let from = self.resolve(up.repo, up.sha)?;
        if !self.in_tree(up.repo, &from, up.path)? && !self.in_tree(up.repo, &head, up.path)? {
            return Err(SfError::general(format!(
                "path {} not found at recorded sha or HEAD",
                up.path
            ))
            .into());
        }
        Ok((from, head))
    }

    fn in_tree(&self, repo: &str, rev: &str, path: &str) -> Result<bool> {
        let out = git::run(
            &self.dir(repo),
            &["ls-tree", "--name-only", rev, "--", path],
        )?;
        Ok(!out.is_empty())
    }

    pub(super) fn changed(&mut self, up: &lint::Upstream) -> Result<(String, bool)> {
        let (from, head) = self.range(up)?;
        let out = git::command()
            .arg("-C")
            .arg(self.dir(up.repo))
            .args([
                "diff",
                "--quiet",
                "--no-ext-diff",
                "--no-textconv",
                &from,
                &head,
                "--",
                up.path,
            ])
            .output()?;
        match out.status.code() {
            Some(0) => Ok((head, false)),
            Some(1) => Ok((head, true)),
            _ => Err(SfError::general(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
            .into()),
        }
    }

    pub(super) fn diff(&mut self, up: &lint::Upstream) -> Result<(String, String)> {
        let (from, head) = self.range(up)?;
        let dir = self.dir(up.repo);
        let diff = git::run(
            &dir,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                &from,
                &head,
                "--",
                up.path,
            ],
        )?;
        Ok((head, diff))
    }
}
