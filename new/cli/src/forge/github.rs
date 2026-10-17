use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{ChangeRequest, ChangeState, Checks, Forge, Unit};

pub struct GitHub {
    program: OsString,
}

impl GitHub {
    pub fn new() -> GitHub {
        GitHub::with_program("gh")
    }

    pub fn with_program(program: impl Into<OsString>) -> GitHub {
        GitHub {
            program: program.into(),
        }
    }

    /// Runs `gh args` in `repo` and returns its stdout.
    fn gh(&self, repo: &Path, args: &[&str]) -> Result<String> {
        let out = Command::new(&self.program)
            .args(args)
            .current_dir(repo)
            .output()
            .context("cannot run gh")?;
        if !out.status.success() {
            bail!(
                "gh {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl Forge for GitHub {
    fn unit_get(&self, repo: &Path, number: u64) -> Result<Unit> {
        let json = self.gh(
            repo,
            &["api", &format!("repos/{{owner}}/{{repo}}/issues/{number}")],
        )?;
        let v: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
        unit_from(&v)
    }

    fn unit_labels_set(
        &self,
        repo: &Path,
        number: u64,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let number = number.to_string();
        let (add, remove) = (add.join(","), remove.join(","));
        let mut args = vec!["issue", "edit", &number];
        for (flag, labels) in [("--add-label", &add), ("--remove-label", &remove)] {
            if !labels.is_empty() {
                args.extend([flag, labels]);
            }
        }
        self.gh(repo, &args)?;
        Ok(())
    }

    fn unit_comment_create(&self, repo: &Path, number: u64, body: &str) -> Result<()> {
        self.gh(
            repo,
            &["issue", "comment", &number.to_string(), "--body", body],
        )?;
        Ok(())
    }

    fn unit_close(&self, repo: &Path, number: u64) -> Result<()> {
        self.gh(
            repo,
            &[
                "issue",
                "close",
                &number.to_string(),
                "--reason",
                "not planned",
            ],
        )?;
        Ok(())
    }

    fn unit_list(&self, repo: &Path) -> Result<Vec<Unit>> {
        let json = self.gh(
            repo,
            &[
                "api",
                "--paginate",
                "--slurp",
                "repos/{owner}/{repo}/issues?state=open&per_page=100",
            ],
        )?;
        let pages: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
        pages
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|page| page.as_array().into_iter().flatten())
            // The forge lists change requests among its issues.
            .filter(|v| v.get("pull_request").is_none())
            .map(unit_from)
            .collect()
    }

    fn change_request_get(&self, repo: &Path, branch: &str) -> Result<Option<ChangeRequest>> {
        let json = self.gh(
            repo,
            &[
                "pr",
                "list",
                "--head",
                branch,
                "--state",
                "all",
                "--limit",
                "1",
                "--json",
                "number,url,state",
            ],
        )?;
        let v: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
        let Some(pr) = v.as_array().and_then(|a| a.first()) else {
            return Ok(None);
        };
        let state = match pr["state"].as_str() {
            Some("OPEN") => ChangeState::Open,
            Some("MERGED") => ChangeState::Merged,
            Some("CLOSED") => ChangeState::Closed,
            other => bail!("unknown change request state {other:?}"),
        };
        Ok(Some(ChangeRequest {
            number: pr["number"]
                .as_u64()
                .context("change request has no number")?,
            url: text(&pr["url"]),
            state,
        }))
    }

    fn change_request_create(
        &self,
        repo: &Path,
        branch: &str,
        unit: u64,
        title: &str,
        body: &str,
    ) -> Result<ChangeRequest> {
        let body = format!("{body}\n\nCloses #{unit}\n");
        self.gh(
            repo,
            &[
                "pr", "create", "--head", branch, "--title", title, "--body", &body,
            ],
        )?;
        self.change_request_get(repo, branch)?
            .context("the change request was created but cannot be found")
    }

    fn change_request_checks_get(&self, repo: &Path, number: u64) -> Result<Checks> {
        // `gh pr checks` exits non-zero while checks are pending or failing, and prints no
        // JSON when none are reported.
        let out = Command::new(&self.program)
            .args(["pr", "checks", &number.to_string(), "--json", "name,bucket"])
            .current_dir(repo)
            .output()
            .context("cannot run gh")?;
        let Ok(v) = serde_json::from_slice::<Value>(&out.stdout) else {
            let said = String::from_utf8_lossy(&out.stderr);
            return Ok(if said.contains("no checks reported") {
                Checks::NoneReported
            } else {
                Checks::Pending
            });
        };
        let checks = v
            .as_array()
            .context("gh printed checks that are not a list")?;
        if checks.is_empty() {
            return Ok(Checks::NoneReported);
        }
        let named = |bucket: &[&str]| -> Vec<String> {
            checks
                .iter()
                .filter(|c| bucket.contains(&c["bucket"].as_str().unwrap_or_default()))
                .map(|c| text(&c["name"]))
                .collect()
        };
        let failed = named(&["fail", "cancel"]);
        Ok(if !failed.is_empty() {
            Checks::Failed(failed)
        } else if !named(&["pending"]).is_empty() {
            Checks::Pending
        } else {
            Checks::Passed
        })
    }

    fn change_request_merge(&self, repo: &Path, number: u64) -> Result<()> {
        self.gh(repo, &["pr", "merge", &number.to_string(), "--squash"])?;
        Ok(())
    }
}

fn unit_from(v: &Value) -> Result<Unit> {
    Ok(Unit {
        number: v["number"].as_u64().context("unit has no number")?,
        title: text(&v["title"]),
        body: text(&v["body"]),
        labels: v["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| text(&l["name"]))
            .collect(),
        from_team: matches!(
            v["author_association"].as_str(),
            Some("OWNER" | "MEMBER" | "COLLABORATOR")
        ),
    })
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A stand-in for `gh` that prints `stdout` whatever it is asked.
    fn fake(dir: &Path, stdout: &str) -> GitHub {
        let path = dir.join("gh");
        std::fs::write(&path, format!("#!/bin/sh\ncat <<'EOF'\n{stdout}\nEOF\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        GitHub::with_program(path)
    }

    #[test]
    fn a_unit_is_read_with_its_label_names() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(
            tmp.path(),
            r#"{"number":7,"title":"t","body":"b","labels":[{"name":"a"},{"name":"b"}],"author_association":"MEMBER"}"#,
        );
        let unit = gh.unit_get(tmp.path(), 7).unwrap();
        assert_eq!(
            unit,
            Unit {
                number: 7,
                title: "t".into(),
                body: "b".into(),
                labels: vec!["a".into(), "b".into()],
                from_team: true,
            }
        );
    }

    #[test]
    fn open_units_are_listed_across_pages_without_change_requests() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(
            tmp.path(),
            r#"[[{"number":1,"title":"a","labels":[],"author_association":"NONE"},
                 {"number":2,"title":"pr","labels":[],"pull_request":{}}],
                [{"number":3,"title":"c","labels":[],"author_association":"OWNER"}]]"#,
        );
        let units = gh.unit_list(tmp.path()).unwrap();
        let seen: Vec<_> = units.iter().map(|u| (u.number, u.from_team)).collect();
        assert_eq!(seen, [(1, false), (3, true)]);
    }

    #[test]
    fn a_branch_with_no_change_request_gives_none() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(tmp.path(), "[]");
        assert_eq!(gh.change_request_get(tmp.path(), "ns/u1").unwrap(), None);
    }

    #[test]
    fn a_merged_change_request_is_read() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(tmp.path(), r#"[{"number":9,"url":"u","state":"MERGED"}]"#);
        let pr = gh.change_request_get(tmp.path(), "ns/u1").unwrap().unwrap();
        assert_eq!(pr.number, 9);
        assert_eq!(pr.state, ChangeState::Merged);
    }
}
