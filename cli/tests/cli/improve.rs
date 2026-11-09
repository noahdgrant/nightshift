use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::{script, Repo};

use assert_cmd::assert::Assert;
use assert_cmd::Command;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Keeps a grade that did not pass, made `days` days ago, for `unit`.
fn fail(repo: &Repo, unit: &str, rubric: &str, days: u64) {
    use std::io::Write;
    let dir = repo.path.join(".git/ns");
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("grades.jsonl"))
        .unwrap();
    writeln!(
        file,
        r#"{{"unit":"{unit}","rubric":"{rubric}","score":0.25,"pass":false,"reason":"padded the summary","version":"1","base":"b","at":{}}}"#,
        now() - days * 86_400
    )
    .unwrap();
}

fn three_fail(repo: &Repo) {
    for unit in ["11", "12", "13"] {
        fail(repo, unit, "verbosity", 1);
    }
}

fn bin(repo: &Repo) -> std::path::PathBuf {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    bin
}

fn proposal(title: &str, cause: &str, layer: &str, body: &str) -> String {
    format!(
        "PROPOSAL\nTITLE: {title}\nRUBRIC: verbosity\nCAUSE: {cause}\nLAYER: {layer}\n\
         LABELS: type:fix, priority:medium, area:cli\nBRIEF:\n{body}\n"
    )
}

/// Sets up the fake `gh`, which has the labels in `labels` and the open units in `open` (a JSON
/// list), and the fake `claude`, which answers `reply` and keeps the prompt it was given.
/// `fail_create_at` makes that call of `issue create` fail.
fn fakes(repo: &Repo, reply: &str, open: &str, fail_create_at: u32, native: bool) {
    let bin = bin(repo);
    let labels = r#"[{"name":"type:fix"},{"name":"priority:medium"},{"name":"status:needs-triage"},{"name":"status:ready-for-agent"}]"#;
    std::fs::write(bin.join("labels.json"), labels).unwrap();
    std::fs::write(bin.join("open.json"), format!("[{open}]")).unwrap();
    std::fs::write(bin.join("count"), "0").unwrap();
    if native {
        std::fs::write(bin.join("types.json"), r#"[{"name":"Bug"}]"#).unwrap();
    }
    script(
        &bin,
        "gh",
        &format!(
            r#"echo "$@" >> "{bin}/gh.log"
case "$1 $2" in
"api --paginate") cat "{bin}/open.json";;
"label list") cat "{bin}/labels.json";;
"api orgs/{{owner}}/issue-types") cat "{bin}/types.json";;
"api --method") ;;
"issue create")
  n=$(( $(cat "{bin}/count") + 1 )); echo $n > "{bin}/count"
  if [ $n = {fail_create_at} ]; then echo "boom" >&2; exit 1; fi
  echo "https://github.com/o/r/issues/$((100 + n))";;
esac"#,
            bin = bin.display()
        ),
    );
    script(
        &bin,
        "claude",
        &format!(
            "cat > \"{bin}/claude.prompt\"\ncat <<'EOF'\n{reply}\nEOF",
            bin = bin.display()
        ),
    );
}

fn improve(repo: &Repo, extra: &[&str]) -> Assert {
    let bin = bin(repo);
    Command::cargo_bin("ns")
        .unwrap()
        .arg("improve")
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

const OTHER_UNIT: &str =
    r#"[{"number":5,"title":"other","body":"","labels":[],"author_association":"OWNER"}]"#;

fn gh_log(repo: &Repo) -> String {
    std::fs::read_to_string(bin(repo).join("gh.log")).unwrap_or_default()
}

fn stdout(assert: &Assert) -> Vec<serde_json::Value> {
    String::from_utf8(assert.get_output().stdout.clone())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_cause_two_units_failed_on_is_not_proposed_and_no_session_runs() {
    let repo = Repo::new();
    fail(&repo, "11", "verbosity", 1);
    fail(&repo, "12", "verbosity", 1);
    fail(&repo, "12", "verbosity", 2);
    fakes(&repo, "unused", OTHER_UNIT, 0, false);
    let out = improve(&repo, &[]).success();
    assert_eq!(
        String::from_utf8_lossy(&out.get_output().stdout),
        "{\"improve\":\"nothing to propose\"}\n"
    );
    assert!(!bin(&repo).join("claude.prompt").exists());
    assert!(!gh_log(&repo).contains("issue create"));
}

#[test]
fn a_session_that_finds_no_cause_files_nothing() {
    let repo = Repo::new();
    three_fail(&repo);
    fakes(&repo, "The reasons differ.\nNOTHING", OTHER_UNIT, 0, false);
    let out = improve(&repo, &[]).success();
    assert_eq!(
        String::from_utf8_lossy(&out.get_output().stdout),
        "{\"improve\":\"nothing to propose\"}\n"
    );
    assert!(!gh_log(&repo).contains("issue create"));
}

#[test]
fn a_cause_three_units_failed_on_is_filed_with_its_marker_and_the_labels_the_repository_has() {
    let repo = Repo::new();
    three_fail(&repo);
    let reply = proposal(
        "fix(cli): shorten build summaries",
        "long-summaries",
        "check",
        "## Goal\n\nShort.",
    );
    fakes(&repo, &reply, OTHER_UNIT, 0, false);
    let out = improve(&repo, &[]).success();
    let lines = stdout(&out);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["unit"], 101);
    assert_eq!(lines[0]["layer"], "check");
    let log = gh_log(&repo);
    assert!(
        log.contains("issue create --title fix(cli): shorten build summaries --body 🤖 *Posted by nightshift, an AI agent.*\n\n<!-- ns:improve verbosity long-summaries -->\n\nLayer to fix: check. Choose the highest layer that removes the whole category (code, then check, then prose), and prose only when neither fits.\n\n## Goal\n\nShort."),
        "{log}"
    );
    // `area:cli` is not a label the repository has; the state label is never ready-for-agent.
    assert!(
        log.contains("--label type:fix,priority:medium,status:needs-triage\n"),
        "{log}"
    );
    let prompt = std::fs::read_to_string(bin(&repo).join("claude.prompt")).unwrap();
    assert!(
        prompt.contains("- unit 11, rubric verbosity, score 0.25: padded the summary"),
        "{prompt}"
    );
}

#[test]
fn at_most_three_units_are_filed_in_one_run() {
    let repo = Repo::new();
    three_fail(&repo);
    let reply: String = ["a", "b", "c", "d"]
        .iter()
        .map(|c| proposal(&format!("fix: {c}"), c, "code", "brief"))
        .collect();
    fakes(&repo, &reply, OTHER_UNIT, 0, false);
    let out = improve(&repo, &[]).success();
    assert_eq!(stdout(&out).len(), 3);
    assert_eq!(gh_log(&repo).matches("issue create").count(), 3);
}

#[test]
fn a_cause_an_open_unit_already_carries_is_skipped() {
    let repo = Repo::new();
    three_fail(&repo);
    let reply = format!(
        "{}{}",
        proposal("fix: a", "known", "code", "brief"),
        proposal("fix: b", "fresh", "code", "brief")
    );
    let open = r#"[{"number":7,"title":"fix: known","body":"x\n<!-- ns:improve verbosity known -->","labels":[],"author_association":"OWNER"}]"#;
    fakes(&repo, &reply, open, 0, false);
    let out = improve(&repo, &[]).success();
    let lines = stdout(&out);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["cause"], "fresh");
    let prompt = std::fs::read_to_string(bin(&repo).join("claude.prompt")).unwrap();
    assert!(prompt.contains("- fix: known"), "{prompt}");
}

#[test]
fn a_dry_run_prints_the_units_and_files_nothing() {
    let repo = Repo::new();
    three_fail(&repo);
    let reply = proposal("fix: a", "slow", "code", "brief");
    fakes(&repo, &reply, OTHER_UNIT, 0, false);
    let out = improve(&repo, &["--dry-run"]).success();
    let lines = stdout(&out);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["dry_run"], true);
    assert_eq!(lines[0]["title"], "fix: a");
    assert!(lines[0].get("unit").is_none());
    assert!(!gh_log(&repo).contains("issue create"));
}

#[test]
fn grades_older_than_since_are_left_out() {
    let repo = Repo::new();
    for unit in ["11", "12", "13"] {
        fail(&repo, unit, "verbosity", 3);
    }
    fakes(&repo, "NOTHING", OTHER_UNIT, 0, false);
    improve(&repo, &["--since", "2"]).success();
    assert!(!bin(&repo).join("claude.prompt").exists());
    improve(&repo, &["--since", "4", "--dry-run"]).success();
    assert!(bin(&repo).join("claude.prompt").exists());
}

#[test]
fn a_cause_in_a_skill_is_filed_as_a_note_to_the_skills_owner() {
    let repo = Repo::new();
    three_fail(&repo);
    let brief = "## Goal\n\n### For the skill's owner\n\nns-build asks for long summaries.";
    let reply = proposal(
        "docs: tell the skill's owner",
        "skill-length",
        "prose",
        brief,
    );
    fakes(&repo, &reply, OTHER_UNIT, 0, false);
    improve(&repo, &[]).success();
    assert!(gh_log(&repo).contains("### For the skill's owner"));
    let prompt = std::fs::read_to_string(bin(&repo).join("claude.prompt")).unwrap();
    assert!(prompt.contains("ordinary repository"), "{prompt}");
    assert!(prompt.contains("repository cannot patch it"), "{prompt}");
}

#[test]
fn a_repository_with_its_own_skills_may_have_them_changed() {
    let repo = Repo::new();
    three_fail(&repo);
    std::fs::create_dir_all(repo.path.join("skills/ns-build")).unwrap();
    fakes(&repo, "NOTHING", OTHER_UNIT, 0, false);
    improve(&repo, &[]).success();
    let prompt = std::fs::read_to_string(bin(&repo).join("claude.prompt")).unwrap();
    assert!(prompt.contains("has its own `skills/ns-*`"), "{prompt}");
    assert!(!prompt.contains("ordinary repository"), "{prompt}");
}

#[test]
fn a_forge_error_on_the_second_unit_keeps_the_first_units_line() {
    let repo = Repo::new();
    three_fail(&repo);
    let reply = format!(
        "{}{}",
        proposal("fix: a", "one", "code", "brief"),
        proposal("fix: b", "two", "code", "brief")
    );
    fakes(&repo, &reply, OTHER_UNIT, 2, false);
    let out = improve(&repo, &[]).failure();
    let lines = stdout(&out);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["unit"], 101);
    assert_eq!(lines[0]["cause"], "one");
}

#[test]
fn a_failed_native_type_names_the_unit_that_was_filed() {
    let repo = Repo::new();
    three_fail(&repo);
    let reply = proposal("fix: a", "one", "code", "brief");
    fakes(&repo, &reply, OTHER_UNIT, 0, true);
    // Make the PATCH that sets the type fail.
    let gh = bin(&repo).join("gh");
    let text = std::fs::read_to_string(&gh).unwrap().replace(
        "\"api --method\") ;;",
        "\"api --method\") echo denied >&2; exit 1;;",
    );
    std::fs::write(&gh, text).unwrap();
    let out = improve(&repo, &[]).failure();
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        err.contains("unit 101 was filed but its type was not set"),
        "{err}"
    );
}
