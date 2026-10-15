#[test]
fn a_free_rebase_that_gives_up_waiting_for_a_lock_still_rebases_on_the_retry() {
    let e = Env::new();
    let lock = e.base.join("locks/bench-1.lock");
    let log = e.root.join(".git/ns/runs.jsonl");
    e.factory("[phases.build]\nrunner = \"bench\"\n");
    e.runner("bench", "kind = \"bench\"\nlocks = [\"bench-1\"]\n");
    e.config(&format!(
        "[runners]\nlock_dir = {:?}\n",
        lock.parent().unwrap().to_str().unwrap()
    ));
    // After the first build lets go of the lock, a holder takes it and keeps it until the
    // run has logged its first give-up, so the rebase build's first wait ends at the timeout.
    e.ctl(
        "build.sh",
        &format!(
            "echo b >> work.txt && git add work.txt && git -c user.name=f -c user.email=f@f commit -qm b && {}; \
             (setsid timeout 30 flock -x {lock:?} sh -c 'until grep -q lock_wait_timeout {log:?}; do sleep 0.05; done' </dev/null >/dev/null 2>&1 &)",
            move_main_sh(&e)
        ),
    );
    e.queue("build", &["pass:script", "pass:commit", "pass:commit"]);
    e.queue("verify", &["fail:base moved", "pass"]);
    let out = e
        .ns()
        .env_remove("NS_NOW")
        .env("NS_PHASE_TIMEOUT_MS", "build=4000")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    let main = remote_main(&e);
    let p = e.prompt(4, "build");
    assert!(
        p.contains(&format!("rebase onto the newest origin/main ({main})")),
        "{p}"
    );
    assert_eq!(rebase_events(&e).len(), 1);
}

#[test]
fn no_artifact_is_a_failed_attempt() {
    let e = Env::new();
    e.queue("build", &["none", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(v["phases"][1]["reason"], "no artifact written");
    assert_eq!(v["phases"][1]["written"], false);
    assert_eq!(&e.calls()[..3], ["triage", "build", "build"]);
}

/// The build phase records its `NS_RUN_PID`, `NS_WATCH_PID` and the sid and pgid of itself and of `ns`.
fn record_phase_ids(e: &Env) {
    e.ctl(
        "build.sh",
        r#"{
  echo "${NS_RUN_PID-unset} ${NS_WATCH_PID-unset}"
  ps -o sid=,pgid= -p $$
  ps -o sid=,pgid= -p "$NS_RUN_PID"
} > "$FAKE_CTRL/ids""#,
    );
    e.queue("build", &["pass:script"]);
}

fn phase_ids(e: &Env) -> Vec<String> {
    fs::read_to_string(e.ctrl.join("ids"))
        .unwrap()
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// Run `ns`, which must succeed, and return the pid it ran as.
fn ok_pid(ns: &mut Ns) -> u32 {
    let (out, pid) = ns.output_with_pid();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    pid
}

#[test]
fn a_phase_runs_in_its_own_session_and_knows_the_run_pid() {
    let e = Env::new();
    record_phase_ids(&e);
    let pid = ok_pid(
        e.ns()
            .env("NS_WATCH_PID", "999999")
            .args(["run", "--issue", "7"]),
    );
    let ids = phase_ids(&e);
    assert_eq!(
        ids[0],
        pid.to_string(),
        "an inherited NS_WATCH_PID leaked: {ids:?}"
    );
    let phase: Vec<&str> = ids[1].split(' ').collect();
    let ns: Vec<&str> = ids[2].split(' ').collect();
    assert_ne!(phase[0], ns[0], "phase shares ns's session: {ids:?}");
    assert_ne!(phase[1], ns[1], "phase shares ns's process group: {ids:?}");
}

#[test]
fn a_phase_under_watch_knows_the_watch_pid() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    record_phase_ids(&e);
    let pid = ok_pid(e.ns().args(["watch", "--once"]));
    // The unit ran in its own ns run, which ns watch started.
    let run = run_events(&e)
        .into_iter()
        .find(|ev| ev["event"] == "worker_start")
        .unwrap()["run_pid"]
        .clone();
    assert_ne!(run, pid);
    assert_eq!(phase_ids(&e)[0], format!("{run} {pid}"));
}

/// Each build phase records its `TMPDIR` and what was in it, and the modes of it and its parent,
/// then leaves a file there.
fn record_tmpdir(e: &Env) {
    e.ctl(
        "build.sh",
        r#"{ echo "$TMPDIR"; ls -A "$TMPDIR"; } >> "$FAKE_CTRL/tmpdir-$NS_UNIT"
stat -c %a "$TMPDIR" "$TMPDIR/.." >> "$FAKE_CTRL/mode-$NS_UNIT"
touch "$TMPDIR/scratch""#,
    );
}

/// The lines `record_tmpdir` wrote for `unit`.
fn phase_tmpdirs(e: &Env, unit: &str) -> Vec<String> {
    fs::read_to_string(e.ctrl.join(format!("tmpdir-{unit}")))
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

/// The directory of `unit`'s phase `TMPDIR`s, in the system temp dir `ns` inherited:
/// `ns-<repo>-<8 hex of sha256 of the git common dir>-<unit>`.
fn unit_tmp(e: &Env, unit: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(e.root.join(".git").as_os_str().as_encoded_bytes());
    let hash: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
    e.base
        .join("systmp")
        .join(format!("ns-myrepo-{hash}-{unit}"))
}

#[test]
fn each_phase_gets_its_own_tmpdir_and_a_passing_phase_leaves_none_behind() {
    let e = Env::new();
    e.issue(8, "Fix other", "OPEN");
    record_tmpdir(&e);
    for (n, unit) in [("7", UNIT), ("8", "8-fix-other")] {
        e.queue(&format!("build.{unit}"), &["pass:script"]);
        let v = e.run(&["run", "--issue", n], 0);
        assert_eq!(v["outcome"], "done", "{v}");
    }
    let dir = unit_tmp(&e, UNIT).join("build-1");
    assert_eq!(phase_tmpdirs(&e, UNIT), [dir.to_str().unwrap()]);
    let other = unit_tmp(&e, "8-fix-other").join("build-1");
    assert_eq!(phase_tmpdirs(&e, "8-fix-other"), [other.to_str().unwrap()]);
    for unit in [UNIT, "8-fix-other"] {
        let left: Vec<_> = fs::read_dir(unit_tmp(&e, unit)).unwrap().collect();
        assert!(left.is_empty(), "{unit}: {left:?}");
        let modes = fs::read_to_string(e.ctrl.join(format!("mode-{unit}"))).unwrap();
        assert_eq!(modes, "700\n700\n", "{unit}");
    }
    let worktrees = e.base.join("myrepo.worktrees");
    assert!(worktrees.join(UNIT).is_dir());
    for d in [&dir, &other] {
        assert!(!d.starts_with(&e.root), "{} is in the repo", d.display());
        assert!(
            !d.starts_with(&worktrees),
            "{} is in a worktree",
            d.display()
        );
    }
}

#[test]
fn a_phase_that_times_out_keeps_its_tmpdir() {
    let e = Env::new();
    record_tmpdir(&e);
    e.queue("build", &["pass:script+sleep", "pass:script"]);
    let v = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "build=1500")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&v).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    let tmp = unit_tmp(&e, UNIT);
    // `ns` makes the directory before it starts the phase, so the kill cannot beat it; what the
    // phase wrote there depends on how soon it started, so is not checked.
    assert!(tmp.join("build-1").is_dir());
    assert!(!tmp.join("build-2").exists());
}

#[test]
fn a_passing_phase_whose_tmpdir_will_not_go_is_logged_not_failed() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let e = Env::new();
    e.ctl(
        "build.sh",
        r#"mkdir "$TMPDIR/locked" && touch "$TMPDIR/locked/f" && chmod 500 "$TMPDIR/locked""#,
    );
    e.queue("build", &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    let dir = unit_tmp(&e, UNIT).join("build-1");
    fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(dir.join("locked/f").is_file());
    let ev = run_events(&e)
        .into_iter()
        .find(|ev| ev["event"] == "tmpdir_not_removed")
        .expect("a tmpdir_not_removed event");
    assert_eq!(ev["phase"], "build", "{ev}");
    assert_eq!(ev["path"], dir.to_str().unwrap(), "{ev}");
    assert!(ev["error"].as_str().is_some_and(|m| !m.is_empty()), "{ev}");
}

#[test]
fn a_phase_tmpdir_parent_another_user_could_use_stops_the_run_before_the_phase() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    let tmp = unit_tmp(&e, UNIT);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let calls = e.calls();
    let review = e.worktree(UNIT).join(format!(".ns/{UNIT}/review.md"));
    assert!(review.is_file());
    let kept = fs::read_dir(&tmp).unwrap().count();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o777)).unwrap();
    e.ns()
        .args(["run", "--issue", "7", "--from", "review"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(format!(
            "{} is not a directory only this user can use",
            tmp.display()
        )));
    assert_eq!(e.calls(), calls);
    assert_eq!(fs::read_dir(&tmp).unwrap().count(), kept);
    assert!(review.is_file(), "the refused phase's artifact stays put");
}

#[test]
fn a_failed_phase_keeps_its_tmpdir() {
    let e = Env::new();
    record_tmpdir(&e);
    e.queue("build", &["fail:script", "pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let tmp = unit_tmp(&e, UNIT);
    let dirs = [tmp.join("build-1"), tmp.join("build-2")];
    assert_eq!(
        phase_tmpdirs(&e, UNIT),
        dirs.iter().map(|d| d.to_str().unwrap()).collect::<Vec<_>>()
    );
    assert!(dirs[0].join("scratch").is_file());
    assert!(!dirs[1].exists());
}

#[test]
fn a_blocked_phase_keeps_its_tmpdir() {
    let e = Env::new();
    record_tmpdir(&e);
    e.queue("build", &["blocked:script"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let dir = unit_tmp(&e, UNIT).join("build-1");
    assert_eq!(phase_tmpdirs(&e, UNIT), [dir.to_str().unwrap()]);
    assert!(dir.join("scratch").is_file());
}

#[test]
fn a_rerun_keeps_the_tmpdirs_a_failed_run_kept() {
    let e = Env::new();
    record_tmpdir(&e);
    e.queue("build", &["fail:script", "fail:script"]);
    e.run(&["run", "--issue", "7"], 1);
    let tmp = unit_tmp(&e, UNIT);
    let kept = [tmp.join("build-1"), tmp.join("build-2")];
    for dir in &kept {
        assert!(dir.join("scratch").is_file(), "{}", dir.display());
    }
    e.queue("build", &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let dirs = ["build-1", "build-2", "build-3"].map(|d| tmp.join(d));
    assert_eq!(
        phase_tmpdirs(&e, UNIT),
        dirs.iter().map(|d| d.to_str().unwrap()).collect::<Vec<_>>()
    );
    for dir in &kept {
        assert!(dir.join("scratch").is_file(), "{}", dir.display());
    }
    assert!(!tmp.join("build-3").exists());
}

#[test]
fn a_tmpdir_that_cannot_be_created_stops_the_run_before_the_phase() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    let tmp = unit_tmp(&e, UNIT);
    fs::create_dir(&tmp).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o500)).unwrap();
    let out = e.ns().args(["run", "--issue", "7"]).assert().failure();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();
    out.stderr(predicates::str::contains(format!(
        "cannot create {}",
        tmp.join("triage-1").display()
    )));
    assert!(e.calls().is_empty());
}

/// The `GIT_SSH_COMMAND` the build phase saw, or `unset`, after `ns run` under `setup`.
fn phase_ssh_command(setup: impl FnOnce(&Env, &mut Ns)) -> String {
    let e = Env::new();
    e.ctl(
        "build.sh",
        r#"echo "${GIT_SSH_COMMAND-unset}" > "$FAKE_CTRL/ssh""#,
    );
    e.queue("build", &["pass:script"]);
    let mut ns = e.ns();
    ns.env_remove("GIT_SSH_COMMAND").env_remove("GIT_SSH");
    setup(&e, &mut ns);
    ok_pid(ns.args(["run", "--issue", "7"]));
    fs::read_to_string(e.ctrl.join("ssh"))
        .unwrap()
        .trim_end()
        .to_string()
}

#[test]
fn a_phase_pushes_with_ssh_keepalive_unless_the_user_chose_an_ssh_command() {
    let keepalive = "ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=40";
    assert_eq!(phase_ssh_command(|_, _| {}), keepalive);
    assert_eq!(
        phase_ssh_command(|_, ns| {
            ns.env("GIT_SSH_COMMAND", "ssh -i key");
        }),
        "ssh -i key"
    );
    assert_eq!(
        phase_ssh_command(|_, ns| {
            ns.env("GIT_SSH", "/usr/bin/ssh");
        }),
        "unset"
    );
    assert_eq!(
        phase_ssh_command(|e, _| {
            git(&e.root, &["config", "core.sshCommand", "ssh -i key"]);
        }),
        "unset"
    );
}

#[test]
fn a_phase_that_kills_its_own_group_is_a_failed_attempt_not_a_dead_watch() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    e.ctl("build.sh", "kill 0");
    e.queue("build", &["pass:script", "pass:commit"]);
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["outcome"], "done", "{v}");
    assert_eq!(&e.calls()[..3], ["triage", "build", "build"]);
    assert_eq!(build_event(&e, 1)["written"], false);
}

/// The `gate` events in the run log.
fn gate_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "gate")
        .collect()
}

#[test]
fn a_green_gate_continues_to_verify_and_is_logged() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"touch gate-ran && echo gate ok\"\n");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    assert!(e.worktree(UNIT).join("gate-ran").exists());
    let g = gate_events(&e);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0]["command"], "touch gate-ran && echo gate ok");
    assert_eq!(g[0]["exit"], 0);
    assert_eq!(g[0]["green"], true);
    assert_eq!(g[0]["phase"], "build");
    assert!(g[0]["wall_s"].is_number(), "{g:?}");
    assert_eq!(
        g[0]["sha"].as_str(),
        Some(git(&e.worktree(UNIT), &["rev-parse", "--short", "HEAD"]).as_str())
    );
}

#[test]
fn a_committing_silent_rebuild_reruns_the_gate_and_can_go_green() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\nmax_attempts = 3\ngate = \"test -f {f} || {{ touch {f}; echo boom; exit 1; }}\"\n",
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "none:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(e.calls().contains(&"verify".to_string()), "{:?}", e.calls());
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["green"], false);
    assert_eq!(g[1]["green"], true);
    assert_ne!(g[0]["sha"], g[1]["sha"]);
}

#[test]
fn a_review_that_commits_runs_the_gate_again() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    e.factory(&format!(
        "[phases.build]\ngate = \"echo ran >> {}\"\n",
        log.display()
    ));
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "verify", "ship"]
    );
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1]["phase"], "review");
    assert!(g.iter().all(|g| g["where"] == "local_full"), "{g:?}");
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);
}

#[test]
fn a_red_gate_after_a_review_commit_goes_back_to_build() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"test ! -f broken\"\n");
    let commit = "git -c user.name=f -c user.email=f@f commit -qm";
    e.ctl(
        "review.sh",
        &format!("touch broken && git add broken && {commit} break"),
    );
    e.ctl("build.sh", &format!("git rm -q broken && {commit} fix"));
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("review", &["pass:script", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(5, "build").contains("test ! -f broken"));
    let g: Vec<_> = gate_events(&e)
        .iter()
        .map(|g| (g["phase"].clone(), g["green"].clone()))
        .collect();
    assert_eq!(
        g,
        [
            ("build".into(), true.into()),
            ("review".into(), false.into()),
            ("build".into(), true.into())
        ]
    );
}

/// `[gate] local = "build-only"` with `extra` before it, a build gate that appends `full` or
/// `fast` to `ci-runs`, a `ci-affected` row that appends the files it got to `affected` and runs
/// `affected.sh` when it exists, and a build script that commits `built.txt`. Returns the two logs.
fn build_only(e: &Env, extra: &str) -> (PathBuf, PathBuf) {
    let runs = e.ctrl.join("ci-runs");
    let affected = e.ctrl.join("affected");
    e.ctl(
        "ci.sh",
        &format!(
            "if [ \"$1\" = --fast ]; then echo fast; else echo full; fi >> {}\n",
            runs.display()
        ),
    );
    e.factory(&format!(
        "{extra}[gate]\nlocal = \"build-only\"\n[phases.build]\ngate = \"sh {}\"\n",
        e.ctrl.join("ci.sh").display()
    ));
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        format!(
            "| Task | Command |\n|---|---|\n| ci-affected | `echo \"[${{NS_GATE_FILES-UNSET}}]\" >> {}; test ! -f {c}/affected.sh || sh {c}/affected.sh` | 1 s |\n",
            affected.display(),
            c = e.ctrl.display()
        ),
    )
    .unwrap();
    git(&e.root, &["add", "."]);
    git(&e.root, &["commit", "-q", "-m", "stack"]);
    e.ctl(
        "build.sh",
        "date >> built.txt && git add built.txt && git -c user.name=f -c user.email=f@f commit -qm built",
    );
    (runs, affected)
}

fn gate_where(e: &Env) -> Vec<(Value, Value)> {
    gate_events(e)
        .iter()
        .map(|g| (g["phase"].clone(), g["where"].clone()))
        .collect()
}

#[test]
fn build_only_runs_one_full_gate_after_build_and_fast_ones_with_the_files_changed_since() {
    let e = Env::new();
    let (runs, affected) = build_only(&e, AUTO);
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:script"]);
    e.queue("review", &["pass:commit"]);
    main_moves(&e, "work.txt", "x\n");
    e.gh_file("pr-12.merge", "BEHIND");
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(d["gate"], format!("sh {}", e.ctrl.join("ci.sh").display()));
    assert!(d["gate_rest"]
        .as_str()
        .unwrap()
        .contains("ci.sh --fast) && ("));
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\n");
    assert_eq!(
        gate_where(&e),
        [
            ("build".into(), "local_full".into()),
            ("review".into(), "local_fast".into())
        ]
    );
    assert_eq!(fs::read_to_string(&affected).unwrap(), "[work.txt]\n");
    assert!(e.gh_calls().contains("pr update-branch 12"));
}

#[test]
fn build_only_skips_a_fast_gate_where_a_gate_already_went_green_but_never_a_full_one() {
    let e = Env::new();
    let (runs, _) = build_only(&e, AUTO);
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\n");

    let e = Env::new();
    let (runs, _) = build_only(&e, AUTO);
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:script"]);
    e.queue("review", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\n");

    let e = Env::new();
    let (runs, _) = build_only(&e, "");
    e.queue("build", &["pass:script", "pass"]);
    e.queue("review", &["pass:commit", "pass"]);
    e.queue("verify", &["pass", "fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "verify", "build", "verify", "review", "ship"]
    );
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\nfull\n");
}

#[test]
fn build_only_without_a_full_gate_this_run_tests_the_files_changed_since_the_base() {
    let e = Env::new();
    let (runs, affected) = build_only(&e, "");
    e.queue("build", &["pass:script"]);
    e.queue("review", &["pass", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let v = e.run(&["run", "--issue", "7", "--from", "review"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\n");
    assert_eq!(
        fs::read_to_string(&affected).unwrap(),
        "[built.txt\nwork.txt]\n"
    );
}

#[test]
fn build_only_fast_gate_runs_everything_when_git_cannot_find_the_changed_files() {
    let e = Env::new();
    let (runs, affected) = build_only(&e, "");
    e.ctl(
        "review.sh",
        "git update-ref -d refs/heads/main && date >> work.txt && git add work.txt && git -c user.name=f -c user.email=f@f commit -qm fix",
    );
    e.queue("build", &["pass:script"]);
    e.queue("review", &["pass", "pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let v = e.run(&["run", "--issue", "7", "--from", "review"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\n");
    assert_eq!(fs::read_to_string(&affected).unwrap(), "[UNSET]\n");
}

#[test]
fn build_only_sends_a_red_fast_gate_back_to_build_and_the_rebuild_runs_the_full_one() {
    let e = Env::new();
    let (runs, _) = build_only(&e, "");
    let failing = e.ctrl.join("affected.sh");
    let commit = "git add work.txt && git -c user.name=f -c user.email=f@f commit -qm fix";
    e.ctl(
        "review.sh",
        &format!(
            "echo 'echo affected tests failed; exit 1' > {}; date >> work.txt && {commit}",
            failing.display()
        ),
    );
    e.ctl(
        "build.sh",
        &format!(
            "rm -f {}; date >> built.txt && git add built.txt && git -c user.name=f -c user.email=f@f commit -qm built",
            failing.display()
        ),
    );
    e.queue("build", &["pass:script", "pass:script"]);
    e.queue("review", &["pass:script", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(5, "build").contains("affected tests failed"));
    assert_eq!(fs::read_to_string(&runs).unwrap(), "full\nfast\nfull\n");
    let g: Vec<_> = gate_events(&e)
        .iter()
        .map(|g| (g["phase"].clone(), g["green"].clone(), g["where"].clone()))
        .collect();
    assert_eq!(
        g,
        [
            ("build".into(), true.into(), "local_full".into()),
            ("review".into(), false.into(), "local_fast".into()),
            ("build".into(), true.into(), "local_full".into())
        ]
    );
}

#[test]
fn the_gate_falls_back_to_ci_local_in_stack_md() {
    let e = Env::new();
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command |\n|---|---|\n| ci-local | `echo from stack` | 1 s |\n",
    )
    .unwrap();
    git(&e.root, &["add", "."]);
    git(&e.root, &["commit", "-q", "-m", "stack"]);
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command |\n|---|---|\n| ci-local | `echo from main` | 1 s |\n",
    )
    .unwrap();
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(d["gate"], "echo from main");
    e.run(&["run", "--issue", "7"], 0);
    let g = gate_events(&e);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0]["command"], "echo from main");
    let in_worktree = fs::read_to_string(e.worktree(UNIT).join("docs/agents/stack.md")).unwrap();
    assert!(in_worktree.contains("echo from stack"), "{in_worktree}");
}

#[test]
fn a_rebuild_that_leaves_head_alone_skips_a_green_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit", "pass"]);
    e.queue("verify", &["fail", "pass"]);
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(&e.calls()[..4], ["triage", "build", "verify", "build"]);
    assert_eq!(gate_events(&e).len(), 1);
}

#[test]
fn a_gate_that_times_out_goes_back_to_build_and_uses_an_attempt() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"sleep 30\"\n");
    let out = e
        .ns()
        .env("NS_GATE_TIMEOUT_MS", "300")
        .args(["run", "--issue", "7"])
        .output();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["reason"], "build is out of attempts (2)");
    assert_eq!(e.calls(), ["triage", "build", "build"]);
    assert!(e.prompt(3, "build").contains("timed out after 0.3 s"));
    assert!(e.prompt(3, "build").contains("attempt 2"));
    let g = gate_events(&e);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["timed_out"], true);
    assert_eq!(g[0]["green"], false);
}

/// A factory with one ratchet, `flash`, recorded at 100, whose command prints `ctrl/flash`.
fn ratchet_env(measured: &str) -> Env {
    let e = Env::new();
    e.factory(&format!(
        "[ratchets.flash]\ncommand = \"cat {}\"\nbetter = \"lower\"\ntolerance = \"5%\"\n",
        e.ctrl.join("flash").display()
    ));
    fs::write(
        e.root.join(".nightshift/ratchets.json"),
        r#"{"version": 1, "values": {"flash": 100}}"#,
    )
    .unwrap();
    e.ctl("flash", measured);
    e
}

#[test]
fn a_ratchet_makes_the_gate_run_ns_ratchet_check() {
    let e = ratchet_env("104\n");
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert!(
        d["gate"]
            .as_str()
            .unwrap()
            .ends_with(" ratchet check --human"),
        "{d}"
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let g = gate_events(&e);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0]["green"], true);
}
