/// The run-log lines of `event`, parsed.
fn logged(e: &Env, event: &str) -> Vec<Value> {
    let log = fs::read_to_string(e.root.join(".git/ns/runs.jsonl")).unwrap();
    log.lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == event)
        .collect()
}

/// A file `NS_FREE_GIB_FILE` names, every filesystem reporting `gib` GiB free.
fn free_gib(e: &Env, gib: u32) -> PathBuf {
    let f = e.base.join("free-gib");
    let tmp = e.base.join("free-gib.tmp");
    fs::write(&tmp, gib.to_string()).unwrap();
    fs::rename(&tmp, &f).unwrap();
    f
}

#[test]
fn watch_stops_the_night_when_disk_space_is_under_the_stop_limit() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let out = e
        .ns()
        .env("NS_FREE_GIB_FILE", free_gib(&e, 5))
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["stopped"], "disk_critical", "{v}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(" has 5 GiB free, under [limits] disk_stop_gb = 10; the night stops"),
        "{err}"
    );
    assert_eq!(v["units"], json!([]), "{v}");
    assert_eq!(e.labels(2), ["status:ready-for-agent", "type:fix"]);
    let critical = logged(&e, "disk_critical");
    assert_eq!(critical.len(), 1, "{critical:?}");
    let want = json!({
        "event": "disk_critical",
        "path": e.base,
        "free_gb": 5,
        "stop_gb": 10,
    });
    let mut got = critical[0].clone();
    got.as_object_mut().unwrap().retain(|k, _| want.get(k).is_some());
    assert_eq!(got, want);
}

#[test]
fn watch_with_nothing_running_waits_on_low_disk_space_and_starts_once_it_returns() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let mut w = Watching::start(
        e.ns()
            .env("NS_FREE_GIB_FILE", free_gib(&e, 15))
            .env("NS_REFILL_MS", "100")
            .args(["watch", "--once"]),
    );
    w.line("15 GiB free, under [limits] disk_wait_gb = 20; no unit starts until there is more");
    free_gib(&e, 25);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(v["stopped"], "max_units", "{v}");
    // The wait is said once, however many refills looked again.
    assert_eq!(err.matches("disk_wait_gb").count(), 1, "{err}");
    let low = logged(&e, "disk_low");
    assert_eq!(low.len(), 1, "{low:?}");
    let want = json!({
        "event": "disk_low",
        "path": e.base,
        "free_gb": 15,
        "wait_gb": 20,
    });
    let mut got = low[0].clone();
    got.as_object_mut().unwrap().retain(|k, _| want.get(k).is_some());
    assert_eq!(got, want);
}

#[test]
fn watch_warns_once_of_free_space_it_cannot_read_and_runs_its_units() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    let out = e
        .ns()
        .env("NS_FREE_GIB_FILE", e.base.join("no-such-file"))
        .args(["watch", "--max-units", "2"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    let err = String::from_utf8_lossy(&out.stderr);
    // Each filesystem once, the worktrees' and, under WSL, /mnt/c.
    let worktrees = format!("free space on {} can't be read", e.base.display());
    assert_eq!(err.matches(&worktrees).count(), 1, "{err}");
}

/// A file `NS_MEMINFO` names, the machine having `total_mb` MB and `available_mb` of it free.
fn meminfo(e: &Env, total_mb: u64, available_mb: u64) -> PathBuf {
    let f = e.base.join("meminfo");
    let tmp = e.base.join("meminfo.tmp");
    let text = format!(
        "MemTotal: {} kB\nMemAvailable: {} kB\n",
        total_mb * 1024,
        available_mb * 1024
    );
    fs::write(&tmp, text).unwrap();
    fs::rename(&tmp, &f).unwrap();
    f
}

#[test]
fn watch_with_nothing_running_waits_for_memory_and_starts_once_it_frees() {
    let e = Env::new();
    break_tool(&e, "systemctl");
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let mut w = Watching::start(
        e.ns()
            .env("NS_MEMINFO", meminfo(&e, 24576, 1000))
            .env("NS_REFILL_MS", "100")
            .args(["watch", "--once"]),
    );
    w.line("free slots wait for memory: 1000 MB available, a unit needs 4096 MB");
    meminfo(&e, 24576, 20000);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(err.matches("free slots wait for memory").count(), 1, "{err}");
    let low = logged(&e, "memory_low");
    assert_eq!(low.len(), 1, "{low:?}");
    assert_eq!(low[0]["available_mb"], 1000);
    assert_eq!(low[0]["need_mb"], 4096);
}

#[test]
fn watch_without_systemd_user_scopes_starts_its_units_without_a_shared_memory_cap() {
    let e = Env::new();
    break_tool(&e, "systemctl");
    logging_systemd_run(&e);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let out = e
        .ns()
        .env("NS_MEMINFO", meminfo(&e, 24576, 20000))
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("units start without a shared memory cap: systemctl --user set-property failed"),
        "{err}"
    );
    // No unit went through a systemd scope.
    assert!(!e.ctrl.join("systemd-run.log").exists());
}

/// Fake `systemd-run` that logs its arguments and the `NS_SLICE` it was started with to
/// `ctrl/systemd-run.log`, then runs the command after `--`.
fn logging_systemd_run(e: &Env) {
    let c = e.ctrl.display();
    write_exe(
        &e.bin.join("systemd-run"),
        format!(
            "#!/bin/sh\necho \"$* NS_SLICE=${{NS_SLICE-unset}}\" >> '{c}/systemd-run.log'\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = -- ]; then shift; break; fi\n  shift\ndone\nexec \"$@\"\n"
        ),
    );
}

/// Fake `systemctl` that logs its arguments to `ctrl/systemctl.log` and succeeds.
fn logging_systemctl(e: &Env) {
    let c = e.ctrl.display();
    write_exe(
        &e.bin.join("systemctl"),
        format!("#!/bin/sh\necho \"$*\" >> '{c}/systemctl.log'\n"),
    );
}

/// Run one unit through `ns watch` with fake systemd tools; the `systemctl` and `systemd-run`
/// lines they logged.
fn watch_one_unit_in_a_slice(e: &Env, total_mb: u64) -> (Vec<String>, Vec<String>) {
    logging_systemctl(e);
    logging_systemd_run(e);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let out = e
        .ns()
        .env("NS_MEMINFO", meminfo(e, total_mb, 20000))
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("without a shared memory cap"), "{err}");
    let lines = |name: &str| -> Vec<String> {
        fs::read_to_string(e.ctrl.join(name))
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    };
    (lines("systemctl.log"), lines("systemd-run.log"))
}

/// The unit's own slice: the one `systemd-run` line that starts a command other than the probe
/// has `--slice=ns-<pid>x1.slice` and the same name in the unit's `NS_SLICE`.
fn assert_unit_started_in_its_slice(runs: &[String]) {
    let unit: Vec<&String> = runs.iter().filter(|l| !l.contains(" -- true ")).collect();
    assert_eq!(unit.len(), 1, "{runs:?}");
    let line = unit[0];
    let slice = line
        .split_whitespace()
        .find_map(|w| w.strip_prefix("--slice="))
        .unwrap();
    let pid = slice
        .strip_prefix("ns-")
        .and_then(|s| s.strip_suffix("x1.slice"))
        .unwrap();
    assert!(!pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit()), "{line}");
    assert!(line.ends_with(&format!("NS_SLICE={slice}")), "{line}");
}

#[test]
fn watch_caps_the_shared_slice_at_total_memory_less_4_gb_and_starts_its_unit_in_a_slice() {
    let e = Env::new();
    let (cap, runs) = watch_one_unit_in_a_slice(&e, 24576);
    assert_eq!(
        cap,
        ["--user set-property --runtime ns.slice MemoryMax=20480M MemorySwapMax=0"]
    );
    assert_unit_started_in_its_slice(&runs);
    // The probe runs in the shared slice itself.
    assert!(runs.iter().any(|l| l.contains("--slice=ns.slice -- true")), "{runs:?}");
}

#[test]
fn watch_caps_the_shared_slice_at_total_memory_mb_when_the_factory_sets_it() {
    let e = Env::new();
    e.factory("[limits]\ntotal_memory_mb = 9000\n");
    let (cap, runs) = watch_one_unit_in_a_slice(&e, 24576);
    assert_eq!(
        cap,
        ["--user set-property --runtime ns.slice MemoryMax=9000M MemorySwapMax=0"]
    );
    assert_unit_started_in_its_slice(&runs);
}

/// `ns run` in the slice `ns-a1.slice` of a cgroup tree `NS_CGROUP_DIR` names, whose slice
/// records `peak` bytes when given; the run's `end` event.
fn run_in_slice(e: &Env, peak: Option<&str>) -> Value {
    let dir = e.base.join("cgroup");
    let slice = dir.join("root/ns.slice/ns-a1.slice");
    fs::create_dir_all(&slice).unwrap();
    fs::write(dir.join("proc-cgroup"), "0::/ns.slice/ns-a1.slice/run.scope\n").unwrap();
    if let Some(p) = peak {
        fs::write(slice.join("memory.peak"), p).unwrap();
    }
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.ns()
        .env("NS_SLICE", "ns-a1.slice")
        .env("NS_CGROUP_DIR", &dir)
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let ends = logged(e, "end");
    assert_eq!(ends.len(), 1, "{ends:?}");
    ends[0].clone()
}

#[test]
fn run_records_the_slice_peak_on_its_end_event_in_whole_mb_rounded_up() {
    let e = Env::new();
    let end = run_in_slice(&e, Some("3145729\n"));
    assert_eq!(end["memory_peak_mb"], 4, "{end}");
}

#[test]
fn run_leaves_memory_peak_mb_off_its_end_event_when_the_cgroup_records_none() {
    let e = Env::new();
    let end = run_in_slice(&e, None);
    assert!(end.get("memory_peak_mb").is_none(), "{end}");
    assert_eq!(end["outcome"], "done", "{end}");
}
