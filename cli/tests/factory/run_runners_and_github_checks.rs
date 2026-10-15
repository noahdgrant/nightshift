#[test]
fn a_runner_with_two_locks_holds_both_for_the_phase() {
    let e = Env::new();
    let locks = e.base.join("locks");
    e.bench(&locks);
    e.runner(
        "bench",
        "kind = \"bench\"\nlocks = [\"b-lock\", \"a-lock\"]\n",
    );
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["pass:script"]);
    let seen = e.ctrl.join("seen");
    e.ctl(
        "verify.sh",
        &format!(
            "for n in a-lock b-lock; do flock -n {l:?}/$n.lock true && echo free >> {s:?} || echo held >> {s:?}; done",
            l = locks,
            s = seen,
        ),
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(fs::read_to_string(&seen).unwrap(), "held\nheld\n");
    assert!(lock_is_free(&locks.join("a-lock.lock")));
    assert!(lock_is_free(&locks.join("b-lock.lock")));
}

#[test]
fn two_runs_needing_one_lock_serialise() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    let order = a.base.join("order");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(
        &a,
        &format!("echo A-in >> {order:?}"),
        &format!("echo A-out >> {order:?}"),
    );
    b.queue("verify", &["pass:script"]);
    b.ctl("verify.sh", &format!("echo B >> {order:?}"));

    // A pinned clock would end B's wait at once: B waits on the real clock.
    let mut run_b = b
        .ns()
        .env_remove("NS_NOW")
        .args(["run", "--issue", "7"])
        .start();
    let b_err = run_b.take_stderr();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut lines = BufReader::new(b_err).lines().map_while(Result::ok);
        let _ = tx.send(lines.any(|l| l.contains("verify waits for lock bench-1")));
        lines.for_each(drop);
    });
    let waited = run_b.recv(&rx);
    let calls_while_waiting = b.calls();
    held.release();
    assert!(run_b.wait().success());
    let mut a_run = held.run;
    assert!(a_run.wait().success());

    assert!(waited, "B never waited for bench-1");
    assert_eq!(calls_while_waiting, ["triage", "build"]);
    assert_eq!(
        fs::read_to_string(&order)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["A-in", "A-out", "B"]
    );
    let log = fs::read_to_string(b.root.join(".git/ns/runs.jsonl")).unwrap();
    assert!(log.contains("\"event\":\"lock_wait\""), "{log}");
}

#[test]
fn a_runner_lock_wait_ends_each_attempt_at_the_phase_timeout() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    b.queue("build", &["pass:commit"]);
    b.queue("verify", &["pass:script"]);
    let out = b
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "verify=60000")
        .args(["run", "--issue", "7"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    held.release();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.starts_with("verify is out of attempts"), "{reason}");
    assert!(
        reason.contains("gave up waiting for lock bench-1 (held by pid ")
            && reason.ends_with("at the phase timeout"),
        "{reason}"
    );
    assert_eq!(b.calls(), ["triage", "build"]);
    let log = fs::read_to_string(b.root.join(".git/ns/runs.jsonl")).unwrap();
    let gave_up: Vec<Value> = log
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|ev| ev["event"] == "lock_wait_timeout")
        .collect();
    assert!(!gave_up.is_empty(), "{log}");
    assert_eq!(gave_up[0]["lock"], "bench-1");
    assert_eq!(gave_up[0]["bound"], "timeout");
    assert_eq!(gave_up[0]["held_by"]["unit"], UNIT);
    let attempts: Vec<_> = gave_up.iter().map(|ev| ev["attempt"].clone()).collect();
    assert_eq!(attempts, [1, 2], "{log}");
}

#[test]
fn a_runner_lock_timeout_retries_the_phase_it_was_given() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    let v = b.run(&["run", "--issue", "7", "--from", "verify"], 1);
    held.release();
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.starts_with("verify is out of attempts"), "{reason}");
    assert_eq!(b.calls(), Vec::<String>::new());
    let phases = v["phases"].as_array().unwrap();
    assert_eq!(phases.len(), 2, "{v}");
    for (i, p) in phases.iter().enumerate() {
        assert_eq!(p["phase"], "verify", "{v}");
        assert_eq!(p["attempt"], i + 1, "{v}");
        assert_eq!(p["written"], false, "{v}");
    }
}

#[test]
fn a_runner_lock_wait_past_until_ends_the_night() {
    let a = Env::new();
    let b = Env::new();
    let locks = a.base.join("locks");
    a.bench(&locks);
    b.bench(&locks);
    let held = Blocked::start(&a, "true", "true");
    b.ready(2, "Fix a", &["type:fix"], "");
    b.queue("build", &["pass:commit"]);
    b.queue("verify", &["pass:script"]);
    let out = b
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "verify=86400000")
        .args(["watch", "--until", "00:30"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    held.release();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["stopped"], "until", "{v}");
    assert_eq!(v["units"][0]["outcome"], "budget", "{v}");
    let reason = v["units"][0]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("gave up waiting for lock bench-1") && reason.ends_with("at --until"),
        "{reason}"
    );
    assert_eq!(b.calls(), ["triage", "build"]);
    assert!(b.gh_calls().contains(
        "issue edit 2 --remove-label status:in-progress --add-label status:ready-for-agent"
    ));
    let log = fs::read_to_string(b.root.join(".git/ns/runs.jsonl")).unwrap();
    assert!(
        log.contains("\"event\":\"lock_wait_timeout\"") && log.contains("\"bound\":\"until\""),
        "{log}"
    );
}

#[test]
fn runner_problems_fail_validate_and_run() {
    let e = Env::new();
    e.factory("[phases.verify]\nrunner = \"bench\"\n");
    let validate = |e: &Env| -> String {
        let out = e
            .ns()
            .args(["factory", "validate"])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    };
    let out = validate(&e);
    assert!(
        out.contains("runner \\\"bench\\\" has no runners/bench.toml"),
        "{out}"
    );
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("runners/bench.toml"));

    e.runner("bench", "kind = \"remote\"\n");
    let out = validate(&e);
    assert!(
        out.contains("runners/bench.toml: unknown variant `remote`"),
        "{out}"
    );
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unknown variant `remote`"));
    assert!(e.calls().is_empty());

    e.runner("bench", "locks = [\"bench-1\"]\n");
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["phases"][2]["runner"], "bench");
    assert_eq!(v["phases"][2]["locks"][0], "bench-1");
}

#[test]
fn default_branch_guard_stops_a_push_to_main() {
    let e = Env::new();
    e.queue("build", &["pass:push"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("default branch moved"),
        "{v}"
    );
    assert_eq!(e.calls(), ["triage", "build"]);
    let main = git(&e.base.join("remote.git"), &["rev-parse", "main"]);
    assert!(
        e.gh_calls().contains(&format!(
            "api repos/{{owner}}/{{repo}}/commits/{main}/pulls"
        )),
        "{}",
        e.gh_calls()
    );
}

/// The build phase commits to the unit, runs `pre` in the worktree, then another actor lands
/// `land` on origin's main from a clone, GitHub links the new main commit to `pulls`, and the
/// build rebases onto it.
fn another_actor_lands(e: &Env, pre: &str, land: &str, pulls: &str) {
    another_actor_lands_linked(e, pre, land, "pulls", pulls);
}

/// [`another_actor_lands`], the forge's list of the new main commit's PRs written to
/// `<file>-<sha>.json` in the fake forge's directory.
fn another_actor_lands_linked(e: &Env, pre: &str, land: &str, file: &str, pulls: &str) {
    e.ctl(
        "build.sh",
        &format!(
            r#"set -e
c="git -c user.name=f -c user.email=f@f"
echo own >> work.txt && git add work.txt && $c commit -qm own
{pre}
o="$FAKE_CTRL/other"
rm -rf "$o" && git clone -q "$(git remote get-url origin)" "$o"
(cd "$o" && {land} && git push -q origin main &&
  printf '%s' '{pulls}' > "$FAKE_GH_DIR/{file}-$(git rev-parse HEAD).json")
git fetch -q origin && $c rebase -q origin/main"#
        ),
    );
    e.queue("build", &["pass:script"]);
}

const SQUASH_B: &str = r#"echo b > b.txt && git add b.txt && $c commit -qm "Fix b (#3)""#;
const MERGED_B: &str =
    r#"[{"number":3,"merged_at":"2026-10-09T00:00:00Z","head":{"ref":"ns/3-fix-b"}}]"#;

fn run_log(e: &Env) -> String {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl")).unwrap()
}

#[test]
fn another_units_merge_then_a_rebase_is_not_this_units_push() {
    let e = Env::new();
    another_actor_lands(&e, "", SQUASH_B, MERGED_B);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let main = git(&e.base.join("remote.git"), &["rev-parse", "main"]);
    let wt = e.worktree(UNIT);
    assert!(git(&wt, &["merge-base", "--is-ancestor", &main, "HEAD"]).is_empty());
    let log = run_log(&e);
    assert!(
        log.contains(&format!("\"to\":\"{main}\""))
            && log.contains("\"event\":\"default_moved_by_other_actor\"")
            && log.contains("\"prs\":[3]")
            && !log.contains("\"breach\""),
        "{log}"
    );
}

#[test]
fn a_human_merge_commit_then_a_rebase_is_not_this_units_push() {
    let e = Env::new();
    another_actor_lands(
        &e,
        "",
        r#"git checkout -qb human-fix && echo h > h.txt && git add h.txt && $c commit -qm h &&
  git checkout -q main && $c merge -q --no-ff human-fix -m "Merge pull request #50""#,
        r#"[{"number":50,"merged_at":"2026-10-09T00:00:00Z","head":{"ref":"human-fix"}}]"#,
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let log = run_log(&e);
    assert!(
        log.contains("\"prs\":[50]") && !log.contains("\"breach\""),
        "{log}"
    );
}

#[test]
fn dry_run_calls_no_harness() {
    let e = Env::new();
    let v = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(v["decision"]["phase"], "triage");
    assert!(v["prompt"].as_str().unwrap().contains("`ns-triage`"));
    assert_eq!(v["worktree_exists"], false);
    assert!(e.calls().is_empty());
    assert!(!e.worktree(UNIT).exists());
    let v = e.run(&["run", "nobrief", "--dry-run"], 0);
    assert_eq!(v["decision"]["action"], "stuck");
}

#[test]
fn missing_subscription_login_refuses_to_start() {
    let e = Env::new();
    fs::remove_file(e.home.join(".claude/.credentials.json")).unwrap();
    e.ns()
        .args(["run", "--issue", "7"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("subscription"));
    assert!(e.calls().is_empty());
    assert!(!e.worktree(UNIT).exists(), "refused before the worktree");
    // A dry run starts no harness, so it needs no login.
    e.run(&["run", "--issue", "7", "--dry-run"], 0);
    // billing = "api" disables the guard.
    e.factory("[defaults]\nbilling = \"api\"\n");
    e.queue("build", &["pass:commit"]);
    e.run(&["run", "--issue", "7"], 0);
    assert!(e.ctrl.join("leak").exists(), "api billing keeps the key");
}

/// The planned phase command is the one that runs: a subscription claude drops `--bare`.
#[test]
fn a_subscription_phase_command_drops_bare() {
    let e = Env::new();
    fs::write(
        e.base.join("no-config.toml"),
        "[harness.claude]\ncommand = [\"claude\"]\ncommand_write = [\"claude\", \"--bare\", \"-p\"]\n",
    )
    .unwrap();
    let bare = |e: &Env| {
        let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
        d["command"].as_array().unwrap().iter().any(|a| a == "--bare")
    };
    assert!(!bare(&e));
    e.factory("[defaults]\nbilling = \"api\"\n");
    assert!(bare(&e));
}

#[test]
fn usage_limit_pauses_without_using_an_attempt() {
    let e = Env::new();
    e.factory("[defaults]\nmax_attempts = 1\n");
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("build", &["limit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 4);
    assert_eq!(v["outcome"], "paused");
    assert_eq!(v["reset_at"], NOW + 3600);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(v["phases"][0]["phase"], "build");
    assert_eq!(v["phases"][0]["attempt"], 1);
}

#[test]
fn budget_stops_new_phases() {
    let e = Env::new();
    e.factory("[limits]\nbudget_usd = 1.0\n");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 3);
    assert_eq!(v["outcome"], "budget");
    assert_eq!(e.calls(), ["triage", "build"]);
}

const AUTO: &str = "[merge]\npolicy = \"auto\"
human_review = [\".github/**\", \".nightshift/**\"]\nci_timeout_minutes = 1\n";
/// `[merge]` with the local merge queue off: update-branch, then the gate on the PR head.
const LEGACY: &str = "local_queue = false\n";
const GREEN: &str = r#"[{"name":"ci","state":"SUCCESS","bucket":"pass","link":"https://github.com/o/r/actions/runs/1/job/2"}]"#;
const RED: &str = r#"[{"name":"ci","state":"FAILURE","bucket":"fail","link":"https://github.com/o/r/actions/runs/99/job/2"}]"#;

#[test]
fn auto_merge_squashes_when_green_and_no_human_review_files() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "src/x.rs\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(
        calls.contains("pr merge 12 --squash --match-head-commit"),
        "{calls}"
    );
    assert!(!calls.contains("update-branch"));
    let r = &quality_records(&e)[0];
    assert_eq!(
        (r["outcome"].as_str(), r["pr"].as_u64()),
        (Some("merged"), Some(12))
    );
}

#[test]
fn build_only_with_no_fast_gate_updates_a_branch_behind_main_and_gates_only_after_build() {
    let e = Env::new();
    e.factory(&format!("{AUTO}[gate]\nlocal = \"build-only\"\n"));
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command |\n|---|---|\n| rules | `true` | 1 s |\n",
    )
    .unwrap();
    git(&e.root, &["add", "."]);
    git(&e.root, &["commit", "-q", "-m", "stack"]);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert!(e.gh_calls().contains("pr update-branch 12"));
    assert_eq!(gate_where(&e), [("build".into(), "local_full".into())]);
}

/// `[gate] local = "build-only"` with auto merge, its fast and full gates logged to the returned
/// file, a build that commits, and PR #12 green on GitHub.
fn remote_queue(e: &Env, merge: &str) -> PathBuf {
    let (runs, _) = build_only(e, merge);
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:script", "pass:script"]);
    runs
}

const UPDATED: &str = "00000000000000000000000000000000000000aa";

#[test]
fn build_only_merges_on_github_updating_the_pr_head_to_main_and_waiting_for_its_checks() {
    let e = Env::new();
    let runs = remote_queue(&e, AUTO);
    // GitHub says CLEAN, as with no "up to date" rule, but the PR head lacks main's tip, which
    // changes a file the PR changes.
    main_moves(&e, "built.txt", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    let upd = calls.find("pr update-branch 12").expect("update-branch");
    let watch = calls.find("pr checks 12 --watch").unwrap();
    let merge = calls
        .find(&format!(
            "pr merge 12 --squash --match-head-commit {UPDATED}"
        ))
        .unwrap_or_else(|| panic!("{calls}"));
    assert!(upd < watch && watch < merge, "{calls}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\n");
    assert_eq!(gate_where(&e), [("build".into(), "local_full".into())]);
    let passed: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "ci_passed")
        .collect();
    assert_eq!(passed.len(), 1, "{passed:?}");
    assert_eq!(
        (&passed[0]["where"], &passed[0]["sha"]),
        (&json!("github"), &json!(UPDATED))
    );
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn build_only_runs_no_local_gate_after_a_ship_that_moves_head() {
    let e = Env::new();
    let runs = remote_queue(&e, AUTO);
    e.queue("ship", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\n");
    assert_eq!(gate_where(&e), [("build".into(), "local_full".into())]);
    assert!(e.gh_calls().contains("pr checks 12 --watch"));
}

#[test]
fn build_only_runs_no_local_gate_after_a_cut_short_ship_that_moves_head() {
    let e = Env::new();
    let runs = remote_queue(&e, "");
    e.queue("ship", &["pass:commit+sleep", "pass"]);
    let v = run_with_timeouts(&e, "ship=1500", 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\n");
    assert_eq!(gate_where(&e), [("build".into(), "local_full".into())]);
}

#[test]
fn every_step_still_gates_after_a_ship_that_moves_head() {
    let e = Env::new();
    let runs = e.ctrl.join("ci-runs");
    e.factory(&format!(
        "{AUTO}[phases.build]\ngate = \"echo ran >> {}\"\n",
        runs.display()
    ));
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let phases: Vec<Value> = gate_events(&e).iter().map(|g| g["phase"].clone()).collect();
    assert!(phases.contains(&json!("ship")), "{phases:?}");
}

#[test]
fn build_only_sends_red_github_checks_back_to_build_naming_the_check() {
    let e = Env::new();
    let runs = remote_queue(&e, AUTO);
    e.gh_file("checks-12-1.json", RED);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.calls();
    let rebuild = calls.iter().rposition(|c| c == "build").unwrap();
    assert!(
        rebuild > calls.iter().position(|c| c == "ship").unwrap(),
        "{calls:?}"
    );
    let p = e.prompt(rebuild + 1, "build");
    assert!(p.contains("CI failed on PR #12: ci (fail)"), "{p}");
    let failed: Vec<Value> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "ci_failed")
        .collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["where"], "github");
    assert_eq!(failed[0]["checks"], json!(["ci (fail)"]));
    // The rebuild's gate is the full one; the merge step runs no local gate.
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfull\n");
}

/// A ship script that commits and clears PR #12's conflict, except on its run `move_on`, where
/// another actor moves main and the PR conflicts again.
fn ship_brings_in_main(e: &Env, move_on: u32) {
    e.ctl(
        "ship.sh",
        &format!(
            "n=$(( $(cat \"$FAKE_CTRL/ships\" 2>/dev/null || echo 0) + 1 )); echo $n > \"$FAKE_CTRL/ships\"\n\
             if [ $n = {move_on} ]; then {} && echo DIRTY > \"$FAKE_GH_DIR/pr-12.merge\"\n\
             else echo $n >> main-in.txt && git add main-in.txt && git -c user.name=f -c user.email=f@f commit -qm \"merge main $n\" && echo CLEAN > \"$FAKE_GH_DIR/pr-12.merge\"; fi\n",
            move_main_sh(e)
        ),
    );
}

#[test]
fn build_only_a_credited_ship_that_gives_up_on_a_lock_is_credited_again_on_its_retry() {
    let e = Env::new();
    let locks = e.base.join("locks");
    fs::create_dir_all(&locks).unwrap();
    remote_queue(&e, &format!("{AUTO}[phases.ship]\nrunner = \"bench\"\n"));
    e.runner("bench", "kind = \"bench\"\nlocks = [\"bench-1\"]\n");
    e.config(&format!(
        "[runners]\nlock_dir = {:?}\n",
        locks.to_str().unwrap()
    ));
    e.gh_file("pr-12.merge", "DIRTY");
    ship_brings_in_main(&e, 0);
    // The review that precedes the credited ship tells the test it is running and waits, so the
    // test takes the ship's lock before the ship asks for it.
    let ready = e.ctrl.join("ready");
    let go = e.ctrl.join("go");
    for f in [&ready, &go] {
        assert!(StdCommand::new("mkfifo").arg(f).status().unwrap().success());
    }
    e.ctl(
        "review.sh",
        &format!("echo in > {ready:?}; cat {go:?} > /dev/null\n"),
    );
    e.queue("ship", &["pass", "pass:script"]);
    e.queue("review", &["pass", "pass:script"]);
    let mut run = e.ns().args(["run", "--issue", "7"]).start();
    let mut stderr = run.take_stderr();
    let (tx, rx) = mpsc::channel();
    let tx_err = tx.clone();
    thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        let _ = tx_err.send(Some(text));
    });
    thread::spawn(move || {
        let _ = fs::read_to_string(&ready);
        let _ = tx.send(None);
    });
    assert!(run.recv(&rx).is_none(), "the run ended before its review");
    let lock = fs::File::create(locks.join("bench-1.lock")).unwrap();
    lock.lock().unwrap();
    fs::write(&go, "go\n").unwrap();
    let stderr = run.recv(&rx).expect("stderr");
    drop(lock);
    assert_eq!(run.wait().code(), Some(1), "{stderr}");
    let log = run_events(&e);
    let attempts = |event: &str| -> Vec<Value> {
        log.iter()
            .filter(|ev| ev["event"] == event)
            .map(|ev| ev["attempt"].clone())
            .collect()
    };
    assert_eq!(attempts("lock_wait_timeout"), [3, 4], "{stderr}");
    assert_eq!(attempts("ship_after_base_moved"), [3, 4, 5], "{stderr}");
}
