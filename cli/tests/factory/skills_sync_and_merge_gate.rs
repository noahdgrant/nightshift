#[test]
fn a_signal_while_draining_for_skills_ends_the_night_without_a_sync() {
    let e = Env::new();
    codex_night(&e);
    skills_installed(&e);
    let from = git(&e.root, &["rev-parse", "HEAD"]);
    two_ready(&e);
    let (in_a, go_a) = hold_phase(&e, A, "build");
    let (in_b, go_b) = hold_phase(&e, B, "build");
    let mut w = Watching::start(e.ns().args(["watch", "--parallel", "2"]));
    w.entered(&in_a);
    w.entered(&in_b);
    push_from_elsewhere(&e, "skills/ns-a/SKILL.md");
    release(&go_b);
    w.line("draining");
    w.signal(libc::SIGTERM, false);
    release(&go_a);
    let (code, v, err) = w.finish();
    assert_eq!(code, Some(128 + libc::SIGTERM), "{err}");
    assert_eq!(v["stopped"], "SIGTERM", "{v}");
    assert_eq!(git(&e.root, &["rev-parse", "HEAD"]), from);
    assert!(events_named(&e, "skills_synced").is_empty(), "{err}");
}

/// `(phase, green)` of each `gate` event in the run log.
fn gate_verdicts(e: &Env) -> Vec<(String, bool)> {
    gate_events(e)
        .iter()
        .map(|g| {
            (
                g["phase"].as_str().unwrap().to_string(),
                g["green"].as_bool().unwrap(),
            )
        })
        .collect()
}

fn verdicts(v: &[(&str, bool)]) -> Vec<(String, bool)> {
    v.iter().map(|(p, g)| (p.to_string(), *g)).collect()
}

const COMMIT: &str = "git -c user.name=f -c user.email=f@f commit -qm";

#[test]
fn a_red_gate_after_a_verify_commit_goes_back_to_build_before_review() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"test ! -f broken\"\n");
    e.ctl(
        "verify.sh",
        &format!("touch broken && git add broken && {COMMIT} break"),
    );
    e.ctl("build.sh", &format!("git rm -q broken && {COMMIT} fix"));
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("verify", &["pass:script", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(4, "build").contains("test ! -f broken"));
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("verify", false), ("build", true)])
    );
}

#[test]
fn a_ship_base_merge_that_breaks_the_gate_goes_back_to_build_before_the_merge_step() {
    let e = Env::new();
    e.factory(&format!(
        "{AUTO}[phases.build]\ngate = \"test ! -f broken\"\n"
    ));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("ship", &["pass:script", "pass:script"]);
    e.ctl(
        "ship.sh",
        r#"set -e
c="git -c user.name=f -c user.email=f@f"
o="$FAKE_CTRL/other"
if [ ! -d "$o" ]; then
  git clone -q "$(git remote get-url origin)" "$o"
  (cd "$o" && touch broken && git add broken && $c commit -qm 'Break b (#3)' && git push -q origin main &&
    printf '%s' '[{"number":3,"merged_at":"2026-10-09T00:00:00Z","head":{"ref":"ns/3-fix-b"}}]' > "$FAKE_GH_DIR/pulls-$(git rev-parse HEAD).json")
  git fetch -q origin main
  $c merge -q --no-edit FETCH_HEAD
fi
git push -q -f origin HEAD"#,
    );
    e.ctl("build.sh", &format!("git rm -q broken && {COMMIT} fix"));
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "build", "verify", "review", "ship"]
    );
    assert!(e.prompt(6, "build").contains("test ! -f broken"));
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("ship", false), ("build", true)])
    );
}

/// Ship pushes the branch; GitHub reports the PR `BEHIND`, and its update-branch lands `updated`,
/// a commit on top of the branch that adds `broken`, unless the PR is already up to date. The
/// factory turns the local merge queue off, so these tests pin today's merge path, and adds
/// `build` to `[phases.build]`.
fn behind_with_a_broken_update(e: &Env, build: &str) {
    e.factory(&format!("{AUTO}{LEGACY}[phases.build]\n{build}"));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("ship", &["pass:script", "pass:script"]);
    e.ctl(
        "ship.sh",
        r#"set -e
c="git -c user.name=f -c user.email=f@f"
git push -q -f origin HEAD
o="$FAKE_CTRL/other"
if [ ! -d "$o" ]; then
  b=$(git symbolic-ref --short HEAD)
  git clone -q -b "$b" "$(git remote get-url origin)" "$o"
  (cd "$o" && touch broken && git add broken && $c commit -qm 'merge main' && git push -q origin "$b")
  git -C "$o" rev-parse HEAD > "$FAKE_GH_DIR/updated-12.head"
  echo BEHIND > "$FAKE_GH_DIR/pr-12.merge"
else
  rm -f "$FAKE_GH_DIR/pr-12.head"
fi"#,
    );
    e.ctl("build.sh", &format!("git rm -q broken && {COMMIT} fix"));
    e.gh_file("checks-12.json", GREEN);
}

#[test]
fn an_update_branch_that_breaks_the_gate_goes_back_to_build_before_merging() {
    let e = Env::new();
    behind_with_a_broken_update(&e, "gate = \"test ! -f broken\"\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "build", "verify", "review", "ship"]
    );
    let p = e.prompt(6, "build");
    assert!(p.contains("test ! -f broken"), "{p}");
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", false), ("build", true)])
    );
    let calls = e.gh_calls();
    assert_eq!(calls.matches("pr merge 12").count(), 1, "{calls}");
    let red = &gate_events(&e)[1];
    let other = git(&e.ctrl.join("other"), &["rev-parse", "--short", "HEAD"]);
    assert_eq!(red["sha"].as_str(), Some(other.as_str()), "{red}");
    assert_eq!(red["attempt"], 1, "{red}");
}

#[test]
fn a_second_update_branch_gates_the_merge_head_again_at_the_next_attempt() {
    let e = Env::new();
    e.factory(&format!(
        "{AUTO}{LEGACY}[phases.build]\nmax_attempts = 3\ngate = \"test ! -f broken\"\n"
    ));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:script"]);
    e.queue("ship", &["pass:script", "pass:script", "pass:script"]);
    e.ctl(
        "ship.sh",
        r#"set -e
c="git -c user.name=f -c user.email=f@f"
rm -f "$FAKE_GH_DIR/pr-12.head"
git push -q -f origin HEAD
n=$(cat "$FAKE_CTRL/ships" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "$FAKE_CTRL/ships"
if [ "$n" -le 2 ]; then
  b=$(git symbolic-ref --short HEAD)
  o="$FAKE_CTRL/other$n"
  git clone -q -b "$b" "$(git remote get-url origin)" "$o"
  f=broken
  [ "$n" -eq 2 ] && f=fine
  (cd "$o" && touch "$f" && git add "$f" && $c commit -qm "merge main $n" && git push -q origin "$b")
  git -C "$o" rev-parse HEAD > "$FAKE_GH_DIR/updated-12.head"
  echo BEHIND > "$FAKE_GH_DIR/pr-12.merge"
else
  rm -f "$FAKE_GH_DIR/pr-12.head"
fi"#,
    );
    e.ctl("build.sh", &format!("git rm -q broken && {COMMIT} fix"));
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[
            ("build", true),
            ("merge", false),
            ("build", true),
            ("merge", true)
        ])
    );
    let merges: Vec<_> = gate_events(&e)
        .into_iter()
        .filter(|g| g["phase"] == "merge")
        .map(|g| g["attempt"].clone())
        .collect();
    assert_eq!(merges, [1, 2]);
}

#[test]
fn an_update_branch_head_that_cannot_be_checked_out_needs_a_human_merge() {
    let e = Env::new();
    e.factory(&format!("{AUTO}{LEGACY}[phases.build]\ngate = \"true\"\n"));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file(
        "updated-12.head",
        "0123456789abcdef0123456789abcdef01234567",
    );
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.contains("cannot check out PR #12 head 0123456789abcdef0123456789abcdef01234567"),
        "{reason}"
    );
    assert!(
        reason.contains("git fetch origin ns/7-fix-the-thing failed"),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
}

#[test]
fn a_green_update_branch_head_is_gated_and_merged_at_that_head() {
    let e = Env::new();
    behind_with_a_broken_update(&e, "gate = \"true\"\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", true)])
    );
    let updated = git(&e.ctrl.join("other"), &["rev-parse", "HEAD"]);
    let merge = &gate_events(&e)[1];
    assert!(
        updated.starts_with(merge["sha"].as_str().unwrap()),
        "{merge}"
    );
    let want = format!("--match-head-commit {updated}");
    assert!(e.gh_calls().contains(&want), "{}", e.gh_calls());
}

#[test]
fn a_rebased_pr_head_is_gated_at_that_head_and_merged() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.factory(&format!("{AUTO}[phases.build]\ngate = \"true\"\n"));
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let old = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    let old = old.trim();
    let merge = gate_events(&e)
        .into_iter()
        .find(|g| g["phase"] == "merge")
        .expect("merge gate");
    assert_eq!(merge["green"], true, "{merge}");
    assert!(old.starts_with(merge["sha"].as_str().unwrap()), "{merge}");
    let want = format!("--match-head-commit {old}");
    assert!(e.gh_calls().contains(&want), "{}", e.gh_calls());
}

#[test]
fn an_update_branch_head_missing_after_a_good_fetch_needs_a_human_merge() {
    let e = Env::new();
    e.factory(&format!("{AUTO}{LEGACY}[phases.build]\ngate = \"true\"\n"));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:script"]);
    e.ctl("ship.sh", "git push -q -f origin HEAD");
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file(
        "updated-12.head",
        "0123456789abcdef0123456789abcdef01234567",
    );
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("cannot check out PR #12 head"), "{reason}");
    assert!(!reason.contains("git fetch"), "{reason}");
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
}

#[test]
fn an_unmoved_head_does_not_rerun_the_gate_through_ship_and_the_merge_step() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    e.factory(&format!(
        "{AUTO}[phases.build]\ngate = \"echo ran >> {}\"\n",
        log.display()
    ));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);
}

#[test]
fn a_gate_killed_at_the_merge_step_says_so_when_build_runs_out() {
    let e = Env::new();
    behind_with_a_broken_update(
        &e,
        "max_attempts = 1\ngate = \"test ! -f broken || kill -9 $$\"\n",
    );
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"], "build is out of attempts (1): the CI gate was killed by SIGKILL",
        "{v}"
    );
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", false)])
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_stop_during_the_merge_gate_merges_nothing() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    let gate = format!("test ! -f broken || {{ {}; }}", blocks(&f));
    behind_with_a_broken_update(&e, &format!("gate = {gate:?}\n"));
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "gate"
        && ev["phase"] == "merge"
        && ev["attempt"] == 1
        && ev["interrupted"] == "SIGTERM"));
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_failed_build_that_commits_does_not_run_the_gate() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    e.factory(&format!(
        "[phases.build]\ngate = \"echo ran >> {}\"\n",
        log.display()
    ));
    e.queue("build", &["fail:commit", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "build", "verify", "review", "ship"]
    );
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
    assert_eq!(gate_events(&e)[0]["attempt"], 2);
}

#[test]
fn a_timed_out_verify_that_moved_head_runs_the_gate() {
    let e = Env::new();
    e.factory("[phases.build]\ngate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["blocked:commit+sleep", "pass"]);
    let v = run_with_timeouts(&e, "verify=1500", 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("verify", true)])
    );
}

/// Land a commit on `origin/main` that adds `file` holding `text`, from a clone under
/// `FAKE_CTRL`: the main a unit's PR merges into has moved on since the unit branched.
fn main_moves(e: &Env, file: &str, text: &str) -> String {
    let o = e.ctrl.join(format!("main-{file}"));
    let remote = e.base.join("remote.git");
    git(
        &e.base,
        &["clone", "-q", remote.to_str().unwrap(), o.to_str().unwrap()],
    );
    fs::write(o.join(file), text).unwrap();
    git(&o, &["add", file]);
    git(&o, &["commit", "-qm", &format!("add {file} on main")]);
    git(&o, &["push", "-q", "origin", "main"]);
    git(&o, &["rev-parse", "HEAD"])
}

/// The scratch worktrees of the local merge queue, as `git worktree list` and the disk see them.
fn merge_queue_leftovers(e: &Env) -> Vec<String> {
    let mut left: Vec<String> = git(&e.root, &["worktree", "list", "--porcelain"])
        .lines()
        .filter(|l| l.contains("merge-queue"))
        .map(String::from)
        .collect();
    if let Ok(dir) = fs::read_dir(e.root.join(".git/ns/merge-queue")) {
        left.extend(dir.map(|d| d.unwrap().path().display().to_string()));
    }
    left
}

fn merge_gate_failures(e: &Env) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|ev| ev["event"] == "merge_gate_failed")
        .collect()
}

/// `ns run` with the local queue on (the default) and `gate` as the build gate.
fn local_queue(e: &Env, build: &str) {
    e.factory(&format!("{AUTO}[phases.build]\n{build}"));
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
}

/// A gate that appends `merged` to `log` when the tree holds main's `from-main`, else `branch`.
fn where_gate(log: &Path) -> String {
    format!(
        "gate = \"if test -f from-main; then echo merged >> {0}; else echo branch >> {0}; fi\"\n",
        log.display()
    )
}

#[test]
fn the_local_queue_gates_the_merge_with_main_and_merges_without_update_branch() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit"]);
    let base = main_moves(&e, "from-main", "x\n");
    e.gh_file("pr-12.merge", "BEHIND");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\nmerged\n");
    let calls = e.gh_calls();
    assert!(!calls.contains("pr update-branch"), "{calls}");
    let merge = gate_events(&e)
        .into_iter()
        .find(|g| g["phase"] == "merge")
        .expect("merge gate");
    assert_eq!(merge["merge_result"], true, "{merge}");
    assert_eq!(merge["base"].as_str(), Some(base.as_str()), "{merge}");
    assert!(merge["wall_s"].is_number(), "{merge}");
    let head = merge["sha"].as_str().unwrap();
    assert!(
        calls.contains(&format!("--match-head-commit {head}")),
        "{calls}"
    );
    assert!(merge_gate_failures(&e).is_empty());
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn every_step_keeps_the_local_queue_and_sends_a_conflict_back_to_build() {
    let every_step = |e: &Env, log: &Path| {
        e.factory(&format!(
            "{AUTO}[gate]\nlocal = \"every-step\"\n[phases.build]\n{}",
            where_gate(log)
        ));
        e.ctl("pr", "12");
        e.gh_file("checks-12.json", GREEN);
    };
    let e = Env::new();
    let log = e.ctrl.join("gates");
    every_step(&e, &log);
    e.queue("build", &["pass:commit"]);
    main_moves(&e, "from-main", "x\n");
    e.gh_file("pr-12.merge", "BEHIND");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\nmerged\n");
    assert!(!e.gh_calls().contains("pr update-branch"));

    let e = Env::new();
    every_step(&e, &e.ctrl.join("gates"));
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("pr-12.merge", "DIRTY");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    // The first conflict's rebuild is free; the next, with main unmoved, uses build's second
    // attempt.
    assert_eq!(
        e.calls(),
        [
            "triage", "build", "verify", "review", "ship", "build", "verify", "review", "ship",
            "build"
        ]
    );
    assert!(e.prompt(6, "build").contains("PR #12 conflicts with main"));
    assert_eq!(rebase_events(&e).len(), 1);
}

#[test]
fn a_main_the_pr_head_already_holds_is_gated_as_the_head_without_a_scratch_worktree() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\n");
    let gates = gate_events(&e);
    assert!(
        gates.iter().all(|g| g.get("merge_result").is_none()),
        "{gates:?}"
    );
    assert!(e.gh_calls().contains("pr merge"));
    assert!(merge_gate_failures(&e).is_empty());
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_main_that_cannot_merge_with_the_pr_head_needs_a_human_merge() {
    let e = Env::new();
    local_queue(&e, "gate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    let o = e.ctrl.join("orphan");
    let remote = e.base.join("remote.git");
    git(
        &e.base,
        &["clone", "-q", remote.to_str().unwrap(), o.to_str().unwrap()],
    );
    git(&o, &["checkout", "-q", "--orphan", "other"]);
    fs::write(o.join("unrelated"), "x\n").unwrap();
    git(&o, &["add", "unrelated"]);
    git(&o, &["commit", "-qm", "unrelated root"]);
    git(&o, &["push", "-qf", "origin", "other:main"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("cannot merge PR #12 into main for the local merge queue: git merge"),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_red_merge_result_goes_back_to_build_and_merges_nothing() {
    let e = Env::new();
    local_queue(&e, "gate = \"test ! -f broken\"\n");
    e.queue("build", &["pass:commit", "fail"]);
    let base = main_moves(&e, "broken", "x\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "build"]
    );
    let p = e.prompt(6, "build");
    assert!(p.contains("test ! -f broken"), "{p}");
    assert!(p.contains("combining it with main"), "{p}");
    assert_eq!(
        v["phases"][5]["decision"], "the CI gate failed on the merge with main",
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", false)])
    );
    // The failed rebuild leaves the PR head and main where they were, so the merge step that
    // follows reuses the red verdict.
    let failed = merge_gate_failures(&e);
    let reused: Vec<_> = failed.iter().map(|f| f["reused"].clone()).collect();
    assert_eq!(reused, [false, true], "{failed:?}");
    assert_eq!(failed[0]["pr"], 12);
    assert_eq!(failed[0]["base"].as_str(), Some(base.as_str()));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_merge_result_that_conflicts_goes_back_to_build_to_rebase() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit", "fail"]);
    main_moves(&e, "work.txt", "main's own line\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let p = e.prompt(6, "build");
    assert!(p.contains("PR #12 conflicts with main"), "{p}");
    assert!(p.contains("rebase onto main"), "{p}");
    assert!(p.contains("rebase onto the newest origin/main"), "{p}");
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "conflict"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

/// CI fails on the first merge step and build passes without a commit, so the second merge
/// step meets the same PR head. `build.sh` runs at that rebuild.
fn ci_red_then_green_at_one_head(e: &Env, log: &Path) {
    local_queue(e, &where_gate(log));
    e.queue("build", &["pass:commit", "pass:script"]);
    e.gh_file("checks-12-1.json", RED);
    e.gh_file("checks-12-2.json", GREEN);
}

fn merge_gate_bases(e: &Env) -> Vec<String> {
    gate_events(e)
        .into_iter()
        .filter(|g| g["phase"] == "merge")
        .map(|g| g["base"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn an_unmoved_pr_head_and_base_reuse_the_merge_result_verdict() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    ci_red_then_green_at_one_head(&e, &log);
    e.ctl("build.sh", "true");
    let base = main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(merge_gate_bases(&e), [base]);
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\nmerged\n");
}

#[test]
fn a_moved_base_runs_the_merge_result_gate_again() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    ci_red_then_green_at_one_head(&e, &log);
    let first = main_moves(&e, "from-main", "x\n");
    let o = e.ctrl.join("main-from-main");
    e.ctl(
        "build.sh",
        &format!(
            "cd '{}' && echo y > later && git add later && {COMMIT} later && git push -q origin main",
            o.display()
        ),
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let second = git(&o, &["rev-parse", "HEAD"]);
    assert_ne!(first, second);
    assert_eq!(merge_gate_bases(&e), [first, second]);
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_stop_during_the_merge_result_gate_merges_nothing_and_removes_the_scratch_worktree() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    let gate = format!("case $PWD in *merge-queue*) {};; esac", blocks(&f));
    local_queue(&e, &format!("gate = {gate:?}\n"));
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:script"]);
    e.ctl(
        "ship.sh",
        &format!(
            "set -e\no=\"$FAKE_CTRL/main\"\ngit clone -q \"$(git remote get-url origin)\" \"$o\"\ncd \"$o\" && touch from-main && git add from-main && {COMMIT} main && git push -q origin main"
        ),
    );
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "gate"
        && ev["phase"] == "merge"
        && ev["interrupted"] == "SIGTERM"));
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_failed_fetch_of_main_for_the_local_queue_needs_a_human_merge() {
    let e = Env::new();
    local_queue(&e, "gate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:script"]);
    e.ctl(
        "ship.sh",
        "git remote set-url origin \"$FAKE_CTRL/gone.git\"",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with(
            "cannot fetch main for the local merge queue: git fetch origin main failed"
        ),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_scratch_worktree_that_cannot_be_added_needs_a_human_merge() {
    let e = Env::new();
    local_queue(&e, "gate = \"true\"\n");
    e.queue("build", &["pass:commit"]);
    main_moves(&e, "from-main", "x\n");
    fs::create_dir_all(e.root.join(".git/ns")).unwrap();
    fs::write(e.root.join(".git/ns/merge-queue"), "in the way").unwrap();
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("cannot add a worktree for the local merge queue: cannot create"),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(gate_verdicts(&e), verdicts(&[("build", true)]));
}

#[test]
fn a_ratchet_over_the_line_on_the_merge_result_is_the_stuck_reason() {
    let e = Env::new();
    local_queue(
        &e,
        "\n[ratchets.flash]\ncommand = \"case $PWD in *merge-queue*) echo 200;; *) echo 100;; esac\"\nbetter = \"lower\"\n",
    );
    fs::write(
        e.root.join(".nightshift/ratchets.json"),
        r#"{"version": 1, "values": {"flash": 100}}"#,
    )
    .unwrap();
    e.queue("build", &["pass:commit", "fail"]);
    main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["reason"],
        "build is out of attempts (2): the CI gate failed on ratchet flash is worse: measured 200, recorded 100, tolerance 0, limit 100, lower is better",
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_gate_killed_on_the_merge_result_says_so_when_build_runs_out() {
    let e = Env::new();
    local_queue(
        &e,
        "gate = \"case $PWD in *merge-queue*) kill -9 $$;; esac\"\n",
    );
    e.queue("build", &["pass:commit", "fail"]);
    main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(
        v["phases"][5]["decision"], "the CI gate was killed by SIGKILL on the merge with main",
        "{v}"
    );
    assert_eq!(
        v["reason"], "build is out of attempts (2): the CI gate was killed by SIGKILL",
        "{v}"
    );
    assert_eq!(
        gate_verdicts(&e),
        verdicts(&[("build", true), ("merge", false), ("merge", false)])
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_moved_pr_head_runs_the_merge_result_gate_again() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12-1.json", RED);
    e.gh_file("checks-12-2.json", GREEN);
    let base = main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let merges: Vec<_> = gate_events(&e)
        .into_iter()
        .filter(|g| g["phase"] == "merge")
        .collect();
    assert_eq!(merges.len(), 2, "{merges:?}");
    assert_ne!(merges[0]["sha"], merges[1]["sha"]);
    assert_eq!(merge_gate_bases(&e), [base.clone(), base]);
}

#[test]
fn the_queue_leaves_the_worktree_on_the_pr_head_when_a_rebase_moved_head() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.factory(&format!("{AUTO}[phases.build]\ngate = \"true\"\n"));
        main_moves(e, "from-main", "x\n");
        e.gh_file(
            "hook.sh",
            "case \"$1 $2\" in \"pr merge\") git rev-parse HEAD > \"$FAKE_GH_DIR/head-at-merge\";; esac\n",
        );
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let old = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    let at_merge = fs::read_to_string(e.ghd.join("head-at-merge")).unwrap();
    assert_eq!(at_merge.trim(), old.trim());
    assert!(gate_events(&e).iter().any(|g| g["merge_result"] == true));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_moved_main_with_the_queue_off_gates_the_pr_head_only() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    e.factory(&format!(
        "{AUTO}{LEGACY}[phases.build]\n{}",
        where_gate(&log)
    ));
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:commit"]);
    main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "branch\n");
    let gates = gate_events(&e);
    assert!(
        gates.iter().all(|g| g.get("merge_result").is_none()),
        "{gates:?}"
    );
    assert!(merge_gate_failures(&e).is_empty());
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_dirty_pr_under_the_queue_takes_the_conflict_path_without_a_merge_gate() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    local_queue(&e, &where_gate(&log));
    e.queue("build", &["pass:commit", "fail"]);
    main_moves(&e, "from-main", "x\n");
    e.gh_file("pr-12.merge", "DIRTY");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let p = e.prompt(6, "build");
    assert!(p.contains("PR #12 conflicts with main"), "{p}");
    assert!(run_events(&e).iter().any(|ev| ev["event"] == "conflict"));
    assert!(gate_events(&e).iter().all(|g| g["phase"] != "merge"));
    assert!(merge_gate_failures(&e).is_empty());
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_pr_head_that_cannot_be_checked_out_under_the_queue_needs_a_human_merge() {
    let lock_the_index = "touch \"$(git rev-parse --git-dir)/index.lock\"\n";
    let (e, v, _) = main_moves_after_review(
        &["pass:script"],
        PRINT_HEAD_SHA_CMD,
        &format!(
            "git update-ref refs/heads/main upstream\n\
             git -c user.name=f -c user.email=f@f rebase -q main\n{lock_the_index}"
        ),
        |e| {
            e.factory(&format!("{AUTO}[phases.build]\ngate = \"true\"\n"));
        },
    );
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("cannot check out PR #12 head"),
        "{reason}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_killed_merge_gate_runs_again_on_an_unchanged_head_and_base() {
    let e = Env::new();
    let log = e.ctrl.join("gates");
    let marker = e.ctrl.join("killed-once");
    local_queue(
        &e,
        &format!(
            "max_attempts = 3\ngate = \"case $PWD in *merge-queue*) echo ran >> {}; test -f {m} || {{ touch {m}; kill -9 $$; }};; esac\"\n",
            log.display(),
            m = marker.display()
        ),
    );
    e.queue("build", &["pass:commit", "pass"]);
    main_moves(&e, "from-main", "x\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "ran\nran\n");
    assert!(merge_gate_failures(&e).iter().all(|f| f["reused"] == false));
}

/// A merge-result gate that fails once the way `fail_once` says, then passes, logging each run.
fn flaky_merge_gate(e: &Env, limits: &str, fail_once: &str) -> PathBuf {
    let log = e.ctrl.join("gates");
    let marker = e.ctrl.join("failed-once");
    e.factory(&format!(
        "{AUTO}{limits}[phases.build]\nmax_attempts = 3\ngate = \"case $PWD in *merge-queue*) echo ran >> {}; test -f {m} || {{ touch {m}; {fail_once}; }};; esac\"\n",
        log.display(),
        m = marker.display()
    ));
    e.ctl("pr", "12");
    e.gh_file("checks-12.json", GREEN);
    e.queue("build", &["pass:commit", "pass"]);
    main_moves(e, "from-main", "x\n");
    log
}

#[test]
fn a_timed_out_merge_gate_runs_again_on_an_unchanged_head_and_base() {
    let e = Env::new();
    let log = flaky_merge_gate(&e, "", "sleep 30");
    let out = e
        .ns()
        .env("NS_GATE_TIMEOUT_MS", "1500")
        .args(["run", "--issue", "7"])
        .output();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "ran\nran\n");
    assert!(merge_gate_failures(&e).iter().all(|f| f["reused"] == false));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}

#[test]
fn a_merge_gate_past_the_memory_cap_runs_again_on_an_unchanged_head_and_base() {
    let e = Env::new();
    let log = flaky_merge_gate(
        &e,
        "[limits]\nmemory_mb = 64\n",
        "python3 -c 'b = bytes(range(256)) * (1 << 20)'",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "ran\nran\n");
    assert!(merge_gate_failures(&e).iter().all(|f| f["reused"] == false));
    assert_eq!(merge_queue_leftovers(&e), Vec::<String>::new());
}
