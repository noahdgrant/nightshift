#[test]
fn run_cpus_sets_all_three_variables_for_phases_and_gates_and_logs_them() {
    let e = Env::new();
    let seen = cpus_seen(&e);
    let v = e.run(&["run", "--issue", "7", "--cpus", "3"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        fs::read_to_string(&seen).unwrap(),
        "build:3:3:3\ngate:3:3:3\n"
    );
    let start = &events_named(&e, "start")[0];
    assert_eq!(
        start["cpus"],
        serde_json::json!({
            "NS_CPUS": {"value": "3", "source": "computed"},
            "RUST_TEST_THREADS": {"value": "3", "source": "computed"},
            "CARGO_BUILD_JOBS": {"value": "3", "source": "computed"},
        }),
        "{start}"
    );
}

#[test]
fn run_without_cpus_sets_none_of_the_variables() {
    let e = Env::new();
    let seen = cpus_seen(&e);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        fs::read_to_string(&seen).unwrap(),
        "build:unset:unset:unset\ngate:unset:unset:unset\n"
    );
    assert!(events_named(&e, "start")[0]["cpus"].is_null());
}

#[test]
fn run_cpus_passes_a_variable_already_set_through_unchanged() {
    let vars = ["NS_CPUS", "RUST_TEST_THREADS", "CARGO_BUILD_JOBS"];
    for (i, set) in vars.iter().enumerate() {
        let e = Env::new();
        let seen = cpus_seen(&e);
        let out = e
            .ns()
            .args(["run", "--issue", "7", "--cpus", "2"])
            .env(set, "16")
            .output();
        assert!(out.status.success(), "{set}: {out:?}");
        let mut values = ["2", "2", "2"];
        values[i] = "16";
        let line = |who: &str| format!("{who}:{}:{}:{}\n", values[0], values[1], values[2]);
        assert_eq!(
            fs::read_to_string(&seen).unwrap(),
            format!("{}{}", line("build"), line("gate")),
            "{set}"
        );
        let cpus = &events_named(&e, "start")[0]["cpus"];
        for (j, var) in vars.iter().enumerate() {
            let (value, source) = if i == j {
                ("16", "inherited")
            } else {
                ("2", "computed")
            };
            assert_eq!(
                cpus[var],
                serde_json::json!({"value": value, "source": source}),
                "{set}: {var}"
            );
        }
    }
}

#[test]
fn run_cpus_reaches_the_worktree_setup_commands() {
    let e = Env::new();
    let seen = e.ctrl.join("setup-seen");
    let cmd = format!(
        "echo \"${{NS_CPUS-unset}}:${{RUST_TEST_THREADS-unset}}:${{CARGO_BUILD_JOBS-unset}}\" >> {}",
        seen.display()
    );
    e.factory(&format!("[worktree]\nsetup = {:?}\n", [cmd]));
    let v = e.run(&["run", "--issue", "7", "--cpus", "3"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(fs::read_to_string(&seen).unwrap(), "3:3:3\n");
}

#[test]
fn run_cpus_must_be_a_number_of_at_least_one() {
    let e = Env::new();
    for bad in ["0", "x"] {
        e.ns()
            .args(["run", "--issue", "7", "--cpus", bad])
            .assert()
            .code(2)
            .stderr(predicates::str::contains("--cpus"));
    }
    assert!(e.calls().is_empty(), "{:?}", e.calls());
}

#[test]
fn watch_gives_each_unit_its_share_of_the_cores() {
    let cores = std::thread::available_parallelism().unwrap().get();
    for parallel in [1, 2, 3, cores + 1] {
        let e = Env::new();
        two_ready(&e);
        let seen = e.ctrl.join("cpus-seen");
        e.ctl(
            "build.sh",
            &format!(
                "echo \"$NS_CPUS:$CARGO_BUILD_JOBS\" >> {}\n",
                seen.display()
            ),
        );
        e.queue("build", &["pass:script", "pass:script"]);
        let p = parallel.to_string();
        let v = e.run(&["watch", "--parallel", &p], 0);
        assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
        let share = (cores / parallel).max(1);
        assert_eq!(
            fs::read_to_string(&seen).unwrap(),
            format!("{share}:{share}\n{share}:{share}\n"),
            "--parallel {parallel} on {cores} cores"
        );
    }
}

#[test]
fn watch_parallel_settles_the_other_unit_when_one_ends_without_a_result() {
    let e = Env::new();
    two_ready(&e);
    // A file where unit 2's worktree goes makes its ns run fail.
    fs::create_dir(e.base.join("myrepo.worktrees")).unwrap();
    fs::write(e.base.join("myrepo.worktrees/2-fix-a"), "").unwrap();
    let out = e.ns().args(["watch", "--parallel", "2"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(
        err.contains("is not the worktree for branch ns/2-fix-a"),
        "{err}"
    );
    assert!(err.contains("ns run --issue 2 ended with exit 1"), "{err}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    let three = e.labels(3);
    assert!(
        !three
            .iter()
            .any(|l| l == "status:in-progress" || l == "status:ready-for-agent"),
        "{three:?}"
    );
    // The summary still reports the unit that finished, and the error.
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|x| panic!("{x}: {err}"));
    assert_eq!(v["stopped"], "error", "{v}");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("ns run --issue 2 ended with exit 1"),
        "{v}"
    );
    assert_eq!(by_issue(&units(&v)), each(&[3], "done"), "{v}");
}

#[test]
fn watch_parallel_starts_an_issue_listed_twice_once() {
    let e = Env::new();
    // The listing shifted between pages and names #2 twice.
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(2, "Fix a", &["type:fix"], "");
    let v = e.run(&["watch", "--parallel", "2"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
    assert_eq!(run_pids(&e).len(), 1, "{:?}", run_pids(&e));
    let calls = e.gh_calls();
    assert_eq!(
        calls.matches("--add-label status:in-progress").count(),
        1,
        "{calls}"
    );
}

#[test]
fn cleanup_between_units_leaves_the_worktree_of_a_unit_still_running() {
    let e = Env::new();
    // Unit 3's worktree looks merged to cleanup once its setup starts: issue closed, PR merged.
    unit_with_pr(&e, 3, "3-done", 30, "OPEN", "OPEN");
    e.ready(3, "done", &["type:fix"], "");
    e.ready(2, "Fix a", &["type:fix"], "");
    let entered = e.ctrl.join("entered-setup");
    let go = e.ctrl.join("go-setup");
    let go_2 = e.ctrl.join("go-setup-2");
    let looked = e.ctrl.join("looked");
    for f in [&entered, &go, &go_2, &looked] {
        mkfifo(f);
    }
    // Unit 2's setup waits for unit 3's, so unit 2 can only end, and cleanup only run after it,
    // once unit 3's worktree looks merged.
    let setup = format!(
        "if [ \"$NS_UNIT\" = 3-done ]; then sed -i 's/OPEN/CLOSED/' \"$FAKE_GH_DIR/issue-3.json\"; \
         echo MERGED > \"$FAKE_GH_DIR/pr-30.state\"; echo in > {entered:?}; cat {go:?} > /dev/null; \
         else cat {go_2:?} > /dev/null; fi"
    );
    e.factory(&format!("[worktree]\nsetup = [{setup:?}]\n"));
    // In place before watch starts, since cleanup may look the moment unit 2 ends. Only a look
    // once the PR is merged counts, which is cleanup's after unit 2.
    e.gh_file(
        "hook.sh",
        &format!(
            "[ \"$*\" = \"pr view 30 --json state,headRefOid,headRefName,body\" ] && \
             [ \"$(cat \"$FAKE_GH_DIR/pr-30.state\" 2>/dev/null)\" = MERGED ] && echo x > {looked:?}\n"
        ),
    );
    let mut w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    // Unit 3 holds its run lock in setup while unit 2 runs to its end and cleanup runs after it.
    w.entered(&entered);
    release(&go_2);
    w.line("ns watch: #2 done (worker");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(&looked));
    });
    w.g.recv(&rx).unwrap();
    fs::remove_file(e.ghd.join("hook.sh")).unwrap();
    release(&go);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    // The worktree went only after unit 3's run ended and let its lock go.
    let ended = event_at(&e, |v| v["event"] == "end" && v["unit"] == "3-done");
    let cleaned = event_at(&e, |v| v["event"] == "cleanup" && v["unit"] == "3-done");
    assert!(ended < cleaned, "{:?}", run_events(&e));
    assert_eq!(v["cleaned"], serde_json::json!(["3-done"]), "{v}");
}

/// Both units mid build when `sig` reaches `ns watch` (its process group, when `group`): every
/// unit goes back to the queue and no process it started outlives it.
fn assert_a_stop_requeues_every_running_unit(sig: i32, group: bool, name: &str) {
    let e = Env::new();
    two_ready(&e);
    let (in_a, _go_a) = hold_phase(&e, A, "build");
    let (in_b, _go_b) = hold_phase(&e, B, "build");
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    let harnesses = [w.entered(&in_a), w.entered(&in_b)];
    w.signal(sig, group);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(128 + sig), "{err}");
    assert_eq!(v["stopped"], name, "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "interrupted"), "{v}");
    for n in [2, 3] {
        assert_eq!(e.labels(n), ["type:fix", "status:ready-for-agent"]);
    }
    let runs: Vec<String> = run_pids(&e).iter().map(|(_, p)| p.to_string()).collect();
    assert_eq!(runs.len(), 2, "{err}");
    for pid in harnesses.iter().chain(&runs) {
        assert!(exits(pid), "process {pid} outlived ns watch: {err}");
    }
    let ends = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "end" && ev["outcome"] == "interrupted")
        .count();
    assert_eq!(ends, 2, "{err}");
}

#[test]
fn sigterm_returns_every_running_unit_to_the_queue() {
    assert_a_stop_requeues_every_running_unit(libc::SIGTERM, false, "SIGTERM");
}

#[test]
fn ctrl_c_returns_every_running_unit_to_the_queue() {
    assert_a_stop_requeues_every_running_unit(libc::SIGINT, true, "SIGINT");
}

/// A night directory as `ns watch` writes it, with `spend` already spent and the hold `hold`.
fn night_dir(e: &Env, spend: f64, hold: Option<i64>) -> PathBuf {
    let dir = e.base.join("night");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("night.json"),
        format!(
            "{{\"watch_pid\":{},\"until\":null,\"gate\":null}}",
            std::process::id()
        ),
    )
    .unwrap();
    fs::write(
        dir.join("spend.jsonl"),
        format!("{{\"unit\":\"other\",\"usd\":{spend}}}\n"),
    )
    .unwrap();
    if let Some(r) = hold {
        fs::write(dir.join("hold.json"), format!("{{\"reset_at\":{r}}}")).unwrap();
    }
    dir
}

#[test]
fn a_run_in_a_night_counts_every_unit_s_spend_against_the_budget() {
    let e = Env::new();
    e.factory("[limits]\nbudget_usd = 40.0\n");
    let night = night_dir(&e, 39.75, None);
    let v = e.run(
        &["run", "--issue", "7", "--night", night.to_str().unwrap()],
        3,
    );
    assert_eq!(v["outcome"], "budget", "{v}");
    assert_eq!(v["reason"], "spent $40.25 of the $40.00 budget", "{v}");
    assert_eq!(v["cost_usd"], 0.5, "{v}");
    assert_eq!(e.calls(), ["triage"]);
    let spend = fs::read_to_string(night.join("spend.jsonl")).unwrap();
    assert!(
        spend.ends_with(&format!("{{\"unit\":\"{UNIT}\",\"usd\":0.5}}\n")),
        "{spend}"
    );
}

#[test]
fn a_run_in_a_night_stops_before_a_phase_while_a_hold_lasts() {
    let e = Env::new();
    let night = night_dir(&e, 0.0, Some(NOW + 3600));
    let v = e.run(
        &["run", "--issue", "7", "--night", night.to_str().unwrap()],
        4,
    );
    assert_eq!(v["outcome"], "paused", "{v}");
    assert_eq!(v["reset_at"], NOW + 3600, "{v}");
    assert!(
        v["reason"].as_str().unwrap().contains("another unit"),
        "{v}"
    );
    assert!(e.calls().is_empty(), "{:?}", e.calls());
    // A hold whose reset has come holds nothing.
    let night = night_dir(&e, 0.0, Some(NOW));
    let v = e.run(
        &["run", "--issue", "7", "--night", night.to_str().unwrap()],
        0,
    );
    assert_eq!(v["outcome"], "done", "{v}");
}

#[test]
fn cleanup_at_a_resume_leaves_the_worktree_of_the_paused_unit() {
    let e = Env::new();
    // Unit 2's worktree has a PR from an earlier night; a human merges it during the pause.
    unit_with_pr(&e, 2, A, 20, "OPEN", "OPEN");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue(&format!("triage.{A}"), &["limit"]);
    let (entered, go) = hold_phase(&e, A, "triage");
    let w = Watching::start(e.ns().args(["watch", "--once", "--until", "06:30"]));
    w.entered(&entered);
    e.issue(2, "Fix a", "CLOSED");
    e.gh_file("pr-20.state", "MERGED");
    release(&go);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    let us = units(&v);
    assert_eq!(us[0]["outcome"], "paused", "{v}");
    assert_eq!(us[1]["issue"], 2, "{v}");
    // The unit resumed in its own worktree; cleanup removed it only once the unit had ended.
    let starts: Vec<usize> = run_events(&e)
        .iter()
        .enumerate()
        .filter(|(_, v)| v["event"] == "worker_start" && v["issue"] == 2)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(starts.len(), 2, "{err}");
    let removed = run_events(&e)
        .iter()
        .position(|v| v["event"] == "cleanup" && v["unit"] == A && v["removed"] == true);
    assert!(
        removed.is_none_or(|r| r > starts[1]),
        "{:?}",
        run_events(&e)
    );
}

/// Make the repo the nightshift source, `cli/Cargo.toml` naming the package `nightshift`, put the
/// fake cargo on PATH, and return the commit pushed, which the `ns` under test counts as its
/// build commit.
fn nightshift_repo(e: &Env) -> String {
    copy_exe(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/factory/cargo"),
        &e.bin.join("cargo"),
    );
    fs::create_dir_all(e.root.join("cli")).unwrap();
    fs::write(
        e.root.join("cli/Cargo.toml"),
        "[package]\nname = \"nightshift\"\n",
    )
    .unwrap();
    land(e, "cli/x", "old\n")
}

/// Commit `text` to `path` and push it to origin's main, as a unit merged tonight would; the
/// commit.
fn land(e: &Env, path: &str, text: &str) -> String {
    let p = e.root.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
    git(&e.root, &["add", "."]);
    git(&e.root, &["commit", "-q", "-m", path]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    git(&e.root, &["rev-parse", "HEAD"])
}

/// `ns watch` built from `from`, whose stub successor execs the `ns` under test.
fn watch_from(e: &Env, from: &str) -> Ns {
    let mut c = e.ns();
    c.env("NS_BUILD_COMMIT", from)
        .env("REAL_NS", common::ns_path());
    c
}

fn cargo_calls(e: &Env) -> usize {
    fs::read_to_string(e.ctrl.join("cargo-calls"))
        .unwrap_or_default()
        .lines()
        .count()
}

fn events_named(e: &Env, name: &str) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|ev| ev["event"] == name)
        .collect()
}

/// Hold the fake cargo's build until [`release`] on the path returned.
fn hold_cargo(e: &Env) -> PathBuf {
    e.ctl("cargo-mode", "held");
    let go = e.ctrl.join("cargo-go");
    mkfifo(&go);
    go
}

fn stub_runs(e: &Env) -> Vec<String> {
    fs::read_to_string(e.ctrl.join("stub-runs"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn an_update_builds_while_units_run_and_new_units_start_on_it() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    two_ready(&e);
    let go_cargo = hold_cargo(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(watch_from(&e, &from).env("NS_REFILL_MS", "100").args([
        "watch",
        "--until",
        "06:30",
        "--parallel",
        "2",
    ]));
    w.entered(&in_a);
    w.entered(&in_b);
    let to = land(&e, "cli/x", "new\n");
    release(&go_b);
    w.line("#3 done");
    // The build runs while unit 2 is still in its build: nothing drains.
    w.line(&format!("self-update: building {}", &to[..12]));
    assert!(!w.seen.join("\n").contains("draining"), "{:?}", w.seen);
    release(&go_cargo);
    w.line("new units start on");
    // A unit that starts now, beside unit 2, starts on the new binary.
    e.ready(4, "Fix c", &["type:fix"], "");
    w.line("#4 done");
    let runs = stub_runs(&e);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0].starts_with("run 4-fix-c --issue 4 "), "{runs:?}");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    // Units 2 and 3 finished on the old binary; once none ran the night handed off, its state
    // crossing into one summary with all the spend and the same deadline.
    assert_eq!(by_issue(&units(&v)), each(&[2, 3, 4], "done"), "{v}");
    assert_eq!(v["cost_usd"], 7.5, "{v}");
    assert_eq!(v["until"], "2026-10-09T06:30:00+00:00", "{v}");
    assert_eq!(cargo_calls(&e), 1);
    assert_eq!(
        fs::read_to_string(e.ctrl.join("cargo-built")).unwrap(),
        "new\n"
    );
    let built = events_named(&e, "self_update_built");
    assert_eq!(built.len(), 1, "{built:?}");
    let stage = fs::canonicalize(e.root.join(".git/ns/self-update")).unwrap();
    let staged = stage.join(format!("ns-{to}"));
    assert_eq!(
        (&built[0]["from"], &built[0]["to"], &built[0]["path"]),
        (&json!(from), &json!(to), &json!(staged))
    );
    let calls = fs::read_to_string(e.ctrl.join("cargo-calls")).unwrap();
    assert!(calls.starts_with("build --release --locked "), "{calls}");
    assert!(
        calls
            .trim()
            .ends_with(&format!("--target-dir {}", stage.join("target").display())),
        "{calls} vs {}",
        stage.display()
    );
    let args = fs::read_to_string(e.ctrl.join("handed-off")).unwrap();
    assert!(
        args.starts_with("watch --until 06:30 --parallel 2 --resume-night "),
        "{args}"
    );
    let state = args.trim().rsplit(' ').next().unwrap();
    assert!(!Path::new(state).exists(), "{state} left behind");
    // The check ran the new binary's dry run with the night's own arguments, on a state copy.
    let dry = fs::read_to_string(e.ctrl.join("dry-run-args")).unwrap();
    let dry = dry.trim();
    assert!(
        dry.starts_with("watch --until 06:30 --parallel 2 --resume-night ")
            && dry.ends_with(" --dry-run"),
        "{dry}"
    );
    assert_eq!(
        PathBuf::from(fs::read_to_string(e.ctrl.join("staged")).unwrap().trim()),
        staged
    );
    assert_ne!(staged, common::ns_path());
    let ev = events_named(&e, "self_update");
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!((&ev[0]["from"], &ev[0]["to"]), (&json!(from), &json!(to)));
    assert!(
        err.contains(&format!("self-update {} -> {}", &from[..12], &to[..12])),
        "{err}"
    );
    let started_4 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 4);
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    let handed = event_at(&e, |ev| ev["event"] == "self_update");
    assert!(
        started_4 < ended_2 && ended_2 < handed,
        "{:?}",
        run_events(&e)
    );
    assert_no_night_state(&e);
}

#[test]
fn the_carried_deadline_beats_the_new_binarys_own() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let w = Watching::start(watch_from(&e, &from).args(["watch", "--until", "06:30"]));
    w.entered(&in_a);
    land(&e, "cli/x", "new\n");
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    // The stub drops --until when it execs, so only the carried deadline can be 06:30.
    let args = fs::read_to_string(e.ctrl.join("handed-off")).unwrap();
    assert!(args.contains("--until 06:30"), "{args}");
    assert_eq!(v["until"], "2026-10-09T06:30:00+00:00", "{v}");
}

/// A carried night with nothing in it, for a `--resume-night` test.
fn empty_night(e: &Env) -> PathBuf {
    let path = e.root.join("night.json");
    fs::write(&path, r#"{"version": 1}"#).unwrap();
    path
}

#[test]
fn a_resumed_night_does_not_say_the_main_ci_notice_again() {
    let e = Env::new();
    let path = empty_night(&e);
    let out = e
        .ns()
        .args(["watch", "--once", "--resume-night"])
        .arg(&path)
        .output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("fix_main_first"), "{err}");
}

#[test]
fn a_stop_that_landed_during_the_hand_off_still_stops_the_new_binary() {
    let e = Env::new();
    two_ready(&e);
    let path = empty_night(&e);
    let out = e
        .ns()
        .args(["watch", "--resume-night"])
        .arg(&path)
        .with_pending_sigterm()
        .output();
    let err = String::from_utf8_lossy(&out.stderr);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    assert_eq!(out.status.code(), Some(143), "{err}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert!(units(&v).is_empty(), "{v}");
}

#[test]
fn a_dry_run_reads_the_carried_state_and_fails_on_what_it_cannot_read() {
    for (state, ok) in [
        (r#"{"version": 1}"#, true),
        (r#"{"version": 99}"#, false),
        ("not json", false),
    ] {
        let e = Env::new();
        two_ready(&e);
        let path = e.root.join("night.json");
        fs::write(&path, state).unwrap();
        let out = e
            .ns()
            .args(["watch", "--dry-run", "--resume-night"])
            .arg(&path)
            .output();
        assert_eq!(out.status.success(), ok, "{state}: {out:?}");
    }
}

#[test]
fn a_dry_run_that_resumes_a_night_asks_gh_nothing() {
    let e = Env::new();
    two_ready(&e);
    let path = empty_night(&e);
    let out = e
        .ns()
        .args(["watch", "--dry-run", "--resume-night"])
        .arg(&path)
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["dry_run"], true, "{v}");
    let calls = fs::read_to_string(e.ghd.join("calls")).unwrap_or_default();
    assert_eq!(calls, "", "a hand-off check must not depend on the network");
}

#[test]
fn state_the_new_binary_cannot_read_starts_a_fresh_night() {
    for (state, why) in [
        ("{\"version\": 99}", "version 99"),
        ("not json", "is not a night"),
    ] {
        let e = Env::new();
        two_ready(&e);
        let path = e.root.join("night.json");
        fs::write(&path, state).unwrap();
        let out = e
            .ns()
            .args(["watch", "--until", "06:30", "--resume-night"])
            .arg(&path)
            .assert()
            .code(0)
            .get_output()
            .clone();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("starting a fresh night") && err.contains(why),
            "{err}"
        );
        assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
        assert_eq!(v["until"], "2026-10-09T06:30:00+00:00", "{v}");
    }
    let e = Env::new();
    two_ready(&e);
    let err = e
        .ns()
        .args(["watch", "--resume-night"])
        .arg(e.root.join("missing.json"))
        .assert()
        .code(0)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&err).contains("starting a fresh night"));
}

/// A night held in a build, with an update on origin, whose fake cargo runs in `mode`.
fn watching_an_update(mode: &str) -> (Env, Watching) {
    watching_an_update_in(Env::new(), mode)
}

fn watching_an_update_in(e: Env, mode: &str) -> (Env, Watching) {
    let from = nightshift_repo(&e);
    two_ready(&e);
    e.ctl("cargo-mode", mode);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = Watching::start(watch_from(&e, &from).args(["watch"]));
    w.entered(&in_a);
    land(&e, "cli/x", "new\n");
    release(&go_a);
    w.line("self-update: building");
    (e, w)
}

fn assert_stopped_without_a_hand_off(e: &Env, v: &Value, code: Option<i32>, err: &str) {
    assert_eq!(code, Some(143), "{err}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert!(!e.ctrl.join("handed-off").exists(), "{err}");
    assert!(events_named(e, "self_update").is_empty());
    assert!(events_named(e, "self_update_failed").is_empty(), "{err}");
    assert!(err.contains("abandoned: stopping"), "{err}");
}

#[test]
fn a_stop_during_the_build_kills_it_and_ends_the_night() {
    let e = Env::new();
    // The fake cargo writes a line here once the build starts.
    let started = e.ctrl.join("cargo-entered");
    mkfifo(&started);
    let (e, mut w) = watching_an_update_in(e, "slow");
    w.entered(&started);
    w.signal(libc::SIGTERM, false);
    w.line("SIGTERM: stopping");
    // A build left running holds the night past the hang guard.
    let (code, v, err) = w.finish();
    assert_stopped_without_a_hand_off(&e, &v, code, &err);
}

#[test]
fn a_stop_that_arrives_as_the_build_ends_is_not_lost_at_the_hand_off() {
    let (e, w) = watching_an_update("stopped");
    let (code, v, err) = w.finish();
    assert_stopped_without_a_hand_off(&e, &v, code, &err);
}

#[test]
fn a_build_past_its_ceiling_is_killed_and_the_night_goes_on() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    let to = land(&e, "cli/x", "new\n");
    e.ctl("cargo-mode", "slow");
    two_ready(&e);
    let out = watch_from(&e, &from)
        .env("NS_SELF_UPDATE_TIMEOUT_MS", "300")
        .args(["watch"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_kept_the_old_binary(&e, &to, &v, &err, "timed out");
}

#[test]
fn a_build_holds_no_slot_and_no_triage_and_the_night_hands_off_once_it_ends() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    land(&e, "cli/x", "new\n");
    two_ready(&e);
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    e.queue("triage", &["pass", "pass", "none"]);
    let go_cargo = hold_cargo(&e);
    let mut w = Watching::start(watch_from(&e, &from).args(["watch"]));
    w.line("self-update: building");
    // While cargo builds, both units run on this ns and the triage pass runs too.
    w.line("#2 done");
    w.line("#3 done");
    w.line("triage #5");
    release(&go_cargo);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert!(stub_runs(&e).is_empty());
    let ended_3 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 3);
    let built = event_at(&e, |ev| ev["event"] == "self_update_built");
    let updated = event_at(&e, |ev| ev["event"] == "self_update");
    assert!(ended_3 < built && built < updated, "{:?}", run_events(&e));
}

#[test]
fn a_refill_does_not_check_for_an_update_so_a_new_ready_issue_still_starts() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    e.ready(2, "Fix a", &["type:fix"], "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_c, go_c) = hold_phase(&e, "4-fix-c", "build");
    let listed = mark_gh_calls(&e);
    let w = Watching::start(watch_from(&e, &from).env("NS_REFILL_MS", "100").args([
        "watch",
        "--parallel",
        "2",
    ]));
    w.entered(&in_a);
    land(&e, "cli/x", "new\n");
    // Refills come with a slot free and a new commit on origin: two after the landing.
    while listed.try_recv().is_ok() {}
    for _ in 0..6 {
        w.g.recv(&listed);
    }
    e.ready(4, "Fix c", &["type:fix"], "");
    w.entered(&in_c);
    release(&go_c);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 4], "done"), "{v}");
    let started_4 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 4);
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    assert!(started_4 < ended_2, "{:?}", run_events(&e));
}

#[test]
fn a_cli_change_after_an_update_is_built_while_units_run_and_handed_off_to() {
    let e = Env::new();
    let from = nightshift_repo(&e);
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let (in_c, go_c) = hold_phase(&e, "4-fix-c", "build");
    let mut w = Watching::start(watch_from(&e, &from).env("NS_REFILL_MS", "100").args([
        "watch",
        "--parallel",
        "2",
    ]));
    w.entered(&in_a);
    w.entered(&in_b);
    let first = land(&e, "cli/x", "new\n");
    release(&go_b);
    w.line("new units start on");
    let newer = land(&e, "cli/x", "newer\n");
    e.ready(4, "Fix c", &["type:fix"], "");
    w.entered(&in_c);
    // Unit 2 ends beside unit 4: the fill builds the newer commit, counted from the first.
    release(&go_a);
    w.line(&format!("({} -> {})", &first[..12], &newer[..12]));
    w.line("new units start on");
    release(&go_c);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3, 4], "done"), "{v}");
    assert_eq!(cargo_calls(&e), 2, "{err}");
    assert_eq!(
        fs::read_to_string(e.ctrl.join("cargo-built")).unwrap(),
        "newer\n"
    );
    let built: Vec<_> = events_named(&e, "self_update_built")
        .iter()
        .map(|b| b["to"].clone())
        .collect();
    assert_eq!(built, [json!(first), json!(newer)]);
    let ev = events_named(&e, "self_update");
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(
        (&ev[0]["from"], &ev[0]["to"]),
        (&json!(from), &json!(newer))
    );
}

/// A night on a nightshift repo whose origin has a `cli/` change the build commit lacks, with
/// the fake cargo in `mode`; its summary and stderr.
fn night_with_a_failed_update(mode: &str) -> (Env, String, Value, String) {
    let e = Env::new();
    let from = nightshift_repo(&e);
    let to = land(&e, "cli/x", "new\n");
    e.ctl("cargo-mode", mode);
    two_ready(&e);
    let out = watch_from(&e, &from)
        .args(["watch"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    (e, to, v, err)
}

fn assert_kept_the_old_binary(e: &Env, to: &str, v: &Value, err: &str, why: &str) {
    assert!(events_named(e, "self_update").is_empty());
    assert_kept_after_an_attempt(e, to, v, err, why);
}

fn assert_kept_after_an_attempt(e: &Env, to: &str, v: &Value, err: &str, why: &str) {
    assert_eq!(by_issue(&units(v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(cargo_calls(e), 1, "{err}");
    assert!(!e.ctrl.join("handed-off").exists());
    assert_no_night_state(e);
    let warning = format!("ns watch: warning: self-update to {} failed", &to[..12]);
    assert!(err.contains(&warning) && err.contains(why), "{err}");
    let failed = events_named(e, "self_update_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["to"], to);
    assert!(
        failed[0]["error"].as_str().unwrap().contains(why),
        "{failed:?}"
    );
}

fn assert_no_night_state(e: &Env) {
    let dir = e.root.join(".git/ns/self-update");
    let left: Vec<_> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("night-"))
        .collect();
    assert!(left.is_empty(), "state left behind: {left:?}");
}

#[test]
fn a_failed_build_keeps_the_old_binary_and_is_not_retried() {
    let (e, to, v, err) = night_with_a_failed_update("fail");
    assert_kept_the_old_binary(&e, &to, &v, &err, "could not compile nightshift");
}

#[test]
fn a_failed_check_keeps_the_old_binary_and_is_not_retried() {
    let (e, to, v, err) = night_with_a_failed_update("badcheck");
    assert_kept_the_old_binary(&e, &to, &v, &err, "dry run failed");
    let (e, to, v, err) = night_with_a_failed_update("versionfails");
    assert_kept_the_old_binary(&e, &to, &v, &err, "--version said");
    let (e, to, v, err) = night_with_a_failed_update("badversion");
    assert_kept_the_old_binary(&e, &to, &v, &err, "--version said \"ns 0.1.0 (0000000)\"");
}

#[test]
fn a_failed_exec_keeps_the_old_binary_and_is_not_retried() {
    let (e, to, v, err) = night_with_a_failed_update("unexecutable");
    assert_kept_after_an_attempt(&e, &to, &v, &err, "cannot exec");
    assert_eq!(events_named(&e, "self_update").len(), 1, "{err}");
}

