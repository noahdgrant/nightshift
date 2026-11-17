use crate::common::{git, script, Repo};

use assert_cmd::Command;
use serde_json::{json, Value};

/// A unit as the fake `gh` lists it.
fn unit(number: u64, labels: &[&str]) -> String {
    let labels: Vec<_> = labels
        .iter()
        .map(|l| format!(r#"{{"name":"{l}"}}"#))
        .collect();
    format!(
        r#"{{"number":{number},"title":"unit {number}","body":"","labels":[{}],"author_association":"OWNER"}}"#,
        labels.join(",")
    )
}

/// Runs `ns status` against a fake `gh` that lists `issues` and has a change request only on the
/// branches of `change_requests`, each with its state. `runs` is the run log.
fn run_status(
    issues: &[String],
    change_requests: &[(u64, &str)],
    runs: &str,
) -> assert_cmd::assert::Assert {
    run_status_in(&Repo::new(), issues, change_requests, runs)
}

fn run_status_in(
    repo: &Repo,
    issues: &[String],
    change_requests: &[(u64, &str)],
    runs: &str,
) -> assert_cmd::assert::Assert {
    run_ns(repo, issues, change_requests, runs, "[]", &["status"])
}

/// Runs `ns` with `args` against a fake `gh`. It answers `gh api .../issues/<n>` with the issue
/// of that number in `issues` and fails for any other; `checks` is what `gh pr checks` prints.
fn run_ns(
    repo: &Repo,
    issues: &[String],
    change_requests: &[(u64, &str)],
    runs: &str,
    checks: &str,
    args: &[&str],
) -> assert_cmd::assert::Assert {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cases: String = change_requests
        .iter()
        .map(|(unit, state)| {
            format!(
                r#"*"--head ns/{unit} "*) echo '[{{"number":9,"url":"https://forge.example/{unit}","state":"{state}"}}]';;"#
            )
        })
        .collect();
    let one: String = issues
        .iter()
        .map(|issue| {
            let number = serde_json::from_str::<Value>(issue).unwrap()["number"].clone();
            format!("*/issues/{number}) cat <<'EOF'\n{issue}\nEOF\n;;\n")
        })
        .collect();
    script(
        &bin,
        "gh",
        &format!(
            r#"case "$1" in
api) case "$*" in
{one}*/issues/*) echo "gh: Not Found (HTTP 404)" >&2; exit 1;;
*) cat <<'EOF'
[[{}]]
EOF
;; esac;;
pr) case "$*" in "pr checks"*) echo '{checks}';; {cases} *) echo '[]';; esac;;
esac"#,
            issues.join(",")
        ),
    );
    std::fs::create_dir_all(repo.path.join(".git/ns")).unwrap();
    std::fs::write(repo.path.join(".git/ns/runs.jsonl"), runs).unwrap();
    Command::cargo_bin("ns")
        .unwrap()
        .args(args)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

fn status(issues: &[String], change_requests: &[(u64, &str)], runs: &str) -> Value {
    let out = run_status(issues, change_requests, runs).success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

#[test]
fn a_unit_that_never_ran_has_no_state_and_no_change_request() {
    let out = status(&[unit(3, &["status:ready-for-agent"])], &[], "");
    assert_eq!(
        out,
        json!({"units": [{
            "unit": 3, "title": "unit 3", "status": "status:ready-for-agent",
            "state": null, "moved_at": null, "change_request": null, "running": null,
        }]})
    );
}

#[test]
fn a_unit_without_a_status_label_has_a_null_status() {
    let out = status(&[unit(3, &["priority:high"])], &[], "");
    assert_eq!(out["units"][0]["status"], Value::Null);
}

#[test]
fn a_done_unit_shows_its_last_move_and_its_open_change_request() {
    let runs = concat!(
        r#"{"unit":"7","from":"setup","to":"build","at":100}"#,
        "\n",
        r#"{"unit":"8","from":"setup","to":"stuck","at":150}"#,
        "\n",
        r#"{"unit":"7","from":"ship","to":"done","at":200}"#,
        "\n",
    );
    let out = status(
        &[unit(7, &["status:waiting-for-human"])],
        &[(7, "OPEN")],
        runs,
    );
    assert_eq!(
        out["units"][0],
        json!({
            "unit": 7, "title": "unit 7", "status": "status:waiting-for-human",
            "state": "done", "moved_at": 200,
            "change_request": {"url": "https://forge.example/7", "state": "open"},
            "running": null,
        })
    );
}

#[test]
fn a_stuck_unit_shows_stuck() {
    let runs = r#"{"unit":"4","from":"verify","to":"stuck","at":300}"#;
    let out = status(&[unit(4, &["status:waiting-for-human"])], &[], runs);
    assert_eq!(out["units"][0]["state"], "stuck");
    assert_eq!(out["units"][0]["moved_at"], 300);
}

#[test]
fn merged_and_closed_change_requests_are_named() {
    let out = status(
        &[unit(1, &[]), unit(2, &[])],
        &[(1, "MERGED"), (2, "CLOSED")],
        "",
    );
    assert_eq!(out["units"][0]["change_request"]["state"], "merged");
    assert_eq!(out["units"][1]["change_request"]["state"], "closed");
}

#[test]
fn units_come_in_the_order_ns_auto_takes_them() {
    let out = status(
        &[
            unit(3, &[]),
            unit(1, &["priority:low"]),
            unit(2, &["priority:high"]),
        ],
        &[],
        "",
    );
    let numbers: Vec<u64> = out["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["unit"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [2, 1, 3]);
}

#[test]
fn a_repository_with_no_open_units_lists_none() {
    assert_eq!(status(&[], &[], ""), json!({"units": []}));
}

#[test]
fn a_last_move_without_a_time_has_a_null_moved_at() {
    let runs = r#"{"unit":"5","from":"ship","to":"done"}"#;
    let out = status(&[unit(5, &[])], &[], runs);
    assert_eq!(out["units"][0]["state"], "done");
    assert_eq!(out["units"][0]["moved_at"], Value::Null);
}

#[test]
fn a_last_move_with_a_bad_from_state_still_gives_its_state() {
    let runs = r#"{"unit":"5","from":"nowhere","to":"done","at":1}"#;
    let out = run_status(&[unit(5, &[])], &[], runs).success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(json["units"][0]["state"], "done");
    assert_eq!(json["units"][0]["moved_at"], 1);
}

/// A repository whose base branch is published and which has a workspace for unit 7 on `ns/7`,
/// made from that base. Unit 7 has an open change request.
fn open_unit_repo() -> Repo {
    let repo = Repo::new();
    git(&repo.path, &["push", "-q", "origin", "main"]);
    git(
        &repo.path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "ns/7",
            &repo.workspace("7").to_string_lossy(),
        ],
    );
    repo
}

/// Adds a commit to the origin's `main`, through a clone of its own.
fn base_moves(repo: &Repo) {
    let other = repo.path.parent().unwrap().join("other");
    let origin = repo.path.parent().unwrap().join("origin.git");
    git(
        repo.path.parent().unwrap(),
        &["clone", "-q", &origin.to_string_lossy(), "other"],
    );
    git(&other, &["commit", "-q", "--allow-empty", "-m", "other"]);
    git(&other, &["push", "-q", "origin", "main"]);
}

fn base_of(repo: &Repo, runs: &str) -> Value {
    let out = run_status_in(repo, &[unit(7, &[])], &[(7, "OPEN")], runs).success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    json["units"][0]["change_request"]["base"].clone()
}

#[test]
fn an_open_change_request_with_the_base_tip_is_current() {
    let repo = open_unit_repo();
    assert_eq!(base_of(&repo, ""), "current");
}

#[test]
fn an_open_change_request_without_the_base_tip_is_behind_and_status_changes_nothing() {
    let repo = open_unit_repo();
    base_moves(&repo);
    let head = git(&repo.workspace("7"), &["rev-parse", "HEAD"]);
    assert_eq!(base_of(&repo, ""), "behind");
    assert_eq!(git(&repo.workspace("7"), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        std::fs::read_to_string(repo.path.join(".git/ns/runs.jsonl")).unwrap(),
        ""
    );
}

#[test]
fn an_unreachable_origin_leaves_the_base_unknown_and_the_rest_listed() {
    let repo = open_unit_repo();
    std::fs::remove_dir_all(repo.path.parent().unwrap().join("origin.git")).unwrap();
    let out = run_status_in(&repo, &[unit(7, &[])], &[(7, "OPEN")], "").success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(json["units"][0]["change_request"]["base"], "unknown");
    assert_eq!(json["units"][0]["unit"], 7);
    assert_eq!(json["units"][0]["change_request"]["state"], "open");
}

#[test]
fn a_base_tip_a_refresh_tested_is_current() {
    let repo = open_unit_repo();
    base_moves(&repo);
    let tip = git(
        &repo.path.parent().unwrap().join("origin.git"),
        &["rev-parse", "main"],
    );
    let runs = format!("{{\"unit\":\"7\",\"base_tested\":\"{}\"}}\n", tip.trim());
    assert_eq!(base_of(&repo, &runs), "current");
}

#[test]
fn with_the_queue_off_status_shows_no_base() {
    let repo = open_unit_repo();
    base_moves(&repo);
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "[merge]\nqueue = false\n",
    )
    .unwrap();
    assert_eq!(base_of(&repo, ""), Value::Null);
}

#[test]
fn a_merged_change_request_shows_no_base() {
    let repo = open_unit_repo();
    base_moves(&repo);
    let out = run_status_in(&repo, &[unit(7, &[])], &[(7, "MERGED")], "").success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(json["units"][0]["change_request"]["base"], Value::Null);
}

/// Holds `unit-<n>.lock` in the repository's `ns` state directory the way `ns run` does.
fn lock_held(repo: &Repo, unit: u64, record: &str) -> std::fs::File {
    let dir = repo.path.join(".git/ns");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join(format!("unit-{unit}.lock"))).unwrap();
    file.lock().unwrap();
    std::fs::write(dir.join(format!("unit-{unit}.lock")), record).unwrap();
    file
}

fn status_in(repo: &Repo, issues: &[String]) -> Value {
    let out = run_status_in(repo, issues, &[], "").success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

#[test]
fn a_unit_whose_lock_is_held_shows_its_holder_and_another_shows_null() {
    let repo = Repo::new();
    let _held = lock_held(&repo, 3, "4242 1700000000\n");
    let out = status_in(&repo, &[unit(3, &[]), unit(4, &[])]);
    assert_eq!(
        out["units"][0]["running"],
        json!({"pid": 4242, "since": 1700000000})
    );
    assert_eq!(out["units"][1]["running"], Value::Null);
}

#[test]
fn a_lock_file_left_by_a_process_killed_with_sigkill_reads_as_not_running() {
    let repo = Repo::new();
    let dir = repo.path.join(".git/ns");
    std::fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("unit-3.lock");
    // Takes the lock, records itself, then waits; `exec` keeps one process, so SIGKILL ends it.
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"exec 9>"$1"; flock 9; echo "$$ 1700000000" >&9; echo ready; exec sleep 60"#)
        .arg("sh")
        .arg(&lock)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(child.stdout.take().unwrap()),
        &mut ready,
    )
    .unwrap();
    let alive = status_in(&repo, &[unit(3, &[])]);
    assert_eq!(
        alive["units"][0]["running"],
        json!({"pid": child.id(), "since": 1700000000})
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let dead = status_in(&repo, &[unit(3, &[])]);
    assert_eq!(dead["units"][0]["running"], Value::Null);
    assert!(lock.exists(), "reading status removes nothing");
}

#[test]
fn an_in_progress_unit_nobody_runs_is_stale() {
    let out = status(&[unit(3, &["status:in-progress"])], &[], "");
    assert_eq!(
        out["units"][0],
        json!({
            "unit": 3, "title": "unit 3", "status": "status:in-progress",
            "state": null, "moved_at": null, "change_request": null,
            "running": null, "stale": true,
        })
    );
}

#[test]
fn an_in_progress_unit_that_is_run_is_not_stale() {
    let repo = Repo::new();
    let _held = lock_held(&repo, 3, "4242 1700000000\n");
    let out = status_in(&repo, &[unit(3, &["status:in-progress"])]);
    assert_eq!(out["units"][0]["running"]["pid"], 4242);
    assert!(out["units"][0].get("stale").is_none());
}

#[test]
fn a_unit_not_in_progress_is_never_stale() {
    let out = status(
        &[
            unit(1, &["status:ready-for-agent"]),
            unit(2, &["status:stuck"]),
            unit(3, &[]),
        ],
        &[],
        "",
    );
    for unit in out["units"].as_array().unwrap() {
        assert!(unit.get("stale").is_none(), "{unit}");
    }
}

// `ns status <unit>`

fn detail_in(
    repo: &Repo,
    issues: &[String],
    crs: &[(u64, &str)],
    runs: &str,
    checks: &str,
) -> Value {
    detail_with(repo, issues, crs, runs, checks, &[])
}

fn detail_with(
    repo: &Repo,
    issues: &[String],
    crs: &[(u64, &str)],
    runs: &str,
    checks: &str,
    flags: &[&str],
) -> Value {
    let mut args = vec!["status", "7"];
    args.extend(flags);
    let out = run_ns(repo, issues, crs, runs, checks, &args).success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

/// A repository with unit 7's workspace, its recorded base at `main`, and `files` committed in it.
fn workspace_with(files: &[(&str, &str)]) -> Repo {
    let repo = open_unit_repo();
    git(&repo.path, &["update-ref", "refs/ns/base/ns/7", "main"]);
    let ws = repo.workspace("7");
    for (name, text) in files {
        let path = ws.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    git(&ws, &["add", "-A", "-f"]);
    git(&ws, &["commit", "-q", "-m", "change", "--allow-empty"]);
    repo
}

#[test]
fn a_unit_is_described_with_its_base_fields_and_nothing_is_changed() {
    let repo = Repo::new();
    let runs = "{\"unit\":\"7\",\"from\":\"setup\",\"to\":\"build\",\"at\":100}\n";
    let issues = [unit(7, &["status:in-progress"])];
    let out = detail_in(&repo, &issues, &[], runs, "[]");
    assert_eq!(out["unit"], 7);
    assert_eq!(out["title"], "unit 7");
    assert_eq!(out["status"], "status:in-progress");
    assert_eq!(out["state"], "build");
    assert_eq!(out["moved_at"], 100);
    assert_eq!(out["running"], Value::Null);
    assert_eq!(out["change_request"], Value::Null);
    assert!(out.get("stopped").is_none());
    assert!(out.get("needs").is_none());
    assert!(out.get("artifacts").is_none());
    assert!(out.get("findings").is_none());
    // Nothing the run changed: the log, the labels (no `gh` call that writes) and the workspace.
    assert_eq!(
        std::fs::read_to_string(repo.path.join(".git/ns/runs.jsonl")).unwrap(),
        runs
    );
    assert!(!repo.workspace("7").exists());
    assert_eq!(
        std::fs::read_dir(repo.path.join(".git/ns"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn a_unit_that_does_not_exist_is_an_error_naming_it() {
    let repo = Repo::new();
    let issues = [unit(7, &[])];
    let failed = run_ns(&repo, &issues, &[], "", "[]", &["status", "999"]).failure();
    let stderr = String::from_utf8_lossy(&failed.get_output().stderr).to_string();
    assert!(stderr.contains("unit 999"), "{stderr}");
}

#[test]
fn findings_without_a_unit_is_refused() {
    let repo = Repo::new();
    run_ns(
        &repo,
        &[unit(7, &[])],
        &[],
        "",
        "[]",
        &["status", "--findings"],
    )
    .failure();
}

#[test]
fn the_attempt_and_the_most_attempts_a_state_gets() {
    let repo = Repo::new();
    let runs = concat!(
        r#"{"unit":"7","from":"setup","to":"build","at":1}"#,
        "\n",
        r#"{"unit":"7","from":"build","to":"build","reason":"failed","attempt":1,"to_attempt":2,"at":2}"#,
        "\n",
    );
    let issues = [unit(7, &["status:in-progress"])];
    let out = detail_in(&repo, &issues, &[], runs, "[]");
    assert_eq!(
        (out["attempt"].clone(), out["max_attempts"].clone()),
        (json!(2), json!(3))
    );
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "[run]\nmax_attempts = 5\n",
    )
    .unwrap();
    let out = detail_in(&repo, &issues, &[], runs, "[]");
    assert_eq!(out["max_attempts"], 5);
}

#[test]
fn checks_are_pending_passing_or_failing_with_the_names_of_the_failing() {
    let repo = Repo::new();
    let issues = [unit(7, &[])];
    let checks =
        |json: &str| detail_in(&repo, &issues, &[(7, "OPEN")], "", json)["change_request"].clone();
    let url = "https://forge.example/7";
    assert_eq!(
        checks(r#"[{"name":"a","bucket":"pass"}]"#),
        json!({"url": url, "state": "open", "checks": {"state": "passing"}})
    );
    assert_eq!(
        checks(r#"[{"name":"a","bucket":"pending"}]"#),
        json!({"url": url, "state": "open", "checks": {"state": "pending"}})
    );
    assert_eq!(
        checks("[]"),
        json!({"url": url, "state": "open", "checks": {"state": "pending"}})
    );
    assert_eq!(
        checks(
            r#"[{"name":"lint","bucket":"fail"},{"name":"test","bucket":"fail"},{"name":"a","bucket":"pass"}]"#
        ),
        json!({"url": url, "state": "open", "checks": {"state": "failing", "failing": ["lint", "test"]}})
    );
    // A merged request has no checks to wait for.
    let merged = detail_in(&repo, &issues, &[(7, "MERGED")], "", "[]")["change_request"].clone();
    assert_eq!(merged, json!({"url": url, "state": "merged"}));
}

#[test]
fn a_stuck_unit_says_why_and_in_which_state() {
    let repo = Repo::new();
    let issues = [unit(7, &["status:stuck"])];
    for reason in ["failed", "went_wrong", "inconclusive", "base_moved"] {
        let runs =
            format!(r#"{{"unit":"7","from":"verify","to":"stuck","reason":"{reason}","at":5}}"#);
        let out = detail_in(&repo, &issues, &[], &runs, "[]");
        assert_eq!(out["stopped"], json!({"reason": reason, "state": "verify"}));
        assert_eq!(out["attempt"], 1);
    }
}

#[test]
fn a_unit_that_is_running_or_done_has_no_stopped() {
    let repo = Repo::new();
    let issues = [unit(7, &[])];
    for last in [
        r#""from":"build","to":"verify""#,
        r#""from":"ship","to":"done""#,
    ] {
        let runs = format!("{{\"unit\":\"7\",{last}}}\n");
        let out = detail_in(&repo, &issues, &[], &runs, "[]");
        assert!(out.get("stopped").is_none(), "{out}");
    }
}

#[test]
fn a_change_held_for_review_names_its_files_and_regions() {
    let repo = workspace_with(&[("src/a.rs", "fn a() {}\n")]);
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "[merge]\nhuman_review = [\"src/**\"]\n",
    )
    .unwrap();
    let issues = [unit(7, &["status:waiting-for-human"])];
    let out = detail_in(&repo, &issues, &[(7, "OPEN")], "", "[]");
    assert_eq!(
        out["needs"],
        json!({"review": ["src/a.rs matches src/** in merge.human_review, so a person merges it"]})
    );
}

#[test]
fn a_change_in_a_human_review_region_is_named_with_its_lines() {
    let marker = |s: &str| format!("// ns:human-{s}\n");
    let before = format!(
        "{}secret\n{}",
        marker("review start: money"),
        marker("review end")
    );
    let repo = Repo::new();
    git(&repo.path, &["push", "-q", "origin", "main"]);
    std::fs::write(repo.path.join("pay.rs"), &before).unwrap();
    git(&repo.path, &["add", "pay.rs"]);
    git(&repo.path, &["commit", "-q", "-m", "pay"]);
    git(
        &repo.path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "ns/7",
            &repo.workspace("7").to_string_lossy(),
        ],
    );
    git(&repo.path, &["update-ref", "refs/ns/base/ns/7", "main"]);
    let ws = repo.workspace("7");
    std::fs::write(ws.join("pay.rs"), before.replace("secret", "changed")).unwrap();
    git(&ws, &["commit", "-q", "-am", "edit"]);
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "[merge]\nhuman_review = []\n",
    )
    .unwrap();
    let issues = [unit(7, &["status:waiting-for-human"])];
    let out = detail_in(&repo, &issues, &[(7, "OPEN")], "", "[]");
    let holds = out["needs"]["review"].as_array().unwrap();
    assert_eq!(holds.len(), 1, "{out}");
    let why = holds[0].as_str().unwrap();
    assert!(
        why.contains("pay.rs changes lines 1-3") && why.contains("money"),
        "{why}"
    );
}

#[test]
fn a_unit_stuck_as_inconclusive_lists_the_requirements_still_unproven() {
    let repo = workspace_with(&[(
        ".ns/7/verify-1.md",
        "### R1\nPASS\n\n### R2\nINCONCLUSIVE: needs staging\n\nVERDICT: INCONCLUSIVE\n",
    )]);
    let runs = r#"{"unit":"7","from":"verify","to":"stuck","reason":"inconclusive","at":5}"#;
    let issues = [unit(7, &["status:stuck"])];
    let out = detail_in(&repo, &issues, &[], runs, "[]");
    assert_eq!(
        out["needs"],
        json!({"unproven": ["### R2\nINCONCLUSIVE: needs staging\n"]})
    );
    // The workspace is where the list comes from: without it there is none.
    git(
        &repo.path,
        &[
            "worktree",
            "remove",
            "--force",
            &repo.workspace("7").to_string_lossy(),
        ],
    );
    let out = detail_in(&repo, &issues, &[], runs, "[]");
    assert!(out.get("needs").is_none(), "{out}");
}

#[test]
fn a_unit_waiting_for_a_person_for_another_reason_has_no_needs() {
    // Waiting with no open change request: ns does not know what the person must do.
    let repo = workspace_with(&[("a.txt", "x\n")]);
    let issues = [unit(7, &["status:waiting-for-human"])];
    let out = detail_in(&repo, &issues, &[], "", "[]");
    assert!(out.get("needs").is_none(), "{out}");
    // An open request that touches nothing a person must review.
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "[merge]\nhuman_review = []\n",
    )
    .unwrap();
    let out = detail_in(&repo, &issues, &[(7, "OPEN")], "", "[]");
    assert!(out.get("needs").is_none(), "{out}");
}

#[test]
fn artifacts_are_the_paths_of_the_build_the_latest_verification_and_the_latest_review() {
    let repo = workspace_with(&[
        (".ns/7/build.md", "built"),
        (".ns/7/verify-1.md", "x"),
        (".ns/7/verify-2.md", "y"),
        (".ns/7/review-1.md", "z"),
    ]);
    let ws = repo.workspace("7");
    let out = detail_in(&repo, &[unit(7, &[])], &[], "", "[]");
    let dir = ws.join(".ns/7");
    assert_eq!(
        out["artifacts"],
        json!({
            "build": dir.join("build.md"),
            "verify": dir.join("verify-2.md"),
            "review": dir.join("review-1.md"),
        })
    );
}

const FAILED_VERIFICATION: &str =
    "### R1\nPASS\n\n### R2\nFAIL: wrong\n\n### R3\nFAIL: also wrong\n\nVERDICT: FAIL\n";
const REVIEW_OF_THREE: &str = "# Review session: Behavior\n\n`a.rs:1` bad\nFix level: none\n\n# Review session: Requirements and tests\n\nVERDICT: PASS\n\n# Review session: Maintainability\n\n`b.rs:2` bad\nFix level: code\nDurable fix: T\n\n`c.rs:3` bad\nFix level: prose\n\nVERDICT: FAIL\n";

#[test]
fn findings_are_counted_for_the_latest_verification_and_each_review_session() {
    let repo = workspace_with(&[
        (".ns/7/verify-1.md", "### R1\nPASS\nVERDICT: PASS\n"),
        (".ns/7/verify-2.md", FAILED_VERIFICATION),
        (".ns/7/review-1.md", REVIEW_OF_THREE),
    ]);
    let issues = [unit(7, &[])];
    let out = detail_in(&repo, &issues, &[], "", "[]");
    assert_eq!(
        out["findings"],
        json!({
            "verification": 2,
            "review": {"Behavior": 1, "Requirements and tests": 0, "Maintainability": 2},
        })
    );
    // With the flag the text comes too, and the counts are as before.
    let out = detail_with(&repo, &issues, &[], "", "[]", &["--findings"]);
    assert_eq!(out["findings"]["verification"], 2);
    assert_eq!(
        out["findings"]["text"]["review"]["Behavior"],
        json!(["`a.rs:1` bad\nFix level: none"])
    );
    assert_eq!(
        out["findings"]["text"]["verification"][0],
        "### R2\nFAIL: wrong"
    );
}

#[test]
fn a_unit_whose_checks_never_ran_has_no_findings() {
    let repo = workspace_with(&[(".ns/7/build.md", "built")]);
    let out = detail_in(&repo, &[unit(7, &[])], &[], "", "[]");
    assert!(out.get("findings").is_none(), "{out}");
}
