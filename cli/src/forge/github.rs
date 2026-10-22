use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{ChangeRequest, ChangeState, Checks, Forge, Unit};
use crate::{config, retry};

/// What `gh` says when a call failed for a reason that can pass: the network, the forge having a
/// bad moment, or a rate limit. Any other error is permanent, or unknown, and is not repeated.
const TRANSIENT: [&str; 11] = [
    "http 5",
    "http 429",
    "rate limit",
    "timeout",
    "timed out",
    "error connecting",
    "connection",
    "dial tcp",
    "no such host",
    "tls handshake",
    "unexpected eof",
];

pub struct GitHub {
    program: OsString,
    /// How long to wait before the second attempt of a call. Each later wait is twice as long.
    retry_delay: Duration,
    /// The token of the account named in the config, got once when the first call needs it.
    token: OnceLock<std::result::Result<Option<String>, String>>,
}

impl GitHub {
    pub fn new() -> GitHub {
        GitHub::with_program("gh")
    }

    pub fn with_program(program: impl Into<OsString>) -> GitHub {
        GitHub {
            program: program.into(),
            retry_delay: retry::DELAY,
            token: OnceLock::new(),
        }
    }

    /// The token `gh` should use: the login of the account the config names. None when a
    /// `GH_TOKEN` is already set, which wins, or when the config names no account.
    fn token(&self) -> Result<Option<&str>> {
        let found = self.token.get_or_init(|| {
            if std::env::var_os("GH_TOKEN").is_some() {
                return Ok(None);
            }
            let account = config::github_account().map_err(|e| format!("{e:#}"))?;
            let Some(account) = account else {
                return Ok(None);
            };
            let out = Command::new(&self.program)
                .args(["auth", "token", "--user", &account])
                .output()
                .map_err(|e| format!("cannot run gh: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "gh has no login for the account {account} named in the config: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Ok(Some(
                String::from_utf8_lossy(&out.stdout).trim().to_string(),
            ))
        });
        match found {
            Ok(token) => Ok(token.as_deref()),
            Err(e) => bail!("{e}"),
        }
    }

    /// Runs `gh args` in `repo` and returns its stdout, trying again when it fails. For calls
    /// that are safe to repeat: reads, and writes that leave the same result when made twice. Only a
    /// transient failure is repeated.
    fn gh(&self, repo: &Path, args: &[&str]) -> Result<String> {
        retry::retry(
            self.retry_delay,
            || self.gh_once(repo, args),
            |r| retry::error_says(r, &TRANSIENT),
        )
    }

    /// Runs `gh args` in `repo` once and returns its stdout.
    fn gh_once(&self, repo: &Path, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new(&self.program);
        cmd.args(args).current_dir(repo);
        if let Some(token) = self.token()? {
            cmd.env("GH_TOKEN", token);
        }
        let out = cmd.output().context("cannot run gh")?;
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
    fn login_get(&self, repo: &Path) -> Result<String> {
        let json = self.gh(repo, &["api", "user"])?;
        let v: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
        let login = v["login"].as_str().context("gh did not name the login")?;
        if let Some(account) = config::github_account()? {
            if account != login {
                bail!("the forge sees {login} but the config names {account}");
            }
        }
        Ok(login.to_string())
    }

    fn labels_list(&self, repo: &Path) -> Result<Vec<String>> {
        let json = self.gh(
            repo,
            &["label", "list", "--limit", "1000", "--json", "name"],
        )?;
        let v: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
        Ok(v.as_array()
            .context("gh did not print a list of labels")?
            .iter()
            .filter_map(|l| l["name"].as_str().map(str::to_string))
            .collect())
    }

    fn label_create(&self, repo: &Path, name: &str, description: &str) -> Result<()> {
        self.gh(
            repo,
            &["label", "create", name, "--description", description],
        )?;
        Ok(())
    }

    fn release_download(
        &self,
        source: &str,
        tag: &str,
        patterns: &[&str],
        dir: &Path,
    ) -> Result<()> {
        let mut args = vec!["release", "download", tag, "--repo", source, "--dir", "."];
        for pattern in patterns {
            args.extend(["--pattern", pattern]);
        }
        self.gh(dir, &args)?;
        Ok(())
    }

    fn unit_get(&self, repo: &Path, number: u64) -> Result<Unit> {
        let json = self.gh(
            repo,
            &["api", &format!("repos/{{owner}}/{{repo}}/issues/{number}")],
        )?;
        let v: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
        unit_from(&v)
    }

    fn unit_types_list(&self, repo: &Path) -> Result<Vec<String>> {
        // Only an organization with types turned on answers; a user-owned repository is a 404.
        let json = self.gh(repo, &["api", "orgs/{owner}/issue-types"])?;
        let v: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
        Ok(v.as_array()
            .context("gh did not print a list of types")?
            .iter()
            .filter(|t| t["is_enabled"] != false)
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect())
    }

    fn unit_type_set(&self, repo: &Path, number: u64, name: &str) -> Result<()> {
        self.gh(
            repo,
            &[
                "api",
                "--method",
                "PATCH",
                &format!("repos/{{owner}}/{{repo}}/issues/{number}"),
                "-f",
                &format!("type={name}"),
            ],
        )?;
        Ok(())
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

    fn unit_body_set(&self, repo: &Path, number: u64, body: &str) -> Result<()> {
        self.gh(
            repo,
            &["issue", "edit", &number.to_string(), "--body", body],
        )?;
        Ok(())
    }

    fn unit_comment_add(&self, repo: &Path, number: u64, body: &str) -> Result<()> {
        self.gh(
            repo,
            &["issue", "comment", &number.to_string(), "--body", body],
        )?;
        Ok(())
    }

    fn unit_assignee_add(&self, repo: &Path, number: u64) -> Result<()> {
        // `@me` is the account `gh` runs as, which `gh` takes from `GH_TOKEN`.
        self.gh(
            repo,
            &[
                "issue",
                "edit",
                &number.to_string(),
                "--add-assignee",
                "@me",
            ],
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

    fn unit_blockers_list(&self, repo: &Path, number: u64) -> Result<Vec<u64>> {
        let json = self.gh(
            repo,
            &[
                "api",
                "--paginate",
                "--slurp",
                &format!(
                    "repos/{{owner}}/{{repo}}/issues/{number}/dependencies/blocked_by?per_page=100"
                ),
            ],
        )?;
        let pages: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
        Ok(pages
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|page| page.as_array().into_iter().flatten())
            .filter(|v| v["state"] == "open")
            .filter_map(|v| v["number"].as_u64())
            .collect())
    }

    fn unit_blocker_add(&self, repo: &Path, number: u64, blocker: u64) -> Result<()> {
        // The relationship names the blocker by its id, which is not its number.
        let json = self.gh(
            repo,
            &["api", &format!("repos/{{owner}}/{{repo}}/issues/{blocker}")],
        )?;
        let v: Value = serde_json::from_str(&json).context("gh printed invalid JSON")?;
        let id = v["id"].as_u64().context("gh did not give the unit's id")?;
        let result = self.gh(
            repo,
            &[
                "api",
                "--method",
                "POST",
                &format!("repos/{{owner}}/{{repo}}/issues/{number}/dependencies/blocked_by"),
                "-F",
                &format!("issue_id={id}"),
            ],
        );
        match result {
            Err(e) if format!("{e:#}").to_lowercase().contains("already") => Ok(()),
            other => other.map(|_| ()),
        }
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
        labels: &[String],
    ) -> Result<ChangeRequest> {
        let body = format!("{body}\n\nCloses #{unit}\n");
        let mut args = vec![
            "pr", "create", "--head", branch, "--title", title, "--body", &body,
        ];
        for label in labels {
            args.extend(["--label", label]);
        }
        // Not repeated: a call that did succeed but reported an error would open a second one.
        self.gh_once(repo, &args)?;
        self.change_request_get(repo, branch)?
            .context("the change request was created but cannot be found")
    }

    fn change_request_assignee_add(&self, repo: &Path, number: u64) -> Result<()> {
        self.gh(
            repo,
            &["pr", "edit", &number.to_string(), "--add-assignee", "@me"],
        )?;
        Ok(())
    }

    fn change_request_label_add(&self, repo: &Path, number: u64, label: &str) -> Result<()> {
        self.gh(
            repo,
            &["pr", "edit", &number.to_string(), "--add-label", label],
        )?;
        Ok(())
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
        // Not repeated: a second merge of a merged change request is an error.
        self.gh_once(repo, &["pr", "merge", &number.to_string(), "--squash"])?;
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
    fn the_enabled_native_types_of_an_organization_are_listed() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(
            tmp.path(),
            r#"[{"name":"Bug","is_enabled":true},{"name":"Feature"},{"name":"Old","is_enabled":false}]"#,
        );
        assert_eq!(gh.unit_types_list(tmp.path()).unwrap(), ["Bug", "Feature"]);
    }

    #[test]
    fn a_list_of_native_types_that_is_not_a_list_is_an_error() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(tmp.path(), r#"{"message":"Not Found"}"#);
        assert!(gh.unit_types_list(tmp.path()).is_err());
    }

    #[test]
    fn a_unit_type_is_set_with_a_patch_of_its_name() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gh");
        let log = tmp.path().join("log");
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho \"$@\" > \"{}\"\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        GitHub::with_program(path)
            .unit_type_set(tmp.path(), 7, "Bug")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(log).unwrap().trim(),
            "api --method PATCH repos/{owner}/{repo}/issues/7 -f type=Bug"
        );
    }

    #[test]
    fn only_the_open_blockers_of_a_unit_are_listed() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = fake(
            tmp.path(),
            r#"[[{"number":4,"state":"open"},{"number":5,"state":"closed"}],[{"number":6,"state":"open"}]]"#,
        );
        assert_eq!(gh.unit_blockers_list(tmp.path(), 1).unwrap(), [4, 6]);
    }

    /// A `gh` that logs its arguments, answers a read of an issue with id 555, and ends a POST as
    /// `post` says.
    fn blocker_fake(dir: &Path, post: &str) -> GitHub {
        let path = dir.join("gh");
        let log = dir.join("gh.log");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$@\" >> {}\ncase \"$*\" in\n*POST*) {post};;\n*) echo '{{\"id\":555}}';;\nesac\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        GitHub::with_program(path)
    }

    #[test]
    fn a_blocker_is_recorded_by_its_id_not_its_number() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = blocker_fake(tmp.path(), "exit 0");
        gh.unit_blocker_add(tmp.path(), 1, 4).unwrap();
        let log = std::fs::read_to_string(tmp.path().join("gh.log")).unwrap();
        assert!(log.contains("api repos/{owner}/{repo}/issues/4\n"), "{log}");
        assert!(
            log.contains("api --method POST repos/{owner}/{repo}/issues/1/dependencies/blocked_by -F issue_id=555"),
            "{log}"
        );
    }

    #[test]
    fn a_blocker_that_is_already_recorded_is_fine() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = blocker_fake(
            tmp.path(),
            "echo 'Dependency already exists (HTTP 422)' >&2; exit 1",
        );
        gh.unit_blocker_add(tmp.path(), 1, 4).unwrap();
    }

    #[test]
    fn a_blocker_the_forge_refuses_is_an_error() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = blocker_fake(tmp.path(), "echo 'HTTP 404' >&2; exit 1");
        assert!(gh.unit_blocker_add(tmp.path(), 1, 4).is_err());
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

    /// A stand-in for `gh` that fails `failures` times, then prints `stdout`. It counts its calls
    /// in a file beside it.
    fn flaky(dir: &Path, failures: u32, said: &str, stdout: &str) -> GitHub {
        let path = dir.join("gh");
        let calls = dir.join("calls");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho x >> '{calls}'\n[ $(wc -l < '{calls}') -le {failures} ] && {{ echo '{said}' >&2; exit 1; }}\ncat <<'EOF'\n{stdout}\nEOF\n",
                calls = calls.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut gh = GitHub::with_program(path);
        gh.retry_delay = Duration::ZERO;
        gh
    }

    const DOWN: &str = "error connecting to api.github.com";

    fn calls(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap()
            .lines()
            .count()
    }

    #[test]
    fn a_read_that_fails_is_tried_again() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = flaky(tmp.path(), 2, DOWN, "[]");
        assert_eq!(gh.change_request_get(tmp.path(), "ns/u1").unwrap(), None);
        assert_eq!(calls(tmp.path()), 3);
    }

    #[test]
    fn a_call_that_keeps_failing_gives_up_with_its_error() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = flaky(tmp.path(), 99, DOWN, "");
        let err = gh
            .unit_labels_set(tmp.path(), 7, &["a".into()], &[])
            .unwrap_err();
        assert!(format!("{err:#}").contains(DOWN), "{err:#}");
        assert_eq!(calls(tmp.path()), retry::ATTEMPTS as usize);
    }

    #[test]
    fn opening_and_merging_a_change_request_are_not_repeated() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let gh = flaky(tmp.path(), 99, DOWN, "");
        gh.change_request_merge(tmp.path(), 9).unwrap_err();
        assert_eq!(calls(tmp.path()), 1);
        gh.change_request_create(tmp.path(), "ns/u1", 7, "t", "b", &[])
            .unwrap_err();
        assert_eq!(calls(tmp.path()), 2);
    }

    #[test]
    fn only_a_failure_that_can_pass_is_tried_again() {
        let _spawn = crate::testutil::spawning();
        for (said, repeated) in [
            ("gh: Not Found (HTTP 404)", false),
            ("GraphQL: user does not have the correct permissions", false),
            ("something nobody has seen", false),
            ("gh: Bad Gateway (HTTP 502)", true),
            ("gh: API rate limit exceeded (HTTP 429)", true),
            ("dial tcp: lookup api.github.com: no such host", true),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let gh = flaky(tmp.path(), 99, said, "");
            gh.unit_get(tmp.path(), 7).unwrap_err();
            let expected = if repeated {
                retry::ATTEMPTS as usize
            } else {
                1
            };
            assert_eq!(calls(tmp.path()), expected, "{said}");
        }
    }

    #[test]
    fn a_unit_comment_is_added_whole() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gh");
        let said = tmp.path().join("said");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\" >> '{}'; done\n",
                said.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        GitHub::with_program(path)
            .unit_comment_add(tmp.path(), 7, "line one\nline two")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(said).unwrap(),
            "[issue][comment][7][--body][line one\nline two]"
        );
    }

    #[test]
    fn a_unit_body_is_replaced_whole() {
        let _spawn = crate::testutil::spawning();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gh");
        let said = tmp.path().join("said");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\" >> '{}'; done\n",
                said.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        GitHub::with_program(path)
            .unit_body_set(tmp.path(), 7, "line one\nline two")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(said).unwrap(),
            "[issue][edit][7][--body][line one\nline two]"
        );
    }
}
