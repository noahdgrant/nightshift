//! The forge `ns watch` reads its queue from and keeps issue state on, chosen by `[queue] source`
//! (docs/FACTORY.md). Credentials are `crate::forge`'s job, not this module's.
//!
//! Every forge call outside the credential checks goes through [`Forge`]: `gh` and `glab` run only
//! here, so a new caller can't reach one forge's CLI and break on the other.

#[cfg(test)]
pub(crate) mod fake;
mod github;
mod gitlab;
mod main_checks;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use self::github::GitHub;
use super::ci::HeadChecks;
use super::{closed_by, Issue};
use crate::config::{Config, ForgeKind};
use crate::error::SfError;
use crate::factory::{self, Queue as QueueConfig};
use crate::git;

#[derive(Clone, Copy)]
pub enum PrState {
    Open,
    Merged,
}

/// Where a PR stands, whichever forge it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrStatus {
    Open,
    Merged,
    /// Closed without merging.
    Closed,
}

impl PrStatus {
    pub fn name(self) -> &'static str {
        match self {
            PrStatus::Open => "OPEN",
            PrStatus::Merged => "MERGED",
            PrStatus::Closed => "CLOSED",
        }
    }
}

/// A PR as `ns run`'s guards and `ns clean` read it. `head_sha` is empty when the forge names
/// none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pr {
    pub status: PrStatus,
    pub head_sha: String,
    pub head_branch: String,
    pub body: String,
}

/// A PR the forge links to a commit: its number, its head branch, and whether it merged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitPr {
    pub number: u64,
    pub head: String,
    pub merged: bool,
}

/// An issue's title and the URL a human opens it at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueLink {
    pub title: String,
    pub url: String,
}

/// Where an issue stands. A closed issue's reason is the forge's; one that names none is completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Completed,
    /// Closed for any other reason: not planned, a duplicate.
    NotPlanned,
}

/// An issue in any state, and when it was opened in unix seconds.
#[derive(Clone, Debug)]
pub struct Filed {
    pub issue: Issue,
    pub opened: i64,
    pub state: IssueState,
}

/// A merged PR, its head branch (empty for a fork's), when it merged, and its added plus deleted
/// lines when the forge's list gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedPr {
    pub number: u64,
    pub head: String,
    pub merged: i64,
    pub changed: Option<u64>,
}

/// The forge's merged PRs, and whether its cap cut the list short.
#[derive(Debug, Default)]
pub struct MergedPrs {
    pub prs: Vec<MergedPr>,
    pub truncated: bool,
}

/// Issue number -> every PR that closes it, in the forge's order, each with its head branch,
/// empty for a fork's.
pub type ClosingPrs = BTreeMap<u64, Vec<(u64, String)>>;

/// Adds PR `pr` with head `head` under each issue its `body` closes, once per issue.
fn add_closing(by_issue: &mut ClosingPrs, body: &str, pr: u64, head: &str) {
    for n in closed_by(body) {
        let prs = by_issue.entry(n).or_default();
        if !prs.iter().any(|(p, _)| *p == pr) {
            prs.push((pr, head.to_string()));
        }
    }
}

/// What the merge step reads of a PR. Each read asks the forge for only its fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrRead {
    State,
    Head,
    StateAndHead,
    MergeState,
    HeadAndMergeState,
}

/// A PR as the merge step reads it, in GitHub's terms: `state` is `OPEN`, `MERGED` or `CLOSED`,
/// and `merge_state` is GitHub's `mergeStateStatus` (`CLEAN`, `BEHIND`, `DIRTY`, ...). A field the
/// [`PrRead`] didn't ask for, or the forge named none for, is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrView {
    pub state: Option<String>,
    pub head: Option<String>,
    pub merge_state: Option<String>,
}

/// One CI check on a PR, in GitHub's terms (`gh pr checks`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrCheck {
    pub name: Option<String>,
    pub state: Option<String>,
    pub bucket: Option<String>,
    pub link: Option<String>,
}

/// A PR as `ns score` grades it: where it stands, its merge commit when the forge names one, and
/// how many reviews it got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrOutcome {
    pub status: PrStatus,
    pub merge_commit: Option<String>,
    pub reviews: usize,
}

/// The line every body nightshift posts to a forge opens with (skills/ns-contract, Disclosure).
pub const DISCLOSURE: &str = "🤖 *Posted by nightshift, an AI agent.*";

/// A body for the forge: [`DISCLOSURE`], a blank line, then the text. The forge's post methods
/// take only this, so nothing reaches the forge without the line.
pub struct Post(String);

impl Post {
    pub fn new(text: &str) -> Self {
        Post(format!("{DISCLOSURE}\n\n{text}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub trait Forge {
    /// Open issues, never PRs; `label` narrows the list to issues carrying it.
    fn open_issues(&self, label: Option<&str>) -> Result<Vec<Issue>>;
    /// Issues carrying `label` in any state, never PRs.
    fn labelled_issues(&self, label: &str) -> Result<Vec<Filed>>;
    fn closing_prs(&self, state: PrState) -> Result<ClosingPrs>;
    /// The PRs merged at or after `since` (all of them when `None`), up to the forge's cap, in
    /// any order. The list is `truncated` when the cap may have cut PRs from it. Forges list by
    /// creation time, so the cut PRs are the oldest created: a PR created long before it merged
    /// can be missing from a week the oldest listed merge precedes.
    fn merged_prs(&self, since: Option<i64>) -> Result<MergedPrs>;
    fn is_closed(&self, n: u64) -> Result<bool>;
    fn issue_link(&self, n: u64) -> Result<IssueLink>;
    /// PR `n` (a GitLab merge request's iid).
    fn pr(&self, n: u64) -> Result<Pr>;
    /// The PRs that hold commit `sha`, in any state.
    fn commit_prs(&self, sha: &str) -> Result<Vec<CommitPr>>;
    fn state(&self, n: u64) -> Result<IssueState>;
    /// The forge's sub-issues of `n`, in any state.
    fn sub_issues(&self, n: u64) -> Result<Vec<u64>>;
    /// The bodies of `n`'s team-authored comments, oldest first.
    fn team_comments(&self, n: u64) -> Result<Vec<String>>;
    /// Issues in any state that a search finds saying `Part of #n`: candidates, whose body the
    /// caller checks.
    fn part_of(&self, n: u64) -> Result<Vec<Issue>>;
    /// Create issue label `label` unless the repo has it.
    fn ensure_label(&self, label: &str, description: &str) -> Result<()>;
    fn labels(&self, n: u64) -> Result<Vec<String>>;
    fn edit_labels(&self, n: u64, remove: &[String], add: Option<&str>) -> Result<()>;
    fn comment(&self, n: u64, body: &Post) -> Result<()>;
    fn close(&self, n: u64) -> Result<()>;
    /// Add `label` to PR `pr`, creating it in the repo first if it is missing.
    fn add_pr_label(&self, pr: u64, label: &str) -> Result<()>;
    /// Remove `label` from PR `pr` if the PR carries it.
    fn remove_pr_label(&self, pr: u64, label: &str) -> Result<()>;
    fn comment_pr(&self, pr: u64, body: &Post) -> Result<()>;
    /// The default branch's head and the checks on it.
    fn main_checks(&self) -> Result<HeadChecks>;
    /// File an issue carrying `labels`, creating any the repo lacks; its number.
    fn create_issue(&self, title: &str, body: &Post, labels: &[&str]) -> Result<u64>;
    /// The log of a failed check, at the [`Check::log`] the forge gave it.
    fn check_log(&self, log: &str) -> Result<String>;
    /// The newest commit on the default branch, other than `red`, that a CI run passed, if the
    /// forge knows one. A passing run on `red` itself (another workflow's) says nothing of when
    /// main broke.
    fn last_green(&self, red: &str) -> Result<Option<String>>;
    /// The remote ref PR `n`'s head lives under, to fetch a head that isn't here.
    fn pr_ref(&self, n: u64) -> String;
    /// PR `n` as `ns score` grades it.
    fn pr_outcome(&self, n: u64) -> Result<PrOutcome>;

    // The merge step's calls (docs/factory/merge.md). GitLab has none of them yet (#323).

    fn view_pr(&self, n: u64, read: PrRead) -> Result<PrView>;
    /// Bring the default branch into PR `n`'s branch on the forge.
    fn update_branch(&self, n: u64) -> Result<()>;
    /// How many check runs, then how many commit statuses, commit `sha` has.
    fn check_counts(&self, sha: &str) -> [Result<u64>; 2];
    /// Wait for PR `n`'s checks to end, at most `limit`; whether the wait was cut off.
    fn watch_checks(&self, n: u64, limit: Duration) -> Result<bool>;
    fn pr_checks(&self, n: u64) -> Result<Vec<PrCheck>>;
    /// The failed jobs' log of CI run `id`, as [`PrCheck::link`] names it.
    fn run_log(&self, id: &str) -> Result<String>;
    /// The paths PR `n` changes.
    fn changed_files(&self, n: u64) -> Result<Vec<String>>;
    /// Squash-merge PR `n` if its head is still `head`. Its branch stays.
    fn squash_merge(&self, n: u64, head: &str) -> Result<()>;
    fn delete_branch(&self, branch: &str) -> Result<()>;
}

const HINT: &str = "set the forge in .nightshift/nightshift.toml:\n  [queue]\n  source = \"github\"\n\nsee docs/FACTORY.md";

/// The forge `[queue] source` names or, unset, the one `origin`'s host is on. `GITLAB_HOST`, set
/// from `[forge.gitlab] host` by then, names one more GitLab host.
pub fn open(q: &QueueConfig, root: &Path) -> Result<Box<dyn Forge>, SfError> {
    let origin = git::run(root, &["remote", "get-url", "origin"]).ok();
    let gitlab_host = gitlab_host(None);
    let root = root.to_path_buf();
    Ok(
        match check(
            q.source.as_deref(),
            origin.as_deref(),
            gitlab_host.as_deref(),
        )? {
            ForgeKind::Gitlab => Box::new(gitlab::GitLab { root }),
            ForgeKind::Github => Box::new(GitHub { root }),
        },
    )
}

/// The forge `source` and `origin` resolve to, if `ns watch` can use it. Only a set `source` can
/// name one it can't.
fn check(
    source: Option<&str>,
    origin: Option<&str>,
    gitlab_host: Option<&str>,
) -> Result<ForgeKind, SfError> {
    match source {
        None => Ok(inferred(origin, gitlab_host)),
        Some(s) => ForgeKind::parse(s).ok_or_else(|| {
            let problem = factory::source_problem(s).unwrap_or_default();
            SfError::usage(problem, HINT)
        }),
    }
}

/// The web address of PR `n` in the repo at `web`: a pull request's on GitHub, a merge request's
/// on GitLab.
pub fn pr_url(kind: ForgeKind, web: &str, n: u64) -> String {
    match kind {
        ForgeKind::Gitlab => format!("{web}/-/merge_requests/{n}"),
        ForgeKind::Github => format!("{web}/pull/{n}"),
    }
}

/// The GitLab host `GITLAB_HOST` names, else `[forge.gitlab] host` from `cfg`.
pub fn gitlab_host(cfg: Option<&Config>) -> Option<String> {
    crate::forge::effective_host(ForgeKind::Gitlab, cfg.and_then(|c| c.forge.gitlab.as_ref()))
}

/// The forge `source` names, else the one inferred from `origin` and `gitlab_host`: what a
/// reader that never fails on a bad `source` shows.
pub fn kind(source: Option<&str>, origin: Option<&str>, gitlab_host: Option<&str>) -> ForgeKind {
    source
        .and_then(ForgeKind::parse)
        .unwrap_or_else(|| inferred(origin, gitlab_host))
}

/// The forge inferred from `origin`: gitlab for a host containing it or equal to `gitlab_host`
/// (hosts are case-insensitive), else github, also for no origin or a local path.
fn inferred(origin: Option<&str>, gitlab_host: Option<&str>) -> ForgeKind {
    let gitlab = origin.and_then(host).is_some_and(|h| {
        h.to_ascii_lowercase().contains("gitlab")
            || gitlab_host.is_some_and(|g| host(g).unwrap_or(g).eq_ignore_ascii_case(h))
    });
    if gitlab {
        ForgeKind::Gitlab
    } else {
        ForgeKind::Github
    }
}

/// The host of a remote URL, `scheme://[user@]host[:port]/path` or scp-like `[user@]host:path`;
/// `None` for a local path.
pub fn host(url: &str) -> Option<&str> {
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest.split('/').next()?,
        None => url.split_once(':').filter(|(h, _)| !h.contains('/'))?.0,
    };
    let host = rest.rsplit('@').next()?;
    Some(host.split(':').next().unwrap_or(host))
}

/// The path of a remote URL, the part after the host: `scheme://[user@]host[:port]/path` or
/// scp-like `[user@]host:path`; `None` for a local path.
pub fn path(url: &str) -> Option<&str> {
    match url.split_once("://") {
        Some((_, rest)) => Some(rest.split_once('/')?.1),
        None => Some(url.split_once(':').filter(|(h, _)| !h.contains('/'))?.1),
    }
}

/// `s` when it is a full commit id: 40 (SHA-1) or 64 (SHA-256) hex digits.
pub(super) fn head_sha(s: &str) -> Result<&str> {
    let hex = s.bytes().all(|b| b.is_ascii_hexdigit());
    if hex && matches!(s.len(), 40 | 64) {
        Ok(s)
    } else {
        anyhow::bail!("the forge gave a head that is not a commit id")
    }
}

/// The first of the runs in `v` whose `key` is a commit id (7 to 64 hex digits, so never an
/// option for `git log`) other than `red`.
pub(super) fn first_other(v: &Value, key: &str, red: &str) -> Option<String> {
    v.as_array()?
        .iter()
        .filter_map(|r| r[key].as_str())
        .find(|sha| {
            *sha != red
                && (7..=64).contains(&sha.len())
                && sha.bytes().all(|b| b.is_ascii_hexdigit())
        })
        .map(String::from)
}

/// `gh issue view --json title,url`, or GitLab's issue with its `web_url`.
fn parse_issue_link(v: &Value, url_key: &str) -> IssueLink {
    IssueLink {
        title: v["title"].as_str().unwrap_or("").to_string(),
        url: v[url_key].as_str().unwrap_or("").to_string(),
    }
}

#[cfg(test)]
mod tests;
