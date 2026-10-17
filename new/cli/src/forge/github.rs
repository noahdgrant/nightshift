use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{ChangeRequest, ChangeState, Forge, Unit};

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
            &[
                "issue",
                "view",
                &number.to_string(),
                "--json",
                "number,title,body,labels",
            ],
        )?;
        let v: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
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
        })
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

    fn change_request_merge(&self, repo: &Path, number: u64) -> Result<()> {
        self.gh(repo, &["pr", "merge", &number.to_string(), "--squash"])?;
        Ok(())
    }
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
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(
            tmp.path(),
            r#"{"number":7,"title":"t","body":"b","labels":[{"name":"a"},{"name":"b"}]}"#,
        );
        let unit = gh.unit_get(tmp.path(), 7).unwrap();
        assert_eq!(
            unit,
            Unit {
                number: 7,
                title: "t".into(),
                body: "b".into(),
                labels: vec!["a".into(), "b".into()],
            }
        );
    }

    #[test]
    fn a_branch_with_no_change_request_gives_none() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(tmp.path(), "[]");
        assert_eq!(gh.change_request_get(tmp.path(), "ns/u1").unwrap(), None);
    }

    #[test]
    fn a_merged_change_request_is_read() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(tmp.path(), r#"[{"number":9,"url":"u","state":"MERGED"}]"#);
        let pr = gh.change_request_get(tmp.path(), "ns/u1").unwrap().unwrap();
        assert_eq!(pr.number, 9);
        assert_eq!(pr.state, ChangeState::Merged);
    }
}
