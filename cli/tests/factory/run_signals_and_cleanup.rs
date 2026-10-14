#[test]
fn a_phase_killed_by_a_signal_fails_its_attempt_naming_it() {
    let e = Env::new();
    e.queue("build", &["pass:commit+kill", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let p = build_event(&e, 1);
    assert_eq!(p["reason"], "was killed by SIGKILL", "{p}");
    assert_eq!(p["written"], false);
    let dir = e.worktree(UNIT).join(".ns").join(UNIT);
    assert!(fs::read_to_string(dir.join("history/build-killed-1.md"))
        .unwrap()
        .contains("status: pass"));
    assert_eq!(
        p["archived"],
        dir.join("history/build-killed-1.md").to_str().unwrap()
    );
}

#[test]
fn a_phase_killed_on_every_attempt_is_stuck_naming_the_signal() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 1\n");
    e.queue("build", &["pass:commit+kill"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (1): the last attempt was killed by SIGKILL"
    );
}

#[test]
fn a_gate_killed_by_a_signal_names_it() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"kill -9 $$\"\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate was killed by SIGKILL"
    );
    let g = gate_events(&e);
    assert_eq!(g[0]["signal"], "SIGKILL");
    assert_eq!(g[0]["memory_exceeded"], false);
    assert!(e.prompt(3, "build").contains("was killed by SIGKILL"));
}

#[test]
fn a_killed_rebuild_keeps_the_red_gate_feedback() {
    let e = Env::new();
    let flag = e.ctrl.join("gate-green");
    e.factory(&format!(
        "[phases.build]\nmax_attempts = 3\ngate = \"test -f {f} || {{ touch {f}; echo boom; exit 1; }}\"\n",
        f = flag.display()
    ));
    e.queue("build", &["pass:commit", "pass:commit+kill", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        build_event(&e, 3)["decision"],
        "was killed by SIGKILL with the CI gate still red"
    );
    assert!(e.prompt(4, "build").contains("boom"));
}

#[test]
fn a_gate_still_red_after_a_silent_build_still_names_the_signal() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"kill -9 $$\"\n");
    e.queue("build", &["pass:commit", "none"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate was killed by SIGKILL"
    );
    assert_eq!(gate_events(&e).len(), 1);
}

#[test]
fn a_systemd_run_that_does_not_enforce_memory_max_falls_back_to_prlimit() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    // Starts and exits 0 but sets no limit, as a systemd with no memory controller would.
    let p = e.bin.join("systemd-run");
    write_exe(&p, "#!/bin/sh\necho max\n");
    e.run(&["run", "--issue", "7"], 0);
    let cap = memory_cap(&e);
    assert_eq!(cap["via"], "prlimit", "{cap}");
    assert!(
        cap["fallback"]
            .as_str()
            .unwrap()
            .contains("did not set the scope's memory.max to 67108864"),
        "{cap}"
    );
}

#[test]
fn a_systemd_run_that_rejects_a_scope_property_falls_back_to_prlimit() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    // Sets memory.max but rejects OOMPolicy, as a systemd older than 253 does on a scope.
    let p = e.bin.join("systemd-run");
    write_exe(
        &p,
        "#!/bin/sh\nfor a; do [ \"$a\" = OOMPolicy=stop ] && { echo 'Unknown assignment' >&2; exit 1; }; done\necho 67108864\n",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(memory_cap(&e)["via"], "prlimit");
}

#[test]
fn a_missing_harness_under_a_cap_is_still_a_usage_error() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n[defaults]\nharness = \"gone\"\n");
    e.config("[harness.gone]\ncommand = [\"ns-no-such-harness\"]\ncommand_write = [\"ns-no-such-harness\"]\n");
    let out = e.ns().args(["run", "--issue", "7"]).assert().code(2);
    let err = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        err.contains("harness binary `ns-no-such-harness` is not on PATH"),
        "{err}"
    );
}

#[test]
fn a_dry_run_probes_no_memory_cap() {
    let e = Env::new();
    e.factory("[limits]\nmemory_mb = 64\n");
    break_tool(&e, "systemd-run");
    break_tool(&e, "prlimit");
    e.run(&["run", "--issue", "7", "--dry-run"], 0);
}

#[test]
fn a_plain_gate_failure_names_no_signal() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"exit 1\"\n");
    e.run(&["run", "--issue", "7"], 1);
    let g = gate_events(&e);
    assert!(g[0]["signal"].is_null());
    assert_eq!(
        build_event(&e, 2)["decision"],
        "the CI gate failed after build"
    );
}

// ---------------------------------------------------------------- cleanup (#211)

fn branch_exists(e: &Env, unit: &str) -> bool {
    !git(&e.root, &["branch", "--list", &format!("ns/{unit}")]).is_empty()
}

/// Index of the first run-log event matching `pred`.
fn event_at(e: &Env, pred: impl Fn(&Value) -> bool) -> usize {
    run_events(e)
        .iter()
        .position(pred)
        .unwrap_or_else(|| panic!("no such event in {:?}", run_events(e)))
}

fn cleanup_events(e: &Env) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|v| v["event"] == "cleanup")
        .collect()
}

/// A pre-receive hook on `origin` that declines every push to the quality branch.
fn decline_quality_pushes(e: &Env) {
    let hook = e.base.join("remote.git/hooks/pre-receive");
    write_exe(
        &hook,
        "#!/bin/sh\nwhile read old new ref; do\n  [ \"$ref\" = refs/heads/nightshift/quality ] && { echo declined >&2; exit 1; }\ndone\nexit 0\n",
    );
}

/// Make the outbox unwritable too: its temp file's path is a directory.
fn break_outbox(e: &Env) {
    fs::create_dir_all(e.root.join(".git/ns/quality-outbox.jsonl.tmp")).unwrap();
}

fn auto_merge_env() -> Env {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.ctl("review.body", FINDING);
    e
}

#[test]
fn auto_merge_removes_the_worktree_and_branch_after_saving_the_record() {
    let e = auto_merge_env();
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
    assert!(!e.worktree(UNIT).exists());
    assert!(!branch_exists(&e, UNIT));
    assert!(!git(&e.root, &["worktree", "list"]).contains(UNIT));
    let r = &quality_records(&e)[0];
    assert_eq!(
        (r["unit"].as_str(), r["outcome"].as_str()),
        (Some(UNIT), Some("merged"))
    );
    let saved = event_at(&e, |v| v["event"] == "quality_record");
    let removed = event_at(&e, |v| v["event"] == "cleanup");
    assert!(saved < removed);
    let ev = &cleanup_events(&e)[0];
    assert_eq!(
        (
            ev["by"].as_str(),
            ev["removed"].as_bool(),
            ev["pr"].as_u64()
        ),
        (Some("run"), Some(true), Some(12))
    );
}

#[test]
fn a_merge_never_checks_out_main_in_the_unit_worktree() {
    let e = auto_merge_env();
    // The main checkout is off main, so nothing stops a checkout of main in the unit worktree.
    git(&e.root, &["checkout", "-qb", "elsewhere"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
    assert!(!e.worktree(UNIT).exists());
    let calls = e.gh_calls();
    assert!(
        calls.contains(&format!(
            "api repos/{{owner}}/{{repo}}/git/refs/heads/ns/{UNIT} -X DELETE"
        )),
        "{calls}"
    );
    git(&e.root, &["checkout", "-q", "main"]);
}

#[test]
fn a_remote_branch_github_will_not_delete_is_logged_and_the_merge_stands() {
    let e = auto_merge_env();
    e.gh_file("branch-delete.fail", "HTTP 422: Reference does not exist");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let ev = &run_events(&e)[event_at(&e, |v| v["event"] == "branch_not_deleted")];
    assert_eq!(ev["pr"], 12);
    assert_eq!(ev["branch"], format!("ns/{UNIT}"));
    assert!(
        ev["error"]
            .as_str()
            .unwrap()
            .contains("Reference does not exist"),
        "{ev}"
    );
}

#[test]
fn auto_merge_with_the_record_in_the_outbox_still_cleans() {
    let e = auto_merge_env();
    decline_quality_pushes(&e);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
    assert!(!e.worktree(UNIT).exists());
    assert_eq!(quality_events(&e)[0]["status"], "outbox");
    let outbox = fs::read_to_string(e.root.join(".git/ns/quality-outbox.jsonl")).unwrap();
    assert!(outbox.contains(UNIT), "{outbox}");
    assert!(quality_records(&e).is_empty());
}

#[test]
fn auto_merge_keeps_the_worktree_when_its_record_is_not_saved() {
    let e = auto_merge_env();
    decline_quality_pushes(&e);
    break_outbox(&e);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["cleanup"]["removed"], false, "{v}");
    assert_eq!(
        v["cleanup"]["reason"], "its quality record is not saved",
        "{v}"
    );
    assert!(e
        .worktree(UNIT)
        .join(format!(".ns/{UNIT}/review.md"))
        .is_file());
    assert!(branch_exists(&e, UNIT));
}

#[test]
fn auto_merge_keeps_a_worktree_with_uncommitted_tracked_changes() {
    let e = auto_merge_env();
    e.queue("ship", &["pass:script"]);
    e.ctl("ship.sh", "echo edited >> README\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        v["cleanup"]["reason"], "uncommitted changes to tracked files (1 paths)",
        "{v}"
    );
    assert_eq!(
        fs::read_to_string(e.worktree(UNIT).join("README")).unwrap(),
        "hi\nedited\n"
    );
    assert!(branch_exists(&e, UNIT));
}

#[test]
fn a_merge_that_leaves_a_pr_head_not_here_keeps_the_worktree() {
    let e = auto_merge_env();
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file(
        "updated-12.head",
        "0123456789abcdef0123456789abcdef01234567",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let reason = v["cleanup"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("has changes merged PR #12 (head 0123456789abcdef0123456789abcdef01234567) does not hold"),
        "{v}"
    );
    assert!(e.worktree(UNIT).exists());
}

/// A unit worktree on `ns/<unit>` with one commit, a review and a `pr.md` naming PR `pr`,
/// issue `n` in `issue_state`, and the PR in `pr_state` with a body that closes the issue.
fn unit_with_pr(
    e: &Env,
    n: u64,
    unit: &str,
    pr: u64,
    issue_state: &str,
    pr_state: &str,
) -> PathBuf {
    let wt = e.worktree(unit);
    git(
        &e.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("ns/{unit}"),
            wt.to_str().unwrap(),
            "main",
        ],
    );
    fs::write(wt.join(format!("{unit}.txt")), "work\n").unwrap();
    git(&wt, &["add", &format!("{unit}.txt")]);
    git(&wt, &["commit", "-q", "-m", unit]);
    let dir = wt.join(".ns").join(unit);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("pr.md"),
        format!("---\nunit: {unit}\nphase: ship\nstatus: pass\npr: https://github.com/o/r/pull/{pr}\n---\n"),
    )
    .unwrap();
    fs::write(
        dir.join("review.md"),
        format!("---\nunit: {unit}\nphase: review\nstatus: pass\nupdated: 2026-10-08T00:00:00Z\n---\n{FINDING}"),
    )
    .unwrap();
    e.issue(n, unit, issue_state);
    e.gh_file(&format!("pr-{pr}.state"), pr_state);
    e.gh_file(&format!("pr-{pr}.body"), &format!("Closes #{n}"));
    wt
}

fn reasons(v: &Value) -> std::collections::BTreeMap<String, String> {
    v["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| {
            (
                k["unit"].as_str().unwrap().to_string(),
                k["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn units_in(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|u| u["unit"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn clean_removes_a_merged_unit_after_saving_its_record_and_keeps_open_ones() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let open = unit_with_pr(&e, 4, "4-open", 40, "OPEN", "MERGED");
    let v = e.run(&["clean"], 0);
    assert_eq!(v["status"], "cleaned", "{v}");
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert_eq!(v["removed"][0]["pr"], 30);
    assert_eq!(reasons(&v)["4-open"], "issue #4 is open", "{v}");
    assert!(!done.exists());
    assert!(!branch_exists(&e, "3-done"));
    assert!(open.join(".ns/4-open/pr.md").is_file());
    assert!(branch_exists(&e, "4-open"));
    let records = quality_records(&e);
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0]["unit"].as_str(),
            records[0]["outcome"].as_str(),
            records[0]["pr"].as_u64(),
            records[0]["issue"].as_u64()
        ),
        (Some("3-done"), Some("merged"), Some(30), Some(3))
    );
    assert!(
        event_at(&e, |v| v["event"] == "quality_record")
            < event_at(&e, |v| v["event"] == "cleanup" && v["unit"] == "3-done")
    );
    assert_eq!(cleanup_events(&e)[0]["by"], "clean");

    // A rerun has nothing to do.
    let v = e.run(&["clean"], 0);
    assert_eq!(v["status"], "nothing-to-clean", "{v}");
    assert_eq!(units_in(&v["removed"]), Vec::<String>::new());
}

#[test]
fn clean_removes_the_tmpdirs_a_merged_unit_kept() {
    let e = Env::new();
    unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    unit_with_pr(&e, 4, "4-open", 40, "OPEN", "MERGED");
    for unit in ["3-done", "4-open"] {
        fs::create_dir_all(unit_tmp(&e, unit).join("build-1")).unwrap();
        fs::write(unit_tmp(&e, unit).join("build-1/scratch"), "x\n").unwrap();
    }
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert!(!unit_tmp(&e, "3-done").exists());
    assert!(unit_tmp(&e, "4-open").join("build-1/scratch").is_file());
}

#[test]
fn clean_keeps_units_it_cannot_prove_merged_and_says_why() {
    let e = Env::new();
    let dirty = unit_with_pr(&e, 5, "5-dirty", 50, "CLOSED", "MERGED");
    fs::write(dirty.join("5-dirty.txt"), "changed\n").unwrap();
    let ahead = unit_with_pr(&e, 6, "6-ahead", 60, "CLOSED", "MERGED");
    let pr_head = git(&ahead, &["rev-parse", "HEAD"]);
    e.gh_file("pr-60.head", &pr_head);
    fs::write(ahead.join("more.txt"), "more\n").unwrap();
    git(&ahead, &["add", "more.txt"]);
    git(&ahead, &["commit", "-q", "-m", "more"]);
    unit_with_pr(&e, 8, "8-open-pr", 80, "CLOSED", "OPEN");
    unit_with_pr(&e, 9, "9-other", 90, "CLOSED", "MERGED");
    e.gh_file("pr-90.body", "Closes #1");
    unit_with_pr(&e, 10, "10-elsewhere", 100, "CLOSED", "MERGED");
    e.gh_file("pr-100.ref", "someone/branch");
    let nopr = unit_with_pr(&e, 11, "11-nopr", 110, "CLOSED", "MERGED");
    fs::remove_file(nopr.join(".ns/11-nopr/pr.md")).unwrap();
    unit_with_pr(&e, 12, "scratch", 120, "CLOSED", "MERGED");
    unit_with_pr(&e, 13, "13-unread", 130, "CLOSED", "MERGED");
    fs::remove_file(e.ghd.join("issue-13.json")).unwrap();
    // Untracked files never keep a worktree: they are not work the PR could hold.
    let untracked = unit_with_pr(&e, 14, "14-untracked", 140, "CLOSED", "MERGED");
    fs::write(untracked.join("build.log"), "noise\n").unwrap();

    let v = e.run(&["clean"], 0);
    let r = reasons(&v);
    assert_eq!(
        r["5-dirty"], "uncommitted changes to tracked files (1 paths)",
        "{v}"
    );
    let tip = git(&ahead, &["rev-parse", "HEAD"]);
    assert_eq!(
        r["6-ahead"],
        format!("ns/6-ahead at {tip} has changes merged PR #60 (head {pr_head}) does not hold"),
        "{v}"
    );
    assert_eq!(r["8-open-pr"], "PR #80 is OPEN");
    assert_eq!(r["9-other"], "PR #90 does not close #9");
    assert_eq!(
        r["10-elsewhere"],
        "PR #100 is from branch \"someone/branch\", not ns/10-elsewhere"
    );
    assert_eq!(r["11-nopr"], "no PR in .ns/11-nopr/pr.md");
    assert_eq!(r["scratch"], "the unit id names no issue");
    assert!(r["13-unread"].starts_with("cannot read issue #13: "), "{v}");
    assert_eq!(r.len(), 8, "{v}");
    assert_eq!(units_in(&v["removed"]), ["14-untracked"], "{v}");
    for unit in [
        "5-dirty",
        "6-ahead",
        "8-open-pr",
        "9-other",
        "10-elsewhere",
        "11-nopr",
        "scratch",
        "13-unread",
    ] {
        assert!(e.worktree(unit).is_dir(), "{unit}");
        assert!(branch_exists(&e, unit), "{unit}");
    }
    assert_eq!(
        fs::read_to_string(dirty.join("5-dirty.txt")).unwrap(),
        "changed\n"
    );
}

/// A merged unit's worktree left on main, as `gh pr merge --delete-branch` left it: the main
/// checkout is on another branch and the unit's branch is gone.
fn unit_left_on_main(e: &Env, n: u64, unit: &str, pr: u64) -> PathBuf {
    unit_left_on_main_as(e, n, unit, pr, "CLOSED", "MERGED")
}

fn unit_left_on_main_as(
    e: &Env,
    n: u64,
    unit: &str,
    pr: u64,
    issue_state: &str,
    pr_state: &str,
) -> PathBuf {
    let wt = unit_with_pr(e, n, unit, pr, issue_state, pr_state);
    e.gh_file(&format!("pr-{pr}.ref"), &format!("ns/{unit}"));
    git(&e.root, &["checkout", "-qb", "elsewhere"]);
    git(&wt, &["checkout", "-q", "main"]);
    git(&wt, &["branch", "-q", "-D", &format!("ns/{unit}")]);
    wt
}

#[test]
fn clean_detaches_and_removes_a_merged_worktree_left_on_main() {
    let e = Env::new();
    let wt = unit_left_on_main(&e, 3, "3-done", 30);
    let v = e.run(&["clean", "--dry-run"], 0);
    assert_eq!(units_in(&v["planned"]), ["3-done"], "{v}");
    assert_eq!(v["planned"][0]["detached"], "main", "{v}");
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert!(!wt.exists());
    let ev = &cleanup_events(&e)[0];
    assert_eq!(
        (ev["removed"].as_bool(), ev["detached"].as_str()),
        (Some(true), Some("main")),
        "{ev}"
    );
    assert_eq!(quality_records(&e)[0]["unit"], "3-done");
    git(&e.root, &["checkout", "-q", "main"]);
}

#[test]
fn clean_removes_the_tmpdirs_of_a_merged_worktree_left_on_main() {
    let e = Env::new();
    unit_left_on_main(&e, 3, "3-done", 30);
    fs::create_dir_all(unit_tmp(&e, "3-done").join("build-1")).unwrap();
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert!(!unit_tmp(&e, "3-done").exists());
    git(&e.root, &["checkout", "-q", "main"]);
}

#[test]
fn clean_keeps_a_worktree_on_main_whose_unit_is_not_proven_merged() {
    // Only one worktree can hold main, so each case gets its own repo.
    let cases = [
        ("3-open-issue", "OPEN", "MERGED", None, "issue #3 is open"),
        ("3-open-pr", "CLOSED", "OPEN", None, "PR #30 is OPEN"),
        (
            "3-elsewhere",
            "CLOSED",
            "MERGED",
            Some("someone/branch"),
            "PR #30 is from branch \"someone/branch\", not ns/3-elsewhere",
        ),
    ];
    for (unit, issue_state, pr_state, head_ref, reason) in cases {
        let e = Env::new();
        let wt = unit_left_on_main_as(&e, 3, unit, 30, issue_state, pr_state);
        if let Some(r) = head_ref {
            e.gh_file("pr-30.ref", r);
        }
        let head = git(&e.root, &["rev-parse", "HEAD"]);
        let v = e.run(&["clean"], 0);
        assert_eq!(reasons(&v)[unit], reason, "{v}");
        assert_eq!(units_in(&v["removed"]), Vec::<String>::new(), "{v}");
        assert!(wt.is_dir(), "{unit}");
        assert_eq!(git(&wt, &["symbolic-ref", "HEAD"]), "refs/heads/main");
        assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), head);
    }
}

#[test]
fn clean_keeps_a_worktree_on_main_with_uncommitted_tracked_changes() {
    let e = Env::new();
    let wt = unit_left_on_main(&e, 3, "3-done", 30);
    fs::write(wt.join("README"), "edited\n").unwrap();
    let v = e.run(&["clean"], 0);
    assert_eq!(
        reasons(&v)["3-done"],
        "uncommitted changes to tracked files (1 paths)",
        "{v}"
    );
    assert_eq!(git(&wt, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(fs::read_to_string(wt.join("README")).unwrap(), "edited\n");
}

#[test]
fn clean_leaves_a_worktree_on_main_outside_the_unit_worktree_dir_alone() {
    let e = Env::new();
    git(&e.root, &["checkout", "-qb", "elsewhere"]);
    let other = e.base.join("3-done");
    git(
        &e.root,
        &["worktree", "add", "-q", other.to_str().unwrap(), "main"],
    );
    e.issue(3, "3-done", "CLOSED");
    let v = e.run(&["clean"], 0);
    assert_eq!(v["status"], "nothing-to-clean", "{v}");
    assert_eq!(v["kept"], serde_json::json!([]), "{v}");
    assert!(other.is_dir());
}

#[test]
fn clean_dry_run_lists_the_plan_and_changes_nothing() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    unit_with_pr(&e, 4, "4-open", 40, "OPEN", "MERGED");
    let v = e.run(&["clean", "--dry-run"], 0);
    assert_eq!(
        (v["dry_run"].as_bool(), v["status"].as_str()),
        (Some(true), Some("planned")),
        "{v}"
    );
    assert_eq!(units_in(&v["planned"]), ["3-done"], "{v}");
    assert_eq!(reasons(&v)["4-open"], "issue #4 is open");
    assert!(v.get("removed").is_none(), "{v}");
    assert!(done.join(".ns/3-done/pr.md").is_file());
    assert!(branch_exists(&e, "3-done"));
    assert!(quality_records(&e).is_empty());
    assert!(run_events(&e).is_empty(), "{:?}", run_events(&e));
}

#[test]
fn clean_keeps_a_unit_a_live_run_holds_and_removes_the_rest() {
    let e = Env::new();
    let held = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let free = unit_with_pr(&e, 4, "4-done", 40, "CLOSED", "MERGED");
    let _held = hold(
        &e.root.join(".git/ns-run-3-done.lock"),
        r#"{"pid":42,"unit":"3-done","issue":3}"#,
    );
    let want = "a live ns run (pid 42) holds its run lock";
    let v = e.run(&["clean", "--dry-run"], 0);
    assert_eq!(units_in(&v["planned"]), ["4-done"], "{v}");
    assert_eq!(reasons(&v)["3-done"], want, "{v}");
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["4-done"], "{v}");
    assert_eq!(reasons(&v)["3-done"], want, "{v}");
    // Only the unit that goes is recorded.
    let units: Vec<_> = quality_records(&e)
        .iter()
        .map(|r| r["unit"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(units, ["4-done"]);
    assert!(held.is_dir());
    assert!(branch_exists(&e, "3-done"));
    assert!(!free.exists());
}

#[test]
fn clean_keeps_a_unit_whose_run_starts_after_the_plan() {
    let e = Env::new();
    let a = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let b = unit_with_pr(&e, 4, "4-done", 40, "CLOSED", "MERGED");
    // Reading one unit's PR starts a run on the other: whichever is read second was planned
    // with its lock free, and must find it taken when it is checked again. The run starts in a
    // session of its own, as a real one does, so it outlives gh's process group.
    let lock = |unit: &str| e.root.join(format!(".git/ns-run-{unit}.lock"));
    let start = |unit: &str| {
        format!(
            "( setsid flock {0} sleep 30 >/dev/null 2>&1 & ); until ! flock -n {0} true; do sleep 0.01; done",
            lock(unit).display()
        )
    };
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$*\" in\n  \"pr view 30 \"*) {} ;;\n  \"pr view 40 \"*) {} ;;\nesac\n",
            start("4-done"),
            start("3-done")
        ),
    );
    let v = e.run(&["clean"], 0);
    let r = reasons(&v);
    let want = "a live ns run (pid ?) holds its run lock";
    assert_eq!(
        (r["3-done"].as_str(), r["4-done"].as_str()),
        (want, want),
        "{v}"
    );
    // One of them got past the plan and was refused at the run lock.
    let refused = cleanup_events(&e);
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0]["reason"], want);
    // The lock is taken before the record: neither unit is recorded.
    assert!(quality_records(&e).is_empty());
    assert!(a.is_dir() && b.is_dir());
}

#[test]
fn clean_names_another_cleanup_and_a_run_lock_it_cannot_take() {
    let e = Env::new();
    let a = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let b = unit_with_pr(&e, 4, "4-done", 40, "CLOSED", "MERGED");
    let _other = hold(
        &e.root.join(".git/ns-run-3-done.lock"),
        r#"{"pid":42,"unit":"3-done","issue":3,"by":"watch"}"#,
    );
    fs::create_dir(e.root.join(".git/ns-run-4-done.lock")).unwrap();
    let v = e.run(&["clean"], 0);
    let r = reasons(&v);
    assert_eq!(r["3-done"], "ns watch (pid 42) is cleaning it up", "{v}");
    assert!(r["4-done"].starts_with("cannot take its run lock: "), "{v}");
    assert!(a.is_dir() && b.is_dir());
    assert!(quality_records(&e).is_empty());
}

#[test]
fn clean_with_the_record_in_the_outbox_still_removes() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    decline_quality_pushes(&e);
    let v = e.run(&["clean"], 0);
    assert_eq!(v["records"]["status"], "outbox", "{v}");
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert!(!done.exists());
    let outbox = fs::read_to_string(e.root.join(".git/ns/quality-outbox.jsonl")).unwrap();
    assert!(outbox.contains("\"unit\":\"3-done\""), "{outbox}");
}

#[test]
fn clean_keeps_a_unit_whose_record_is_not_saved() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    decline_quality_pushes(&e);
    break_outbox(&e);
    let v = e.run(&["clean"], 0);
    assert!(v["records"]["error"].is_string(), "{v}");
    let r = reasons(&v);
    assert!(
        r["3-done"].starts_with("its quality record is not saved: "),
        "{v}"
    );
    assert!(done.join(".ns/3-done/review.md").is_file());
    assert!(branch_exists(&e, "3-done"));
}

#[test]
fn clean_fails_when_a_removal_fails() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    // A locked worktree refuses a single --force.
    git(&e.root, &["worktree", "lock", done.to_str().unwrap()]);
    let v = e.run(&["clean"], 1);
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["kept"][0]["error"], true, "{v}");
    assert!(
        reasons(&v)["3-done"].starts_with("worktree not removed: "),
        "{v}"
    );
    assert!(done.is_dir());
}

#[test]
fn watch_cleans_a_unit_a_human_merged_at_start_and_keeps_open_ones() {
    let e = Env::new();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    // A stuck unit's issue stays open: its worktree stays.
    let stuck = unit_with_pr(&e, 4, "4-stuck", 40, "OPEN", "OPEN");

    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["clean"], serde_json::json!([]), "{v}");

    // A human merges PR 12, which closes #7.
    e.issue(7, "Fix the thing", "CLOSED");
    e.gh_file("pr-12.state", "MERGED");
    e.gh_file("pr-12.body", "Closes #7");
    let v = e.run(&["watch", "--dry-run"], 0);
    assert_eq!(v["clean"], serde_json::json!([UNIT]), "{v}");
    assert!(e.worktree(UNIT).is_dir());

    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["cleaned"], serde_json::json!([UNIT]), "{v}");
    assert!(!e.worktree(UNIT).exists());
    assert!(!branch_exists(&e, UNIT));
    assert!(stuck.is_dir());
    // The record is written again with the merge as its outcome.
    let last = quality_records(&e).pop().unwrap();
    assert_eq!(
        (last["unit"].as_str(), last["outcome"].as_str()),
        (Some(UNIT), Some("merged"))
    );
    assert_eq!(cleanup_events(&e)[0]["by"], "watch");
}

#[test]
fn watch_cleans_a_unit_merged_while_another_unit_ran() {
    let e = Env::new();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    e.ctl("pr", "13");
    e.ready(2, "Fix a", &["type:fix"], "");
    // A human merges PR 12 while unit 2 runs.
    e.gh_file(
        "hook.sh",
        "if [ \"$1 $2\" = \"issue edit\" ] && [ \"$3\" = 2 ]; then\n  sed -i 's/\"OPEN\"/\"CLOSED\"/' \"$d/issue-7.json\"\n  echo MERGED > \"$d/pr-12.state\"\n  echo 'Closes #7' > \"$d/pr-12.body\"\nfi\n",
    );
    let v = e.run(&["watch", "--once"], 0);
    assert_eq!(v["units"][0]["issue"], 2, "{v}");
    assert_eq!(v["cleaned"], serde_json::json!([UNIT]), "{v}");
    assert!(!e.worktree(UNIT).exists());
    let unit2_ended = event_at(&e, |v| v["event"] == "end" && v["unit"] == "2-fix-a");
    let cleaned = event_at(&e, |v| v["event"] == "cleanup" && v["unit"] == UNIT);
    assert!(unit2_ended < cleaned);
}

#[test]
fn auto_merge_keeps_the_worktree_when_the_merged_head_cannot_be_read() {
    let e = auto_merge_env();
    // Only once merged: the guard after ship reads the same fields while the PR is open.
    e.gh_file(
        "hook.sh",
        "[ \"$*\" = \"pr view 12 --json state,headRefOid,headRefName,body\" ] && grep -q MERGED \"$d/pr-12.state\" 2>/dev/null && { echo 'HTTP 502' >&2; exit 1; }\n",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let reason = v["cleanup"]["reason"].as_str().unwrap();
    assert!(reason.starts_with("cannot read PR #12: "), "{v}");
    assert!(e.worktree(UNIT).is_dir());
}

#[test]
fn clean_keeps_the_worktree_it_runs_from() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let out = e.ns().current_dir(&done).args(["clean"]).output();
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        reasons(&v)["3-done"],
        "the current directory is inside it",
        "{v}"
    );
    assert!(done.is_dir());
}

#[test]
fn clean_keeps_the_main_checkout_and_a_worktree_git_cannot_read() {
    let e = Env::new();
    // The main checkout on a merged unit's branch.
    git(&e.root, &["checkout", "-qb", "ns/15-main"]);
    let art = e.root.join(".ns/15-main");
    fs::create_dir_all(&art).unwrap();
    fs::write(art.join("pr.md"), "---\nstatus: pass\npr: 150\n---\n").unwrap();
    e.issue(15, "main", "CLOSED");
    e.gh_file("pr-150.state", "MERGED");
    e.gh_file("pr-150.body", "Closes #15");
    // A worktree whose .git link is broken.
    let broken = unit_with_pr(&e, 16, "16-broken", 160, "CLOSED", "MERGED");
    let head = git(&broken, &["rev-parse", "HEAD"]);
    e.gh_file("pr-160.head", &head);
    e.gh_file("pr-160.ref", "ns/16-broken");
    fs::write(broken.join(".git"), "gitdir: /nonexistent\n").unwrap();

    let v = e.run(&["clean"], 0);
    let r = reasons(&v);
    assert_eq!(r["15-main"], "the main checkout is on its branch", "{v}");
    assert!(
        r["16-broken"].starts_with("cannot read its status: "),
        "{v}"
    );
    assert!(art.join("pr.md").is_file());
    assert!(broken.is_dir());
}

#[test]
fn clean_reports_a_branch_it_could_not_delete() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    fs::write(e.root.join(".git/refs/heads/ns/3-done.lock"), "").unwrap();
    fs::create_dir_all(unit_tmp(&e, "3-done").join("build-1")).unwrap();
    let v = e.run(&["clean"], 1);
    let reason = &reasons(&v)["3-done"];
    assert!(
        reason.starts_with("worktree removed, branch ns/3-done kept: "),
        "{v}"
    );
    assert!(!done.exists());
    assert!(branch_exists(&e, "3-done"));
    assert!(
        !unit_tmp(&e, "3-done").exists(),
        "a failed branch deletion kept it"
    );
}

#[test]
fn clean_reports_a_tmpdir_it_could_not_remove() {
    let e = Env::new();
    let done = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    let tmp = unit_tmp(&e, "3-done");
    fs::write(&tmp, "a file, not a directory\n").unwrap();
    let v = e.run(&["clean"], 1);
    let reason = &reasons(&v)["3-done"];
    let kept = format!("worktree removed, {} kept: ", tmp.display());
    assert!(reason.starts_with(&kept), "{v}");
    assert!(!done.exists());
    assert!(!branch_exists(&e, "3-done"));
}

#[test]
fn auto_merge_in_the_main_checkout_keeps_it() {
    let e = auto_merge_env();
    git(&e.root, &["checkout", "-qb", &format!("ns/{UNIT}")]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["worktree"], e.root.to_str().unwrap(), "{v}");
    assert_eq!(
        v["cleanup"]["reason"], "the main checkout is on its branch",
        "{v}"
    );
    assert!(e.root.join(format!(".ns/{UNIT}/pr.md")).is_file());
    assert!(branch_exists(&e, UNIT));
}

#[test]
fn auto_merge_keeps_a_worktree_whose_head_left_the_branch() {
    // Detached, or on another branch at the same commit: what HEAD holds is not the branch's.
    for leave in ["git checkout -q --detach", "git checkout -qb elsewhere"] {
        let e = auto_merge_env();
        e.queue("ship", &["pass:script"]);
        e.ctl("ship.sh", &format!("{leave}\n"));
        let v = e.run(&["run", "--issue", "7"], 0);
        assert_eq!(v["outcome"], "merged", "{leave}: {v}");
        assert_eq!(
            v["cleanup"]["reason"],
            format!("its HEAD is not on ns/{UNIT}"),
            "{leave}: {v}"
        );
        assert!(e.worktree(UNIT).is_dir(), "{leave}");
        assert!(branch_exists(&e, UNIT), "{leave}");
    }
}
