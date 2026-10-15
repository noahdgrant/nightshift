#[test]
fn watch_dry_run_lists_triage_candidates_in_triage_order() {
    let e = Env::new();
    e.ready(2, "Ready", &["type:fix"], "");
    e.untriaged(5, "Needs triage", &["status:needs-triage", "priority:low"]);
    e.untriaged(6, "No status", &["type:fix", "priority:high"]);
    e.untriaged(8, "Needs info", &["status:needs-info"]);
    e.untriaged(9, "In review", &["status:in-review"]);
    e.open_by(10, "Stranger", &["status:needs-triage"], "", Some("NONE"));
    e.untriaged(11, "Open PR", &[]);
    e.untriaged(12, "Merged PR", &[]);
    e.untriaged(13, "Plain", &[]);
    e.gh_file("prs.json", r#"[{"number":40,"body":"Closes #11"}]"#);
    e.gh_file("prs-merged.json", r#"[{"number":41,"body":"Fixes #12"}]"#);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(numbers(&v["queue"]), [2], "{v}");
    assert_eq!(numbers(&v["triage"]), [6, 5, 13], "{v}");
    assert_eq!(v["triage"][0]["reason"], "no status label");
    assert_eq!(v["triage"][0]["priority_label"], "priority:high");
    assert_eq!(v["triage"][1]["reason"], "status:needs-triage");
    assert!(v["triage"][2]["priority_label"].is_null(), "{v}");
    let skipped: Vec<(u64, &str)> = v["triage_skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["number"].as_u64().unwrap(), i["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        skipped,
        [
            (10, "author outside the team"),
            (11, "open PR #40 closes it"),
            (12, "merged PR #41 closes it"),
        ]
    );
    assert_eq!(v["triage_pass"], true, "{v}");
    // #2 is ready, so the next pass leaves the backlog for later passes.
    assert_eq!(v["triage_next_pass"], serde_json::json!([]), "{v}");
    assert!(v.get("triage_per_night").is_none(), "{v}");
    assert!(e.calls().is_empty());
    assert!(!e.gh_calls().contains("issue edit"));
    assert!(e
        .gh_calls()
        .contains("api --paginate repos/{owner}/{repo}/issues?state=open&per_page=100 --jq"));
}

#[test]
fn watch_dry_run_with_no_ready_issue_shows_the_next_pass_taking_the_backlog_in_order() {
    let e = Env::new();
    e.untriaged(5, "Low", &["status:needs-triage", "priority:low"]);
    e.untriaged(6, "High", &["status:needs-triage", "priority:high"]);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert!(v["queue"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["triage_next_pass"], serde_json::json!([6, 5]), "{v}");
}

#[test]
fn watch_dry_run_counts_a_stale_in_progress_issue_as_ready_for_the_next_pass() {
    let e = Env::new();
    // The night returns #2 to the queue before its first pass, so the pass takes no backlog.
    e.open_by(
        2,
        "Fix a",
        &["type:fix", "status:in-progress"],
        "",
        Some("MEMBER"),
    );
    e.untriaged(5, "Backlog", &["status:needs-triage"]);
    let v = e.run(&["watch", "--dry-run"], 0);
    assert!(v["queue"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["requeue"], serde_json::json!([2]), "{v}");
    assert_eq!(numbers(&v["triage"]), [5], "{v}");
    assert_eq!(v["triage_next_pass"], serde_json::json!([]), "{v}");
}

#[test]
fn an_old_triage_per_night_warns_and_is_ignored() {
    let e = Env::new();
    e.factory("[queue]\ntriage_per_night = 0\n");
    e.untriaged(5, "A", &["status:needs-triage"]);
    e.untriaged(6, "B", &["status:needs-triage"]);
    e.queue("triage", &["none", "none"]);
    let out = e.ns().args(["watch"]).assert().code(0).get_output().clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("warning:") && err.contains("triage_per_night is no longer used"),
        "{err}"
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    // The old cap of 0 would have run none: the key changes nothing.
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
    let check = e.run(&["factory", "validate"], 0);
    assert_eq!(check["ok"], true, "{check}");
    let warnings = check["warnings"].to_string();
    assert!(warnings.contains("triage_per_night"), "{check}");
}

#[test]
fn no_triage_run_starts_after_until() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    // b's triage needs the bench a holds, so its first triage waits out the night.
    b.factory("[phases.triage]\nrunner = \"bench\"\n");
    let held = Blocked::start(&a, "true", "true");
    b.untriaged(5, "A", &["status:needs-triage"]);
    b.untriaged(6, "B", &["status:needs-triage"]);
    let out = b
        .ns()
        .args(["watch", "--until", "00:30"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    held.release();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["stopped"], "until", "{v}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(outcomes(&v["triaged"]), ["budget"], "{v}");
    assert!(b.calls().is_empty(), "{:?}", b.calls());
}

#[test]
fn a_timed_out_triage_is_recorded_and_the_night_goes_on() {
    let e = Env::new();
    e.untriaged(5, "Slow", &["status:needs-triage", "priority:high"]);
    e.untriaged(6, "Next", &["status:needs-triage"]);
    e.queue("triage", &["pass:sleep", "none"]);
    let out = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "triage=1500")
        .args(["watch"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(outcomes(&v["triaged"]), ["stuck", "done"], "{v}");
    let reason = v["triaged"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("timed out"), "{reason}");
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
    assert_eq!(e.labels(5), ["status:needs-triage", "priority:high"]);
}

#[test]
fn a_killed_triage_is_recorded_and_the_night_goes_on() {
    let e = Env::new();
    e.untriaged(5, "Killed", &["status:needs-triage", "priority:high"]);
    e.untriaged(6, "Next", &["status:needs-triage"]);
    // Kill the harness itself, so ns sees a signal instead of an exit code.
    e.ctl("triage.sh", "kill -KILL $PPID\n");
    e.queue("triage", &["pass:script", "none"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["stuck", "done"], "{v}");
    assert_eq!(v["triaged"][0]["reason"], "was killed by SIGKILL");
    assert_eq!(numbers(&v["triaged"]), [5, 6], "{v}");
}

#[test]
fn a_triage_only_run_triages_again_over_an_earlier_brief() {
    let e = Env::new();
    e.untriaged(5, "Escape", &["status:needs-triage"]);
    // A brief that routes nowhere leaves the issue untriaged.
    e.queue("triage", &["fail", "none"]);
    e.run(&["watch"], 0);
    // The next night the issue is still untriaged, and its worktree holds the first brief.
    let v = e.run(&["watch"], 0);
    assert_eq!(outcomes(&v["triaged"]), ["done"], "{v}");
    assert_eq!(e.calls(), ["triage", "triage"]);
}

#[test]
fn gates_stop_runs_no_triage() {
    let e = Env::new();
    e.factory("gates = \"stop\"\n");
    e.untriaged(5, "Untriaged", &["status:needs-triage"]);
    let dry = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(dry["triage_pass"], false, "{dry}");
    assert!(dry["triage"].as_array().unwrap().is_empty(), "{dry}");
    assert_eq!(dry["triage_next_pass"], serde_json::json!([]), "{dry}");
    assert!(
        dry["triage_skipped"].as_array().unwrap().is_empty(),
        "{dry}"
    );
    let v = e.run(&["watch"], 0);
    assert!(v["triaged"].as_array().unwrap().is_empty(), "{v}");
    assert!(e.calls().is_empty());
}

// ---------------------------------------------------------------- memory cap

/// Allocates and touches 256 MB: past a 64 MB cap, never near a real machine's memory.
const HOG: &str = "python3 -c \"b = b'x' * (256 << 20)\"";

/// The `start` event's `memory_cap` for the first unit in the run log.
fn memory_cap(e: &Env) -> Value {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["event"] == "start")
        .unwrap()["memory_cap"]
        .clone()
}

/// Shadow `name` on the fakes' PATH with a command that fails, as if it were missing.
fn break_tool(e: &Env, name: &str) {
    let p = e.bin.join(name);
    write_exe(&p, "#!/bin/sh\necho \"$0: not here\" >&2\nexit 1\n");
}

/// The build script records how its process is capped: `ulimit -v` and its cgroup.
fn record_limits(e: &Env) {
    e.ctl(
        "build.sh",
        &format!(
            "ulimit -v > {c}/ulimit; cat /proc/self/cgroup > {c}/cgroup\n",
            c = e.ctrl.display()
        ),
    );
}

fn recorded(e: &Env, name: &str) -> String {
    fs::read_to_string(e.ctrl.join(name)).unwrap()
}

#[test]
fn a_phase_past_the_memory_cap_fails_its_attempt_and_watch_goes_on() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.ctl(
        "build.sh",
        &format!("case \"$NS_UNIT\" in 2-*) {HOG} ;; esac\n"),
    );
    e.queue("build", &["pass:script", "pass:script", "pass:script"]);
    let v = e.run(&["watch"], 0);
    let units = v["units"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{v}");
    assert_eq!(units[0]["outcome"], "stuck", "{v}");
    assert_eq!(
        units[0]["reason"],
        "build is out of attempts (2): the last attempt exceeded the 64 MB memory limit"
    );
    assert_eq!(units[1]["outcome"], "done", "{v}");
    let failed: Vec<Value> = phase_events(&e)
        .into_iter()
        .filter(|p| p["phase"] == "build" && p["unit"] == "2-fix-a")
        .collect();
    assert_eq!(failed.len(), 2);
    for p in failed {
        assert_eq!(p["reason"], "exceeded the 64 MB memory limit", "{p}");
        assert_eq!(p["written"], false);
    }
    assert_eq!(memory_cap(&e)["mb"], 64);
}

#[test]
fn a_capped_phase_runs_in_a_scope_or_under_an_rlimit_and_logs_which() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    record_limits(&e);
    e.queue("build", &["pass:script"]);
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(0);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    let cap = memory_cap(&e);
    match cap["via"].as_str().unwrap() {
        "systemd-run" => {
            assert!(cap["fallback"].is_null(), "{cap}");
            assert!(
                err.contains("64 MB memory cap (cgroup, systemd-run)"),
                "{err}"
            );
            let cg = recorded(&e, "cgroup");
            assert!(cg.contains("/ns-") && cg.contains(".scope"), "{cg}");
        }
        "prlimit" => {
            assert!(
                err.contains("64 MB memory cap (RLIMIT_AS, prlimit)"),
                "{err}"
            );
            assert_eq!(recorded(&e, "ulimit").trim(), "65536");
        }
        other => panic!("{other}"),
    }
}

#[test]
fn without_systemd_run_the_cap_falls_back_to_prlimit() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    break_tool(&e, "systemd-run");
    let flag = e.ctrl.join("hogged");
    e.ctl(
        "build.sh",
        &format!(
            "ulimit -v > {c}/ulimit\ntest -f {f} || {{ touch {f}; {HOG}; }}\n",
            c = e.ctrl.display(),
            f = flag.display()
        ),
    );
    e.queue("build", &["pass:script", "pass:script"]);
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(0);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        err.contains("64 MB memory cap (RLIMIT_AS, prlimit); no cgroup: systemd-run failed"),
        "{err}"
    );
    let cap = memory_cap(&e);
    assert_eq!(cap["via"], "prlimit", "{cap}");
    assert!(
        cap["fallback"].as_str().unwrap().contains("systemd-run"),
        "{cap}"
    );
    assert_eq!(recorded(&e, "ulimit").trim(), "65536");
    assert_eq!(
        build_event(&e, 1)["reason"],
        "exceeded the 64 MB memory limit"
    );
    assert_eq!(build_event(&e, 2)["written"], true);
}

#[test]
fn a_capped_phase_that_times_out_fails_on_the_timeout() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[limits]\nmemory_mb = 64\n[phases.build]\nmax_attempts = 3\ngate = \"test -f {f} || {{ touch {f}; exit 1; }}\"\n",
        f = flag.display()
    ));
    break_tool(&e, "systemd-run");
    // Under RLIMIT_AS this message alone would read as the cap; the timeout must win.
    e.ctl("build.sh", "echo MemoryError >&2\n");
    e.queue(
        "build",
        &["pass:commit", "pass:script+sleep", "pass:commit"],
    );
    let v = run_with_phase_timeout(&e, 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(build_event(&e, 2)["reason"], "timed out after 1.5 s");
    assert_eq!(
        build_event(&e, 3)["decision"],
        "timed out with the CI gate still red"
    );
}

/// Fake `systemd-run` that passes the cgroup probe and runs each command in a "scope" logged
/// to `ctrl/scopes`.
fn fake_systemd_run(e: &Env) {
    let c = e.ctrl.display();
    write_exe(
        &e.bin.join("systemd-run"),
        format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --collect) echo 67108864; exit 0 ;;\n    --unit=*) echo \"${{1#--unit=}}\" >> '{c}/scopes' ;;\n    --) shift; break ;;\n  esac\n  shift\ndone\nexec \"$@\"\n"
        ),
    );
}

/// Fake `systemctl` whose `show` runs the shell `report` for each ask, with the scope's ask
/// count so far in `$n` and its directory of state in `$d`; it prints the `ActiveState` and
/// `Result` lines. `reset-failed` logs the scope, how many asks it had and the state the last
/// one reported to `ctrl/reset`.
fn fake_systemctl(e: &Env, report: &str) {
    let c = e.ctrl.display();
    write_exe(
        &e.bin.join("systemctl"),
        format!(
            "#!/bin/sh\nd='{c}/asks'; mkdir -p \"$d\"\ncase \"$2\" in\n  show) n=$(( $(cat \"$d/$3\" 2>/dev/null || echo 0) + 1 )); echo $n > \"$d/$3\"\n    {report}\n    printf 'ActiveState=%s\\nResult=%s\\n' \"$state\" \"$result\"; echo \"$state\" > \"$d/$3.last\" ;;\n  reset-failed) echo \"$3 $(cat \"$d/$3\" 2>/dev/null || echo 0) $(cat \"$d/$3.last\" 2>/dev/null)\" >> '{c}/reset' ;;\nesac\n"
        ),
    );
}

/// Fake `systemd-run` and `systemctl` where `systemctl show` reports `first`
/// (`ActiveState Result`) for a scope's first `n` asks and `then` after.
fn fake_systemd(e: &Env, n: u32, first: &str, then: &str) {
    fake_systemd_run(e);
    let (fs, fr) = first.split_once(' ').unwrap();
    let (ts, tr) = then.split_once(' ').unwrap();
    fake_systemctl(
        e,
        &format!(
            "if [ $n -le {n} ]; then state={fs}; result={fr}; else state={ts}; result={tr}; fi"
        ),
    );
}

/// Fake `systemd-run` and `systemctl` where a scope reports `active success` until `secs`
/// seconds after the first ask about it, then `failed oom-kill`.
fn fake_systemd_failing_after(e: &Env, secs: u32) {
    fake_systemd_run(e);
    fake_systemctl(
        e,
        &format!(
            "now=$(date +%s); [ -f \"$d/$3.t\" ] || echo $now > \"$d/$3.t\"\n    if [ $(( now - $(cat \"$d/$3.t\") )) -lt {secs} ]; then state=active; result=success; else state=failed; result=oom-kill; fi"
        ),
    );
}

/// Each scope the run started was reset once, after systemd had been asked about it more
/// than `asks` times.
fn assert_every_scope_reset(e: &Env, asks: u32) {
    let scopes = recorded(e, "scopes");
    let reset = fs::read_to_string(e.ctrl.join("reset")).unwrap_or_default();
    assert!(!scopes.is_empty());
    for unit in scopes.lines() {
        let lines: Vec<&str> = reset
            .lines()
            .filter(|l| l.split(' ').next() == Some(unit))
            .collect();
        assert_eq!(lines.len(), 1, "{unit} reset: {reset}");
        let n: u32 = lines[0].split(' ').nth(1).unwrap().parse().unwrap();
        assert!(
            n > asks,
            "{unit} reset after {n} asks, want more than {asks}"
        );
    }
}

#[test]
fn every_capped_phase_and_gate_scope_is_reset_after_it_exits_zero_or_not() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[limits]\nmemory_mb = 64\n[phases.build]\ngate = \"test -f {f} || {{ touch {f}; exit 1; }}\"\n",
        f = flag.display()
    ));
    fake_systemd(&e, 0, "active success", "inactive success");
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(memory_cap(&e)["via"], "systemd-run");
    assert_eq!(gate_events(&e)[0]["green"], false);
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_capped_phase_that_times_out_still_resets_its_scope() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    fake_systemd(&e, 0, "active success", "inactive success");
    e.ctl("build.sh", "true\n");
    e.queue("build", &["pass:script+sleep", "pass:commit"]);
    let v = run_with_phase_timeout(&e, 0);
    assert_eq!(build_event(&e, 1)["reason"], "timed out after 1.5 s", "{v}");
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_capped_gate_killed_by_a_signal_still_resets_its_scope() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n[phases.build]\ngate = \"kill -9 $$\"\n");
    fake_systemd(&e, 0, "active success", "inactive success");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate was killed by SIGKILL"
    );
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_capped_phase_whose_run_process_errors_on_a_stop_still_resets_its_scope() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    fake_systemd(&e, 0, "active success", "inactive success");
    let f = blocking_build(&e);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_capped_gate_whose_run_process_errors_on_a_stop_still_resets_its_scope() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    e.factory(&format!(
        "[limits]\nmemory_mb = 64\n[phases.build]\ngate = {:?}\n",
        blocks(&f)
    ));
    fake_systemd(&e, 0, "active success", "inactive success");
    e.queue("build", &["pass:commit"]);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert_every_scope_reset(&e, 0);
}

#[test]
fn a_scope_that_fails_after_the_settle_deadline_is_not_an_overrun_but_is_still_reset() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    // Active past the 3 s ns waits for the result and failed before the 10 s it waits to
    // release the scope: the late failure is cleared, not reported.
    fake_systemd_failing_after(&e, 6);
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_every_scope_reset(&e, 0);
    for line in recorded(&e, "reset").lines() {
        assert!(line.ends_with(" failed"), "reset before it failed: {line}");
    }
}

#[test]
fn an_oom_kill_seen_while_the_scope_deactivates_is_detected_and_reset_once_it_fails() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    fake_systemd(&e, 5, "deactivating oom-kill", "failed oom-kill");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "triage is out of attempts (2): the last attempt exceeded the 64 MB memory limit",
        "{v}"
    );
    assert_every_scope_reset(&e, 5);
}

#[test]
fn a_cap_nothing_here_can_enforce_is_a_usage_error_before_any_phase() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    break_tool(&e, "systemd-run");
    break_tool(&e, "prlimit");
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(2);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        err.contains("limits.memory_mb = 64, but no memory cap works here"),
        "{err}"
    );
    assert!(e.calls().is_empty());
}

#[test]
fn without_a_cap_phases_run_directly() {
    let e = Env::new();
    record_limits(&e);
    e.queue("build", &["pass:script"]);
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(0);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(!err.contains("memory cap"), "{err}");
    assert!(memory_cap(&e).is_null());
    let own = StdCommand::new("sh")
        .args(["-c", "ulimit -v"])
        .output()
        .unwrap();
    assert_eq!(recorded(&e, "ulimit"), String::from_utf8_lossy(&own.stdout));
    assert_eq!(
        recorded(&e, "cgroup"),
        fs::read_to_string("/proc/self/cgroup").unwrap()
    );
}

#[test]
fn a_gate_past_the_memory_cap_sends_build_back_and_names_the_cap() {
    let e = Env::new();
    e.factory(&format!(
        "[limits]\nmemory_mb = 64\n[phases.build]\ngate = {HOG:?}\n"
    ));
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate exceeded the 64 MB memory limit"
    );
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["memory_exceeded"], true);
    assert_eq!(g[0]["green"], false);
    let p = e.prompt(3, "build");
    assert!(p.contains("exceeded the 64 MB memory limit"), "{p}");
    assert_eq!(
        build_event(&e, 2)["decision"],
        "the CI gate exceeded the 64 MB memory limit after build"
    );
}
