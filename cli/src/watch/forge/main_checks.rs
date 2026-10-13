//! The default branch's head and checks, read from GitHub in one GraphQL query.

use anyhow::Result;
use serde_json::Value;

use super::head_sha;
use crate::watch::ci::{Check, HeadChecks, Outcome};

/// The default branch's name and head, the checks its rulesets require, and the head's check
/// runs and commit statuses, all in one query.
pub(super) const MAIN_CHECKS_QUERY: &str = "query($owner: String!, $repo: String!) {
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

/// The head and checks in `MAIN_CHECKS_QUERY`'s answer. A check is required when a ruleset names
/// it; when no ruleset names any check on the head (classic branch protection, which a reader
/// can't see, or none), every check is.
pub(super) fn parse_main_checks(v: &Value) -> Result<HeadChecks> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
