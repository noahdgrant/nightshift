//! The forge `ns watch` reads its queue from and keeps issue state on, chosen by `[queue] source`
//! (docs/FACTORY.md). Credentials are `crate::forge`'s job, not this module's.

mod gitlab;
mod main_checks;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use self::main_checks::{parse_main_checks, MAIN_CHECKS_QUERY};
use super::ci::HeadChecks;
use super::{closed_by, Issue};
use crate::config::{Config, ForgeKind};
use crate::error::SfError;
use crate::factory::{self, Queue as QueueConfig};
use crate::git;
use crate::run::{gh, gh_json};

#[derive(Clone, Copy)]
pub enum PrState {
    Open,
    Merged,
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

/// How many PRs one `gh pr list` reads.
const PR_LIMIT: usize = 200;

/// Issue number -> the PR that closes it and that PR's head branch, empty for a fork's.
pub type ClosingPrs = BTreeMap<u64, (u64, String)>;

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
}

/// The colour a missing label is created with, and the description of each kind.
const LABEL_COLOR: &str = "d73a4a";
const PR_LABEL_DESCRIPTION: &str = "nightshift left this PR for a human to merge";
const ISSUE_LABEL_DESCRIPTION: &str = "nightshift files main's fix issue with this label";

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

/// GitHub through `gh`.
pub struct GitHub {
    root: PathBuf,
}

/// Author associations whose issues `ns watch` will run (docs/FACTORY.md, Trust).
const TEAM: [&str; 3] = ["OWNER", "MEMBER", "COLLABORATOR"];

/// One issue per line, as printed by the `--jq` filter in `open_issues`.
fn parse_issues(text: &str) -> Vec<Issue> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|i| to_issue(&i))
        .filter(|i| i.number > 0)
        .collect()
}

fn to_issue(i: &Value) -> Issue {
    Issue {
        number: i["number"].as_u64().unwrap_or(0),
        title: i["title"].as_str().unwrap_or("").to_string(),
        labels: i["labels"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(|x| x["name"].as_str().or(x.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        body: i["body"].as_str().unwrap_or("").to_string(),
        team: TEAM.contains(&i["authorAssociation"].as_str().unwrap_or("")),
    }
}

/// One issue per line, as printed by the `--jq` filter in `labelled_issues`. An issue with no
/// opening time it can read is left out.
fn parse_filed(text: &str) -> Vec<Filed> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|i| {
            Some(Filed {
                issue: to_issue(&i),
                opened: crate::clock::parse_iso(i["createdAt"].as_str()?)?,
                state: issue_state(&i),
            })
        })
        .filter(|f| f.issue.number > 0)
        .collect()
}

/// A PR's head branch, empty for a fork's: its name says nothing about this repo's units.
fn own_head(p: &Value) -> String {
    match p["isCrossRepository"].as_bool() {
        Some(false) => p["headRefName"].as_str().unwrap_or("").to_string(),
        _ => String::new(),
    }
}

/// `gh pr list`'s merged PRs. One with no merge time it can read is left out.
fn parse_merged(prs: &Value) -> MergedPrs {
    let all = prs.as_array().map_or(&[][..], Vec::as_slice);
    MergedPrs {
        prs: all
            .iter()
            .filter_map(|p| {
                Some(MergedPr {
                    number: p["number"].as_u64()?,
                    head: own_head(p),
                    merged: crate::clock::parse_iso(p["mergedAt"].as_str()?)?,
                    changed: p["additions"]
                        .as_u64()
                        .zip(p["deletions"].as_u64())
                        .map(|(a, d)| a + d),
                })
            })
            .collect(),
        truncated: all.len() >= PR_LIMIT,
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

/// The state `gh issue view --json state,stateReason` gives, or the REST API's lowercase one.
fn issue_state(v: &Value) -> IssueState {
    let upper = |k: &str| v[k].as_str().map(str::to_ascii_uppercase);
    match (upper("state").as_deref(), upper("stateReason").as_deref()) {
        (Some("CLOSED"), None | Some("" | "COMPLETED")) => IssueState::Completed,
        (Some("CLOSED"), _) => IssueState::NotPlanned,
        _ => IssueState::Open,
    }
}

/// The issue number at the end of the URL `gh issue create` prints.
fn created_number(out: &str) -> Result<u64> {
    out.trim()
        .rsplit('/')
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("gh issue create printed no issue URL: {}", out.trim()))
}

impl GitHub {
    /// The default branch, as the forge names it: the one [`Forge::main_checks`] reads.
    fn default_branch(&self) -> Result<String> {
        let v = gh_json(&self.root, &["repo", "view", "--json", "defaultBranchRef"])?;
        match v["defaultBranchRef"]["name"].as_str() {
            Some(b) if !b.is_empty() && !b.starts_with('-') => Ok(b.to_string()),
            _ => anyhow::bail!("GitHub names no default branch"),
        }
    }
}

impl Forge for GitHub {
    fn open_issues(&self, label: Option<&str>) -> Result<Vec<Issue>> {
        let filter = label.map_or(String::new(), |l| format!("&labels={l}"));
        let path = format!("repos/{{owner}}/{{repo}}/issues?state=open{filter}&per_page=100");
        // `gh issue list --json` has no author association, so read the REST list.
        Ok(parse_issues(&gh(
            &self.root,
            &[
                "api",
                "--paginate",
                &path,
                "--jq",
                ".[] | select(.pull_request | not) | {number, title, labels, body, authorAssociation: .author_association}",
            ],
        )?))
    }

    fn labelled_issues(&self, label: &str) -> Result<Vec<Filed>> {
        let path = format!("repos/{{owner}}/{{repo}}/issues?state=all&labels={label}&per_page=100");
        Ok(parse_filed(&gh(
            &self.root,
            &[
                "api",
                "--paginate",
                &path,
                "--jq",
                ".[] | select(.pull_request | not) | {number, title, labels, body, authorAssociation: .author_association, createdAt: .created_at, state, stateReason: .state_reason}",
            ],
        )?))
    }

    fn closing_prs(&self, state: PrState) -> Result<ClosingPrs> {
        let state = match state {
            PrState::Open => "open",
            PrState::Merged => "merged",
        };
        let prs = gh_json(
            &self.root,
            &[
                "pr",
                "list",
                "--state",
                state,
                "--limit",
                &PR_LIMIT.to_string(),
                "--json",
                "number,body,headRefName,isCrossRepository",
            ],
        )?;
        let mut by_issue = BTreeMap::new();
        for p in prs.as_array().cloned().unwrap_or_default() {
            for n in closed_by(p["body"].as_str().unwrap_or("")) {
                by_issue.insert(n, (p["number"].as_u64().unwrap_or(0), own_head(&p)));
            }
        }
        Ok(by_issue)
    }

    fn merged_prs(&self, since: Option<i64>) -> Result<MergedPrs> {
        let mut args = vec![
            "pr".to_string(),
            "list".into(),
            "--state".into(),
            "merged".into(),
            "--limit".into(),
            PR_LIMIT.to_string(),
            "--json".into(),
            "number,headRefName,isCrossRepository,mergedAt,additions,deletions".into(),
        ];
        if let Some(t) = since {
            args.push("--search".into());
            args.push(format!("merged:>={}", &crate::clock::iso(t)[..10]));
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let prs = gh_json(&self.root, &args)?;
        Ok(parse_merged(&prs))
    }

    /// `n` may be a pull request, which `gh issue view` reports as `MERGED` once merged.
    fn is_closed(&self, n: u64) -> Result<bool> {
        let v = gh_json(
            &self.root,
            &["issue", "view", &n.to_string(), "--json", "state"],
        )?;
        Ok(matches!(v["state"].as_str(), Some("CLOSED" | "MERGED")))
    }

    fn state(&self, n: u64) -> Result<IssueState> {
        let v = gh_json(
            &self.root,
            &[
                "issue",
                "view",
                &n.to_string(),
                "--json",
                "state,stateReason",
            ],
        )?;
        Ok(issue_state(&v))
    }

    fn sub_issues(&self, n: u64) -> Result<Vec<u64>> {
        let path = format!("repos/{{owner}}/{{repo}}/issues/{n}/sub_issues?per_page=100");
        let out = gh(
            &self.root,
            &["api", &path, "--paginate", "--jq", ".[].number"],
        )?;
        Ok(out.lines().filter_map(|l| l.trim().parse().ok()).collect())
    }

    fn team_comments(&self, n: u64) -> Result<Vec<String>> {
        let path = format!("repos/{{owner}}/{{repo}}/issues/{n}/comments?per_page=100");
        let out = gh(
            &self.root,
            &[
                "api",
                &path,
                "--paginate",
                "--jq",
                ".[] | {number: 1, body, authorAssociation: .author_association}",
            ],
        )?;
        Ok(parse_issues(&out)
            .into_iter()
            .filter(|c| c.team)
            .map(|c| c.body)
            .collect())
    }

    fn part_of(&self, n: u64) -> Result<Vec<Issue>> {
        let q = format!("q=repo:{{owner}}/{{repo}} is:issue in:body \"Part of #{n}\"");
        Ok(parse_issues(&gh(
            &self.root,
            &[
                "api",
                "search/issues",
                "--method",
                "GET",
                "-F",
                &q,
                "-F",
                "per_page=100",
                "--jq",
                ".items[] | {number, title, labels, body, authorAssociation: .author_association}",
            ],
        )?))
    }

    /// Without `--force`, an existing label keeps the colour and description a human gave it.
    fn ensure_label(&self, label: &str, description: &str) -> Result<()> {
        let created = gh(
            &self.root,
            &[
                "label",
                "create",
                label,
                "--color",
                LABEL_COLOR,
                "--description",
                description,
            ],
        );
        match created {
            Err(e) if !format!("{e:#}").contains("already exists") => Err(e),
            _ => Ok(()),
        }
    }

    fn labels(&self, n: u64) -> Result<Vec<String>> {
        let v = gh_json(
            &self.root,
            &["issue", "view", &n.to_string(), "--json", "labels"],
        )?;
        Ok(v["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l["name"].as_str().map(String::from))
            .collect())
    }

    fn edit_labels(&self, n: u64, remove: &[String], add: Option<&str>) -> Result<()> {
        let ns = n.to_string();
        let mut args = vec!["issue", "edit", ns.as_str()];
        for l in remove {
            args.extend(["--remove-label", l.as_str()]);
        }
        if let Some(a) = add {
            args.extend(["--add-label", a]);
        }
        gh(&self.root, &args).map(|_| ())
    }

    fn comment(&self, n: u64, body: &Post) -> Result<()> {
        gh(
            &self.root,
            &["issue", "comment", &n.to_string(), "--body", body.as_str()],
        )
        .map(|_| ())
    }

    fn close(&self, n: u64) -> Result<()> {
        gh(
            &self.root,
            &["issue", "close", &n.to_string(), "--reason", "completed"],
        )
        .map(|_| ())
    }

    fn add_pr_label(&self, pr: u64, label: &str) -> Result<()> {
        self.ensure_label(label, PR_LABEL_DESCRIPTION)?;
        gh(
            &self.root,
            &["pr", "edit", &pr.to_string(), "--add-label", label],
        )
        .map(|_| ())
    }

    fn remove_pr_label(&self, pr: u64, label: &str) -> Result<()> {
        let ns = pr.to_string();
        let v = gh_json(&self.root, &["pr", "view", &ns, "--json", "labels"])?;
        let carried = v["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|l| l["name"].as_str() == Some(label));
        if !carried {
            return Ok(());
        }
        gh(&self.root, &["pr", "edit", &ns, "--remove-label", label]).map(|_| ())
    }

    fn comment_pr(&self, pr: u64, body: &Post) -> Result<()> {
        gh(
            &self.root,
            &["pr", "comment", &pr.to_string(), "--body", body.as_str()],
        )
        .map(|_| ())
    }

    fn main_checks(&self) -> Result<HeadChecks> {
        let query = format!("query={MAIN_CHECKS_QUERY}");
        parse_main_checks(&gh_json(
            &self.root,
            &[
                "api",
                "graphql",
                "-f",
                &query,
                "-F",
                "owner={owner}",
                "-F",
                "repo={repo}",
            ],
        )?)
    }

    fn create_issue(&self, title: &str, body: &Post, labels: &[&str]) -> Result<u64> {
        let mut args = vec!["issue", "create", "--title", title, "--body", body.as_str()];
        for l in labels {
            self.ensure_label(l, ISSUE_LABEL_DESCRIPTION)?;
            args.extend(["--label", l]);
        }
        created_number(&gh(&self.root, &args)?)
    }

    /// A check run's id is its Actions job's; a check from another app has no log here.
    fn check_log(&self, log: &str) -> Result<String> {
        gh(&self.root, &["run", "view", "--job", log, "--log-failed"])
    }

    fn last_green(&self, red: &str) -> Result<Option<String>> {
        let branch = self.default_branch()?;
        let v = gh_json(
            &self.root,
            &[
                "run", "list", "--branch", &branch, "--status", "success", "--limit", "30",
                "--json", "headSha",
            ],
        )?;
        Ok(first_other(&v, "headSha", red))
    }
}

#[cfg(test)]
mod tests;
