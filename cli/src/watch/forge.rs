//! The forge `ns watch` reads its queue from and keeps issue state on, chosen by `[queue] source`
//! (docs/FACTORY.md). Credentials are `crate::forge`'s job, not this module's.

mod gitlab;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::{closed_by, Issue};
use crate::config::ForgeKind;
use crate::error::SfError;
use crate::factory::{self, Queue as QueueConfig};
use crate::git;
use crate::run::{gh, gh_json};

#[derive(Clone, Copy)]
pub enum PrState {
    Open,
    Merged,
}

/// Issue number -> the PR that closes it and that PR's head branch, empty for a fork's.
pub type ClosingPrs = BTreeMap<u64, (u64, String)>;

pub trait Forge {
    /// Open issues, never PRs; `label` narrows the list to issues carrying it.
    fn open_issues(&self, label: Option<&str>) -> Result<Vec<Issue>>;
    fn closing_prs(&self, state: PrState) -> Result<ClosingPrs>;
    fn is_closed(&self, n: u64) -> Result<bool>;
    fn labels(&self, n: u64) -> Result<Vec<String>>;
    fn edit_labels(&self, n: u64, remove: &[String], add: Option<&str>) -> Result<()>;
    fn comment(&self, n: u64, body: &str) -> Result<()>;
    fn close(&self, n: u64) -> Result<()>;
}

const HINT: &str = "set the forge in .nightshift/nightshift.toml:\n  [queue]\n  source = \"github\"\n\nsee docs/FACTORY.md";

/// The forge `[queue] source` names or, unset, the one `origin`'s host is on. `GITLAB_HOST`, set
/// from `[forge.gitlab] host` by then, names one more GitLab host.
pub fn open(q: &QueueConfig, root: &Path) -> Result<Box<dyn Forge>, SfError> {
    let origin = git::run(root, &["remote", "get-url", "origin"]).ok();
    let gitlab_host = std::env::var(ForgeKind::Gitlab.host_var()).ok();
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
        .map(|i| Issue {
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
        })
        .filter(|i| i.number > 0)
        .collect()
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
                "200",
                "--json",
                "number,body,headRefName,isCrossRepository",
            ],
        )?;
        let mut by_issue = BTreeMap::new();
        for p in prs.as_array().cloned().unwrap_or_default() {
            for n in closed_by(p["body"].as_str().unwrap_or("")) {
                // A fork's branch name says nothing about this repo's units.
                let head = match p["isCrossRepository"].as_bool() {
                    Some(false) => p["headRefName"].as_str().unwrap_or("").to_string(),
                    _ => String::new(),
                };
                by_issue.insert(n, (p["number"].as_u64().unwrap_or(0), head));
            }
        }
        Ok(by_issue)
    }

    fn is_closed(&self, n: u64) -> Result<bool> {
        let v = gh_json(
            &self.root,
            &["issue", "view", &n.to_string(), "--json", "state"],
        )?;
        Ok(v["state"].as_str() == Some("CLOSED"))
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

    fn comment(&self, n: u64, body: &str) -> Result<()> {
        gh(
            &self.root,
            &["issue", "comment", &n.to_string(), "--body", body],
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_inferred_from_the_origin_host() {
        let cases = [
            (Some("https://gitlab.com/o/r.git"), "gitlab"),
            (Some("git@gitlab.example:o/r.git"), "gitlab"),
            (Some("https://gitlab.corp.example/o/r"), "gitlab"),
            (Some("https://GitLab.com/o/r"), "gitlab"),
            (Some("https://github.com/o/r"), "github"),
            (Some("/srv/git/gitlab-mirror.git"), "github"),
            (None, "github"),
        ];
        for (origin, want) in cases {
            assert_eq!(inferred(origin, None).name(), want, "{origin:?}");
        }
        assert_eq!(
            check(Some("github"), Some("https://gitlab.com/o/r"), None)
                .unwrap()
                .name(),
            "github"
        );
    }

    #[test]
    fn source_is_gitlab_on_the_configured_gitlab_host() {
        let origin = Some("git@Code.example.com:o/r.git");
        for h in ["code.example.com", "https://CODE.example.com"] {
            assert_eq!(inferred(origin, Some(h)).name(), "gitlab", "{h}");
        }
        assert_eq!(inferred(origin, Some("other.example")).name(), "github");
        assert_eq!(
            check(Some("github"), origin, Some("code.example.com"))
                .unwrap()
                .name(),
            "github"
        );
    }

    #[test]
    fn check_takes_github_and_gitlab_and_refuses_unknown_with_the_hint() {
        assert_eq!(
            check(None, Some("https://github.com/o/r"), None).unwrap(),
            ForgeKind::Github
        );
        assert_eq!(
            check(Some("github"), None, None).unwrap(),
            ForgeKind::Github
        );
        assert_eq!(
            check(Some("gitlab"), None, None).unwrap(),
            ForgeKind::Gitlab
        );
        assert_eq!(
            check(None, Some("https://gitlab.com/o/r"), None).unwrap(),
            ForgeKind::Gitlab
        );
        for (s, o) in [
            (Some("bitbucket"), None),
            (Some("svn"), Some("https://github.com/o/r")),
        ] {
            let e = check(s, o, None).unwrap_err().to_string();
            assert!(e.contains("[queue] source"), "{e}");
        }
    }

    #[test]
    fn host_reads_url_and_scp_forms_and_no_host_from_a_path() {
        assert_eq!(host("https://gitlab.com/o/r.git"), Some("gitlab.com"));
        assert_eq!(
            host("ssh://git@gitlab.example:2222/o/r"),
            Some("gitlab.example")
        );
        assert_eq!(host("https://user:pw@github.com/o/r"), Some("github.com"));
        assert_eq!(host("git@github.com:o/r.git"), Some("github.com"));
        assert_eq!(host("/srv/git/gitlab:mirror.git"), None);
        assert_eq!(host("../gitlab-mirror"), None);
    }
}
