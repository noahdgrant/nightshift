//! GitLab through `glab`. A GitLab issue carries no author association, so the team is read from
//! the project's members instead (docs/FACTORY.md, Trust).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use super::{ClosingPrs, Filed, Forge, IssueState, MergedPr, MergedPrs, Post, PrState};
use crate::config::ForgeKind;
use crate::error::SfError;
use crate::proc::{Kind, Proc};
use crate::watch::ci::{Check, HeadChecks, Outcome};
use crate::watch::{closed_by, Issue};

/// Developer, the lowest access level on the team. Reporter (20), Planner (15) and Guest (10) are
/// outside it.
const DEVELOPER: u64 = 30;

pub struct GitLab {
    pub(super) root: PathBuf,
}

fn glab(cwd: &Path, args: &[&str]) -> Result<String> {
    glab_within(cwd, args, Kind::Forge.timeout())
}

fn glab_within(cwd: &Path, args: &[&str], limit: Duration) -> Result<String> {
    let out = Proc::forge("glab")
        .args(args)
        .current_dir(cwd)
        .timeout(limit)
        .until_stop()
        .output()
        .map_err(glab_unrunnable)?;
    if !out.status.success() {
        let token = std::env::var(ForgeKind::Gitlab.token_var()).ok();
        return Err(SfError::general(format!(
            "glab {} failed: {}",
            args.join(" "),
            redact(
                String::from_utf8_lossy(&out.stderr).trim(),
                token.as_deref()
            )
        ))
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn glab_unrunnable(e: std::io::Error) -> anyhow::Error {
    match e.kind() {
        std::io::ErrorKind::Interrupted | std::io::ErrorKind::TimedOut => e.into(),
        _ => anyhow::Error::new(e).context("cannot run glab; is it installed and on PATH?"),
    }
}

/// `text` with every copy of `token` hidden: a server may echo the credential it refused.
fn redact(text: &str, token: Option<&str>) -> String {
    match token.filter(|t| !t.is_empty()) {
        Some(t) => text.replace(t, "[token]"),
        None => text.to_string(),
    }
}

/// The items of the JSON arrays `glab api` prints, one per page.
fn items(text: &str) -> Result<Vec<Value>> {
    let mut all = Vec::new();
    for page in serde_json::Deserializer::from_str(text).into_iter::<Vec<Value>>() {
        all.extend(page.context("glab api gave something other than JSON arrays")?);
    }
    Ok(all)
}

/// `s` with everything but RFC 3986 unreserved characters as `%XX`, for a query value.
fn encode(s: &str) -> String {
    s.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        out
    })
}

fn names(labels: &Value) -> Vec<String> {
    labels
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l.as_str().map(String::from))
        .collect()
}

/// A GitLab issue as `ns watch` reads it, team-authored when `team` holds its author.
fn to_issue(i: &Value, team: &BTreeSet<u64>) -> Issue {
    Issue {
        number: i["iid"].as_u64().unwrap_or(0),
        title: i["title"].as_str().unwrap_or("").to_string(),
        labels: names(&i["labels"]),
        body: i["description"].as_str().unwrap_or("").to_string(),
        team: i["author"]["id"]
            .as_u64()
            .is_some_and(|id| team.contains(&id)),
    }
}

/// GitLab records no close reason, so a closed issue counts as completed. One with no opening
/// time it can read is left out.
fn to_filed(i: &Value, team: &BTreeSet<u64>) -> Option<Filed> {
    Some(Filed {
        issue: to_issue(i, team),
        opened: time(i["created_at"].as_str()?)?,
        state: match i["state"].as_str() {
            Some("closed") => IssueState::Completed,
            _ => IssueState::Open,
        },
    })
}

/// How many merge requests one list reads.
const MR_LIMIT: usize = 100;

/// The merged MRs in one list, truncated when it is full.
fn to_merged_prs(all: &[Value]) -> MergedPrs {
    MergedPrs {
        prs: all.iter().filter_map(to_merged).collect(),
        truncated: all.len() >= MR_LIMIT,
    }
}

/// The API path listing the merged MRs from `since` on.
fn merged_path(since: Option<i64>) -> String {
    let after = since.map_or(String::new(), |t| {
        format!("&merged_after={}", crate::clock::iso(t).replace(':', "%3A"))
    });
    format!(
        "projects/:id/merge_requests?state=merged&order_by=created_at&per_page={MR_LIMIT}{after}"
    )
}

/// An MR's source branch, empty for a fork's: its name says nothing about this project's units.
fn own_branch(mr: &Value) -> String {
    let own =
        mr["source_project_id"].is_u64() && mr["source_project_id"] == mr["target_project_id"];
    if own {
        mr["source_branch"].as_str().unwrap_or("").to_string()
    } else {
        String::new()
    }
}

/// A merged MR, its source branch empty when it came from a fork. One with no merge time it can
/// read is left out.
fn to_merged(mr: &Value) -> Option<MergedPr> {
    Some(MergedPr {
        number: mr["iid"].as_u64()?,
        head: own_branch(mr),
        merged: time(mr["merged_at"].as_str()?)?,
        changed: None,
    })
}

/// GitLab's `2026-10-05T10:00:00.123Z` in unix seconds, the fraction dropped.
fn time(s: &str) -> Option<i64> {
    let whole = match (s.get(..19), s.get(19..).and_then(|r| r.strip_prefix('.'))) {
        (Some(head), Some(frac)) => format!(
            "{head}{}",
            frac.trim_start_matches(|c: char| c.is_ascii_digit())
        ),
        _ => s.to_string(),
    };
    crate::clock::parse_iso(&whole)
}

impl GitLab {
    fn list(&self, path: &str) -> Result<Vec<Value>> {
        items(&glab(&self.root, &["api", "--paginate", path])?)
    }

    fn get(&self, path: &str) -> Result<Value> {
        let text = glab(&self.root, &["api", path])?;
        serde_json::from_str(text.trim()).with_context(|| format!("glab api {path} gave bad JSON"))
    }

    fn default_branch(&self) -> Result<String> {
        let project = self.get("projects/:id")?;
        match project["default_branch"].as_str() {
            Some(b) => Ok(b.to_string()),
            None => anyhow::bail!("GitLab names no default branch"),
        }
    }

    fn issue(&self, n: u64) -> Result<Value> {
        self.get(&format!("projects/:id/issues/{n}"))
    }

    /// Ids of the active members, inherited ones too, at Developer or above.
    fn team(&self) -> Result<BTreeSet<u64>> {
        Ok(self
            .list("projects/:id/members/all?per_page=100")?
            .iter()
            .filter(|m| m["state"].as_str().is_none_or(|s| s == "active"))
            .filter(|m| m["access_level"].as_u64().unwrap_or(0) >= DEVELOPER)
            .filter_map(|m| m["id"].as_u64())
            .collect())
    }
}

/// A commit status, one per pipeline job or external status: required unless it may fail.
fn status_check(s: &Value) -> Check {
    let outcome = match s["status"].as_str().unwrap_or("") {
        "success" | "skipped" | "manual" => Outcome::Pass,
        "failed" | "canceled" => Outcome::Fail,
        _ => Outcome::Running,
    };
    Check {
        name: s["name"].as_str().unwrap_or("").to_string(),
        outcome,
        required: !s["allow_failure"].as_bool().unwrap_or(false),
        // A pipeline job's status shares the job's id; an external status has no trace.
        log: s["id"].as_u64().map(|id| id.to_string()),
    }
}

fn statuses_path(sha: &str, branch: &str) -> Result<String> {
    Ok(format!(
        "projects/:id/repository/commits/{}/statuses?ref={}&per_page=100",
        super::head_sha(sha)?,
        encode(branch)
    ))
}

impl Forge for GitLab {
    fn open_issues(&self, label: Option<&str>) -> Result<Vec<Issue>> {
        let team = self.team()?;
        let filter = label.map_or(String::new(), |l| format!("&labels={}", encode(l)));
        let path = format!("projects/:id/issues?state=opened{filter}&per_page=100");
        Ok(self
            .list(&path)?
            .iter()
            .map(|i| to_issue(i, &team))
            .collect())
    }

    fn labelled_issues(&self, label: &str) -> Result<Vec<Filed>> {
        let team = self.team()?;
        let path = format!(
            "projects/:id/issues?state=all&labels={}&per_page=100",
            encode(label)
        );
        Ok(self
            .list(&path)?
            .iter()
            .filter_map(|i| to_filed(i, &team))
            .collect())
    }

    /// The 100 most recently updated MRs in `state`, as `gh`'s list for GitHub is capped too.
    fn closing_prs(&self, state: PrState) -> Result<ClosingPrs> {
        let state = match state {
            PrState::Open => "opened",
            PrState::Merged => "merged",
        };
        let path = format!(
            "projects/:id/merge_requests?state={state}&order_by=updated_at&per_page={MR_LIMIT}"
        );
        let mut by_issue = BTreeMap::new();
        for mr in items(&glab(&self.root, &["api", &path])?)? {
            for n in closed_by(mr["description"].as_str().unwrap_or("")) {
                by_issue.insert(n, (mr["iid"].as_u64().unwrap_or(0), own_branch(&mr)));
            }
        }
        Ok(by_issue)
    }

    /// The merged MRs from `since` on, up to 100. GitLab's list gives no line counts.
    fn merged_prs(&self, since: Option<i64>) -> Result<MergedPrs> {
        let path = merged_path(since);
        Ok(to_merged_prs(&items(&glab(&self.root, &["api", &path])?)?))
    }

    /// `n` is always an issue iid: a merge request is `!n`, numbered apart, so it never blocks.
    fn is_closed(&self, n: u64) -> Result<bool> {
        Ok(self.issue(n)?["state"].as_str() == Some("closed"))
    }

    /// GitLab records no close reason, so a closed issue counts as completed.
    fn state(&self, n: u64) -> Result<IssueState> {
        Ok(match self.issue(n)?["state"].as_str() {
            Some("closed") => IssueState::Completed,
            _ => IssueState::Open,
        })
    }

    /// GitLab's child items are work-item links the REST API doesn't list, so none are read: a
    /// split parent's children come from its notes and `Part of` lines instead.
    fn sub_issues(&self, _: u64) -> Result<Vec<u64>> {
        Ok(Vec::new())
    }

    fn team_comments(&self, n: u64) -> Result<Vec<String>> {
        let team = self.team()?;
        let path = format!("projects/:id/issues/{n}/notes?sort=asc&per_page=100");
        Ok(self
            .list(&path)?
            .iter()
            .filter(|c| c["system"].as_bool() != Some(true))
            .filter(|c| {
                c["author"]["id"]
                    .as_u64()
                    .is_some_and(|id| team.contains(&id))
            })
            .filter_map(|c| c["body"].as_str().map(String::from))
            .collect())
    }

    fn part_of(&self, n: u64) -> Result<Vec<Issue>> {
        let team = self.team()?;
        let path = format!(
            "projects/:id/issues?state=all&in=description&search={}&per_page=100",
            encode(&format!("Part of #{n}"))
        );
        Ok(self
            .list(&path)?
            .iter()
            .map(|i| to_issue(i, &team))
            .collect())
    }

    /// GitLab creates a label an issue edit names.
    fn ensure_label(&self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }

    fn labels(&self, n: u64) -> Result<Vec<String>> {
        Ok(names(&self.issue(n)?["labels"]))
    }

    fn edit_labels(&self, n: u64, remove: &[String], add: Option<&str>) -> Result<()> {
        let ns = n.to_string();
        let mut args = vec!["issue", "update", ns.as_str()];
        for l in remove {
            args.extend(["--unlabel", l.as_str()]);
        }
        if let Some(a) = add {
            args.extend(["--label", a]);
        }
        glab(&self.root, &args).map(|_| ())
    }

    fn comment(&self, n: u64, body: &Post) -> Result<()> {
        glab(
            &self.root,
            &["issue", "note", &n.to_string(), "--message", body.as_str()],
        )
        .map(|_| ())
    }

    fn close(&self, n: u64) -> Result<()> {
        glab(&self.root, &["issue", "close", &n.to_string()]).map(|_| ())
    }

    // Merge requests come with the glab forge work (#322-#324); until then they are left alone.
    fn add_pr_label(&self, _: u64, _: &str) -> Result<()> {
        Ok(())
    }

    fn remove_pr_label(&self, _: u64, _: &str) -> Result<()> {
        Ok(())
    }

    fn comment_pr(&self, _: u64, _: &Post) -> Result<()> {
        Ok(())
    }

    /// The latest commit status of each job in the head's pipelines, and of each external status.
    fn main_checks(&self) -> Result<HeadChecks> {
        let branch = &self.default_branch()?;
        let head = self.get(&format!(
            "projects/:id/repository/branches/{}",
            encode(branch)
        ))?;
        let Some(sha) = head["commit"]["id"].as_str() else {
            anyhow::bail!("GitLab names no head for {branch}");
        };
        let path = statuses_path(sha, branch)?;
        Ok(HeadChecks {
            sha: sha.to_string(),
            checks: self.list(&path)?.iter().map(status_check).collect(),
        })
    }

    /// GitLab makes a label it doesn't have yet when an issue names it.
    fn create_issue(&self, title: &str, body: &Post, labels: &[&str]) -> Result<u64> {
        let fields = [
            format!("title={title}"),
            format!("description={}", body.as_str()),
            format!("labels={}", labels.join(",")),
        ];
        let mut args = vec!["api", "--method", "POST", "projects/:id/issues"];
        for f in &fields {
            args.extend(["--raw-field", f.as_str()]);
        }
        let text = glab(&self.root, &args)?;
        let v: Value = serde_json::from_str(text.trim()).context("glab gave bad JSON")?;
        v["iid"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("GitLab named no number for the new issue"))
    }

    fn check_log(&self, log: &str) -> Result<String> {
        let id: u64 = log.parse().context("a GitLab job id is a number")?;
        glab(
            &self.root,
            &["api", &format!("projects/:id/jobs/{id}/trace")],
        )
    }

    fn last_green(&self, red: &str) -> Result<Option<String>> {
        let branch = self.default_branch()?;
        let path = format!(
            "projects/:id/pipelines?ref={}&status=success&per_page=30",
            encode(&branch)
        );
        let v = self.get(&path)?;
        Ok(super::first_other(&v, "sha", red))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_is_not_reported_as_a_missing_glab() {
        let stopped = std::io::Error::new(std::io::ErrorKind::Interrupted, "`glab api` stopped");
        assert_eq!(glab_unrunnable(stopped).to_string(), "`glab api` stopped");
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(
            glab_unrunnable(missing).to_string(),
            "cannot run glab; is it installed and on PATH?"
        );
    }

    const STOP_MARK: &str = "NS_GLAB_STOP_PROBE";

    #[test]
    fn a_stop_ends_glab_as_interrupted_not_as_a_missing_glab() {
        crate::testutil::probe("watch::forge::gitlab::tests::stop_probe", STOP_MARK, "1");
    }

    #[test]
    fn stop_probe() {
        if std::env::var_os(STOP_MARK).is_none() {
            return;
        }
        let dir = crate::testutil::fake_on_path("glab", "exec sleep 20");
        crate::stop::request(libc::SIGTERM);
        let e = glab(dir.path(), &["api", "user"]).unwrap_err();
        let io = e.downcast_ref::<std::io::Error>().expect("an io error");
        assert_eq!(io.kind(), std::io::ErrorKind::Interrupted);
        assert!(e.to_string().ends_with("stopped by a signal"), "{e}");
        assert!(e.to_string().contains("glab api user"), "{e}");
    }

    const LIMIT_MARK: &str = "NS_GLAB_LIMIT_PROBE";

    #[test]
    fn a_glab_past_its_limit_reports_the_timeout_naming_the_command() {
        crate::testutil::probe("watch::forge::gitlab::tests::limit_probe", LIMIT_MARK, "1");
    }

    #[test]
    fn limit_probe() {
        if std::env::var_os(LIMIT_MARK).is_none() {
            return;
        }
        let _glab = crate::testutil::fake_on_path("glab", "exec sleep 600");
        let e = glab_within(
            Path::new("."),
            &["api", "projects/:id"],
            std::time::Duration::ZERO,
        )
        .unwrap_err();
        let io = e.downcast_ref::<std::io::Error>().expect("an io error");
        assert_eq!(io.kind(), std::io::ErrorKind::TimedOut);
        assert!(e.to_string().contains("glab api projects/:id"), "{e}");
        assert!(e.to_string().contains("timed out"), "{e}");
        assert!(!e.to_string().contains("cannot run glab"), "{e}");
    }

    #[test]
    fn to_merged_prs_reads_each_mrs_branch_and_merge_time_but_no_lines() {
        let mrs = serde_json::json!([
            {"iid": 4, "source_branch": "ns/9-b", "source_project_id": 1, "target_project_id": 1,
             "merged_at": "2026-10-05T10:00:00.123Z"},
            {"iid": 5, "source_branch": "ns/9-b", "source_project_id": 2, "target_project_id": 1,
             "merged_at": "2026-10-06T00:00:00Z"},
            {"iid": 6, "source_branch": "x", "merged_at": null},
            {"iid": 7, "source_branch": "ns/9-b", "merged_at": "2026-10-07T00:00:00Z"},
        ]);
        let m = to_merged_prs(mrs.as_array().unwrap());
        let at = |s| crate::clock::parse_iso(s).unwrap();
        assert_eq!(
            m.prs,
            [
                MergedPr {
                    number: 4,
                    head: "ns/9-b".into(),
                    merged: at("2026-10-05T10:00:00Z"),
                    changed: None
                },
                MergedPr {
                    number: 5,
                    head: String::new(),
                    merged: at("2026-10-06T00:00:00Z"),
                    changed: None
                },
                MergedPr {
                    number: 7,
                    head: String::new(),
                    merged: at("2026-10-07T00:00:00Z"),
                    changed: None
                },
            ]
        );
        assert!(!m.truncated);
        let full = vec![serde_json::json!({}); MR_LIMIT];
        assert!(to_merged_prs(&full).truncated);
        assert!(!to_merged_prs(&full[1..]).truncated);
    }

    #[test]
    fn the_merged_list_asks_for_100_merged_after_the_start() {
        assert_eq!(MR_LIMIT, 100);
        assert_eq!(
            merged_path(None),
            "projects/:id/merge_requests?state=merged&order_by=created_at&per_page=100"
        );
        let since = crate::clock::parse_iso("2026-09-07").unwrap();
        assert_eq!(
            merged_path(Some(since)),
            "projects/:id/merge_requests?state=merged&order_by=created_at&per_page=100\
             &merged_after=2026-09-07T00%3A00%3A00Z"
        );
    }

    #[test]
    fn glab_that_cannot_start_says_so() {
        let e = glab(Path::new("/nonexistent/dir"), &["version"]).unwrap_err();
        assert!(format!("{e:#}").contains("cannot run glab"), "{e:#}");
    }

    #[test]
    fn to_filed_reads_the_opening_time_and_counts_a_closed_issue_completed() {
        let team = BTreeSet::from([5]);
        let i = serde_json::json!({"iid": 4, "title": "fix: x", "labels": ["type:fix"], "description": "d", "author": {"id": 5}, "created_at": "2026-10-05T10:00:00.000Z", "state": "closed"});
        let f = to_filed(&i, &team).unwrap();
        assert_eq!((f.issue.number, f.issue.team), (4, true));
        assert_eq!(
            f.opened,
            crate::clock::parse_iso("2026-10-05T10:00:00Z").unwrap()
        );
        assert_eq!(f.state, IssueState::Completed);
        let open =
            serde_json::json!({"iid": 5, "created_at": "2026-10-05T10:00:00Z", "state": "opened"});
        assert_eq!(to_filed(&open, &team).unwrap().state, IssueState::Open);
        assert!(to_filed(&serde_json::json!({"iid": 6, "state": "opened"}), &team).is_none());
        let at = crate::clock::parse_iso("2026-10-05T10:00:00Z");
        assert_eq!(time("2026-10-05T10:00:00Z"), at);
        assert_eq!(time("2026-10-05T12:00:00.5+02:00"), at);
        assert_eq!(time("2026-10-05T10:00:00."), at);
        assert_eq!(time("2026-10-05"), crate::clock::parse_iso("2026-10-05"));
    }

    #[test]
    fn redact_hides_every_copy_of_the_token() {
        assert_eq!(redact("tok and tok", Some("tok")), "[token] and [token]");
        assert_eq!(redact("no token", None), "no token");
        assert_eq!(redact("empty", Some("")), "empty");
    }

    #[test]
    fn a_status_is_required_unless_it_may_fail() {
        let check = |status: &str, allow: Value| {
            let c = status_check(
                &serde_json::json!({"name": "j", "status": status, "allow_failure": allow}),
            );
            (c.outcome, c.required)
        };
        let t = Value::Bool(true);
        for s in ["success", "skipped", "manual"] {
            assert_eq!(check(s, Value::Null), (Outcome::Pass, true), "{s}");
        }
        for s in ["failed", "canceled"] {
            assert_eq!(check(s, Value::Bool(false)), (Outcome::Fail, true), "{s}");
        }
        for s in [
            "created",
            "pending",
            "running",
            "waiting_for_resource",
            "preparing",
            "scheduled",
        ] {
            assert_eq!(check(s, Value::Null), (Outcome::Running, true), "{s}");
        }
        assert_eq!(check("failed", t), (Outcome::Fail, false));
        let log = |s: Value| status_check(&s).log;
        assert_eq!(log(serde_json::json!({"id": 7})).as_deref(), Some("7"));
        assert_eq!(log(serde_json::json!({})), None);
    }

    #[test]
    fn statuses_path_takes_only_a_commit_id() {
        let sha = "a".repeat(40);
        assert_eq!(
            statuses_path(&sha, "dev/x").unwrap(),
            format!("projects/:id/repository/commits/{sha}/statuses?ref=dev%2Fx&per_page=100")
        );
        let bad = ["", "abc", "../../x", "a/../b?x=1"].map(String::from);
        let wrong_shape = [
            format!("{sha}\n"),
            format!("{}/", "a".repeat(39)),
            format!("{}..", "a".repeat(38)),
            "a".repeat(41),
            "a".repeat(63),
            "a".repeat(65),
        ];
        for bad in bad.iter().chain(&wrong_shape) {
            assert!(statuses_path(bad, "main").is_err(), "{bad}");
        }
    }

    #[test]
    fn items_joins_pages_and_refuses_anything_else() {
        let v = items("[{\"a\":1}]\n[{\"a\":2},{\"a\":3}][]").unwrap();
        assert_eq!(v.len(), 3);
        assert!(items("").unwrap().is_empty());
        assert!(items("{\"message\":\"404\"}").is_err());
    }
}
