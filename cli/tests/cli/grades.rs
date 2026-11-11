use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::Repo;

use assert_cmd::Command;
use chrono::Datelike;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Keeps one grade made `days` days ago by `version`.
fn keep(repo: &Repo, rubric: &str, pass: bool, days: u64, version: &str) {
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
        r#"{{"unit":"1","rubric":"{rubric}","score":0.5,"pass":{pass},"reason":"r","version":"{version}","base":"b","at":{}}}"#,
        now() - days * 86_400
    )
    .unwrap();
}

fn grades(repo: &Repo, args: &[&str]) -> String {
    let out = Command::cargo_bin("ns")
        .unwrap()
        .arg("grades")
        .args(args)
        .current_dir(&repo.path)
        .assert()
        .success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

#[test]
fn no_grades_prints_one_line_and_succeeds() {
    let repo = Repo::new();
    assert_eq!(grades(&repo, &[]), "There are no grades yet.\n");
}

#[test]
fn the_tables_show_pass_rate_and_count_per_week_and_per_version_with_dashes() {
    let repo = Repo::new();
    keep(&repo, "efficiency", true, 0, "0.9.0");
    keep(&repo, "efficiency", false, 0, "0.9.0");
    keep(&repo, "efficiency", true, 0, "0.9.0");
    keep(&repo, "verbosity", true, 0, "0.10.0");
    let out = grades(&repo, &[]);
    let tables: Vec<&str> = out.split("\n\n").collect();
    assert_eq!(tables.len(), 2, "{out}");
    let weekly: Vec<&str> = tables[0].lines().collect();
    assert_eq!(weekly.len(), 9, "a header and eight weeks: {out}");
    assert_eq!(weekly[0], "week      efficiency  verbosity");
    assert!(weekly[1].ends_with("-           -"), "{}", weekly[1]);
    assert!(weekly[8].ends_with("67% (3)     100% (1)"), "{}", weekly[8]);
    assert_eq!(
        tables[1].trim_end(),
        "version  efficiency  verbosity\n0.9.0    67% (3)     -\n0.10.0   -           100% (1)"
    );
}

#[test]
fn weeks_sets_how_far_back_the_table_goes() {
    let repo = Repo::new();
    keep(&repo, "efficiency", true, 0, "1.0.0");
    keep(&repo, "efficiency", true, 20, "1.0.0");
    let out = grades(&repo, &["--weeks", "2"]);
    let weekly = out.split("\n\n").next().unwrap();
    assert_eq!(weekly.lines().count(), 3, "{out}");
    assert!(out.contains("100% (1)"));
    assert!(!out.contains("(2)"));
    let wide = grades(&repo, &["--weeks", "4"]);
    assert!(wide.contains("100% (2)"), "{wide}");
}

#[test]
fn json_prints_one_line_for_each_rubric_and_period() {
    let repo = Repo::new();
    keep(&repo, "efficiency", true, 0, "1.0.0");
    keep(&repo, "efficiency", false, 0, "1.0.0");
    let out = grades(&repo, &["--json"]);
    let lines: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["kind"], "week");
    let week = chrono::Utc::now().iso_week();
    assert_eq!(
        lines[0]["period"],
        format!("{}-W{:02}", week.year(), week.week())
    );
    assert_eq!(lines[0]["grades"], 2);
    assert_eq!(lines[0]["passed"], 1);
    assert_eq!(lines[0]["pass_rate"], 0.5);
    assert_eq!(lines[1]["kind"], "version");
    assert_eq!(lines[1]["period"], "1.0.0");
    assert_eq!(lines[1]["rubric"], "efficiency");
    assert_eq!(lines[1]["grades"], 2);
    assert_eq!(lines[1]["passed"], 1);
    assert_eq!(lines[1]["pass_rate"], 0.5);
}

#[test]
fn grades_changes_nothing() {
    let repo = Repo::new();
    keep(&repo, "efficiency", true, 0, "1.0.0");
    let before = std::fs::read(repo.path.join(".git/ns/grades.jsonl")).unwrap();
    grades(&repo, &[]);
    assert_eq!(
        std::fs::read(repo.path.join(".git/ns/grades.jsonl")).unwrap(),
        before
    );
    assert!(!repo.path.join(".git/ns/runs.jsonl").exists());
}

#[test]
fn json_with_no_grades_prints_nothing_and_succeeds() {
    let repo = Repo::new();
    assert_eq!(grades(&repo, &["--json"]), "");
}

#[test]
fn grades_all_older_than_the_window_are_not_called_absent() {
    let repo = Repo::new();
    keep(&repo, "efficiency", true, 60, "1.0.0");
    assert_eq!(
        grades(&repo, &["--weeks", "1"]),
        "There are no grades in the last 1 weeks.\n"
    );
}

/// Keeps one grade of `unit` whose sessions are `sessions`, a JSON array.
fn keep_sessions(repo: &Repo, unit: &str, pass: bool, sessions: &str) {
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
        r#"{{"unit":"{unit}","rubric":"efficiency","score":0.5,"pass":{pass},"reason":"r","version":"1.0.0","base":"b","at":{},"sessions":{sessions}}}"#,
        now()
    )
    .unwrap();
}

#[test]
fn the_pass_rate_is_split_by_build_model_with_the_units_left_out_counted() {
    let repo = Repo::new();
    let opus = r#"[{"state":"build","model":"opus","tokens":null}]"#;
    keep_sessions(&repo, "1", true, opus);
    keep_sessions(&repo, "2", false, opus);
    keep_sessions(
        &repo,
        "3",
        true,
        r#"[{"state":"build","model":null,"tokens":null}]"#,
    );
    keep(&repo, "efficiency", true, 0, "1.0.0");
    let out = grades(&repo, &[]);
    let model = out.split("\n\n").nth(2).expect(&out);
    assert_eq!(
        model,
        "model  efficiency\nopus   50% (2)\n2 units left out: their build session names no model\n"
    );
    let json = grades(&repo, &["--json"]);
    assert!(json.contains(r#""kind":"model","#), "{json}");
    assert!(
        json.contains(r#"{"kind":"model_left_out","units":2}"#),
        "{json}"
    );
}

#[test]
fn a_retrospective_share_outside_0_to_100_is_an_error() {
    let repo = Repo::new();
    keep(&repo, "efficiency", true, 0, "1.0.0");
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "[grades]\nretrospective_share = 101\n",
    )
    .unwrap();
    let out = Command::cargo_bin("ns")
        .unwrap()
        .arg("grades")
        .current_dir(&repo.path)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(stderr.contains("grades.retrospective_share"), "{stderr}");
}
