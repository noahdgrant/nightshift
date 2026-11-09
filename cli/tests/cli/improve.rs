use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::{labels_json, script, Repo};
use crate::run::set_manifest;

use assert_cmd::Command;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Keeps a grade of `unit` on `rubric` that did not pass, made `days` days ago.
fn fail(repo: &Repo, unit: u32, rubric: &str, days: u64) {
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
        r#"{{"unit":"{unit}","rubric":"{rubric}","score":0.25,"pass":false,"reason":"padded the diff","version":"1.0.0","base":"b","at":{}}}"#,
        now() - days * 86_400
    )
    .unwrap();
}

fn bin(repo: &Repo) -> PathBuf {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    bin
}

fn proposal(title: &str, cause: &str, brief: &str) -> String {
    format!(
        "PROPOSAL\nTITLE: {title}\nRUBRIC: efficiency\nCAUSE: {cause}\nLAYER: check\nLABELS: type:fix, priority:high, area:cli\nBRIEF:\n{brief}\n"
    )
}

/// Runs `ns improve` where the fake `claude` answers `reply`. The forge has the labels `type:fix`,
/// `priority:high` and `status:needs-triage` and the open units `open` (units, as one page).
fn improve(repo: &Repo, reply: &str, open: &str, args: &[&str]) -> assert_cmd::assert::Assert {
    let bin = bin(repo);
    std::fs::write(bin.join("reply.txt"), reply).unwrap();
    std::fs::write(bin.join("open.json"), format!("[[{open}]]")).unwrap();
    let labels = labels_json(&[
        "type:fix",
        "priority:high",
        "status:needs-triage",
        "status:ready-for-agent",
    ]);
    script(
        &bin,
        "gh",
        &format!(
            r#"echo "$@" >> "{b}/gh.log"
case "$1 $2" in
"api --paginate") cat "{b}/open.json";;
"label list") echo '{labels}';;
"issue create") echo https://github.com/o/r/issues/90;;
"api orgs/{{owner}}/issue-types") cat "{b}/types.json";;
esac"#,
            b = bin.display()
        ),
    );
    script(
        &bin,
        "claude",
        &format!(
            "echo \"$@\" > \"{b}/claude.args\"\ncat > \"{b}/prompt.txt\"\nif [ -f \"{b}/claude-fails\" ]; then exit 1; fi\ncat \"{b}/reply.txt\"",
            b = bin.display()
        ),
    );
    Command::cargo_bin("ns")
        .unwrap()
        .arg("improve")
        .args(args)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

fn gh_log(repo: &Repo) -> String {
    std::fs::read_to_string(bin(repo).join("gh.log")).unwrap_or_default()
}

fn stdout(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(a.get_output().stdout.clone()).unwrap()
}

fn three_units_failed(repo: &Repo) {
    for unit in 1..=3 {
        fail(repo, unit, "efficiency", 1);
    }
}

#[test]
fn a_cause_below_three_units_proposes_nothing_and_starts_no_session() {
    let repo = Repo::new();
    fail(&repo, 1, "efficiency", 1);
    fail(&repo, 1, "efficiency", 2);
    fail(&repo, 2, "efficiency", 1);
    fail(&repo, 3, "efficiency", 30);
    let out = improve(&repo, "NOTHING TO PROPOSE", "", &[]).success();
    assert_eq!(stdout(&out), "{\"improve\":\"nothing to propose\"}\n");
    assert!(!bin(&repo).join("prompt.txt").exists());
    assert!(!gh_log(&repo).contains("issue create"));
}

#[test]
fn a_session_that_finds_no_repeated_cause_files_nothing() {
    let repo = Repo::new();
    three_units_failed(&repo);
    let out = improve(&repo, "Nothing repeats.\nNOTHING TO PROPOSE", "", &[]).success();
    assert_eq!(stdout(&out), "{\"improve\":\"nothing to propose\"}\n");
    assert!(!gh_log(&repo).contains("issue create"));
}

#[test]
fn a_cause_at_three_units_is_filed_with_its_marker_and_labels_and_never_ready() {
    let repo = Repo::new();
    three_units_failed(&repo);
    let reply = proposal(
        "fix(cli): lint padded diffs",
        "padded-diff",
        "## Goal\n\nUnits 1, 2, 3.",
    );
    let out = improve(&repo, &reply, "", &[]).success();
    let line: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(line["unit"], 90);
    assert_eq!(line["title"], "fix(cli): lint padded diffs");
    assert_eq!(line["layer"], "check");
    let log = gh_log(&repo);
    assert!(
        log.contains("--title fix(cli): lint padded diffs --body 🤖 *Posted by nightshift, an AI agent.*\n\n<!-- ns:improve efficiency padded-diff -->\n\n## Goal\n\nUnits 1, 2, 3.\n\nLayer to fix: `check`. Choose the highest layer that removes the whole category: `code`, then `check`, and `prose` only when neither fits.\n --label type:fix --label priority:high --label status:needs-triage"),
        "{log}"
    );
    assert!(
        !log.contains("area:cli"),
        "the repository has no such label: {log}"
    );
    assert!(!log.contains("ready-for-agent"), "{log}");
    let prompt = std::fs::read_to_string(bin(&repo).join("prompt.txt")).unwrap();
    assert!(
        prompt.contains("unit 2, rubric efficiency, score 0.25: padded the diff"),
        "{prompt}"
    );
}

#[test]
fn a_proposed_status_label_is_dropped_so_a_person_looks_first() {
    let repo = Repo::new();
    three_units_failed(&repo);
    let reply = proposal("fix(cli): lint padded diffs", "padded-diff", "Brief.")
        .replace("area:cli", "area:cli, status:ready-for-agent");
    let out = improve(&repo, &reply, "", &[]).success();
    let line: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(
        line["labels"],
        serde_json::json!(["type:fix", "priority:high", "status:needs-triage"])
    );
    assert!(!gh_log(&repo).contains("ready-for-agent"));
}

#[test]
fn at_most_three_units_are_filed_in_one_run() {
    let repo = Repo::new();
    three_units_failed(&repo);
    let reply: String = (1..=4)
        .map(|n| {
            proposal(
                &format!("fix(cli): fix {n}"),
                &format!("cause-{n}"),
                "Brief.",
            )
        })
        .collect();
    let out = improve(&repo, &reply, "", &[]).success();
    assert_eq!(stdout(&out).lines().count(), 3);
    assert_eq!(gh_log(&repo).matches("issue create").count(), 3);
}

#[test]
fn a_marker_an_open_unit_already_carries_is_skipped() {
    let repo = Repo::new();
    three_units_failed(&repo);
    let open = r#"{"number":5,"title":"fix(cli): the first fix","body":"<!-- ns:improve efficiency padded-diff -->\nx","labels":[],"author_association":"OWNER"}"#;
    let reply = format!(
        "{}{}",
        proposal("fix(cli): again", "padded-diff", "Brief."),
        proposal("fix(cli): other", "other-cause", "Brief.")
    );
    let out = improve(&repo, &reply, open, &[]).success();
    assert_eq!(stdout(&out).lines().count(), 1);
    let log = gh_log(&repo);
    assert_eq!(log.matches("issue create").count(), 1, "{log}");
    assert!(
        log.contains("other-cause") && !log.contains("fix(cli): again"),
        "{log}"
    );
    let prompt = std::fs::read_to_string(bin(&repo).join("prompt.txt")).unwrap();
    assert!(prompt.contains("fix(cli): the first fix"), "{prompt}");
}

#[test]
fn dry_run_prints_the_units_and_files_nothing() {
    let repo = Repo::new();
    three_units_failed(&repo);
    let reply = proposal("fix(cli): lint padded diffs", "padded-diff", "Brief.");
    let out = improve(&repo, &reply, "", &["--dry-run"]).success();
    let line: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(line["title"], "fix(cli): lint padded diffs");
    assert!(line.get("unit").is_none());
    assert!(!gh_log(&repo).contains("issue create"));
}

#[test]
fn a_cause_in_a_skill_is_filed_as_a_note_to_the_skills_owner() {
    let repo = Repo::new();
    three_units_failed(&repo);
    let reply = proposal(
        "docs(cli): tell the skill's owner",
        "skill-pads",
        "## Goal\n\n## For the skill's owner\n\nns-build pads diffs.",
    );
    improve(&repo, &reply, "", &[]).success();
    let log = gh_log(&repo);
    assert!(
        log.contains("## For the skill's owner\n\nns-build pads diffs."),
        "{log}"
    );
    let prompt = std::fs::read_to_string(bin(&repo).join("prompt.txt")).unwrap();
    assert!(prompt.contains("cannot change it"), "{prompt}");
}

#[test]
fn a_repository_with_its_own_skills_may_have_them_changed() {
    let repo = Repo::new();
    three_units_failed(&repo);
    std::fs::create_dir_all(repo.path.join("skills/ns-build")).unwrap();
    improve(&repo, "NOTHING TO PROPOSE", "", &[]).success();
    let prompt = std::fs::read_to_string(bin(&repo).join("prompt.txt")).unwrap();
    assert!(prompt.contains("a proposal may change them"), "{prompt}");
}

#[test]
fn the_fix_levels_of_recent_findings_are_given_to_the_session() {
    let repo = Repo::new();
    three_units_failed(&repo);
    std::fs::create_dir_all(repo.path.join(".ns/7")).unwrap();
    std::fs::write(
        repo.path.join(".ns/7/evidence.md"),
        "finding\nFix level: check\nDurable fix: a lint\n",
    )
    .unwrap();
    improve(&repo, "NOTHING TO PROPOSE", "", &[]).success();
    let prompt = std::fs::read_to_string(bin(&repo).join("prompt.txt")).unwrap();
    assert!(prompt.contains("- Fix level: check"), "{prompt}");
}

fn claude_args(repo: &Repo) -> String {
    std::fs::read_to_string(bin(repo).join("claude.args")).unwrap()
}

#[test]
fn the_session_uses_the_configured_triage_model() {
    let repo = Repo::new();
    three_units_failed(&repo);
    set_manifest(&repo, "[model]\ndefault = \"sonnet\"\ntriage = \"haiku\"\n");
    improve(&repo, "NOTHING TO PROPOSE", "", &[]).success();
    assert!(claude_args(&repo).contains("--model haiku"));
}

#[test]
fn the_model_option_beats_the_configured_model() {
    let repo = Repo::new();
    three_units_failed(&repo);
    set_manifest(&repo, "[model]\ndefault = \"sonnet\"\n");
    improve(&repo, "NOTHING TO PROPOSE", "", &["--model", "opus"]).success();
    assert!(claude_args(&repo).contains("--model opus"));
}

#[test]
fn an_unknown_model_key_stops_improve_before_a_session() {
    let repo = Repo::new();
    three_units_failed(&repo);
    set_manifest(&repo, "[model]\nbogus = \"x\"\n");
    improve(&repo, "NOTHING TO PROPOSE", "", &[]).failure();
    assert!(!bin(&repo).join("claude.args").exists());
}

#[test]
fn a_proposal_for_a_rubric_below_three_units_is_left_out() {
    let repo = Repo::new();
    three_units_failed(&repo);
    fail(&repo, 1, "tests", 1);
    let reply = proposal("fix(cli): x", "c", "## Goal\n\nx").replace("efficiency", "tests");
    let out = improve(&repo, &reply, "", &[]).success();
    assert_eq!(stdout(&out), "{\"improve\":\"nothing to propose\"}\n");
    assert!(!gh_log(&repo).contains("issue create"));
}

#[test]
fn with_native_types_the_proposed_type_is_set_and_not_labelled() {
    let repo = Repo::new();
    three_units_failed(&repo);
    std::fs::write(
        bin(&repo).join("types.json"),
        r#"[{"name":"Task"},{"name":"Bug"},{"name":"Feature"}]"#,
    )
    .unwrap();
    let reply = proposal("fix(cli): lint padded diffs", "padded-diff", "## Goal\n\nx");
    improve(&repo, &reply, "", &[]).success();
    let log = gh_log(&repo);
    assert!(log.contains("-f type=Bug"), "{log}");
    assert!(!log.contains("--label type:fix"), "{log}");
}

#[test]
fn a_session_that_fails_files_nothing() {
    let repo = Repo::new();
    three_units_failed(&repo);
    std::fs::write(bin(&repo).join("claude-fails"), "").unwrap();
    let reply = proposal("fix(cli): x", "c", "## Goal\n\nx");
    improve(&repo, &reply, "", &[]).failure();
    assert!(!gh_log(&repo).contains("issue create"));
}
