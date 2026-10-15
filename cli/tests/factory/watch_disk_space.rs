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
