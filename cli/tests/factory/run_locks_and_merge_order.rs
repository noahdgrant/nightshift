/// Gh's CI wait blocks for whichever PR reaches it first, named in `ctrl/first-pr`, until
/// another PR merges, and each wait's start and end land in `ctrl/order`. Merging a PR moves
/// `origin`'s main, as a squash on GitHub does, and puts every other open one in `prs` behind
/// main, reported as `UNKNOWN` for one view while GitHub works that out.
fn merges_in_order(e: &Env, prs: &[u64]) {
    let go = e.ctrl.join("go");
    assert!(StdCommand::new("mkfifo")
        .arg(&go)
        .status()
        .unwrap()
        .success());
    let others: Vec<String> = prs.iter().map(u64::to_string).collect();
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$1 $2 $4\" in\n\
             \"pr checks --watch\")\n\
               echo \"in $3\" >> {order:?}\n\
               mkdir {first:?} 2>/dev/null && echo \"$3\" > {first_pr:?} && cat {go:?} > /dev/null\n\
               echo \"out $3\" >> {order:?} ;;\n\
             \"pr merge \"*)\n\
               {move_main}\n\
               for n in {others}; do\n\
                 [ \"$n\" != \"$3\" ] && [ \"$(cat \"$d/pr-$n.state\" 2>/dev/null)\" != MERGED ] \\\n\
                   && echo UNKNOWN > \"$d/pr-$n.merge\"\n\
               done\n\
               [ -d {first:?} ] && mkdir {released:?} 2>/dev/null && echo go > {go:?} ;;\n\
             \"pr view \"*)\n\
               if [ \"$(cat \"$d/pr-$3.merge\" 2>/dev/null)\" = UNKNOWN ]; then\n\
                 [ -f \"$d/unknown-$3\" ] && echo BEHIND > \"$d/pr-$3.merge\"\n\
                 touch \"$d/unknown-$3\"\n\
               fi ;;\n\
             esac\n",
            order = e.ctrl.join("order"),
            first = e.ctrl.join("first"),
            first_pr = e.ctrl.join("first-pr"),
            released = e.ctrl.join("released"),
            move_main = move_main_sh(e),
            others = others.join(" "),
        ),
    );
}

/// After [`merges_in_order`]: the PR that reached CI first waited there, without the merge
/// lock, while the other merged; then it saw main move, was updated to it, waited for CI again
/// and merged.
fn assert_merged_while_the_other_waited(e: &Env, prs: [u64; 2]) {
    let first: u64 = fs::read_to_string(e.ctrl.join("first-pr"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let second = if first == prs[0] { prs[1] } else { prs[0] };
    let order = fs::read_to_string(e.ctrl.join("order")).unwrap();
    let order: Vec<&str> = order.lines().collect();
    let of = |n: u64| -> Vec<&str> {
        order
            .iter()
            .copied()
            .filter(|l| l.ends_with(&format!(" {n}")))
            .collect()
    };
    let (f, s) = (format!("{first}"), format!("{second}"));
    assert_eq!(
        of(first),
        [
            &*format!("in {f}"),
            &*format!("out {f}"),
            &*format!("in {f}"),
            &*format!("out {f}")
        ],
        "{order:?}"
    );
    assert_eq!(
        of(second),
        [&*format!("in {s}"), &*format!("out {s}")],
        "{order:?}"
    );
    let at = |l: String| order.iter().position(|o| *o == l).unwrap();
    assert!(at(format!("out {s}")) < at(format!("out {f}")), "{order:?}");

    let calls: Vec<String> = e.gh_calls().lines().map(String::from).collect();
    let pos = |want: &str, from: usize| -> usize {
        from + calls[from..]
            .iter()
            .position(|c| c.starts_with(want))
            .unwrap_or_else(|| panic!("no {want:?} after call {from}: {calls:?}"))
    };
    let merged = pos(&format!("pr merge {s} --squash"), 0);
    let updated = pos(&format!("pr update-branch {f}"), merged);
    let checked = pos(&format!("pr checks {f} --watch"), updated);
    pos(&format!("pr merge {f} --squash"), checked);
    assert!(lock_is_free(&e.root.join(".git/ns-merge.lock")));
    let moved: Vec<Value> = run_events(e)
        .into_iter()
        .filter(|ev| ev["event"] == "merge_base_moved")
        .collect();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(
        (&moved[0]["pr"], &moved[0]["attempt"]),
        (&json!(first), &json!(1))
    );
    // Each merge added one commit to main: `from` is main before both, `to` after the first.
    let remote = e.base.join("remote.git");
    let main_at = |rev: &str| json!(git(&remote, &["rev-parse", rev]));
    assert_eq!(
        (&moved[0]["from"], &moved[0]["to"]),
        (&main_at("main~2"), &main_at("main~1")),
        "{moved:?}"
    );
}

/// Start `ns run --issue <n>` for each of `issues`, their stderr lines merged into one channel,
/// with `None` when one of them closes its stderr.
fn start_runs(e: &Env, issues: &[u64]) -> (Vec<Group>, mpsc::Receiver<Option<String>>) {
    let (tx, rx) = mpsc::channel();
    let runs = issues
        .iter()
        .map(|n| {
            let mut g = e.ns().args(["run", "--issue", &n.to_string()]).start();
            send_lines(g.take_stderr(), tx.clone());
            g
        })
        .collect();
    (runs, rx)
}

/// Send each line `r` gives on `tx`, then `None` when it closes.
fn send_lines(r: impl Read + Send + 'static, tx: mpsc::Sender<Option<String>>) {
    thread::spawn(move || {
        for l in BufReader::new(r).lines().map_while(Result::ok) {
            let _ = tx.send(Some(l));
        }
        let _ = tx.send(None);
    });
}

/// Read `rx` until a line contains `want` (true) or a stream closes (false), keeping every
/// line read in `seen`.
fn line_until(
    g: &Group,
    rx: &mpsc::Receiver<Option<String>>,
    want: &str,
    seen: &mut Vec<String>,
) -> bool {
    while let Some(l) = g.recv(rx) {
        let hit = l.contains(want);
        seen.push(l);
        if hit {
            return true;
        }
    }
    false
}

#[test]
fn two_units_merge_one_at_a_time_each_against_the_main_the_other_left() {
    two_units_merge_one_at_a_time(AUTO);
}

#[test]
fn under_build_only_two_units_still_merge_one_at_a_time() {
    two_units_merge_one_at_a_time(&format!("{AUTO}[gate]\nlocal = \"build-only\"\n"));
}

fn two_units_merge_one_at_a_time(factory: &str) {
    let e = Env::new();
    e.factory(factory);
    e.issue(8, "Fix the other", "OPEN");
    e.ctl(&format!("pr-{UNIT}"), "12");
    e.ctl("pr-8-fix-the-other", "13");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    // The first run's CI wait ends only once the other run merged, so the other must take the
    // merge lock while the first waits on CI. A step that held the lock through its CI wait
    // would never let it, and the runs would hang.
    merges_in_order(&e, &[12, 13]);
    let (mut runs, _rx) = start_runs(&e, &[7, 8]);
    for r in &mut runs {
        assert!(r.wait().success());
    }
    assert_merged_while_the_other_waited(&e, [12, 13]);
}

#[test]
fn a_unit_with_nothing_to_merge_does_not_wait_for_the_merge_lock() {
    let e = Env::new();
    e.factory(AUTO);
    let _held = hold(
        &e.root.join(".git/ns-merge.lock"),
        r#"{"pid":42,"unit":"other"}"#,
    );
    // ship's pr.md names no PR, so the merge step ends before it needs the lock.
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        v["reason"], "pr.md has no pr: number; needs a human merge",
        "{v}"
    );
    assert!(!err.contains("merge lock"), "{err}");
}

#[test]
fn a_merge_state_github_never_works_out_is_merged_on_after_the_register_timeout() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "UNKNOWN");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    let views = calls
        .lines()
        .filter(|l| *l == "pr view 12 --json mergeStateStatus")
        .count();
    assert!(views > 1, "{calls}");
    assert!(!calls.contains("pr update-branch"), "{calls}");
}

#[test]
fn a_worktree_waits_for_the_worktree_lock() {
    let e = Env::new();
    let held = hold(
        &e.root.join(".git/ns-worktree.lock"),
        r#"{"pid":42,"unit":"other"}"#,
    );
    let mut g = e.ns().args(["worktree", "new", "u1"]).start();
    let (tx, rx) = mpsc::channel();
    send_lines(g.take_stderr(), tx);
    let mut seen = Vec::new();
    let waited = line_until(
        &g,
        &rx,
        "ns worktree: u1 waits for the worktree lock (held by pid 42, unit other)",
        &mut seen,
    );
    assert!(waited, "{}", seen.join("\n"));
    assert!(!e.worktree("u1").exists());
    drop(held);
    assert!(g.wait().success());
    assert!(e.worktree("u1").is_dir());
}

#[test]
fn a_stop_during_a_merge_lock_wait_returns_the_issue_to_the_queue() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    let _held = hold(
        &e.root.join(".git/ns-merge.lock"),
        r#"{"pid":42,"unit":"other"}"#,
    );
    let mut g = e.ns().args(["watch", "--once"]).start_piped();
    let stdout = read_all(g.take_stdout());
    let (tx, rx) = mpsc::channel();
    send_lines(g.take_stderr(), tx);
    let mut seen = Vec::new();
    let waited = line_until(
        &g,
        &rx,
        "ns run: 2-fix-a merge waits for the merge lock (held by pid 42, unit other)",
        &mut seen,
    );
    assert!(waited, "{}", seen.join("\n"));
    // SAFETY: kill(2) on the ns process this test started.
    assert_eq!(
        unsafe { libc::kill(g.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    assert_eq!(g.wait().code(), Some(143));
    let v: Value = serde_json::from_slice(&g.recv(&stdout)).unwrap();
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert_eq!(v["units"][0]["outcome"], "interrupted", "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    // CI is waited on without the lock, so only the merge itself waits for it.
    let calls = e.gh_calls();
    assert!(calls.contains("pr checks 12 --watch"), "{calls}");
    assert!(!calls.contains("pr merge"), "{calls}");
}

fn read_all(mut r: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

struct Interrupted {
    code: Option<i32>,
    json: Value,
    stderr: String,
    /// The blocked process's parent, the process itself and its background `sleep`.
    pids: Vec<String>,
}

/// A fifo under `ctrl` that [`blocks`] writes to.
fn fifo(e: &Env) -> PathBuf {
    let f = e.ctrl.join("blocked.fifo");
    assert!(StdCommand::new("mkfifo")
        .arg(&f)
        .status()
        .unwrap()
        .success());
    f
}

/// Shell that starts a long `sleep`, tells the test through `fifo` which processes to watch
/// (`$PPID $$ <sleep>`) and waits, so it ends only when it is killed.
fn blocks(fifo: &Path) -> String {
    format!(
        "sleep 600 & echo \"$PPID $$ $!\" > '{}'; wait",
        fifo.display()
    )
}

/// Run `ns watch --once`. Once something runs [`blocks`] on `fifo`, send `sig`: to ns's process
/// group when `ctrl_c`, as a terminal's Ctrl-C does (a phase, in its own session, gets nothing),
/// else to ns alone.
fn interrupt(e: &Env, fifo: PathBuf, sig: i32, ctrl_c: bool) -> Interrupted {
    let mut g = e.ns().args(["watch", "--once"]).start_piped();
    let stdout = read_all(g.take_stdout());
    let stderr = read_all(g.take_stderr());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(&fifo));
    });
    let pids: Vec<String> = g
        .recv(&rx)
        .unwrap()
        .split_whitespace()
        .map(String::from)
        .collect();
    assert_eq!(pids.len(), 3, "{pids:?}");
    let pid = g.id() as libc::pid_t;
    let target = if ctrl_c { -pid } else { pid };
    // SAFETY: kill(2) on the ns process, or its process group, that this test started.
    assert_eq!(unsafe { libc::kill(target, sig) }, 0);
    let code = g.wait().code();
    let stderr = String::from_utf8_lossy(&g.recv(&stderr)).into_owned();
    let out = g.recv(&stdout);
    let json = serde_json::from_slice(&out).unwrap_or_else(|x| {
        panic!(
            "{x}: stdout={} stderr={stderr}",
            String::from_utf8_lossy(&out)
        )
    });
    Interrupted {
        code,
        json,
        stderr,
        pids,
    }
}

/// `ns watch` stopped on `sig` with issue 2's unit interrupted, nothing it started still
/// running, and the issue back in the queue.
fn assert_interrupted(e: &Env, r: &Interrupted, sig: &str, code: i32) {
    let (v, err) = (&r.json, &r.stderr);
    assert_eq!(r.code, Some(code), "{err}");
    assert_eq!(v["stopped"], sig, "{v}");
    assert_eq!(v["units"][0]["issue"], 2, "{v}");
    assert_eq!(v["units"][0]["outcome"], "interrupted", "{v}");
    for pid in &r.pids {
        assert!(exits(pid), "process {pid} outlived ns watch: {err}");
    }
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    assert!(
        run_events(e)
            .iter()
            .any(|ev| ev["event"] == "end" && ev["outcome"] == "interrupted"),
        "{err}"
    );
}

/// Issue 2 is ready, and its build phase writes a partial `build.md` and blocks.
fn blocking_build(e: &Env) -> PathBuf {
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(e);
    e.ctl(
        "build.sh",
        &format!("echo partial > \".ns/$NS_UNIT/build.md\"\n{}\n", blocks(&f)),
    );
    e.queue("build", &["pass:script"]);
    f
}

/// The interrupted build's partial `build.md` was set aside and its `phase` event logged.
fn assert_build_set_aside(e: &Env, event: &str, sig: &str, partial: &str) {
    let unit = e.worktree("2-fix-a").join(".ns/2-fix-a");
    assert!(unit.join("brief.md").is_file());
    assert!(!unit.join("build.md").exists());
    let kept = fs::read_to_string(unit.join("history/build-interrupted-1.md")).unwrap();
    assert!(kept.contains(partial), "{kept}");
    assert!(run_events(e)
        .iter()
        .any(|ev| ev["event"] == event && ev["phase"] == "build" && ev["interrupted"] == sig));
}

#[test]
fn sigterm_kills_the_phase_and_returns_the_issue_to_the_queue() {
    let e = Env::new();
    let f = blocking_build(&e);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert_build_set_aside(&e, "phase", "SIGTERM", "partial");

    // The next night resumes the unit at build.
    fs::remove_file(e.ctrl.join("calls")).unwrap();
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.calls(), ["build", "verify", "review", "ship"]);
}

#[test]
fn ctrl_c_kills_the_phase_and_returns_the_issue_to_the_queue() {
    let e = Env::new();
    let f = blocking_build(&e);
    let r = interrupt(&e, f, libc::SIGINT, true);
    assert_interrupted(&e, &r, "SIGINT", 130);
    assert_build_set_aside(&e, "phase", "SIGINT", "partial");
}

/// Whether `pid` has a handler installed for `sig`, from the `SigCgt` mask in its status.
fn catches(pid: u32, sig: i32) -> bool {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let mask = status
        .lines()
        .find_map(|l| l.strip_prefix("SigCgt:"))
        .map(|m| u64::from_str_radix(m.trim(), 16).unwrap())
        .unwrap();
    mask & (1 << (sig - 1)) != 0
}

#[test]
fn a_second_signal_ends_watch_at_once() {
    use std::os::unix::process::ExitStatusExt;
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    // ns hangs on a gh call, which a SIGTERM sent to ns alone doesn't reach.
    let f = fifo(&e);
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$*\" in *\"--add-label status:in-progress\"*) echo hung > '{}'; sleep 600 ;; esac\n",
            f.display()
        ),
    );
    let mut g = e.ns().args(["watch", "--once"]).start();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(&f));
    });
    g.recv(&rx).unwrap();
    assert!(catches(g.id(), libc::SIGTERM));
    // SAFETY: kill(2) on the ns process this test started.
    assert_eq!(
        unsafe { libc::kill(g.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    // The handler has run once SIGTERM is no longer caught.
    let deadline = std::time::Instant::now() + common::EXIT_WAIT;
    while catches(g.id(), libc::SIGTERM) {
        assert!(
            std::time::Instant::now() < deadline,
            "SIGTERM never handled"
        );
        thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: as above.
    assert_eq!(
        unsafe { libc::kill(g.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    assert_eq!(g.wait().signal(), Some(libc::SIGTERM));
}

#[test]
fn a_stop_during_the_build_gate_runs_the_build_again() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    e.factory(&format!("[phases.build]\ngate = {:?}\n", blocks(&f)));
    e.queue("build", &["pass:commit"]);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert_build_set_aside(&e, "gate", "SIGTERM", "status: pass");

    // The build the gate never passed runs again, and its gate with it.
    e.factory("[phases.build]\ngate = \"touch gate-ran\"\n");
    fs::remove_file(e.ctrl.join("calls")).unwrap();
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(e.calls(), ["build", "verify", "review", "ship"]);
    assert!(e.worktree("2-fix-a").join("gate-ran").exists());
}

#[test]
fn a_stop_during_the_gate_after_an_empty_build_leaves_the_last_build_md() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    // The gate is red after the first build, and blocks after the second.
    let gate = format!(
        "if [ -f '{seen}' ]; then {}; else touch '{seen}'; exit 1; fi",
        blocks(&f),
        seen = e.ctrl.join("gate-seen").display()
    );
    e.factory(&format!("[phases.build]\ngate = {gate:?}\n"));
    // The second build commits but writes nothing, so its moves are undone before the gate.
    e.queue("build", &["pass:commit", "none:commit"]);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    let unit = e.worktree("2-fix-a").join(".ns/2-fix-a");
    let build = fs::read_to_string(unit.join("build.md")).unwrap();
    assert!(build.contains("status: pass"), "{build}");
    assert!(!unit.join("history/build-interrupted-1.md").exists());
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "gate"
        && ev["attempt"] == 2
        && ev["interrupted"] == "SIGTERM"
        && ev.get("archived").is_none()));
}

#[test]
fn a_stop_during_the_ci_wait_resumes_at_the_merge_the_next_night() {
    let e = Env::new();
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    // The unit's own PR closes the issue, as ship's does.
    e.gh_file(
        "prs.json",
        r#"[{"number":12,"body":"Closes #2","headRefName":"ns/2-fix-a","isCrossRepository":false}]"#,
    );
    let f = fifo(&e);
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$*\" in *\"pr checks\"*--watch*) {}; exit 0 ;; esac\n",
            blocks(&f)
        ),
    );
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert!(!e.gh_calls().contains("pr merge"));

    fs::remove_file(e.ghd.join("hook.sh")).unwrap();
    fs::remove_file(e.ctrl.join("calls")).unwrap();
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert!(e.calls().is_empty(), "{:?}", e.calls());
    assert!(e.gh_calls().contains("pr merge 12"));
}

#[test]
fn watch_still_skips_an_issue_a_pr_from_another_branch_closes() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    // Another unit's branch, an older title's unit branch, and a fork's branch of the same name.
    for head in [
        r#""headRefName":"ns/21-other","isCrossRepository":false"#,
        r#""headRefName":"ns/2-old-title","isCrossRepository":false"#,
        r#""headRefName":"ns/2-fix-a","isCrossRepository":true"#,
    ] {
        e.gh_file(
            "prs.json",
            &format!(r#"[{{"number":12,"body":"Closes #2",{head}}}]"#),
        );
        let v = e.run(&["watch", "--dry-run"], 0);
        assert_eq!(v["queue"], serde_json::json!([]), "{head}: {v}");
        assert_eq!(v["skipped"][0]["reason"], "open PR #12 closes it", "{v}");
    }
}

/// `[worktree] setup` that logs each start to `setup.log` beside the repo, runs `cut` (which
/// ends setup partway) the first time only, then logs the finish to `setup.done`.
fn setup_cut_once(e: &Env, cut: &str) {
    let commands = [
        "echo run >> \"$NS_MAIN_ROOT/../setup.log\"".to_string(),
        format!(
            "if [ ! -e \"$NS_MAIN_ROOT/../cut\" ]; then touch \"$NS_MAIN_ROOT/../cut\"; {cut}; fi"
        ),
        "echo done >> \"$NS_MAIN_ROOT/../setup.done\"".to_string(),
    ];
    e.factory(&format!("[worktree]\nsetup = {commands:?}\n"));
}

/// How many times setup started and finished.
fn setup_runs(e: &Env) -> (usize, usize) {
    let count = |f: &str| {
        fs::read_to_string(e.base.join(f))
            .unwrap_or_default()
            .lines()
            .count()
    };
    (count("setup.log"), count("setup.done"))
}

#[test]
fn a_crash_during_setup_reruns_setup_on_the_next_run() {
    use std::os::unix::process::ExitStatusExt;
    let e = Env::new();
    // Setup's shell is a child of ns.
    setup_cut_once(&e, "kill -KILL \"$PPID\"");
    let out = e.ns().args(["run", "--issue", "7"]).output();
    assert_eq!(out.status.signal(), Some(libc::SIGKILL), "{out:?}");
    assert!(e.worktree(UNIT).is_dir());
    assert_eq!(setup_runs(&e), (1, 0));
    assert!(e.calls().is_empty());

    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(setup_runs(&e), (2, 1));

    // Finished setup is on record, so reusing the worktree doesn't run it again.
    let v = e.run(&["worktree", "new", UNIT], 0);
    assert_eq!(v["setup"], serde_json::json!([]));
    assert_eq!(setup_runs(&e), (2, 1));
}

#[test]
fn a_stop_during_setup_reruns_setup_on_the_next_run() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    setup_cut_once(&e, &blocks(&f));
    // SIGTERM to the group, as a service manager stopping ns watch sends it.
    let r = interrupt(&e, f, libc::SIGTERM, true);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert!(
        !run_events(&e)
            .iter()
            .any(|ev| ev["event"] == "end" && ev["outcome"] == "stuck"),
        "{}",
        r.stderr
    );
    assert_eq!(setup_runs(&e), (1, 0));
    assert!(e.calls().is_empty());

    let v = e.run(&["run", "--issue", "2"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(setup_runs(&e), (2, 1));
}

/// Run `ns watch --once` on ready issue 2 through to the merge step, with a `gh` that sends ns
/// SIGTERM when it is called with `call`, then answers normally.
fn stop_at_gh(e: &Env, call: &str) -> (std::process::Output, Value) {
    e.factory(AUTO);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    // ns runs gh directly, so gh's parent is ns.
    e.gh_file(
        "hook.sh",
        &format!("case \"$*\" in \"{call}\"*) kill -TERM \"$PPID\" ;; esac\n"),
    );
    let out = e.ns().args(["watch", "--once"]).output();
    let v = serde_json::from_slice(&out.stdout).unwrap_or_else(|x| panic!("{x}: {out:?}"));
    (out, v)
}

#[test]
fn a_stop_that_lands_while_a_unit_ends_needing_a_human_returns_the_issue() {
    let e = Env::new();
    e.gh_file("diff-12.txt", ".github/workflows/ci.yml\n");
    let (out, v) = stop_at_gh(&e, "pr diff 12");
    assert_eq!(out.status.code(), Some(143), "{out:?}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert_eq!(v["units"][0]["outcome"], "interrupted", "{v}");
    assert!(v["units"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("needs a human merge"));
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
    assert!(!e.gh_calls().contains("issue comment"));
}

#[test]
fn a_stop_that_lands_while_a_unit_merges_keeps_the_merge() {
    let e = Env::new();
    e.gh_file("diff-12.txt", "src/x.rs\n");
    let (out, v) = stop_at_gh(&e, "pr merge 12");
    assert_eq!(out.status.code(), Some(143), "{out:?}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert_eq!(v["units"][0]["outcome"], "merged", "{v}");
    assert_eq!(e.labels(2), ["type:fix"]);
    assert!(e.gh_calls().contains("issue close 2"));
}

#[test]
fn sigterm_ends_a_usage_limit_sleep_and_returns_the_issue() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    e.ctl("reset", &(now + 3600).to_string());
    e.queue("build", &["limit"]);
    let mut g = e
        .ns()
        .env_remove("NS_NOW")
        .args(["watch", "--once"])
        .start_piped();
    let stdout = read_all(g.take_stdout());
    let stderr = g.take_stderr();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if line.contains("usage limit, sleeping until") {
                let _ = tx.send(());
            }
        }
    });
    g.recv(&rx);
    // SAFETY: kill(2) on the ns process this test started.
    assert_eq!(
        unsafe { libc::kill(g.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    assert_eq!(g.wait().code(), Some(143));
    let v: Value = serde_json::from_slice(&g.recv(&stdout)).unwrap();
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    let outcomes: Vec<&Value> = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| &u["outcome"])
        .collect();
    assert_eq!(outcomes, ["paused", "interrupted"], "{v}");
    assert_eq!(e.labels(2), ["type:fix", "status:ready-for-agent"]);
}

/// Whether another process could take `lock` right now, asked through flock(1).
fn lock_is_free(lock: &Path) -> bool {
    StdCommand::new("flock")
        .args(["-n", lock.to_str().unwrap(), "true"])
        .status()
        .unwrap()
        .success()
}

#[test]
fn runner_locks_are_held_for_the_phase_and_released_on_failure() {
    let e = Env::new();
    let locks = e.base.join("locks");
    let lock = locks.join("bench-1.lock");
    e.bench(&locks);
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("verify", &["fail:script", "fail:script"]);
    e.ctl(
        "verify.sh",
        &format!(
            "flock -n {l:?} true && echo free >> {s:?} || echo held >> {s:?}; cat {l:?} >> {s:?}",
            l = lock,
            s = e.ctrl.join("seen"),
        ),
    );
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck");
    let seen = fs::read_to_string(e.ctrl.join("seen")).unwrap();
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(lines.len(), 4, "{seen}");
    for pair in lines.chunks(2) {
        assert_eq!(pair[0], "held", "{seen}");
        let holder: Value = serde_json::from_str(pair[1]).unwrap();
        assert_eq!(holder["unit"], UNIT);
        assert!(holder["pid"].as_u64().is_some(), "{seen}");
    }
    assert!(lock_is_free(&lock));
    assert_eq!(fs::read_to_string(&lock).unwrap(), "");
}

#[test]
fn the_default_lock_dir_is_under_the_git_common_dir() {
    let e = Env::new();
    e.factory("[phases.verify]\nrunner = \"bench\"\n");
    e.runner("bench", "kind = \"bench\"\nlocks = [\"bench-1\"]\n");
    let lock = e.root.join(".git/ns/locks/bench-1.lock");
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["pass:script"]);
    e.ctl(
        "verify.sh",
        &format!("cat {:?} > {:?}", lock, e.ctrl.join("seen")),
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let holder: Value =
        serde_json::from_str(&fs::read_to_string(e.ctrl.join("seen")).unwrap()).unwrap();
    assert_eq!(holder["unit"], UNIT);
    assert!(lock.exists());
    assert!(lock_is_free(&lock));
}

#[test]
fn leftover_content_in_a_free_runner_lock_is_overwritten() {
    let e = Env::new();
    let locks = e.base.join("locks");
    let lock = locks.join("bench-1.lock");
    e.bench(&locks);
    fs::create_dir_all(&locks).unwrap();
    let stale = serde_json::json!({"pid": 2147483000, "unit": "x".repeat(200)});
    fs::write(&lock, format!("{stale}\n")).unwrap();
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["pass:script"]);
    e.ctl(
        "verify.sh",
        &format!("cat {:?} > {:?}", lock, e.ctrl.join("seen")),
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let holder: Value =
        serde_json::from_str(&fs::read_to_string(e.ctrl.join("seen")).unwrap()).unwrap();
    assert_eq!(holder["unit"], UNIT);
    assert!(lock_is_free(&lock));
}

enum Event {
    Entered,
    Exited(String),
}

/// A run whose verify phase holds its lock until `release` is called.
struct Blocked {
    run: Group,
    go: PathBuf,
}

impl Blocked {
    /// Returns once the run's verify phase is inside the harness, or panics with its stderr if
    /// the run ended first.
    fn start(e: &Env, before: &str, after: &str) -> Blocked {
        let entered = e.ctrl.join("entered");
        let go = e.ctrl.join("go");
        for f in [&entered, &go] {
            assert!(StdCommand::new("mkfifo").arg(f).status().unwrap().success());
        }
        e.queue("verify", &["pass:script"]);
        e.ctl(
            "verify.sh",
            &format!("{before}; echo in > {entered:?}; cat {go:?} > /dev/null; {after}"),
        );
        let mut run = e.ns().args(["run", "--issue", "7"]).start();
        let mut stderr = run.take_stderr();
        let (tx, rx) = mpsc::channel();
        let tx_err = tx.clone();
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            let _ = tx_err.send(Event::Exited(text));
        });
        thread::spawn(move || {
            let _ = fs::read_to_string(&entered);
            let _ = tx.send(Event::Entered);
        });
        match run.recv(&rx) {
            Event::Entered => Blocked { run, go },
            Event::Exited(text) => panic!("the run ended before its verify phase began:\n{text}"),
        }
    }

    fn release(&self) {
        fs::write(&self.go, "go\n").unwrap();
    }
}

#[test]
fn dropping_a_background_run_kills_the_phase_in_its_own_session() {
    let e = Env::new();
    let pid = e.ctrl.join("phase.pid");
    let held = Blocked::start(&e, &format!("echo $$ > {pid:?}"), "true");
    drop(held);
    let pid = fs::read_to_string(&pid).unwrap();
    assert!(exits(&pid), "phase {pid} outlived its run");
}

#[test]
fn a_run_past_its_timeout_fails_and_kills_the_phase_in_its_own_session() {
    let e = Env::new();
    let entered = e.ctrl.join("entered");
    mkfifo(&entered);
    e.queue("build", &["pass:script"]);
    e.ctl("build.sh", &format!("echo $$ > {entered:?}; sleep 600"));
    let mut run = e
        .ns()
        .args(["run", "--issue", "7"])
        .timeout(Duration::from_secs(3))
        .start();
    // The run's timeout starts at the wait, once the phase is in, however slow the start.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(fs::read_to_string(entered));
    });
    let pid = rx.recv_timeout(common::TIMEOUT).unwrap().unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.wait())).unwrap_err();
    let msg = panic.downcast_ref::<String>().unwrap();
    assert!(msg.contains("timed out after 3s"), "{msg}");
    assert!(exits(&pid), "phase {pid} outlived its run");
}

#[test]
fn a_runner_lock_held_by_a_killed_run_is_recovered() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    let lock = locks.join("bench-1.lock");
    a.bench(&locks);
    b.bench(&locks);
    let mut held = Blocked::start(&a, "true", "true");
    assert!(!lock_is_free(&lock));
    held.run.kill();
    held.run.wait();
    held.release();
    assert!(lock_is_free(&lock));

    b.queue("build", &["pass:commit"]);
    b.queue("verify", &["pass:script"]);
    b.ctl(
        "verify.sh",
        &format!("cat {:?} > {:?}", lock, b.ctrl.join("seen")),
    );
    let v = b.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    let holder: Value =
        serde_json::from_str(&fs::read_to_string(b.ctrl.join("seen")).unwrap()).unwrap();
    assert_ne!(holder["pid"].as_u64().unwrap(), u64::from(held.run.id()));
    assert!(lock_is_free(&lock));
}
