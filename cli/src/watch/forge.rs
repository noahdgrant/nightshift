//! The forge `ns watch` reads its queue from and keeps issue state on, chosen by `[queue] source`
//! (docs/FACTORY.md). Credentials are `crate::forge`'s job, not this module's.

mod gitlab;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::ci::{Check, HeadChecks, Outcome};
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
    /// Add `label` to PR `pr`, creating it in the repo first if it is missing.
    fn add_pr_label(&self, pr: u64, label: &str) -> Result<()>;
    /// Remove `label` from PR `pr` if the PR carries it.
    fn remove_pr_label(&self, pr: u64, label: &str) -> Result<()>;
    fn comment_pr(&self, pr: u64, body: &str) -> Result<()>;
    /// The default branch's head and the checks on it.
    fn main_checks(&self) -> Result<HeadChecks>;
    /// File an issue carrying `labels`, creating any the repo lacks; its number.
    fn create_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<u64>;
    /// The log of a failed check, at the [`Check::log`] the forge gave it.
    fn check_log(&self, log: &str) -> Result<String>;
    /// The newest commit on the default branch that a CI run passed, if the forge knows one.
    fn last_green(&self) -> Result<Option<String>>;
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

/// The default branch's name and head, the checks its rulesets require, and the head's check
/// runs and commit statuses, all in one query.
const MAIN_CHECKS_QUERY: &str = "query($owner: String!, $repo: String!) {
  repository(owner: $owner, name: $repo) {
    defaultBranchRef {
      rules(first: 100) {
        pageInfo { hasNextPage }
        nodes { parameters { ... on RequiredStatusChecksParameters { requiredStatusChecks { context } } } }
      }
      target { ... on Commit {
        oid
        statusCheckRollup { contexts(first: 100) { pageInfo { hasNextPage } nodes {
          ... on CheckRun { name status conclusion databaseId }
          ... on StatusContext { context state }
        } } }
      } }
    }
  }
}";

/// `s` when it is a full commit id: 40 (SHA-1) or 64 (SHA-256) hex digits.
pub(super) fn head_sha(s: &str) -> Result<&str> {
    let hex = s.bytes().all(|b| b.is_ascii_hexdigit());
    if hex && matches!(s.len(), 40 | 64) {
        Ok(s)
    } else {
        anyhow::bail!("the forge gave a head that is not a commit id")
    }
}

/// The head and checks in `MAIN_CHECKS_QUERY`'s answer. A check is required when a ruleset names
/// it; when no ruleset names any check on the head (classic branch protection, which a reader
/// can't see, or none), every check is.
fn parse_main_checks(v: &Value) -> Result<HeadChecks> {
    let main = &v["data"]["repository"]["defaultBranchRef"];
    let Some(oid) = main["target"]["oid"].as_str() else {
        anyhow::bail!("GitHub names no default branch head");
    };
    let sha = head_sha(oid)?;
    if main["rules"]["pageInfo"]["hasNextPage"].as_bool() == Some(true) {
        anyhow::bail!("GitHub's list of the branch's rules is truncated at 100");
    }
    if main["target"]["statusCheckRollup"]["contexts"]["pageInfo"]["hasNextPage"].as_bool()
        == Some(true)
    {
        anyhow::bail!("GitHub's list of the head's checks is truncated at 100");
    }
    let named: Vec<&str> = main["rules"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|r| {
            r["parameters"]["requiredStatusChecks"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .filter_map(|c| c["context"].as_str())
        .collect();
    let mut checks: Vec<Check> = main["target"]["statusCheckRollup"]["contexts"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let (name, outcome) = match (c["name"].as_str(), c["context"].as_str()) {
                (Some(n), _) => (n, check_run_outcome(c)),
                (_, Some(n)) => (n, status_outcome(c["state"].as_str().unwrap_or(""))),
                _ => return None,
            };
            Some(Check {
                name: name.to_string(),
                outcome,
                required: named.contains(&name),
                log: c["databaseId"].as_u64().map(|id| id.to_string()),
            })
        })
        .collect();
    if !checks.iter().any(|c| c.required) {
        checks.iter_mut().for_each(|c| c.required = true);
    }
    Ok(HeadChecks {
        sha: sha.to_string(),
        checks,
    })
}

fn check_run_outcome(c: &Value) -> Outcome {
    match (c["status"].as_str(), c["conclusion"].as_str()) {
        (Some("COMPLETED"), Some("SUCCESS" | "NEUTRAL" | "SKIPPED")) => Outcome::Pass,
        (Some("COMPLETED"), _) => Outcome::Fail,
        _ => Outcome::Running,
    }
}

fn status_outcome(state: &str) -> Outcome {
    match state {
        "SUCCESS" => Outcome::Pass,
        "FAILURE" | "ERROR" => Outcome::Fail,
        _ => Outcome::Running,
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
    /// Create `label` unless the repo has it. Without `--force`, an existing label keeps the
    /// colour and description a human gave it.
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

    fn comment_pr(&self, pr: u64, body: &str) -> Result<()> {
        gh(
            &self.root,
            &["pr", "comment", &pr.to_string(), "--body", body],
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

    fn create_issue(&self, title: &str, body: &str, labels: &[&str]) -> Result<u64> {
        let mut args = vec!["issue", "create", "--title", title, "--body", body];
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

    /// The head of the newest Actions run on the default branch that succeeded.
    fn last_green(&self) -> Result<Option<String>> {
        let Some((branch, _)) = crate::run::remote_default(&self.root) else {
            anyhow::bail!("cannot read origin's default branch");
        };
        let v = gh_json(
            &self.root,
            &[
                "run", "list", "--branch", &branch, "--status", "success", "--limit", "1",
                "--json", "headSha",
            ],
        )?;
        Ok(v[0]["headSha"].as_str().map(String::from))
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

    const SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

    /// `MAIN_CHECKS_QUERY`'s answer for head `SHA`, with `rules` and check `nodes`.
    fn answer(rules: &Value, nodes: &Value) -> Value {
        serde_json::json!({"data": {"repository": {"defaultBranchRef": {
            "rules": {"nodes": rules},
            "target": {"oid": SHA, "statusCheckRollup": {"contexts": {"nodes": nodes}}},
        }}}})
    }

    fn outcomes(h: &HeadChecks) -> Vec<(&str, Outcome, bool)> {
        h.checks
            .iter()
            .map(|c| (c.name.as_str(), c.outcome, c.required))
            .collect()
    }

    #[test]
    fn main_checks_reads_check_runs_and_statuses_and_ruleset_required_checks() {
        use Outcome::*;
        let run = |name, status, conclusion: Option<&str>| serde_json::json!({"name": name, "status": status, "conclusion": conclusion});
        let status = |context, state| serde_json::json!({"context": context, "state": state});
        let rules = serde_json::json!([
            {"parameters": null},
            {"parameters": {"requiredStatusChecks": [{"context": "test"}, {"context": "ci/ext"}, {"context": "title"}]}},
        ]);
        let nodes = serde_json::json!([
            run("test", "COMPLETED", Some("SUCCESS")),
            run("lint", "COMPLETED", Some("NEUTRAL")),
            run("docs", "COMPLETED", Some("SKIPPED")),
            run("build", "COMPLETED", Some("FAILURE")),
            run("deploy", "COMPLETED", Some("CANCELLED")),
            run("slow", "IN_PROGRESS", None),
            run("queued", "QUEUED", None),
            status("ci/ext", "FAILURE"),
            status("ci/err", "ERROR"),
            status("ci/ok", "SUCCESS"),
            status("ci/wait", "PENDING"),
            {"unknown": 1},
        ]);
        let h = parse_main_checks(&answer(&rules, &nodes)).unwrap();
        assert_eq!(h.sha, SHA);
        assert_eq!(
            outcomes(&h),
            [
                ("test", Pass, true),
                ("lint", Pass, false),
                ("docs", Pass, false),
                ("build", Fail, false),
                ("deploy", Fail, false),
                ("slow", Running, false),
                ("queued", Running, false),
                ("ci/ext", Fail, true),
                ("ci/err", Fail, false),
                ("ci/ok", Pass, false),
                ("ci/wait", Running, false),
            ]
        );
    }

    #[test]
    fn main_checks_requires_every_check_when_no_ruleset_names_one_on_the_head() {
        let nodes =
            serde_json::json!([{"name": "test", "status": "COMPLETED", "conclusion": "FAILURE"}]);
        for rules in [
            serde_json::json!([]),
            serde_json::json!([{"parameters": {"requiredStatusChecks": [{"context": "title"}]}}]),
        ] {
            let h = parse_main_checks(&answer(&rules, &nodes)).unwrap();
            assert_eq!(outcomes(&h), [("test", Outcome::Fail, true)]);
        }
        let mut none = answer(&serde_json::json!([]), &Value::Null);
        none["data"]["repository"]["defaultBranchRef"]["target"]["statusCheckRollup"] = Value::Null;
        assert!(parse_main_checks(&none).unwrap().checks.is_empty());
        let empty = serde_json::json!({"data": {"repository": {"defaultBranchRef": null}}});
        assert!(parse_main_checks(&empty).is_err());
    }

    #[test]
    fn a_check_run_keeps_its_job_id_as_its_log_and_a_status_has_none() {
        let nodes = serde_json::json!([
            {"name": "test", "status": "COMPLETED", "conclusion": "FAILURE", "databaseId": 42},
            {"context": "ci/ext", "state": "FAILURE"},
        ]);
        let h = parse_main_checks(&answer(&serde_json::json!([]), &nodes)).unwrap();
        let logs: Vec<Option<&str>> = h.checks.iter().map(|c| c.log.as_deref()).collect();
        assert_eq!(logs, [Some("42"), None]);
        assert!(MAIN_CHECKS_QUERY.contains("conclusion databaseId"));
    }

    #[test]
    fn last_green_needs_origin_to_name_the_branch() {
        let gh = GitHub {
            root: "/nonexistent".into(),
        };
        let e = gh.last_green().unwrap_err();
        assert!(
            format!("{e:#}").contains("origin's default branch"),
            "{e:#}"
        );
    }

    #[test]
    fn created_number_reads_the_url_gh_prints() {
        assert_eq!(
            created_number("https://github.com/o/r/issues/123\n").unwrap(),
            123
        );
        assert!(created_number("").is_err());
        assert!(created_number("https://github.com/o/r/issues/new").is_err());
    }

    #[test]
    fn main_checks_refuses_a_head_that_is_not_a_commit_id() {
        let bad = [
            "abc".to_string(),
            "../x?y=1".to_string(),
            "g".repeat(40),
            format!("{SHA}/x"),
            format!("{}/", "a".repeat(39)),
            format!("{}..", "a".repeat(38)),
            "a".repeat(41),
            "a".repeat(63),
            "a".repeat(65),
        ];
        for bad in &bad {
            let mut v = answer(&serde_json::json!([]), &serde_json::json!([]));
            v["data"]["repository"]["defaultBranchRef"]["target"]["oid"] = bad.as_str().into();
            assert!(parse_main_checks(&v).is_err(), "{bad}");
        }
        assert_eq!(head_sha(&"a".repeat(64)).unwrap().len(), 64);
    }

    #[test]
    fn main_checks_refuses_a_truncated_list_and_dumps_no_response() {
        let truncated = |path: &[&str]| {
            let mut v = answer(&serde_json::json!([]), &serde_json::json!([]));
            let mut at = &mut v["data"]["repository"]["defaultBranchRef"];
            for p in path {
                at = &mut at[*p];
            }
            at["pageInfo"] = serde_json::json!({"hasNextPage": true});
            parse_main_checks(&v).unwrap_err().to_string()
        };
        assert!(truncated(&["rules"]).contains("truncated"));
        assert!(truncated(&["target", "statusCheckRollup", "contexts"]).contains("truncated"));
        let empty = serde_json::json!({"data": {"secret": "tok"}});
        assert!(!parse_main_checks(&empty)
            .unwrap_err()
            .to_string()
            .contains("tok"));
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
