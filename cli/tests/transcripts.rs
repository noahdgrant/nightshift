use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tempfile::TempDir;

mod common;

const ALPHA: &str = "aaaa1111-0000-4000-8000-000000000001";
const BETA: &str = "bbbb2222-0000-4000-8000-000000000002";
const ALPHA_MTIME: u64 = 1_790_900_000;
const BETA_MTIME: u64 = 1_791_300_000;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transcripts/projects")
}

fn session_path(home: &Path, project: &str, id: &str) -> PathBuf {
    home.join(".claude/projects")
        .join(project)
        .join(format!("{id}.jsonl"))
}

fn set_mtime(path: &Path, secs: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

fn home() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for project in ["-work-alpha", "-work-beta"] {
        let dst = tmp.path().join(".claude/projects").join(project);
        fs::create_dir_all(&dst).unwrap();
        for e in fs::read_dir(fixtures().join(project)).unwrap() {
            let e = e.unwrap();
            fs::copy(e.path(), dst.join(e.file_name())).unwrap();
        }
    }
    set_mtime(&session_path(tmp.path(), "-work-alpha", ALPHA), ALPHA_MTIME);
    set_mtime(&session_path(tmp.path(), "-work-beta", BETA), BETA_MTIME);
    tmp
}

fn ns(home: &Path) -> common::Ns {
    let mut c = common::ns();
    c.env("HOME", home)
        .env_remove("XDG_DATA_HOME")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("NS_CONFIG");
    c
}

fn run_json(home: &Path, args: &[&str]) -> Value {
    let out = ns(home).args(args).assert().success().get_output().clone();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

fn ids(v: &Value) -> Vec<String> {
    v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn list_prints_each_session_with_id_date_project_and_size() {
    let home = home();
    let v = run_json(home.path(), &["transcripts", "list"]);
    let sessions = v["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "{v}");
    let alpha = sessions.iter().find(|s| s["id"] == ALPHA).unwrap();
    assert_eq!(alpha["source"], "claude");
    assert_eq!(alpha["date"], "2026-10-01T09:00:00.000Z");
    assert_eq!(alpha["project"], "/work/alpha");
    let size = fs::metadata(session_path(home.path(), "-work-alpha", ALPHA))
        .unwrap()
        .len();
    assert_eq!(alpha["size"], size);
    let beta = sessions.iter().find(|s| s["id"] == BETA).unwrap();
    assert_eq!(beta["project"], "/work/beta");
}

#[test]
fn list_since_keeps_sessions_on_or_after_the_date() {
    let home = home();
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--since", "2026-10-05"],
    );
    assert_eq!(ids(&v), [BETA]);
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--since", "2026-10-01"],
    );
    assert_eq!(ids(&v).len(), 2);
}

#[test]
fn list_since_rejects_a_malformed_date() {
    let home = home();
    ns(home.path())
        .args(["transcripts", "list", "--since", "yesterday"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--since 2026-10-01"));
}

#[test]
fn list_project_maps_the_path_to_its_claude_directory() {
    let home = home();
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--project", "/work/alpha"],
    );
    assert_eq!(ids(&v), [ALPHA]);
}

#[test]
fn read_prints_prompts_text_tools_and_errors_with_truncation() {
    let home = home();
    let v = run_json(home.path(), &["transcripts", "read", ALPHA, "--compact"]);
    assert_eq!(v["id"], ALPHA);
    assert_eq!(v["project"], "/work/alpha");
    let entries = v["entries"].as_array().unwrap();
    let kinds: Vec<&str> = entries
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "user",
            "assistant",
            "tool",
            "output",
            "tool",
            "error",
            "user"
        ],
        "{v:#}"
    );
    assert_eq!(entries[0]["text"], "Fix the UART timeout");
    assert_eq!(entries[1]["text"], "Looking at the driver.");
    assert_eq!(entries[2]["name"], "Bash");
    let args = entries[2]["input"].as_str().unwrap();
    assert!(args.contains("[truncated "), "{args}");
    assert!(args.len() < 1_000, "{}", args.len());
    let output = entries[3]["text"].as_str().unwrap();
    assert!(output.starts_with("line\n"), "{output}");
    assert!(output.contains("[truncated "), "{output}");
    assert!(output.len() < 400, "{}", output.len());
    assert_eq!(entries[4]["name"], "Read");
    assert!(entries[4]["input"].as_str().unwrap().contains("uart.c"));
    assert_eq!(entries[5]["text"], "File does not exist.");
    assert_eq!(entries[6]["text"], "Try src/uart.c");
    assert!(!v.to_string().contains("secret plan"));
    assert!(!v.to_string().contains("local-command-caveat"));
}

#[test]
fn read_without_compact_keeps_tool_output_truncated() {
    let home = home();
    let v = run_json(home.path(), &["transcripts", "read", ALPHA]);
    let entries = v["entries"].as_array().unwrap();
    let output = entries
        .iter()
        .find(|e| e["kind"] == "output")
        .unwrap_or_else(|| panic!("{v:#}"));
    let text = output["text"].as_str().unwrap();
    assert!(text.starts_with("line\n"));
    assert!(text.contains("[truncated "), "{text}");
    assert!(text.len() < 5_000 - 100);
}

#[test]
fn read_accepts_a_unique_id_prefix() {
    let home = home();
    let v = run_json(home.path(), &["transcripts", "read", "bbbb"]);
    assert_eq!(v["id"], BETA);
}

#[test]
fn read_an_unknown_id_fails_with_the_list_command() {
    let home = home();
    ns(home.path())
        .args(["transcripts", "read", "nope"])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("ns transcripts list"));
}

#[test]
fn new_lists_only_sessions_changed_since_the_last_mark() {
    let home = home();
    let v = run_json(home.path(), &["transcripts", "list", "--new"]);
    assert_eq!(ids(&v).len(), 2);
    assert!(!home
        .path()
        .join(".local/share/nightshift/transcripts-index.json")
        .exists());

    let v = run_json(home.path(), &["transcripts", "list", "--new", "--mark"]);
    assert_eq!(ids(&v).len(), 2);
    assert!(home
        .path()
        .join(".local/share/nightshift/transcripts-index.json")
        .exists());

    let v = run_json(home.path(), &["transcripts", "list", "--new"]);
    assert!(ids(&v).is_empty(), "{v}");

    let alpha = session_path(home.path(), "-work-alpha", ALPHA);
    let mut body = fs::read_to_string(&alpha).unwrap();
    body.push_str("{\"type\":\"user\",\"message\":{\"content\":\"more\"}}\n");
    fs::write(&alpha, body).unwrap();
    set_mtime(&alpha, ALPHA_MTIME);
    let v = run_json(home.path(), &["transcripts", "list", "--new"]);
    assert_eq!(ids(&v), [ALPHA], "size change");

    run_json(home.path(), &["transcripts", "list", "--new", "--mark"]);
    set_mtime(
        &session_path(home.path(), "-work-beta", BETA),
        BETA_MTIME + 1_000,
    );
    let v = run_json(home.path(), &["transcripts", "list", "--new"]);
    assert_eq!(ids(&v), [BETA], "mtime change");
}

#[test]
fn mark_with_a_filter_records_only_the_listed_sessions() {
    let home = home();
    run_json(
        home.path(),
        &["transcripts", "list", "--project", "/work/beta", "--mark"],
    );
    let v = run_json(home.path(), &["transcripts", "list", "--new"]);
    assert_eq!(ids(&v), [ALPHA]);
}

#[test]
fn index_honours_xdg_data_home() {
    let home = home();
    let data = home.path().join("data");
    ns(home.path())
        .env("XDG_DATA_HOME", &data)
        .args(["transcripts", "list", "--mark"])
        .assert()
        .success();
    assert!(data.join("nightshift/transcripts-index.json").exists());
}

fn write_session(home: &Path, project: &str, id: &str, lines: &[String]) -> PathBuf {
    let path = session_path(home, project, id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

fn user_line(text: &str, timestamp: &str, cwd: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": text},
        "timestamp": timestamp,
        "cwd": cwd,
    })
    .to_string()
}

#[test]
fn read_keeps_a_long_entry_intact_in_full_mode_and_cuts_it_in_compact() {
    let home = home();
    let long = "x".repeat(1_500);
    write_session(
        home.path(),
        "-work-long",
        "cccc3333-0000-4000-8000-000000000003",
        &[user_line(&long, "2026-10-06T00:00:00.000Z", "/work/long")],
    );
    let v = run_json(home.path(), &["transcripts", "read", "cccc3333"]);
    assert_eq!(v["entries"][0]["text"], long.as_str());
    let v = run_json(
        home.path(),
        &["transcripts", "read", "cccc3333", "--compact"],
    );
    let text = v["entries"][0]["text"].as_str().unwrap();
    assert!(text.contains("[truncated 1300 chars]"), "{text}");
}

#[test]
fn list_orders_oldest_first_and_breaks_date_ties_by_id() {
    let home = home();
    let same = "2026-10-03T00:00:00.000Z";
    write_session(
        home.path(),
        "-work-tie",
        "zzzz9999-0000-4000-8000-000000000009",
        &[user_line("z", same, "/work/tie")],
    );
    write_session(
        home.path(),
        "-work-tie",
        "dddd4444-0000-4000-8000-000000000004",
        &[user_line("d", same, "/work/tie")],
    );
    let v = run_json(home.path(), &["transcripts", "list"]);
    assert_eq!(
        ids(&v),
        [
            ALPHA,
            "dddd4444-0000-4000-8000-000000000004",
            "zzzz9999-0000-4000-8000-000000000009",
            BETA
        ]
    );
}

#[test]
fn read_an_ambiguous_prefix_exits_2_and_lists_the_candidates() {
    let home = home();
    let other = "aaaa5555-0000-4000-8000-000000000005";
    write_session(
        home.path(),
        "-work-alpha",
        other,
        &[user_line("hi", "2026-10-02T00:00:00.000Z", "/work/alpha")],
    );
    ns(home.path())
        .args(["transcripts", "read", "aaaa"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(ALPHA))
        .stderr(predicates::str::contains(other));
}

#[test]
fn read_an_exact_id_beats_a_longer_id_sharing_the_prefix() {
    let home = home();
    let longer = format!("{ALPHA}-extra");
    write_session(
        home.path(),
        "-work-alpha",
        &longer,
        &[user_line("hi", "2026-10-02T00:00:00.000Z", "/work/alpha")],
    );
    let v = run_json(home.path(), &["transcripts", "read", ALPHA]);
    assert_eq!(v["id"], ALPHA);
}

#[test]
fn list_project_ignores_a_trailing_slash_and_matches_by_working_directory() {
    let home = home();
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--project", "/work/alpha/"],
    );
    assert_eq!(ids(&v), [ALPHA]);

    let renamed = "eeee6666-0000-4000-8000-000000000006";
    write_session(
        home.path(),
        "-some-other-dir",
        renamed,
        &[user_line("hi", "2026-10-02T00:00:00.000Z", "/work/gamma")],
    );
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--project", "/work/gamma"],
    );
    assert_eq!(ids(&v), [renamed]);
}

#[test]
fn list_falls_back_to_mtime_and_directory_name_when_the_header_has_no_cwd() {
    let home = home();
    let id = "ffff7777-0000-4000-8000-000000000007";
    let mut lines: Vec<String> = (0..60).map(|_| "{\"type\":\"summary\"}".into()).collect();
    lines.push(user_line("late", "2026-10-09T00:00:00.000Z", "/work/late"));
    let path = write_session(home.path(), "-work-late", id, &lines);
    set_mtime(&path, 86_400 * 3);
    let v = run_json(home.path(), &["transcripts", "list"]);
    let s = v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap();
    assert_eq!(s["project"], "-work-late");
    assert_eq!(s["date"], "1970-01-04T00:00:00Z");
}

#[test]
fn read_skips_lines_that_are_not_utf8_or_json() {
    let home = home();
    let id = "abab8888-0000-4000-8000-000000000008";
    let path = write_session(
        home.path(),
        "-work-bad",
        id,
        &[user_line("before", "2026-10-02T00:00:00.000Z", "/work/bad")],
    );
    let mut body = fs::read(&path).unwrap();
    body.extend_from_slice(b"\xff\xfe not json\nnot json either\n");
    body.extend_from_slice(user_line("after", "2026-10-02T00:00:01.000Z", "/work/bad").as_bytes());
    body.push(b'\n');
    fs::write(&path, body).unwrap();
    let v = run_json(home.path(), &["transcripts", "list"]);
    assert!(ids(&v).contains(&id.to_string()));
    let v = run_json(home.path(), &["transcripts", "read", id]);
    let texts: Vec<&str> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["before", "after"]);
}

#[test]
fn read_prints_system_errors_as_error_entries() {
    let home = home();
    let id = "acac9999-0000-4000-8000-000000000009";
    write_session(
        home.path(),
        "-work-sys",
        id,
        &[
            serde_json::json!({"type": "system", "level": "error", "content": "boom"}).to_string(),
            serde_json::json!({"type": "system", "level": "info", "content": "fine"}).to_string(),
        ],
    );
    let v = run_json(home.path(), &["transcripts", "read", id]);
    assert_eq!(v["entries"].as_array().unwrap().len(), 1, "{v:#}");
    assert_eq!(v["entries"][0]["kind"], "error");
    assert_eq!(v["entries"][0]["text"], "boom");
}

#[test]
fn a_corrupt_index_fails_the_run_and_stays_untouched() {
    let home = home();
    let index = home
        .path()
        .join(".local/share/nightshift/transcripts-index.json");
    fs::create_dir_all(index.parent().unwrap()).unwrap();
    fs::write(&index, "{not json").unwrap();
    ns(home.path())
        .args(["transcripts", "list", "--new", "--mark"])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("transcripts-index.json"));
    assert_eq!(fs::read_to_string(&index).unwrap(), "{not json");
}

#[test]
fn a_plain_list_ignores_a_corrupt_index() {
    let home = home();
    let index = home
        .path()
        .join(".local/share/nightshift/transcripts-index.json");
    fs::create_dir_all(index.parent().unwrap()).unwrap();
    fs::write(&index, "{not json").unwrap();
    let v = run_json(home.path(), &["transcripts", "list"]);
    assert_eq!(ids(&v).len(), 2);
}

#[test]
fn an_empty_claude_config_dir_falls_back_to_the_home_default() {
    let home = home();
    let v = {
        let out = ns(home.path())
            .env("CLAUDE_CONFIG_DIR", "")
            .args(["transcripts", "list"])
            .assert()
            .success()
            .get_output()
            .clone();
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    assert_eq!(ids(&v).len(), 2);
}

#[test]
fn read_pins_the_full_mode_limit_at_2000_characters() {
    let home = home();
    let id = "c0c0aaaa-0000-4000-8000-00000000000a";
    let exact = "y".repeat(2_000);
    let over = "y".repeat(2_001);
    let tool = serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {"command": "z".repeat(2_100)}}]},
    })
    .to_string();
    write_session(
        home.path(),
        "-work-limit",
        id,
        &[
            user_line(&exact, "2026-10-06T00:00:00.000Z", "/work/limit"),
            user_line(&over, "2026-10-06T00:00:01.000Z", "/work/limit"),
            tool,
        ],
    );
    let v = run_json(home.path(), &["transcripts", "read", id]);
    assert_eq!(v["entries"][0]["text"], exact.as_str());
    let cut = v["entries"][1]["text"].as_str().unwrap();
    assert!(
        cut.starts_with(&exact) && cut.ends_with("…[truncated 1 chars]"),
        "{cut}"
    );
    let input = v["entries"][2]["input"].as_str().unwrap();
    assert!(input.contains("[truncated "), "{input}");
    assert!(input.len() < 2_100, "{}", input.len());
}

#[test]
fn the_same_session_id_in_two_project_dirs_is_tracked_and_read_separately() {
    let home = home();
    let id = "d0d0bbbb-0000-4000-8000-00000000000b";
    for dir in ["-work-one", "-work-two"] {
        write_session(
            home.path(),
            dir,
            id,
            &[user_line("hi", "2026-10-02T00:00:00.000Z", "/work/dup")],
        );
    }
    let v = run_json(home.path(), &["transcripts", "list", "--new", "--mark"]);
    assert_eq!(ids(&v).iter().filter(|i| *i == id).count(), 2, "{v}");
    let index = fs::read_to_string(
        home.path()
            .join(".local/share/nightshift/transcripts-index.json"),
    )
    .unwrap();
    assert_eq!(index.matches(id).count(), 2, "{index}");
    let v = run_json(home.path(), &["transcripts", "list", "--new"]);
    assert!(ids(&v).is_empty(), "{v}");
    ns(home.path())
        .args(["transcripts", "read", id])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("-work-one/"))
        .stderr(predicates::str::contains("-work-two/"));
}

#[test]
fn list_project_does_not_confuse_directories_that_share_a_claude_slug() {
    let home = home();
    let id = "e0e0cccc-0000-4000-8000-00000000000c";
    write_session(
        home.path(),
        "-work-a-b",
        id,
        &[user_line("hi", "2026-10-02T00:00:00.000Z", "/work/a/b")],
    );
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--project", "/work/a-b"],
    );
    assert!(ids(&v).is_empty(), "{v}");
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--project", "/work/a/b"],
    );
    assert_eq!(ids(&v), [id]);
}

#[test]
fn list_project_falls_back_to_the_directory_name_when_a_session_has_no_cwd() {
    let home = home();
    let id = "f0f0dddd-0000-4000-8000-00000000000d";
    write_session(
        home.path(),
        "-work-nocwd",
        id,
        &["{\"type\":\"summary\"}".to_string()],
    );
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--project", "/work/nocwd"],
    );
    assert_eq!(ids(&v), [id]);
}

#[test]
fn a_projects_path_that_is_not_a_directory_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    fs::write(tmp.path().join(".claude/projects"), "x").unwrap();
    ns(tmp.path())
        .args(["transcripts", "list"])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("projects"));
}

#[test]
fn a_missing_projects_dir_lists_no_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let v = run_json(tmp.path(), &["transcripts", "list"]);
    assert_eq!(v["sessions"], serde_json::json!([]));
}

#[test]
fn mark_accumulates_across_filtered_runs() {
    let home = home();
    run_json(
        home.path(),
        &["transcripts", "list", "--project", "/work/beta", "--mark"],
    );
    run_json(
        home.path(),
        &["transcripts", "list", "--project", "/work/alpha", "--mark"],
    );
    let v = run_json(home.path(), &["transcripts", "list", "--new"]);
    assert!(ids(&v).is_empty(), "{v}");
}

#[test]
fn a_set_claude_config_dir_is_read_instead_of_the_home_default() {
    let home = home();
    let cfg = home.path().join("cfg");
    let id = "01010eee-0000-4000-8000-00000000000e";
    let path = cfg.join("projects/-work-cfg").join(format!("{id}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        user_line("hi", "2026-10-02T00:00:00.000Z", "/work/cfg") + "\n",
    )
    .unwrap();
    let out = ns(home.path())
        .env("CLAUDE_CONFIG_DIR", &cfg)
        .args(["transcripts", "list"])
        .assert()
        .success()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(ids(&v), [id]);
}

#[test]
fn list_since_keeps_a_session_whose_mtime_is_exactly_midnight_of_the_date() {
    let home = home();
    let path = write_session(
        home.path(),
        "-work-edge",
        "dddd4444-0000-4000-8000-000000000004",
        &[user_line("hi", "2026-10-05T00:00:00.000Z", "/work/edge")],
    );
    set_mtime(&path, 1_791_158_400);
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--since", "2026-10-05"],
    );
    assert!(ids(&v).contains(&"dddd4444-0000-4000-8000-000000000004".to_string()));
}

#[test]
fn list_since_drops_a_recently_touched_session_that_started_earlier() {
    let home = home();
    let path = write_session(
        home.path(),
        "-work-old",
        "eeee5555-0000-4000-8000-000000000005",
        &[user_line("hi", "2026-09-01T00:00:00.000Z", "/work/old")],
    );
    set_mtime(&path, BETA_MTIME);
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--since", "2026-10-05"],
    );
    assert_eq!(ids(&v), [BETA]);
}

#[test]
fn an_empty_home_without_a_claude_config_dir_is_an_error() {
    let home = home();
    ns(home.path())
        .env("HOME", "")
        .args(["transcripts", "list"])
        .assert()
        .code(1);
}

#[test]
fn transcripts_help_has_examples() {
    for args in [
        vec!["transcripts", "--help"],
        vec!["transcripts", "list", "--help"],
        vec!["transcripts", "read", "--help"],
    ] {
        let home = home();
        ns(home.path())
            .args(&args)
            .assert()
            .success()
            .stdout(predicates::str::contains("Examples:"));
    }
}

#[test]
fn list_since_rejects_an_impossible_calendar_date() {
    let home = home();
    for bad in [
        "2026-13-01",
        "2026-00-10",
        "2026-02-30",
        "2026-10-00",
        "2025-02-29",
    ] {
        ns(home.path())
            .args(["transcripts", "list", "--since", bad])
            .assert()
            .code(2)
            .stderr(predicates::str::contains("--since 2026-10-01"));
    }
    run_json(
        home.path(),
        &["transcripts", "list", "--since", "2024-02-29"],
    );
}

#[test]
fn read_an_empty_id_is_a_usage_error() {
    let home = home();
    ns(home.path())
        .args(["transcripts", "read", ""])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("ns transcripts list"));
}

#[test]
fn list_new_with_a_filter_neither_lists_nor_marks_outside_sessions() {
    let home = home();
    run_json(home.path(), &["transcripts", "list", "--mark"]);
    let id = "dddd4444-0000-4000-8000-000000000004";
    write_session(
        home.path(),
        "-work-delta",
        id,
        &[user_line("hi", "2026-10-07T00:00:00.000Z", "/work/delta")],
    );
    let v = run_json(
        home.path(),
        &[
            "transcripts",
            "list",
            "--new",
            "--project",
            "/work/alpha",
            "--mark",
        ],
    );
    assert!(ids(&v).is_empty(), "{v}");
    let v = run_json(home.path(), &["transcripts", "list", "--new"]);
    assert_eq!(ids(&v), [id], "unmarked by a project-filtered run");

    let v = run_json(
        home.path(),
        &[
            "transcripts",
            "list",
            "--new",
            "--since",
            "2026-10-08",
            "--mark",
        ],
    );
    assert!(ids(&v).is_empty(), "{v}");
    let v = run_json(home.path(), &["transcripts", "list", "--new"]);
    assert_eq!(ids(&v), [id], "unmarked by a since-filtered run");
}

#[test]
fn compact_mode_pins_the_limit_at_200_characters() {
    let home = home();
    let id = "c0c0bbbb-0000-4000-8000-00000000000b";
    let tool = |len: usize| {
        serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "name": "Bash", "input": "w".repeat(len)}]},
        })
        .to_string()
    };
    write_session(
        home.path(),
        "-work-compact",
        id,
        &[
            user_line(
                &"y".repeat(200),
                "2026-10-06T00:00:00.000Z",
                "/work/compact",
            ),
            user_line(
                &"y".repeat(201),
                "2026-10-06T00:00:01.000Z",
                "/work/compact",
            ),
            tool(198),
            tool(199),
        ],
    );
    let v = run_json(home.path(), &["transcripts", "read", id, "--compact"]);
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries[0]["text"], "y".repeat(200).as_str());
    let cut = entries[1]["text"].as_str().unwrap();
    assert!(
        cut.starts_with(&"y".repeat(200)) && cut.ends_with("…[truncated 1 chars]"),
        "{cut}"
    );
    let kept = entries[2]["input"].as_str().unwrap();
    assert_eq!(kept.chars().count(), 200, "{kept}");
    assert!(!kept.contains("truncated"), "{kept}");
    let cut = entries[3]["input"].as_str().unwrap();
    assert!(cut.ends_with("…[truncated 1 chars]"), "{cut}");
}

#[cfg(unix)]
#[test]
fn list_project_follows_a_symlink_to_the_recorded_cwd() {
    let home = home();
    let real = home.path().join("real-proj");
    fs::create_dir_all(&real).unwrap();
    let link = home.path().join("link-proj");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let id = "cccc3333-0000-4000-8000-000000000003";
    write_session(
        home.path(),
        "-real-proj",
        id,
        &[user_line(
            "hi",
            "2026-10-02T00:00:00.000Z",
            real.to_str().unwrap(),
        )],
    );
    let v = run_json(
        home.path(),
        &["transcripts", "list", "--project", link.to_str().unwrap()],
    );
    assert_eq!(ids(&v), vec![id.to_string()], "{v}");
}

#[cfg(unix)]
#[test]
fn list_project_resolves_dot_dot_segments() {
    let home = home();
    let real = home.path().join("real-proj");
    fs::create_dir_all(real.join("sub")).unwrap();
    let link = home.path().join("link-proj");
    std::os::unix::fs::symlink(real.join("sub"), &link).unwrap();
    let id = "dddd4444-0000-4000-8000-000000000004";
    write_session(
        home.path(),
        "-real-proj",
        id,
        &[user_line(
            "hi",
            "2026-10-02T00:00:00.000Z",
            real.to_str().unwrap(),
        )],
    );
    let through = link.join("..");
    let v = run_json(
        home.path(),
        &[
            "transcripts",
            "list",
            "--project",
            through.to_str().unwrap(),
        ],
    );
    assert_eq!(ids(&v), vec![id.to_string()], "{v}");
}

#[test]
fn list_ignores_stray_files_and_jsonl_named_directories() {
    let home = home();
    let projects = home.path().join(".claude/projects");
    fs::write(projects.join("notes.txt"), "x").unwrap();
    fs::create_dir_all(projects.join("-work-alpha/stray.jsonl")).unwrap();
    let v = run_json(home.path(), &["transcripts", "list"]);
    assert_eq!(ids(&v).len(), 2, "{v}");
}
