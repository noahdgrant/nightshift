#[test]
fn a_sigint_to_a_plain_run_ends_the_setup_command() {
    let e = Env::new();
    let f = fifo(&e);
    e.factory(&format!(
        "[worktree]\nsetup = {:?}\n",
        [
            blocks(&f),
            "echo done >> \"$NS_MAIN_ROOT/../setup.done\"".to_string()
        ]
    ));
    let mut g = e.ns().args(["run", "--issue", "7"]).start_piped();
    let stderr = read_all(g.take_stderr());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(&f));
    });
    let pids: Vec<String> = g
        .recv(&rx)
        .unwrap()
        .split_whitespace()
        .map(String::from)
        .collect();
    assert_eq!(pids.len(), 3, "{pids:?}");
    // SAFETY: kill(2) on the ns process this test started.
    assert_eq!(
        unsafe { libc::kill(g.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    let code = g.wait().code();
    let stderr = String::from_utf8_lossy(&g.recv(&stderr)).into_owned();
    assert_eq!(code, Some(130), "{stderr}");
    for pid in &pids[1..] {
        assert!(common::exits(pid), "pid {pid} outlived ns: {stderr}");
    }
    assert!(!e.base.join("setup.done").exists());
    assert!(e.calls().is_empty());
}

#[test]
fn a_run_in_a_night_survives_a_second_signal_during_setup() {
    let e = Env::new();
    let f = fifo(&e);
    e.factory(&format!(
        "[worktree]\nsetup = {:?}\n",
        [
            blocks(&f),
            "echo done >> \"$NS_MAIN_ROOT/../setup.done\"".to_string()
        ]
    ));
    let night = night_dir(&e, 0.0, None);
    let mut g = e
        .ns()
        .args(["run", "--issue", "7", "--night", night.to_str().unwrap()])
        .start_piped();
    let stderr = read_all(g.take_stderr());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(&f));
    });
    let pids: Vec<String> = g
        .recv(&rx)
        .unwrap()
        .split_whitespace()
        .map(String::from)
        .collect();
    assert_eq!(pids.len(), 3, "{pids:?}");
    let pid = g.id() as libc::pid_t;
    // SAFETY: kill(2) on the ns process this test started.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    for p in &pids[1..] {
        assert!(common::exits(p), "pid {p} outlived the first signal");
    }
    // SAFETY: as above; the process may be a zombie by now, which still takes a signal.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let code = g.wait().code();
    let stderr = String::from_utf8_lossy(&g.recv(&stderr)).into_owned();
    assert_eq!(code, Some(143), "{stderr}");
    assert!(!e.base.join("setup.done").exists());
}
