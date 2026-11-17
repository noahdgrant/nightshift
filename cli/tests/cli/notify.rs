use crate::auto::auto_env;
use crate::common::Repo;
use crate::run::{lines, manifest_set, tools, Fakes};

use assert_cmd::Command;

/// Writes a machine config whose `[notify]` runs a command that appends the `NS_NOTIFY_*` it is
/// given to `calls`, and returns the config's path and `calls`.
fn machine(repo: &Repo, on: Option<&str>) -> (std::path::PathBuf, std::path::PathBuf) {
    let tmp = repo.path.parent().unwrap();
    let calls = tmp.join("calls");
    let mut text = format!(
        "[notify]\ncommand = \"env | grep '^NS_NOTIFY_' | sort >> {}; echo --- >> {}\"\n",
        calls.display(),
        calls.display()
    );
    if let Some(on) = on {
        text.push_str(&format!("on = {on}\n"));
    }
    let config = tmp.join("machine.toml");
    std::fs::write(&config, text).unwrap();
    (config, calls)
}

/// Writes a machine config with `text` and returns its path.
fn config_with(repo: &Repo, text: &str) -> std::path::PathBuf {
    let config = repo.path.parent().unwrap().join("machine.toml");
    std::fs::write(&config, text).unwrap();
    config
}

fn run_with_config(
    repo: &Repo,
    fakes: &Fakes,
    config: &std::path::Path,
    extra: &[&str],
) -> assert_cmd::assert::Assert {
    let mut args = vec!["run", "5", "--ci-poll-seconds", "0", "--max-attempts", "2"];
    args.extend(extra);
    Command::cargo_bin("ns")
        .unwrap()
        .args(args)
        .env("PATH", tools(repo, fakes))
        .env("NS_CONFIG", config)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .current_dir(&repo.path)
        .assert()
}

fn calls(file: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .split("---\n")
        .filter(|c| !c.is_empty())
        .map(String::from)
        .collect()
}

fn log_lines(repo: &Repo) -> Vec<serde_json::Value> {
    lines(&std::fs::read(repo.path.join(".git/ns/runs.jsonl")).unwrap_or_default())
}

fn broken() -> Fakes {
    Fakes {
        exit: 1,
        ..Fakes::passing()
    }
}

#[test]
fn a_bad_notify_setting_stops_run_and_auto_before_anything_runs() {
    for bad in [
        "[notify]\ncommand = 3\n",
        "[notify]\ncommand = \"c\"\non = [\"asleep\"]\n",
        "[notify]\ncommand = \"c\"\non = \"stuck\"\n",
    ] {
        let repo = Repo::new();
        let config = config_with(&repo, bad);
        let out = run_with_config(&repo, &Fakes::passing(), &config, &[]).failure();
        let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
        assert!(err.contains("notify."), "{bad}: {err}");
        assert!(out.get_output().stdout.is_empty(), "{bad}");
        assert!(log_lines(&repo).is_empty(), "{bad}");
        let out = auto_env(
            &repo,
            &Fakes::passing(),
            &["--dry-run"],
            &[("NS_CONFIG", config.to_str().unwrap())],
        )
        .failure();
        assert!(out.get_output().stdout.is_empty(), "{bad}");
    }
}

#[test]
fn a_stuck_unit_runs_the_command_once_with_its_facts() {
    let repo = Repo::new();
    let (config, calls_file) = machine(&repo, None);
    run_with_config(&repo, &broken(), &config, &["--gates", "auto"]).failure();
    let made = calls(&calls_file);
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(
        made[0],
        "NS_NOTIFY_MOVE=stuck\nNS_NOTIFY_REASON=went_wrong\nNS_NOTIFY_STATE=stuck\n\
         NS_NOTIFY_TITLE=Add a thing\nNS_NOTIFY_UNIT=5\nNS_NOTIFY_URL=\n"
    );
}

#[test]
fn nothing_runs_without_a_notify_table_or_for_a_move_not_listed() {
    let repo = Repo::new();
    let (_, calls_file) = machine(&repo, None);
    let none = config_with(&repo, "[other]\nkey = 1\n");
    run_with_config(&repo, &broken(), &none, &["--gates", "auto"]).failure();
    let (merged_only, _) = machine(&repo, Some("[\"merged\"]"));
    run_with_config(&repo, &broken(), &merged_only, &["--gates", "auto"]).failure();
    assert!(calls(&calls_file).is_empty());
}

#[test]
fn a_notify_table_in_the_repositorys_file_is_ignored() {
    let repo = Repo::new();
    let tmp = repo.path.parent().unwrap();
    let calls_file = tmp.join("calls");
    manifest_set(
        &repo,
        &format!(
            "[notify]\ncommand = \"echo x >> {}\"\n",
            calls_file.display()
        ),
    );
    let none = config_with(&repo, "");
    run_with_config(&repo, &broken(), &none, &["--gates", "auto"]).failure();
    assert!(!calls_file.exists());
}

#[test]
fn a_command_that_fails_changes_nothing_about_the_unit() {
    let moves = |out: &assert_cmd::assert::Assert| -> Vec<String> {
        lines(&out.get_output().stdout)
            .iter()
            .filter_map(|e| e["to"].as_str().map(String::from))
            .collect()
    };
    let plain = Repo::new();
    let quiet = config_with(&plain, "");
    let expected =
        moves(&run_with_config(&plain, &broken(), &quiet, &["--gates", "auto"]).failure());
    let repo = Repo::new();
    let config = config_with(&repo, "[notify]\ncommand = \"exit 1\"\n");
    let out = run_with_config(&repo, &broken(), &config, &["--gates", "auto"]).failure();
    assert_eq!(moves(&out), expected);
    let log = log_lines(&repo);
    let failed: Vec<_> = log.iter().filter(|e| e["notify"] == "failed").collect();
    assert_eq!(failed.len(), 1, "{log:?}");
    assert_eq!(failed[0]["move"], "stuck");
    let last_move = log.iter().rfind(|e| e.get("to").is_some()).unwrap();
    assert_eq!(last_move["to"], "stuck");
}

#[test]
fn a_gate_stop_tells_waiting_for_human_once() {
    let repo = Repo::new();
    let (config, calls_file) = machine(&repo, None);
    run_with_config(&repo, &Fakes::passing(), &config, &[]).success();
    let made = calls(&calls_file);
    assert_eq!(made.len(), 1, "{made:?}");
    assert!(
        made[0].contains("NS_NOTIFY_MOVE=waiting_for_human\n"),
        "{made:?}"
    );
    assert!(made[0].contains("NS_NOTIFY_REASON=gate\n"), "{made:?}");
}

#[test]
fn a_change_request_ready_to_merge_tells_waiting_for_human_with_its_url() {
    let repo = Repo::new();
    let (config, calls_file) = machine(&repo, None);
    run_with_config(&repo, &Fakes::passing(), &config, &["--gates", "auto"]).success();
    let made = calls(&calls_file);
    assert_eq!(made.len(), 1, "{made:?}");
    assert!(
        made[0].contains("NS_NOTIFY_MOVE=waiting_for_human\n"),
        "{made:?}"
    );
    assert!(made[0].contains("NS_NOTIFY_STATE=done\n"), "{made:?}");
    assert!(
        made[0].contains("NS_NOTIFY_URL=https://forge.example/9\n"),
        "{made:?}"
    );
}

#[test]
fn a_merged_unit_tells_merged_once_and_not_waiting() {
    let repo = Repo::new();
    manifest_set(&repo, "[merge]\nhuman_review = [\".github/**\"]\n");
    let (config, calls_file) = machine(&repo, None);
    run_with_config(
        &repo,
        &Fakes::passing(),
        &config,
        &["--gates", "auto", "--merge"],
    )
    .success();
    let made = calls(&calls_file);
    assert_eq!(made.len(), 1, "{made:?}");
    assert!(made[0].contains("NS_NOTIFY_MOVE=merged\n"), "{made:?}");
    assert!(
        made[0].contains("NS_NOTIFY_URL=https://forge.example/9\n"),
        "{made:?}"
    );
}

#[test]
fn a_unit_resumed_into_stuck_again_does_not_tell_stuck_twice() {
    let repo = Repo::new();
    let (config, calls_file) = machine(&repo, None);
    run_with_config(&repo, &broken(), &config, &["--gates", "auto"]).failure();
    run_with_config(&repo, &broken(), &config, &["--gates", "auto"]).failure();
    let made = calls(&calls_file);
    assert_eq!(made.len(), 1, "{made:?}");
    // A move it has not made yet still tells.
    run_with_config(&repo, &Fakes::passing(), &config, &["--gates", "auto"]).success();
    let made = calls(&calls_file);
    assert_eq!(made.len(), 2, "{made:?}");
    assert!(
        made[1].contains("NS_NOTIFY_MOVE=waiting_for_human\n"),
        "{made:?}"
    );
}

#[test]
fn auto_tells_night_ended_once_with_the_summary_and_no_unit() {
    for fakes in [
        Fakes {
            units: &[],
            ..Fakes::passing()
        },
        Fakes::passing(),
    ] {
        let repo = Repo::new();
        let (config, calls_file) = machine(&repo, Some("[\"night_ended\"]"));
        auto_env(
            &repo,
            &fakes,
            &[],
            &[("NS_CONFIG", config.to_str().unwrap())],
        )
        .success();
        let made = calls(&calls_file);
        assert_eq!(made.len(), 1, "{made:?}");
        let lines: Vec<_> = made[0].lines().collect();
        assert_eq!(lines[0], "NS_NOTIFY_MOVE=night_ended");
        assert_eq!(lines.len(), 2, "{made:?}");
        let summary = lines[1].strip_prefix("NS_NOTIFY_SUMMARY=").unwrap();
        assert!(std::path::Path::new(summary).is_file(), "{summary}");
        assert!(summary.contains("night-"), "{summary}");
    }
}

#[test]
fn auto_tells_nothing_on_a_dry_run_or_when_night_ended_is_not_listed() {
    let repo = Repo::new();
    let (config, calls_file) = machine(&repo, None);
    let env = [("NS_CONFIG", config.to_str().unwrap())];
    auto_env(&repo, &Fakes::passing(), &["--dry-run"], &env).success();
    assert!(calls(&calls_file).is_empty());
    let (config, calls_file) = machine(&repo, Some("[\"merged\"]"));
    auto_env(
        &repo,
        &Fakes::passing(),
        &[],
        &[("NS_CONFIG", config.to_str().unwrap())],
    )
    .success();
    assert!(calls(&calls_file).is_empty());
}
