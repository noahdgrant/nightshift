#[test]
fn a_unit_that_stays_over_a_ratchet_ends_stuck_naming_its_values() {
    let e = ratchet_env("120\n");
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    let line = "ratchet flash is worse: measured 120, recorded 100, tolerance 5%, limit 105, lower is better";
    assert_eq!(v["outcome"], "stuck");
    assert_eq!(
        v["reason"],
        format!("build is out of attempts (2): the CI gate failed on {line}")
    );
    assert_eq!(e.calls(), ["triage", "build", "build"]);
    let p = e.prompt(3, "build");
    assert!(p.contains(&format!("It failed on {line}.")), "{p}");
    assert!(p.contains("never edit ratchets.json"), "{p}");
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["green"], false);
    let report = g[0]["log"]
        .as_str()
        .unwrap()
        .replace(".log", ".ratchets.json");
    let r: Value = serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
    assert_eq!(r["ratchets"][0]["status"], "worse");
}

#[test]
fn a_build_that_gets_back_under_the_line_goes_on() {
    let e = ratchet_env("120\n");
    e.ctl(
        "build.sh",
        &format!("echo 99 > {}\n", e.ctrl.join("flash").display()),
    );
    e.queue("build", &["pass:commit", "pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(
        (g[0]["green"].as_bool(), g[1]["green"].as_bool()),
        (Some(false), Some(true))
    );
}

#[test]
fn a_build_that_writes_nothing_after_a_red_ratchet_is_stuck_naming_its_values() {
    let e = ratchet_env("120\n");
    e.queue("build", &["pass:commit", "none"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate failed on ratchet flash is worse: measured 120, recorded 100, tolerance 5%, limit 105, lower is better"
    );
    assert_eq!(gate_events(&e).len(), 1);
}

/// A factory whose ratchet `steady` prints 5, recorded at 5, and logs each run to `ctrl/count`.
fn counted_ratchet() -> String {
    "[ratchets.steady]\ncommand = \"echo x >> $FAKE_CTRL/count; echo 5\"\nbetter = \"lower\"\n"
        .into()
}

fn ratchet_runs(e: &Env) -> usize {
    fs::read_to_string(e.ctrl.join("count"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn without_a_night_every_gate_runs_a_ratchet_twice_whatever_the_environment_says() {
    let e = Env::new();
    e.factory(&counted_ratchet());
    fs::write(
        e.root.join(".nightshift/ratchets.json"),
        r#"{"version": 1, "values": {"steady": 5}}"#,
    )
    .unwrap();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["pass:commit"]);
    let steady = e.base.join("leaked-steady.json");
    let out = e
        .ns()
        .env("NS_RATCHET_STEADY", &steady)
        .args(["run", "--issue", "7"])
        .output();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(gate_events(&e).len(), 2);
    assert_eq!(ratchet_runs(&e), 4);
    assert!(!steady.exists());
}

#[test]
fn under_ns_watch_only_a_ratchets_first_check_of_the_night_runs_it_twice() {
    let e = Env::new();
    e.factory(&counted_ratchet());
    fs::write(
        e.root.join(".nightshift/ratchets.json"),
        r#"{"version": 1, "values": {"steady": 5}}"#,
    )
    .unwrap();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(gate_events(&e).len(), 2);
    assert_eq!(ratchet_runs(&e), 3);
}

fn phase_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "phase")
        .collect()
}

fn run_with_phase_timeout(e: &Env, code: i32) -> Value {
    run_with_timeouts(e, "build=1500", code)
}

fn run_with_timeouts(e: &Env, timeouts: &str, code: i32) -> Value {
    let out = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", timeouts)
        .args(["run", "--issue", "7"])
        .assert()
        .code(code)
        .get_output()
        .clone();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn build_event(e: &Env, attempt: u64) -> Value {
    phase_events(e)
        .into_iter()
        .find(|p| p["phase"] == "build" && p["attempt"] == attempt)
        .unwrap()
}

#[test]
fn a_timed_out_phase_ignores_its_artifact_and_retries() {
    let e = Env::new();
    e.queue("build", &["blocked:sleep", "pass:commit"]);
    let v = run_with_phase_timeout(&e, 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "verify", "review", "ship"]
    );
    let dir = e.worktree(UNIT).join(".ns").join(UNIT);
    let archived = dir.join("history/build-timeout-1.md");
    assert!(fs::read_to_string(&archived)
        .unwrap()
        .contains("status: blocked"));
    assert!(fs::read_to_string(dir.join("build.md"))
        .unwrap()
        .contains("status: pass"));
    let p = build_event(&e, 1);
    assert_eq!(p["timed_out"], true);
    assert_eq!(p["written"], false);
    assert_eq!(p["reason"], "timed out after 1.5 s");
    assert_eq!(p["archived"], archived.to_str().unwrap());
    assert!(e.prompt(3, "build").contains("attempt 2"));
}

#[test]
fn a_phase_that_times_out_on_every_attempt_is_stuck_on_the_timeout() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 1\n");
    e.queue("build", &["blocked:sleep"]);
    let v = run_with_phase_timeout(&e, 1);
    assert_eq!(v["outcome"], "stuck");
    let reason = v["reason"].as_str().unwrap();
    assert_eq!(
        reason,
        "build is out of attempts (1): the last attempt timed out after 1.5 s; raise its timeout_minutes or split the unit"
    );
    assert_eq!(e.calls(), ["triage", "build"]);
    let dir = e.worktree(UNIT).join(".ns").join(UNIT);
    assert!(!dir.join("build.md").exists());
    assert!(dir.join("history/build-timeout-1.md").exists());
}

#[test]
fn a_timeout_then_a_plain_failure_is_not_stuck_on_a_timeout() {
    let e = Env::new();
    e.queue("build", &["blocked:sleep", "none"]);
    let v = run_with_phase_timeout(&e, 1);
    assert_eq!(v["outcome"], "stuck");
    assert_eq!(v["reason"], "build is out of attempts (2)");
    assert_eq!(build_event(&e, 2)["reason"], "no artifact written");
}

#[test]
fn repeated_timeouts_archive_each_artifact_separately() {
    let e = Env::new();
    e.queue("build", &["blocked:sleep", "fail:sleep"]);
    let v = run_with_phase_timeout(&e, 1);
    assert_eq!(v["outcome"], "stuck");
    let hist = e.worktree(UNIT).join(".ns").join(UNIT).join("history");
    assert!(fs::read_to_string(hist.join("build-timeout-1.md"))
        .unwrap()
        .contains("status: blocked"));
    assert!(fs::read_to_string(hist.join("build-timeout-2.md"))
        .unwrap()
        .contains("status: fail"));
}

#[test]
fn a_timed_out_review_that_moved_head_still_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["blocked:commit+sleep", "pass"]);
    let v = run_with_timeouts(&e, "review=1500", 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "review");
    assert_eq!(g[1]["attempt"], 1);
}

#[test]
fn a_timed_out_ship_keeps_its_pr_number_for_the_merged_guard() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:merge+sleep"]);
    let v = run_with_timeouts(&e, "ship=1500", 1);
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("PR merged by run"),
        "{v}"
    );
    let dir = e.worktree(UNIT).join(".ns").join(UNIT);
    assert!(!dir.join("pr.md").exists());
    assert!(dir.join("history/pr-timeout-1.md").exists());
}

#[test]
fn a_timed_out_build_does_not_run_the_gate() {
    let e = Env::new();
    let runs = e.ctrl.join("gate-runs");
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\nmax_attempts = 3\ngate = \"echo ran >> {r}; test -f {f} || {{ touch {f}; exit 1; }}\"\n",
        r = runs.display(),
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "blocked:sleep", "pass:commit"]);
    let v = run_with_phase_timeout(&e, 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "build", "verify", "review", "ship"]
    );
    assert_eq!(fs::read_to_string(&runs).unwrap().lines().count(), 2);
    let attempts: Vec<_> = gate_events(&e)
        .iter()
        .map(|g| g["attempt"].clone())
        .collect();
    assert_eq!(attempts, [1, 3]);
    assert_eq!(build_event(&e, 2)["reason"], "timed out after 1.5 s");
    assert_eq!(
        build_event(&e, 3)["decision"],
        "timed out with the CI gate still red"
    );
    assert!(e.prompt(4, "build").contains("attempt 3"));
}

#[test]
fn a_red_gate_is_not_skipped_when_the_rebuild_writes_no_artifact() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\nmax_attempts = 3\ngate = \"test -f {f} || {{ touch {f}; echo boom; exit 1; }}\"\n",
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "none", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(4, "build").contains("boom"));
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["green"], true);
}

#[test]
fn a_red_gate_with_a_silent_rebuild_never_reaches_verify() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"exit 1\"\n");
    e.queue("build", &["pass:commit", "none"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["reason"], "build is out of attempts (2)");
    assert_eq!(e.calls(), ["triage", "build", "build"]);
}

#[test]
fn a_rebuild_that_passes_at_the_same_head_reruns_a_red_gate() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\ngate = \"test -f {f} || {{ touch {f}; echo boom; exit 1; }}\"\n",
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "verify", "review", "ship"]
    );
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["green"], false);
    assert_eq!(g[1]["green"], true);
    assert_eq!(g[0]["sha"], g[1]["sha"]);
}

#[test]
fn a_red_gate_with_a_committing_rebuild_that_writes_no_artifact_never_reaches_verify() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 3\ngate = \"exit 1\"\n");
    e.queue("build", &["pass:commit", "none:commit", "none:commit"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    assert!(
        !e.calls().contains(&"verify".to_string()),
        "{:?}",
        e.calls()
    );
}

#[test]
fn a_review_that_fails_after_moving_head_still_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "review");
}

#[test]
fn a_verify_that_fails_after_moving_head_still_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["fail:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "verify");
}

#[test]
fn a_ship_that_fails_after_moving_head_still_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["fail:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "ship");
}

#[test]
fn a_build_that_did_not_pass_runs_no_gate() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 5\ngate = \"true\"\n");
    e.queue("build", &["none", "fail", "blocked:no"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    assert_eq!(e.calls(), ["triage", "build", "build", "build"]);
    assert!(gate_events(&e).is_empty());
}

#[test]
fn without_a_gate_none_runs() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    assert!(gate_events(&e).is_empty());
}

#[test]
fn blocked_artifact_is_stuck() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["blocked:needs hardware"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert!(v["reason"]
        .as_str()
        .unwrap()
        .contains("evidence.md is blocked"));
    assert!(v["artifact"].as_str().unwrap().ends_with("evidence.md"));
}

const QUALITY_BRANCH: &str = "refs/heads/nightshift/quality";

/// The quality records on the bare `origin`'s data branch.
fn quality_records(e: &Env) -> Vec<Value> {
    let remote = e.base.join("remote.git");
    if git(&remote, &["for-each-ref", QUALITY_BRANCH]).is_empty() {
        return Vec::new();
    }
    git(
        &remote,
        &["show", &format!("{QUALITY_BRANCH}:records.jsonl")],
    )
    .lines()
    .map(|l| serde_json::from_str(l).unwrap())
    .collect()
}

fn quality_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "quality_record")
        .collect()
}

const FINDING: &str = "## Important\n### I1. Untested branch\n- Location: `work.txt:1`\n- Axis: tests\n- Scope: changed\n- Cycle: 0\n- Status: fixed (cycle 1, abc1234)\n";

#[test]
fn a_unit_that_ends_pushes_its_quality_record() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.ctl("review.body", FINDING);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let records = quality_records(&e);
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert_eq!(r["unit"], UNIT);
    assert_eq!(
        (r["issue"].as_u64(), r["outcome"].as_str()),
        (Some(7), Some("done"))
    );
    assert_eq!(r["artifact"], format!(".ns/{UNIT}/review.md"));
    assert_eq!(r["status"], "pass");
    assert_eq!(r["findings"][0]["id"], "I1");
    assert_eq!(r["findings"][0]["axes"], serde_json::json!(["tests"]));
    let ev = quality_events(&e);
    assert_eq!(ev.len(), 1);
    assert_eq!(
        (ev[0]["status"].as_str(), ev[0]["records"].as_u64()),
        (Some("pushed"), Some(1))
    );
    assert_eq!(ev[0]["unit"], UNIT);
}

#[test]
fn every_review_attempt_gets_a_record() {
    let e = Env::new();
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("review", &["fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let got: Vec<_> = quality_records(&e)
        .iter()
        .map(|r| {
            (
                r["attempt"].as_u64().unwrap(),
                r["attempts"].as_u64().unwrap(),
                r["status"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [(1, 2, "fail".to_string()), (2, 2, "pass".to_string())]
    );
}

#[test]
fn a_blocked_review_is_recorded_as_stuck() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue(
        "review",
        &["blocked:Open after at least 2 fix attempts: I1."],
    );
    e.ctl("review.body", FINDING);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    let records = quality_records(&e);
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0]["outcome"].as_str(),
            records[0]["status"].as_str()
        ),
        (Some("stuck"), Some("blocked"))
    );
}

#[test]
fn a_unit_with_no_review_writes_no_record() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["blocked:needs hardware"]);
    e.run(&["run", "--issue", "7"], 1);
    assert!(quality_records(&e).is_empty());
    let ev = quality_events(&e);
    assert_eq!(
        (ev[0]["status"].as_str(), ev[0]["attempts"].as_u64()),
        (Some("nothing"), Some(0))
    );
}

#[test]
fn a_failed_push_keeps_the_record_in_the_outbox_and_the_next_unit_pushes_it() {
    let e = Env::new();
    let remote = e.base.join("remote.git");
    git(
        &e.root,
        &[
            "remote",
            "set-url",
            "origin",
            e.base.join("gone.git").to_str().unwrap(),
        ],
    );
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let outbox = e.root.join(".git/ns/quality-outbox.jsonl");
    assert_eq!(fs::read_to_string(&outbox).unwrap().lines().count(), 1);
    assert_eq!(quality_events(&e)[0]["status"], "outbox");
    assert!(quality_records(&e).is_empty());

    git(
        &e.root,
        &["remote", "set-url", "origin", remote.to_str().unwrap()],
    );
    e.issue(8, "Other thing", "OPEN");
    let v = e.run(&["run", "--issue", "8"], 0);
    assert_eq!(v["outcome"], "done");
    let units: Vec<_> = quality_records(&e)
        .iter()
        .map(|r| r["unit"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(units, [UNIT, "8-other-thing"]);
    assert!(!outbox.exists());
}

#[test]
fn lock_refuses_a_second_runner_and_takes_over_a_free_one() {
    let e = Env::new();
    let lock = e.root.join(format!(".git/ns-run-{UNIT}.lock"));
    let held = hold(&lock, r#"{"pid":42,"unit":"7-fix-the-thing","issue":7}"#);
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(5)
        .stderr(predicates::str::contains(format!(
            "another ns run (pid 42, unit {UNIT}) holds"
        )));
    assert!(e.calls().is_empty());
    drop(held);

    // A file nobody holds is free, even when it names a live pid (this test's).
    fs::write(
        &lock,
        format!("{{\"pid\":{},\"unit\":\"{UNIT}\"}}\n", std::process::id()),
    )
    .unwrap();
    e.queue("build", &["pass:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    assert!(lock_is_free(&lock));
    assert_eq!(fs::read_to_string(&lock).unwrap(), "");
}

/// An exclusive `flock` on `path`, holding `text`, as a live `ns` holds its lock; released on
/// drop.
struct Hold {
    _file: fs::File,
}

fn hold(path: &Path, text: &str) -> Hold {
    use std::os::unix::io::AsRawFd;
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .unwrap();
    // SAFETY: flock(2) on a descriptor this function owns.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(r, 0, "cannot lock {}", path.display());
    fs::write(path, text).unwrap();
    Hold { _file: f }
}

/// Set issue `n`'s labels, as a night that ended mid-unit leaves them.
fn set_labels(e: &Env, n: u64, labels: &[&str]) {
    let lines: String = labels.iter().map(|l| format!("{l}\n")).collect();
    e.gh_file(&format!("labels-{n}"), &lines);
}

fn run_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

#[test]
fn watch_returns_a_stale_in_progress_issue_to_the_queue_and_resumes_its_unit() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.queue("build", &["crash", "crash"]);
    e.run(&["run", "--issue", "2"], 1);
    assert_eq!(e.calls(), ["triage", "build", "build"]);
    fs::remove_file(e.ctrl.join("calls")).unwrap();
    // The crashed night's lock file names a live pid (this test's), but nobody holds it.
    fs::write(
        e.root.join(".git/ns-run-2-fix-a.lock"),
        format!(
            "{{\"pid\":{},\"unit\":\"2-fix-a\",\"issue\":2}}\n",
            std::process::id()
        ),
    )
    .unwrap();
    set_labels(&e, 2, &["type:fix", "status:in-progress"]);

    let out = e.ns().args(["watch", "--once"]).output();
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["requeued"], serde_json::json!([2]), "{v}");
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "ns watch: #2 was in progress with no live ns run; back to status:ready-for-agent"
        ),
        "{err}"
    );
    // The unit resumed from its brief: no second triage.
    assert_eq!(e.calls(), ["build", "verify", "review", "ship"]);
    assert!(e.gh_calls().contains(
        "issue edit 2 --remove-label status:in-progress --add-label status:ready-for-agent"
    ));
    assert!(!e.labels(2).contains(&"status:in-progress".to_string()));
    assert!(run_events(&e)
        .iter()
        .any(|ev| ev["event"] == "requeued" && ev["issue"] == 2));
}

#[test]
fn watch_leaves_an_in_progress_issue_a_live_run_holds() {
    let e = Env::new();
    let wip = ["type:fix", "status:in-progress"];
    e.open_by(2, "Fix a", &wip, "", Some("MEMBER"));
    e.open_by(3, "Fix b", &wip, "", Some("MEMBER"));
    e.open_by(4, "Fix c", &wip, "", Some("MEMBER"));
    let _held = hold(
        &e.root.join(".git/ns-run-3-fix-b.lock"),
        r#"{"pid":42,"unit":"3-fix-b","issue":3}"#,
    );
    // A run that has taken its lock but not yet written its record is known by the file name.
    let _starting = hold(&e.root.join(".git/ns-run-4-fix-c.lock"), "");
    let out = e.ns().args(["watch", "--max-units", "0"]).output();
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["requeued"], serde_json::json!([2]), "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    assert_eq!(e.labels(3), wip);
    assert_eq!(e.labels(4), wip);
    assert!(!e.gh_calls().contains("issue edit 3"));
    assert!(!e.gh_calls().contains("issue edit 4"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "ns watch: #3 is held by a live ns run (pid 42, unit 3-fix-b); left in progress"
        ),
        "{err}"
    );
    assert!(
        err.contains(
            "ns watch: #4 is held by a live ns run (pid ?, unit 4-fix-c); left in progress"
        ),
        "{err}"
    );
    assert!(e.calls().is_empty());
}

#[test]
fn watch_dry_run_lists_a_stale_in_progress_issue_and_changes_nothing() {
    let e = Env::new();
    let wip = ["type:fix", "status:in-progress"];
    e.open_by(2, "Fix a", &wip, "", Some("MEMBER"));
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["requeue"], serde_json::json!([2]), "{v}");
    assert_eq!(e.labels(2), wip);
    assert!(!e.gh_calls().contains("issue edit"));
}

/// Issue `n`'s team comment: an Agent Brief whose `touches` are `globs`.
fn brief_comment(e: &Env, n: u64, globs: &str) {
    let c = serde_json::json!({
        "number": 1,
        "body": format!("## Agent Brief\n**Touches:** {globs}\n"),
        "authorAssociation": "MEMBER",
    });
    e.gh_file(&format!("comments-{n}.jsonl"), &format!("{c}\n"));
}

#[test]
fn watch_dry_run_names_the_running_units_each_ready_issue_overlaps() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    brief_comment(&e, 2, "`cli/src/run.rs`");
    e.ready(4, "Fix c", &["type:fix"], "");
    brief_comment(&e, 4, "`docs/**`");
    e.ready(5, "Fix d", &["type:fix"], "");
    let overlaps = |v: &Value| -> Vec<(u64, Value)> {
        v["queue"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["number"].as_u64().unwrap(), i["overlaps"].clone()))
            .collect()
    };
    let none = serde_json::json!([]);
    let v = e.run(&["watch", "--dry-run"], 0);
    let nothing_runs = [(2, none.clone()), (4, none.clone()), (5, none.clone())];
    assert_eq!(overlaps(&v), nothing_runs, "{v}");

    let wip = ["type:fix", "status:in-progress"];
    // A run lock record with no unit: the unit is named from its issue, its touches read from
    // the issue's brief, as no worktree has a brief.md.
    e.open_by(6, "Fix e", &wip, "", Some("MEMBER"));
    let _held_e = hold(
        &e.root.join(".git/ns-run-6-fix-e.lock"),
        r#"{"pid":43,"issue":6}"#,
    );
    brief_comment(&e, 6, "`docs/FACTORY.md`");
    e.open_by(3, "Fix b", &wip, "", Some("MEMBER"));
    // A unit named apart from its issue's title, as `ns run 3-uart --issue 3` names it.
    let _held = hold(
        &e.root.join(".git/ns-run-3-uart.lock"),
        r#"{"pid":42,"unit":"3-uart","issue":3}"#,
    );
    // The running unit's own brief.md wins over its issue's comment.
    brief_comment(&e, 3, "`docs/**`");
    let name = e.root.file_name().unwrap().to_str().unwrap();
    let artifacts = e.base.join(format!("{name}.worktrees/3-uart/.ns/3-uart"));
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(
        artifacts.join("brief.md"),
        "---\ntouches: [\"cli/**\"]\n---\n",
    )
    .unwrap();
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(
        overlaps(&v),
        [
            (2, serde_json::json!(["3-uart"])),
            (4, serde_json::json!(["6-fix-e"])),
            (5, none)
        ],
        "{v}"
    );
}

#[test]
fn watch_starts_a_ready_issue_clear_of_running_units_before_a_higher_one() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    brief_comment(&e, 2, "`cli/**`");
    e.ready(3, "Fix b", &["type:fix"], "");
    brief_comment(&e, 3, "`cli/src/run.rs`");
    e.ready(4, "Fix c", &["type:fix"], "");
    brief_comment(&e, 4, "`docs/**`");
    // #2 holds at its triage, so the next pick reads its prediction from its issue.
    let (in_a, go_a) = hold_phase(&e, A, "triage");
    let w = Watching::start(
        e.ns()
            .args(["watch", "--parallel", "2", "--max-units", "2"]),
    );
    w.entered(&in_a);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 4], "done"), "{v}");
}

#[test]
fn watch_dry_run_leaves_out_overlap_on_files_overlap_ignore_names() {
    let e = Env::new();
    e.factory("[queue]\noverlap_ignore = [\"docs/FACTORY.md\"]\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    brief_comment(&e, 2, "`docs/FACTORY.md`, `cli/src/run.rs`");
    e.open_by(
        6,
        "Fix e",
        &["type:fix", "status:in-progress"],
        "",
        Some("MEMBER"),
    );
    let _held = hold(
        &e.root.join(".git/ns-run-6-fix-e.lock"),
        r#"{"pid":43,"issue":6}"#,
    );
    brief_comment(&e, 6, "`docs/FACTORY.md`");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["queue"][0]["overlaps"], serde_json::json!([]), "{v}");
}

#[test]
fn watch_keeps_the_queue_order_over_overlap_on_files_overlap_ignore_names() {
    let e = Env::new();
    e.factory("[queue]\noverlap_ignore = [\"docs/FACTORY.md\"]\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    brief_comment(&e, 2, "`docs/FACTORY.md`");
    e.ready(3, "Fix b", &["type:fix"], "");
    brief_comment(&e, 3, "`docs/FACTORY.md`");
    e.ready(4, "Fix c", &["type:fix"], "");
    brief_comment(&e, 4, "`docs/DESIGN.md`");
    let (in_a, go_a) = hold_phase(&e, A, "triage");
    let w = Watching::start(
        e.ns()
            .args(["watch", "--parallel", "2", "--max-units", "2"]),
    );
    w.entered(&in_a);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
}

#[test]
fn a_second_watch_on_the_repo_is_refused() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.open_by(
        3,
        "Fix b",
        &["type:fix", "status:in-progress"],
        "",
        Some("MEMBER"),
    );
    let _held = hold(&e.root.join(".git/ns-watch.lock"), r#"{"pid":42}"#);
    e.ns()
        .args(["watch", "--once"])
        .assert()
        .code(5)
        .stderr(predicates::str::contains(
            "another ns watch (pid 42) is running",
        ));
    assert!(e.calls().is_empty());
    assert!(!e.gh_calls().contains("issue edit"));
}

#[test]
fn two_runs_on_different_units_in_one_repo_run_together() {
    let e = Env::new();
    e.issue(8, "Fix the other", "OPEN");
    // Unit 7 holds its run lock, blocked in verify, while unit 8 runs from start to end.
    let held = Blocked::start(&e, "true", "true");
    let v = e.run(&["run", "--issue", "8"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(5)
        .stderr(predicates::str::contains(format!(
            "another ns run (pid {}, unit {UNIT}) holds",
            held.run.id()
        )));
    held.release();
    let mut run = held.run;
    assert!(run.wait().success());
    for unit in [UNIT, "8-fix-the-other"] {
        let lock = e.root.join(format!(".git/ns-run-{unit}.lock"));
        assert!(lock_is_free(&lock), "{}", lock.display());
    }
}

#[test]
fn worktrees_cut_at_once_from_a_remote_base_all_land() {
    let e = Env::new();
    git(&e.root, &["fetch", "-q", "origin"]);
    // `git worktree add` from a remote base writes the branch's upstream to .git/config, which
    // only one git at a time can lock.
    let mut runs: Vec<Group> = (0..12)
        .map(|i| {
            e.ns()
                .args(["worktree", "new", &format!("u{i}"), "--base", "origin/main"])
                .start()
        })
        .collect();
    for r in &mut runs {
        let err = read_all(r.take_stderr());
        let status = r.wait();
        let err = String::from_utf8_lossy(&r.recv(&err)).into_owned();
        assert!(status.success(), "{err}");
    }
    let list = git(&e.root, &["worktree", "list"]);
    assert_eq!(list.lines().count(), 13, "{list}");
    assert!(lock_is_free(&e.root.join(".git/ns-worktree.lock")));
}
