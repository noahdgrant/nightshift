#[test]
fn auto_merge_keeps_a_worktree_with_a_review_artifact_no_record_holds() {
    let e = auto_merge_env();
    e.queue("ship", &["pass:script"]);
    e.ctl(
        "ship.sh",
        "mkdir -p .ns/$NS_UNIT/history && echo notes > .ns/$NS_UNIT/history/review-1.md\n",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        v["cleanup"]["reason"], "its quality record is not saved",
        "{v}"
    );
    assert_eq!(
        quality_events(&e)[0]["unparsed"][0]["reason"],
        "no frontmatter"
    );
    assert!(e
        .worktree(UNIT)
        .join(format!(".ns/{UNIT}/history/review-1.md"))
        .is_file());
}

#[test]
fn clean_keeps_a_unit_with_a_review_artifact_no_record_holds() {
    let e = Env::new();
    let wt = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    fs::write(wt.join(".ns/3-done/review.md"), "notes, no frontmatter\n").unwrap();
    let v = e.run(&["clean"], 0);
    assert_eq!(
        reasons(&v)["3-done"],
        "its quality record is not saved: cannot read .ns/3-done/review.md: no frontmatter",
        "{v}"
    );
    assert!(wt.join(".ns/3-done/review.md").is_file());
}

#[test]
fn watch_tries_a_unit_that_stays_once_a_night() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    git(&e.root, &["worktree", "lock", done.to_str().unwrap()]);
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(4, "Fix b", &["type:fix"], "");
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["watch"], 0);
    assert_eq!(numbers(&v["units"]), [2, 4], "{v}");
    assert_eq!(v["cleaned"], serde_json::json!([]), "{v}");
    let tries: Vec<Value> = cleanup_events(&e)
        .into_iter()
        .filter(|v| v["unit"] == "3-done")
        .collect();
    assert_eq!(tries.len(), 1, "{tries:?}");
    assert_eq!(tries[0]["error"], true);
    let writes = run_events(&e)
        .into_iter()
        .filter(|v| v["event"] == "quality_record" && v["units"] == serde_json::json!(["3-done"]))
        .count();
    assert_eq!(writes, 1);
    assert!(done.is_dir());
}

// ---------------------------------------------------------------- parallel units

const A: &str = "2-fix-a";
const B: &str = "3-fix-b";

fn two_ready(e: &Env) {
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ready(3, "Fix b", &["type:fix"], "");
}

fn mkfifo(p: &Path) {
    assert!(StdCommand::new("mkfifo").arg(p).status().unwrap().success());
}

/// Hold `unit`'s next `phase` at its start: the fake harness writes its pid to the first fifo
/// returned, then waits for a line on the second.
fn hold_phase(e: &Env, unit: &str, phase: &str) -> (PathBuf, PathBuf) {
    let entered = e.ctrl.join(format!("entered-{unit}-{phase}"));
    let go = e.ctrl.join(format!("go-{unit}-{phase}"));
    mkfifo(&entered);
    mkfifo(&go);
    (entered, go)
}

/// Write a line to `go` from a thread, since a fifo write blocks until someone reads it.
fn release(go: &Path) {
    let go = go.to_path_buf();
    thread::spawn(move || fs::write(go, "go\n"));
}

/// A running `ns watch`, its stderr lines on a channel and its stdout collected.
struct Watching {
    g: Group,
    rx: mpsc::Receiver<Option<String>>,
    stdout: mpsc::Receiver<Vec<u8>>,
    seen: Vec<String>,
}

impl Watching {
    fn start(ns: &mut Ns) -> Watching {
        let mut g = ns.start_piped();
        let stdout = read_all(g.take_stdout());
        let (tx, rx) = mpsc::channel();
        send_lines(g.take_stderr(), tx);
        Watching {
            g,
            rx,
            stdout,
            seen: Vec::new(),
        }
    }

    /// Wait until the phase held on `entered` starts; the harness's pid.
    fn entered(&self, entered: &Path) -> String {
        let (tx, rx) = mpsc::channel();
        let f = entered.to_path_buf();
        thread::spawn(move || {
            let _ = tx.send(fs::read_to_string(f));
        });
        self.g.recv(&rx).unwrap().trim().to_string()
    }

    /// Read stderr until a line contains `want`; panics with what it read if watch ends first.
    fn line(&mut self, want: &str) -> String {
        if !line_until(&self.g, &self.rx, want, &mut self.seen) {
            panic!("no line with {want:?}:\n{}", self.seen.join("\n"));
        }
        self.seen.last().unwrap().clone()
    }

    fn signal(&self, sig: i32, group: bool) {
        let pid = self.g.id() as libc::pid_t;
        // SAFETY: kill(2) on the ns process, or its process group, that this test started.
        assert_eq!(
            unsafe { libc::kill(if group { -pid } else { pid }, sig) },
            0
        );
    }

    /// Wait for watch to end: its exit code, summary and whole stderr.
    fn finish(mut self) -> (Option<i32>, Value, String) {
        let code = self.g.wait().code();
        while let Some(l) = self.g.recv(&self.rx) {
            self.seen.push(l);
        }
        let err = self.seen.join("\n");
        let out = self.g.recv(&self.stdout);
        let v = serde_json::from_slice(&out).unwrap_or_else(|x| {
            panic!("{x}: stdout={} stderr={err}", String::from_utf8_lossy(&out))
        });
        (code, v, err)
    }
}

/// Each unit record's issue and outcome, sorted by issue.
fn by_issue(units: &[Value]) -> Vec<(u64, String)> {
    let mut o: Vec<(u64, String)> = units
        .iter()
        .map(|u| {
            (
                u["issue"].as_u64().unwrap(),
                u["outcome"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    o.sort();
    o
}

fn units(v: &Value) -> Vec<Value> {
    v["units"].as_array().unwrap().clone()
}

fn each(issues: &[u64], outcome: &str) -> Vec<(u64, String)> {
    issues.iter().map(|n| (*n, outcome.to_string())).collect()
}

/// The pid of each `ns run` that started a unit, by unit, from the run log's start events.
fn run_pids(e: &Env) -> Vec<(String, u64)> {
    let mut pids: Vec<(String, u64)> = run_events(e)
        .into_iter()
        .filter(|ev| ev["event"] == "start")
        .map(|ev| {
            (
                ev["unit"].as_str().unwrap().to_string(),
                ev["pid"].as_u64().unwrap(),
            )
        })
        .collect();
    pids.sort();
    pids
}

fn phase_count(e: &Env, phase: &str) -> usize {
    e.calls().iter().filter(|c| *c == phase).count()
}

#[test]
fn watch_parallel_runs_two_units_together_each_in_its_own_ns_run() {
    let e = Env::new();
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    let watch_pid = u64::from(w.g.id());
    // Both builds are in at once: one unit at a time never reaches the second.
    w.entered(&in_a);
    w.entered(&in_b);
    release(&go_a);
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert_eq!(v["cost_usd"], 5.0, "{v}");
    let pids = run_pids(&e);
    assert_eq!(pids.len(), 2, "{pids:?}");
    assert_ne!(pids[0].1, pids[1].1, "{pids:?}");
    assert!(pids.iter().all(|(_, p)| *p != watch_pid), "{pids:?}");
    // The output and the run log name the worker that ran each unit.
    let mut workers: Vec<u64> = units(&v)
        .iter()
        .map(|u| u["worker"].as_u64().unwrap())
        .collect();
    workers.sort();
    assert_eq!(workers, [1, 2], "{v}");
    for u in units(&v) {
        let title = if u["issue"] == 2 { "Fix a" } else { "Fix b" };
        let line = format!("ns watch: #{} {title} (worker {})", u["issue"], u["worker"]);
        assert!(err.contains(&line), "{line}\n{err}");
    }
    let starts: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "worker_start")
        .collect();
    assert_eq!(starts.len(), 2, "{starts:?}");
    for s in &starts {
        let unit = format!(
            "{}-fix-{}",
            s["issue"],
            if s["issue"] == 2 { "a" } else { "b" }
        );
        let run = pids.iter().find(|(u, _)| *u == unit).unwrap();
        assert_eq!(s["run_pid"], run.1, "{s}");
        assert!(s["worker"] == 1 || s["worker"] == 2, "{s}");
    }
}

#[test]
fn watch_parallel_units_take_a_runner_lock_one_at_a_time() {
    let e = Env::new();
    let locks = e.base.join("locks");
    e.bench(&locks);
    e.factory("[limits]\nparallel = 2\n[phases.verify]\nrunner = \"bench\"\n");
    two_ready(&e);
    let holds = [hold_phase(&e, A, "verify"), hold_phase(&e, B, "verify")];
    // A pinned clock would end a lock wait at once: watch runs on the real clock.
    let mut w = Watching::start(e.ns().env_remove("NS_NOW").arg("watch"));
    let line = w.line("verify waits for lock bench-1");
    let (waiter, holder) = if line.contains(&format!("ns run: {A} ")) {
        (0, 1)
    } else {
        (1, 0)
    };
    w.entered(&holds[holder].0);
    release(&holds[holder].1);
    // The waiter enters only once the holder's verify ends and lets the lock go.
    w.entered(&holds[waiter].0);
    release(&holds[waiter].1);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    let waits: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "lock_wait" && ev["lock"] == "bench-1")
        .collect();
    assert_eq!(waits.len(), 1, "{waits:?}");
}

#[test]
fn watch_parallel_units_merge_one_at_a_time() {
    let e = Env::new();
    e.factory(AUTO);
    two_ready(&e);
    e.ctl(&format!("pr-{A}"), "12");
    e.ctl(&format!("pr-{B}"), "13");
    e.queue(&format!("build.{A}"), &["pass:commit"]);
    e.queue(&format!("build.{B}"), &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("checks-13.json", GREEN);
    merges_in_order(&e, &[12, 13]);
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "merged"), "{v}");
    assert_merged_while_the_other_waited(&e, [12, 13]);
}

/// Unit 2 is ready and #5 sits in the backlog, untriaged: the night's first fill starts unit 2
/// and leaves #5, since the queue had a ready issue. A slot left free is filled every
/// `refill_ms`.
fn a_long_unit_beside_the_backlog(e: &Env, refill_ms: u64, args: &[&str]) -> Watching {
    e.ready(2, "Fix a", &["type:fix"], "");
    e.untriaged(5, "Fix e", &["status:needs-triage", "type:fix"]);
    Watching::start(
        e.ns()
            .env("NS_REFILL_MS", refill_ms.to_string())
            .arg("watch")
            .args(args),
    )
}

/// A fifo at `path` and a thread that sends a message on the returned channel for each line
/// written to it, so a hook can tell the test something happened without being held up.
fn fifo_lines(path: &Path) -> mpsc::Receiver<()> {
    mkfifo(path);
    let path = path.to_path_buf();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || loop {
        let Ok(text) = fs::read_to_string(&path) else {
            return;
        };
        for _ in text.lines() {
            if tx.send(()).is_err() {
                return;
            }
        }
    });
    rx
}

/// Mark each `gh` call in the run log, among the night's own events, as a `test_gh` event whose
/// `call` is `listing` (an issue listing), `view` (an issue or PR view) or `other`, and whose
/// `pid` is the caller's, then run `hook`. A message comes on the returned channel at each
/// listing.
fn mark_gh_calls(e: &Env, hook: &str) -> mpsc::Receiver<()> {
    let listed = fifo_lines(&e.ghd.join("listed"));
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$*\" in\n\
               *issues?state=open*) k=listing ;;\n\
               *\"issue view\"*|*\"pr view\"*) k=view ;;\n\
               *) k=other ;;\n\
             esac\n\
             echo \"{{\\\"event\\\":\\\"test_gh\\\",\\\"call\\\":\\\"$k\\\",\\\"pid\\\":$PPID}}\" >> \"$(git rev-parse --git-common-dir)/ns/runs.jsonl\"\n\
             if [ $k = listing ]; then echo x > \"$d/listed\"; fi\n\
             {hook}"
        ),
    );
    listed
}

/// Each refill of the night: the night's clock when it began, and the `gh` calls `ns watch`
/// made after it, marked by [`mark_gh_calls`], up to its next event.
fn refills(e: &Env) -> Vec<(i64, Vec<String>)> {
    let mut out: Vec<(i64, Vec<String>)> = Vec::new();
    let mut open = None;
    for ev in run_events(e) {
        if ev["event"] == "refill" {
            out.push((ev["at"].as_i64().unwrap(), Vec::new()));
            open = Some(ev["pid"].clone());
        } else if open.as_ref() == Some(&ev["pid"]) {
            if ev["event"] == "test_gh" {
                let calls = &mut out.last_mut().unwrap().1;
                calls.push(ev["call"].as_str().unwrap().to_string());
            } else {
                open = None;
            }
        }
    }
    out
}

#[test]
fn an_idle_slot_triages_the_backlog_and_starts_it_while_a_unit_runs() {
    let e = Env::new();
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "build");
    let w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    // Unit 5's build begins while unit 2's is still held.
    w.entered(&in_e);
    release(&go_a);
    release(&go_e);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 5], "done"), "{v}");
    assert_eq!(e.triage_only_runs(), 1);
    let started_5 = event_at(&e, |ev| ev["event"] == "worker_start" && ev["issue"] == 5);
    let ended_2 = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    assert!(started_5 < ended_2, "{:?}", run_events(&e));
}

/// Wait for the refill that triages #5, then for `n` more listings, so for `n / 2` refills more.
fn refills_after_the_triage(w: &mut Watching, listed: &mpsc::Receiver<()>, n: usize) {
    w.line("ns watch: triage #5");
    while listed.try_recv().is_ok() {}
    for _ in 0..n {
        w.g.recv(listed);
    }
}

#[test]
fn an_idle_slot_with_nothing_to_start_does_not_spin() {
    let e = Env::new();
    // #5's triage leaves it untriaged: nothing comes out ready.
    e.queue("triage.5-fix-e", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let listed = mark_gh_calls(&e, "");
    // A refill each 3 s of the night's clock, which the free slot's wait moves a second a poll.
    let mut w = a_long_unit_beside_the_backlog(&e, 3000, &["--parallel", "2"]);
    w.entered(&in_a);
    refills_after_the_triage(&mut w, &listed, 7);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
    assert_eq!(e.triage_only_runs(), 1);
    // The first refill triages #5; the ones after it have nothing to do.
    let idle = &refills(&e)[1..];
    assert!(idle.len() >= 3, "{idle:?}");
    for pair in idle.windows(2) {
        assert_eq!(pair[1].0 - pair[0].0, 3, "{idle:?}");
    }
    // One candidate listing and one queue read each.
    for (_, calls) in idle {
        let listings = calls.iter().filter(|c| *c == "listing").count();
        assert_eq!(listings, 2, "{idle:?}");
    }
}

#[test]
fn an_idle_refill_skips_the_worktree_cleanup() {
    let e = Env::new();
    e.queue("triage.5-fix-e", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let listed = mark_gh_calls(&e, "");
    let mut w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    refills_after_the_triage(&mut w, &listed, 5);
    release(&go_a);
    let (code, _, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    let idle = &refills(&e)[1..];
    assert!(idle.len() >= 2, "{idle:?}");
    for (_, calls) in idle {
        assert!(calls.iter().any(|c| c == "listing"), "{idle:?}");
        assert!(!calls.iter().any(|c| c == "view"), "{idle:?}");
    }
}

#[test]
fn a_failed_listing_on_a_refill_does_not_fail_the_night() {
    let e = Env::new();
    e.queue("triage.5-fix-e", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let mut w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    w.line("ns watch: triage #5");
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *issues?state=open*)\n\
           if [ -f \"$d/boom\" ]; then echo 'HTTP 502' >&2; touch \"$d/failed\"; exit 1; fi ;;\n\
         esac\n",
    );
    e.gh_file("boom", "");
    w.line("refill failed");
    assert!(e.ghd.join("failed").exists());
    fs::remove_file(e.ghd.join("boom")).unwrap();
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
}

#[test]
fn a_claim_that_fails_on_a_refill_does_not_spend_a_max_units_claim() {
    let e = Env::new();
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    let failed = fifo_lines(&e.ghd.join("failed"));
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *\"issue edit 5 \"*\"--add-label status:in-progress\"*)\n\
           if [ -f \"$d/boom\" ]; then echo 'HTTP 502' >&2; echo x > \"$d/failed\"; exit 1; fi ;;\n\
         esac\n",
    );
    e.gh_file("boom", "");
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "build");
    let w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2", "--max-units", "2"]);
    w.entered(&in_a);
    for _ in 0..3 {
        w.g.recv(&failed);
    }
    fs::remove_file(e.ghd.join("boom")).unwrap();
    w.entered(&in_e);
    release(&go_a);
    release(&go_e);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 5], "done"), "{v}");
}

#[test]
fn a_claim_whose_verifying_read_fails_on_a_refill_is_put_back_to_ready() {
    let e = Env::new();
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    let put_back = fifo_lines(&e.ghd.join("put-back"));
    e.gh_file(
        "hook.sh",
        "case \"$*\" in\n\
         \"issue edit 5 --remove-label status:in-progress --add-label status:ready-for-agent\"*) echo x > \"$d/put-back\" ;;\n\
         *\"issue edit 5 \"*\"--add-label status:in-progress\"*) touch \"$d/edited\" ;;\n\
         *\"issue view 5 --json labels\"*)\n\
           if [ -f \"$d/edited\" ] && [ ! -f \"$d/failed\" ]; then echo 'HTTP 502' >&2; touch \"$d/failed\"; exit 1; fi ;;\n\
         esac\n",
    );
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "build");
    let w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    w.g.recv(&put_back);
    assert!(e.ghd.join("failed").exists());
    w.entered(&in_e);
    release(&go_a);
    release(&go_e);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 5], "done"), "{v}");
}

#[test]
fn a_claim_whose_unit_cannot_start_on_a_refill_does_not_spend_a_max_units_claim() {
    let e = Env::new();
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-fix-e", &["pass:script"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_t, go_t) = hold_phase(&e, "5-fix-e", "triage");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "build");
    let mut w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2", "--max-units", "2"]);
    w.entered(&in_a);
    w.entered(&in_t);
    let night = fs::read_dir(e.root.join(".git/ns"))
        .unwrap()
        .map(|d| d.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("watch-")
        })
        .unwrap();
    // A directory where unit 5's result file goes: spawning its `ns run` fails.
    let blocks: Vec<_> = (2..60)
        .map(|n| night.join(format!("result-5-{n}.json")))
        .collect();
    for b in &blocks {
        fs::create_dir(b).unwrap();
    }
    release(&go_t);
    w.line("refill failed");
    w.line("refill failed");
    for b in &blocks {
        let _ = fs::remove_dir(b);
    }
    w.entered(&in_e);
    release(&go_a);
    release(&go_e);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 5], "done"), "{v}");
}

#[test]
fn a_hold_that_cannot_be_written_on_a_refill_fails_the_night() {
    let e = Env::new();
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("triage.5-fix-e", &["limit"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_e, go_e) = hold_phase(&e, "5-fix-e", "triage");
    let w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "2"]);
    w.entered(&in_a);
    w.entered(&in_e);
    let night = fs::read_dir(e.root.join(".git/ns"))
        .unwrap()
        .map(|d| d.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("watch-")
        })
        .unwrap();
    // A directory where the hold's temp file goes: the hold write on the refill fails.
    fs::create_dir(night.join("hold.tmp")).unwrap();
    release(&go_e);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_ne!(code, Some(0), "{v}\n{err}");
    assert!(err.contains("cannot write"), "{err}");
    assert!(!err.contains("refill failed"), "{err}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[2], "done"), "{v}");
}

#[test]
fn no_refill_while_every_slot_is_busy_or_no_claim_may_start() {
    for (args, done) in [
        (&["--parallel", "1"][..], &[2, 3][..]),
        (&["--parallel", "2", "--max-units", "1"], &[2]),
    ] {
        let e = Env::new();
        // A second ready issue: the night's first fill meets `--max-units`, before unit 2 builds.
        e.ready(3, "Fix b", &["type:fix"], "");
        e.queue("triage.5-fix-e", &["none"]);
        let (in_a, go_a) = hold_phase(&e, A, "build");
        // Refills would come every poll, while unit 2 is held and while its later phases run.
        let w = a_long_unit_beside_the_backlog(&e, 100, args);
        w.entered(&in_a);
        release(&go_a);
        let (code, v, err) = w.finish();
        assert_eq!(code, Some(0), "{err}");
        assert_eq!(by_issue(&units(&v)), each(done, "done"), "{v}");
        assert_eq!(refills(&e), [], "{args:?}");
    }
}

#[test]
fn no_refill_while_a_usage_limit_pause_holds_a_free_slot() {
    let e = Env::new();
    e.ready(3, "Fix b", &["type:fix"], "");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue(&format!("build.{A}"), &["limit", "pass:commit"]);
    e.queue("triage.5-fix-e", &["none"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = a_long_unit_beside_the_backlog(&e, 100, &["--parallel", "3", "--until", "06:30"]);
    w.entered(&in_b);
    w.entered(&in_a);
    release(&go_a);
    w.line("ns watch: #2 paused on a usage limit until");
    // Unit 3's build stays held, then its later phases run: the pause lasts until unit 3 ends,
    // with a slot free and #5 untriaged. Refills would come every poll.
    release(&go_b);
    let (code, _, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    let paused = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 2);
    let lifted = event_at(&e, |ev| ev["event"] == "worker_end" && ev["issue"] == 3);
    let during = run_events(&e)[paused..lifted]
        .iter()
        .filter(|ev| ev["event"] == "refill")
        .count();
    assert_eq!(during, 0, "{:?}", run_events(&e));
}

#[test]
fn watch_parallel_claims_an_issue_before_its_unit_starts() {
    let e = Env::new();
    two_ready(&e);
    let seen = e.ctrl.join("seen");
    e.ctl(
        "triage.sh",
        &format!("cat \"$FAKE_GH_DIR/labels-${{NS_UNIT%%-*}}\" >> {seen:?}\n"),
    );
    e.queue(&format!("triage.{A}"), &["pass:script"]);
    e.queue(&format!("triage.{B}"), &["pass:script"]);
    // Each claim leaves a mark in the run log, which watch also writes each unit's start to.
    e.gh_file(
        "hook.sh",
        "case \"$*\" in *\"--add-label status:in-progress\"*)\n\
           echo \"{\\\"event\\\":\\\"test_claim\\\",\\\"issue\\\":$3}\" >> \"$(git rev-parse --git-common-dir)/ns/runs.jsonl\" ;;\n\
         esac\n",
    );
    let v = e.run(&["watch", "--parallel", "2"], 0);
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
    for n in [2, 3] {
        let claimed = event_at(&e, |v| v["event"] == "test_claim" && v["issue"] == n);
        let started = event_at(&e, |v| v["event"] == "worker_start" && v["issue"] == n);
        assert!(claimed < started, "#{n}: {:?}", run_events(&e));
    }
    let at_start = fs::read_to_string(&seen).unwrap();
    assert_eq!(
        at_start.matches("status:in-progress").count(),
        2,
        "{at_start}"
    );
    assert!(!at_start.contains("status:ready-for-agent"), "{at_start}");
}

#[test]
fn watch_parallel_pauses_every_unit_on_a_usage_limit_and_resumes_them_together() {
    let e = Env::new();
    two_ready(&e);
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue(&format!("build.{A}"), &["limit", "pass:commit"]);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(
        e.ns()
            .args(["watch", "--parallel", "2", "--until", "06:30"]),
    );
    // Unit 2's build hits its limit only once unit 3's build has begun.
    w.entered(&in_b);
    w.entered(&in_a);
    release(&go_a);
    w.line("ns watch: #2 paused on a usage limit until 2026-10-09T01:00:00+00:00");
    // Unit 3's build ends after the pause began; it pauses before its next phase.
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    let us = units(&v);
    assert_eq!(us.len(), 4, "{v}");
    assert_eq!(by_issue(&us[..1]), each(&[2], "paused"), "{v}");
    assert_eq!(by_issue(&us[1..2]), each(&[3], "paused"), "{v}");
    for u in &us[..2] {
        assert_eq!(u["reset_at"], "2026-10-09T01:00:00+00:00", "{v}");
    }
    assert!(
        us[1]["reason"].as_str().unwrap().contains("another unit"),
        "{v}"
    );
    assert_eq!(by_issue(&us[2..]), each(&[2, 3], "done"), "{v}");
    assert_eq!(v["stopped"], "queue empty", "{v}");
    assert_eq!(
        err.matches("usage limit, sleeping until").count(),
        1,
        "{err}"
    );
    assert!(err.contains("usage limit, sleeping until 2026-10-09T01:00:00+00:00"));
    // Unit 3 resumed at verify; unit 2 ran its build again.
    assert_eq!(phase_count(&e, "build"), 3, "{:?}", e.calls());
    assert_eq!(phase_count(&e, "verify"), 2, "{:?}", e.calls());
    let calls = e.gh_calls();
    assert_eq!(
        calls.matches("--add-label status:in-progress").count(),
        2,
        "{calls}"
    );
}

#[test]
fn watch_parallel_budget_stop_reaches_every_unit() {
    let e = Env::new();
    e.factory("[limits]\nbudget_usd = 2.0\nparallel = 2\n");
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(e.ns().args(["watch"]));
    // Both triages ran: $1.00 of $2.00 spent.
    w.entered(&in_a);
    w.entered(&in_b);
    // Unit 2's build and verify bring the night to $2.00; it stops before review.
    release(&go_a);
    w.line("ns watch: #2 budget");
    // Unit 3's build brings it to $2.50, and unit 3 stops before verify: it counts unit 2's spend.
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3], "budget"), "{v}");
    assert_eq!(v["stopped"], "budget", "{v}");
    assert_eq!(v["cost_usd"], 2.5, "{v}");
    assert_eq!(phase_count(&e, "verify"), 1, "{:?}", e.calls());
    assert_eq!(phase_count(&e, "review"), 0, "{:?}", e.calls());
    for n in [2, 3] {
        assert_eq!(e.labels(n), ["type:fix", "status:ready-for-agent"]);
    }
}

#[test]
fn watch_parallel_runs_the_triage_pass_when_a_worker_finishes() {
    let e = Env::new();
    two_ready(&e);
    e.untriaged(5, "Escape", &["status:needs-triage"]);
    e.triage_sets(&["status:ready-for-agent"]);
    e.queue("triage.5-escape", &["pass:script"]);
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let (in_t, go_t) = hold_phase(&e, "5-escape", "triage");
    let w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.entered(&in_b);
    // Unit 2 ended; the pass triages #5 before its slot refills, while unit 3 still builds.
    w.entered(&in_t);
    release(&go_t);
    release(&go_b);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(numbers(&v["triaged"]), [5], "{v}");
    assert_eq!(by_issue(&units(&v)), each(&[2, 3, 5], "done"), "{v}");
    // Units' harnesses ran at once and may share a call number, so count the prompts, not the
    // calls; the triage-only run ran with no other harness beside it.
    let triage_only = fs::read_dir(&e.ctrl)
        .unwrap()
        .filter_map(|f| fs::read_to_string(f.unwrap().path()).ok())
        .filter(|t| t.contains("Triage only") && t.contains("unit `5-escape`"))
        .count();
    assert_eq!(triage_only, 1, "{err}");
}

#[test]
fn watch_runs_one_unit_at_a_time_unless_told_otherwise() {
    for (factory, args) in [
        ("", vec!["watch"]),
        ("[limits]\nparallel = 3\n", vec!["watch", "--parallel", "1"]),
    ] {
        let e = Env::new();
        e.factory(factory);
        two_ready(&e);
        let v = e.run(&args, 0);
        assert_eq!(by_issue(&units(&v)), each(&[2, 3], "done"), "{v}");
        let calls = e.gh_calls();
        let last_of_2 = calls.rfind("issue edit 2 ").unwrap();
        let claim_3 = calls
            .find(
                "issue edit 3 --remove-label status:ready-for-agent --add-label status:in-progress",
            )
            .unwrap();
        assert!(last_of_2 < claim_3, "{args:?}: {calls}");
        assert!(units(&v).iter().all(|u| u["worker"] == 1), "{v}");
    }
}

#[test]
fn watch_parallel_must_be_at_least_one_and_the_flag_wins() {
    let e = Env::new();
    e.ns()
        .args(["watch", "--parallel", "0"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--parallel"));
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["parallel"], 1, "{v}");
    e.factory("[limits]\nparallel = 3\n");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["parallel"], 3, "{v}");
    let v = e.run(&["watch", "--dry-run", "--parallel", "2"], 0);
    assert_eq!(v["parallel"], 2, "{v}");
    e.factory("[limits]\nparallel = 0\n");
    e.ns()
        .args(["watch", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("parallel"));
}

/// A build that records its CPU variables to `seen` and commits, and a build gate that records
/// them too. Returns `seen`, one `<who>:<NS_CPUS>:<RUST_TEST_THREADS>:<CARGO_BUILD_JOBS>` line
/// per build and gate.
fn cpus_seen(e: &Env) -> PathBuf {
    let seen = e.ctrl.join("cpus-seen");
    let dump = |who: &str| {
        format!(
            "echo \"{who}:${{NS_CPUS-unset}}:${{RUST_TEST_THREADS-unset}}:${{CARGO_BUILD_JOBS-unset}}\" >> {}\n",
            seen.display()
        )
    };
    build_only(e, "");
    e.ctl("ci.sh", &dump("gate"));
    e.ctl(
        "build.sh",
        &format!(
            "{}date >> built.txt && git add built.txt && git -c user.name=f -c user.email=f@f commit -qm built",
            dump("build")
        ),
    );
    e.queue("build", &["pass:script"]);
    seen
}
