//! GitLab through `glab`. A GitLab issue carries no author association, so the team is read from
//! the project's members instead (docs/FACTORY.md, Trust).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde_json::Value;

use super::{ClosingPrs, Forge, PrState};
use crate::config::ForgeKind;
use crate::error::SfError;
use crate::watch::ci::{Check, HeadChecks, Outcome};
use crate::watch::{closed_by, Issue};

/// Developer, the lowest access level on the team. Reporter (20), Planner (15) and Guest (10) are
/// outside it.
const DEVELOPER: u64 = 30;

pub struct GitLab {
    pub(super) root: PathBuf,
}

fn glab(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = crate::git::scrub(&mut Command::new("glab"))
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .context("cannot run glab; is it installed and on PATH?")?;
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

impl GitLab {
    fn list(&self, path: &str) -> Result<Vec<Value>> {
        items(&glab(&self.root, &["api", "--paginate", path])?)
    }

    fn get(&self, path: &str) -> Result<Value> {
        let text = glab(&self.root, &["api", path])?;
        serde_json::from_str(text.trim()).with_context(|| format!("glab api {path} gave bad JSON"))
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
            .map(|i| Issue {
                number: i["iid"].as_u64().unwrap_or(0),
                title: i["title"].as_str().unwrap_or("").to_string(),
                labels: names(&i["labels"]),
                body: i["description"].as_str().unwrap_or("").to_string(),
                team: i["author"]["id"]
                    .as_u64()
                    .is_some_and(|id| team.contains(&id)),
            })
            .collect())
    }

    /// The 100 most recently updated MRs in `state`, as `gh`'s list for GitHub is capped too.
    fn closing_prs(&self, state: PrState) -> Result<ClosingPrs> {
        let state = match state {
            PrState::Open => "opened",
            PrState::Merged => "merged",
        };
        let path =
            format!("projects/:id/merge_requests?state={state}&order_by=updated_at&per_page=100");
        let mut by_issue = BTreeMap::new();
        for mr in items(&glab(&self.root, &["api", &path])?)? {
            for n in closed_by(mr["description"].as_str().unwrap_or("")) {
                // A fork's branch name says nothing about this project's units.
                let own = mr["source_project_id"].is_u64()
                    && mr["source_project_id"] == mr["target_project_id"];
                let head = if own {
                    mr["source_branch"].as_str().unwrap_or("").to_string()
                } else {
                    String::new()
                };
                by_issue.insert(n, (mr["iid"].as_u64().unwrap_or(0), head));
            }
        }
        Ok(by_issue)
    }

    fn is_closed(&self, n: u64) -> Result<bool> {
        Ok(self.issue(n)?["state"].as_str() == Some("closed"))
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

    fn comment(&self, n: u64, body: &str) -> Result<()> {
        glab(
            &self.root,
            &["issue", "note", &n.to_string(), "--message", body],
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

    fn comment_pr(&self, _: u64, _: &str) -> Result<()> {
        Ok(())
    }

    /// The latest commit status of each job in the head's pipelines, and of each external status.
    fn main_checks(&self) -> Result<HeadChecks> {
        let project = self.get("projects/:id")?;
        let Some(branch) = project["default_branch"].as_str() else {
            anyhow::bail!("GitLab names no default branch");
        };
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glab_that_cannot_start_says_so() {
        let e = glab(Path::new("/nonexistent/dir"), &["version"]).unwrap_err();
        assert!(format!("{e:#}").contains("cannot run glab"), "{e:#}");
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
    }

    #[test]
    fn statuses_path_takes_only_a_commit_id() {
        let sha = "a".repeat(40);
        assert_eq!(
            statuses_path(&sha, "dev/x").unwrap(),
            format!("projects/:id/repository/commits/{sha}/statuses?ref=dev%2Fx&per_page=100")
        );
        for bad in ["", "abc", "../../x", "a/../b?x=1", &format!("{sha}\n")] {
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
