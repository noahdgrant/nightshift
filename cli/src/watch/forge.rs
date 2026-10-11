//! The forge `ns watch` reads its queue from and keeps issue state on, chosen by `[queue] source`
//! (docs/FACTORY.md). Credentials are `crate::forge`'s job, not this module's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::{closed_by, Issue};
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

/// The forge `[queue] source` names or, unset, the one `origin`'s host is on.
pub fn open(q: &QueueConfig, root: &Path) -> Result<Box<dyn Forge>, SfError> {
    let origin = git::run(root, &["remote", "get-url", "origin"]).ok();
    check(q.source.as_deref(), origin.as_deref())?;
    Ok(Box::new(GitHub {
        root: root.to_path_buf(),
    }))
}

/// Whether `ns watch` can use the forge `source` and `origin` resolve to. Names the host, never
/// the origin URL, which may carry credentials.
fn check(source: Option<&str>, origin: Option<&str>) -> Result<(), SfError> {
    let name = source_name(source, origin);
    match factory::source_problem(&name) {
        None => Ok(()),
        Some(p) if source.is_some() => Err(SfError::usage(p, HINT)),
        Some(p) => {
            let host = origin.and_then(host).unwrap_or("");
            Err(SfError::usage(
                format!("{p} (unset, inferred from origin host {host})"),
                HINT,
            ))
        }
    }
}

/// The configured source name or, unset, the one inferred from `origin`: `gitlab` for a host
/// containing it (hosts are case-insensitive), else `github`, also for no origin or a local path.
fn source_name(source: Option<&str>, origin: Option<&str>) -> String {
    if let Some(s) = source {
        return s.to_string();
    }
    let gitlab = origin
        .and_then(host)
        .is_some_and(|h| h.to_ascii_lowercase().contains("gitlab"));
    if gitlab { "gitlab" } else { "github" }.to_string()
}

/// The host of a remote URL, `scheme://[user@]host[:port]/path` or scp-like `[user@]host:path`;
/// `None` for a local path.
fn host(url: &str) -> Option<&str> {
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
            assert_eq!(source_name(None, origin), want, "{origin:?}");
        }
        assert_eq!(
            source_name(Some("github"), Some("https://gitlab.com/o/r")),
            "github"
        );
    }

    #[test]
    fn check_refuses_gitlab_and_unknown_with_the_hint() {
        assert!(check(None, Some("https://github.com/o/r")).is_ok());
        assert!(check(Some("github"), None).is_ok());
        for (s, o) in [
            (Some("gitlab"), None),
            (Some("bitbucket"), None),
            (None, Some("https://gitlab.com/o/r")),
        ] {
            let e = check(s, o).unwrap_err().to_string();
            assert!(e.contains("[queue] source"), "{e}");
        }
    }

    #[test]
    fn check_error_never_prints_origin_credentials() {
        let e = check(None, Some("https://user:s3cret@gitlab.example/o/r.git"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("gitlab.example"), "{e}");
        assert!(!e.contains("s3cret") && !e.contains("user"), "{e}");
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
