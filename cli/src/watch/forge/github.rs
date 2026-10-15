//! GitHub through `gh`.

use std::path::PathBuf;

use anyhow::Result;
use serde_json::Value;

use super::main_checks::{parse_main_checks, MAIN_CHECKS_QUERY};
use super::{
    add_closing, first_other, head_sha, parse_issue_link, ClosingPrs, CommitPr, Filed, Forge,
    IssueLink, IssueState, MergedPr, MergedPrs, Post, Pr, PrState, PrStatus,
};
use crate::run::{gh, gh_json};
use crate::watch::ci::HeadChecks;
use crate::watch::Issue;

/// How many PRs one `gh pr list` reads.
const PR_LIMIT: usize = 200;

/// The colour a missing label is created with, and the description of each kind.
const LABEL_COLOR: &str = "d73a4a";
const PR_LABEL_DESCRIPTION: &str = "nightshift left this PR for a human to merge";
const ISSUE_LABEL_DESCRIPTION: &str = "nightshift files main's fix issue with this label";

/// GitHub through `gh`.
pub struct GitHub {
    pub(super) root: PathBuf,
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

/// `gh pr list`'s PRs, under each issue their bodies close.
fn parse_closing(prs: &Value) -> ClosingPrs {
    let mut by_issue = ClosingPrs::new();
    for p in prs.as_array().map(Vec::as_slice).unwrap_or_default() {
        let body = p["body"].as_str().unwrap_or("");
        add_closing(
            &mut by_issue,
            body,
            p["number"].as_u64().unwrap_or(0),
            &own_head(p),
        );
    }
    by_issue
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

/// The state `gh issue view --json state,stateReason` gives, or the REST API's lowercase one.
fn issue_state(v: &Value) -> IssueState {
    let upper = |k: &str| v[k].as_str().map(str::to_ascii_uppercase);
    match (upper("state").as_deref(), upper("stateReason").as_deref()) {
        (Some("CLOSED"), None | Some("" | "COMPLETED")) => IssueState::Completed,
        (Some("CLOSED"), _) => IssueState::NotPlanned,
        _ => IssueState::Open,
    }
}

/// `gh pr view --json state,headRefOid,headRefName,body`.
fn parse_pr(v: &Value) -> Pr {
    let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Pr {
        status: match v["state"].as_str() {
            Some("MERGED") => PrStatus::Merged,
            Some("CLOSED") => PrStatus::Closed,
            _ => PrStatus::Open,
        },
        head_sha: text("headRefOid"),
        head_branch: text("headRefName"),
        body: text("body"),
    }
}

/// GitHub's `commits/<sha>/pulls` list. One with no number is left out.
fn parse_commit_prs(pulls: &Value) -> Vec<CommitPr> {
    pulls
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(CommitPr {
                number: p["number"].as_u64()?,
                head: p["head"]["ref"].as_str().unwrap_or("").to_string(),
                merged: p["merged_at"].as_str().is_some_and(|s| !s.is_empty()),
            })
        })
        .collect()
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
        Ok(parse_closing(&prs))
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

    fn issue_link(&self, n: u64) -> Result<IssueLink> {
        let v = gh_json(
            &self.root,
            &["issue", "view", &n.to_string(), "--json", "title,url"],
        )?;
        Ok(parse_issue_link(&v, "url"))
    }

    fn pr(&self, n: u64) -> Result<Pr> {
        let v = gh_json(
            &self.root,
            &[
                "pr",
                "view",
                &n.to_string(),
                "--json",
                "state,headRefOid,headRefName,body",
            ],
        )?;
        Ok(parse_pr(&v))
    }

    fn commit_prs(&self, sha: &str) -> Result<Vec<CommitPr>> {
        let path = format!("repos/{{owner}}/{{repo}}/commits/{}/pulls", head_sha(sha)?);
        Ok(parse_commit_prs(&gh_json(&self.root, &["api", &path])?))
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
