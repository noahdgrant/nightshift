#[test]
fn auto_merge_conflict_goes_back_to_build() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("update-12.fail", "");
    e.gh_file("checks-12.json", GREEN);
    // The second update attempt succeeds.
    let v = e
        .ns()
        .args(["run", "--issue", "7"])
        .assert()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&v.stdout).unwrap();
    let p = e.prompt(6, "build");
    assert!(p.contains("rebase onto main"), "{p}");
    // The first conflict's rebuild is free. Main stays where it was, so the later ones use
    // build's two attempts and the unit ends stuck instead of looping.
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(v["reason"], "build is out of attempts (2)", "{v}");
    assert!(!e.gh_calls().contains("pr merge"));
    let r = rebase_events(&e);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["cause"], "PR #12 conflicts with main");
}

fn update_branch_events(e: &Env) -> usize {
    update_branch_vias(e).len()
}

/// How each logged branch update was made: `api` or `git`.
fn update_branch_vias(e: &Env) -> Vec<String> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "update_branch")
        .map(|v| v["via"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn head_polls(e: &Env) -> usize {
    e.gh_calls()
        .lines()
        .filter(|l| *l == "pr view 12 --json headRefOid,mergeStateStatus")
        .count()
}

#[test]
fn a_lagging_head_after_update_branch_is_waited_for() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("updated-12.head", UPDATED_HEAD);
    e.gh_file("updated-12.lag", "2");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert_eq!(head_polls(&e), 3, "{calls}");
    assert_eq!(update_branch_events(&e), 1);
    let polls = register_polls(&e);
    assert!(!polls.is_empty(), "{calls}");
    assert!(
        polls
            .iter()
            .all(|p| p.contains(&format!("/commits/{UPDATED_HEAD}/"))),
        "{calls}"
    );
    assert!(
        calls.contains(&format!("--match-head-commit {UPDATED_HEAD}")),
        "{calls}"
    );
}

#[test]
fn a_head_that_never_changes_after_update_branch_needs_a_human_merge() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("updated-12.lag", "1000");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let head = git(&e.worktree(UNIT), &["rev-parse", "HEAD"]);
    assert_eq!(
        v["reason"],
        format!(
            "PR #12 head {head} did not change after update-branch within 3 min; needs a human merge"
        )
    );
    let calls = e.gh_calls();
    assert_eq!(head_polls(&e), 9, "{calls}");
    assert_eq!(update_branch_events(&e), 0);
    assert!(register_polls(&e).is_empty(), "{calls}");
    assert!(!calls.contains("pr merge"), "{calls}");
}

#[test]
fn a_dirty_view_while_waiting_for_the_head_goes_back_to_build() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("updated-12.merge", "DIRTY");
    e.gh_file("updated-12.lag", "1000");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let p = e.prompt(6, "build");
    assert!(p.contains("rebase onto main"), "{p}");
    assert!(!e.gh_calls().contains("pr merge"));
}

fn update_failures(e: &Env) -> Vec<Value> {
    run_log(e)
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "update_branch_failed")
        .collect()
}

#[test]
fn a_failed_update_branch_logs_its_cause() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file(
        "update-12.fail",
        "GraphQL: merge conflict between base and head",
    );
    e.run(&["run", "--issue", "7"], 1);
    let failed = update_failures(&e);
    // The first failure's rebuild is a free rebase; the next two use build's attempts.
    assert_eq!(failed.len(), 3, "{failed:?}");
    let err = failed[0]["error"].as_str().unwrap();
    assert!(
        err.contains("merge conflict between base and head"),
        "{err}"
    );
    assert!(failed[0].get("workflow_scope").is_none(), "{failed:?}");
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_pr_still_behind_after_update_branch_logs_its_merge_state() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("updated-12.merge", "BEHIND");
    e.gh_file("updated-12.lag", "1000");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let failed = update_failures(&e);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["merge_state"], "BEHIND", "{failed:?}");
    let err = failed[0]["error"].as_str().unwrap();
    assert!(err.contains("did not change"), "{err}");
}

const WORKFLOW_REFUSAL: &str = "GraphQL: refusing to allow an OAuth App to create or update workflow `.github/workflows/ci.yml` without `workflow` scope (updatePullRequestBranch)";

/// Ship pushes the unit's branch, then another actor lands `land` on origin's main from a clone,
/// and `after` runs in the worktree. GitHub reports the PR `BEHIND` and refuses `update-branch`
/// for the workflow scope.
fn workflow_refused(land: &str, after: &str) -> (Env, Value, i32) {
    let e = Env::new();
    // The build and every `land` change shared.txt, so the PR always overlaps main.
    fs::write(e.root.join("shared.txt"), SHARED).unwrap();
    git(&e.root, &["add", "shared.txt"]);
    git(&e.root, &["commit", "-q", "-m", "shared"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    e.ctl("edit", EDIT_BOTH);
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("ship", &["pass:script", "pass:script"]);
    e.ctl(
        "ship.sh",
        &format!(
            r#"set -e
c="git -c user.name=f -c user.email=f@f"
git push -q -f origin HEAD
o="$FAKE_CTRL/other"
if [ ! -d "$o" ]; then
  git clone -q "$(git remote get-url origin)" "$o"
  (cd "$o" && {land} && sed -i 6s/.*/main/ shared.txt && git add shared.txt && $c commit -qm shared && git push -q origin main)
fi
{after}"#
        ),
    );
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("update-12.fail", WORKFLOW_REFUSAL);
    e.gh_file("checks-12.json", GREEN);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let v = serde_json::from_slice(&out.stdout).unwrap();
    (e, v, out.status.code().unwrap())
}

const LAND_UP: &str = r#"echo up > UP && git add UP && $c commit -qm up"#;

#[test]
fn a_workflow_refusal_merges_main_with_git_and_pushes() {
    let (e, v, code) = workflow_refused(LAND_UP, "");
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let remote = e.base.join("remote.git");
    let head = git(&remote, &["rev-parse", &format!("ns/{UNIT}")]);
    let main = git(&remote, &["rev-parse", "main"]);
    git(&remote, &["merge-base", "--is-ancestor", &main, &head]);
    assert_eq!(
        git(&remote, &["rev-list", "--count", "--merges", "-1", &head]),
        "1"
    );
    let calls = e.gh_calls();
    let polls = register_polls(&e);
    assert!(!polls.is_empty(), "{calls}");
    assert!(
        polls
            .iter()
            .all(|p| p.contains(&format!("/commits/{head}/"))),
        "{calls}"
    );
    let upd = calls.find("pr update-branch 12").unwrap();
    let watch = calls.find("pr checks 12 --watch").unwrap();
    assert!(upd < watch, "{calls}");
    assert!(
        calls.contains(&format!("--match-head-commit {head}")),
        "{calls}"
    );
    let failed = update_failures(&e);
    assert_eq!(failed[0]["workflow_scope"], true, "{failed:?}");
    assert!(
        failed[0]["error"]
            .as_str()
            .unwrap()
            .contains("without `workflow` scope"),
        "{failed:?}"
    );
    assert_eq!(update_branch_vias(&e), ["git"]);
}

#[test]
fn a_conflict_in_the_git_fallback_goes_back_to_build() {
    let (e, v, code) = workflow_refused(
        r#"echo theirs > work.txt && git add work.txt && $c commit -qm theirs"#,
        "",
    );
    assert_eq!((code, &v["outcome"]), (1, &"stuck".into()), "{v}");
    let p = e.prompt(6, "build");
    assert!(p.contains("conflicts with main"), "{p}");
    assert_eq!(update_failures(&e)[0]["workflow_scope"], true);
    let wt = e.worktree(UNIT);
    assert_eq!(
        git(&wt, &["status", "--porcelain"]),
        "",
        "the failed merge is aborted"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_failed_fetch_in_the_git_fallback_needs_a_human_merge() {
    let (e, v, code) = workflow_refused(LAND_UP, "git remote set-url origin /nonexistent");
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("`workflow` scope"), "{reason}");
    assert!(reason.contains("git fetch"), "{reason}");
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_rebased_pr_head_is_not_overwritten_by_the_git_fallback() {
    let rebased = r#"git rev-parse HEAD > "$FAKE_GH_DIR/pr-12.head"
git push -q -f origin HEAD
$c commit -q --amend -m reworded"#;
    let (e, v, code) = workflow_refused(LAND_UP, rebased);
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("`workflow` scope"), "{reason}");
    assert!(reason.contains("needs a human merge"), "{reason}");
    let pushed = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    let remote = e.base.join("remote.git");
    assert_eq!(
        git(&remote, &["rev-parse", &format!("ns/{UNIT}")]),
        pushed.trim()
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_merge_that_fails_without_a_conflict_needs_a_human_merge() {
    let landed = r#"echo up > UNTRACKED && git add UNTRACKED && $c commit -qm up"#;
    let (e, v, code) = workflow_refused(landed, "echo mine > UNTRACKED");
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("`workflow` scope"), "{reason}");
    assert!(reason.contains("UNTRACKED"), "{reason}");
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_failed_push_in_the_git_fallback_needs_a_human_merge() {
    // Someone else moves the PR branch after ship, so the lease on the PR head fails.
    let moved = r#"(cd "$o" && git fetch -q origin "$(git -C "$OLDPWD" symbolic-ref --short HEAD)" \
  && git checkout -q FETCH_HEAD && echo other > OTHER && git add OTHER && $c commit -qm other \
  && git push -q origin "HEAD:$(git -C "$OLDPWD" symbolic-ref --short HEAD)")"#;
    let (e, v, code) = workflow_refused(LAND_UP, moved);
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    assert_eq!(v["needs_human"], true, "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("`workflow` scope"), "{reason}");
    assert!(reason.contains("git push"), "{reason}");
    assert!(reason.contains("needs a human merge"), "{reason}");
    assert!(!reason.contains("required status checks"), "{reason}");
    let wt = e.worktree(UNIT);
    let shipped = git(&wt, &["rev-parse", "HEAD"]);
    assert_eq!(
        git(&wt, &["rev-list", "--count", "--merges", "-1", &shipped]),
        "0"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn ci_failure_rebuilds_then_merges_on_the_same_pr() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.gh_file("checks-12-1.json", RED);
    e.gh_file("checks-12-2.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "build", "verify", "review", "ship"]
    );
    let p = e.prompt(6, "build");
    assert!(p.contains("CI failed on PR #12: ci (fail)"), "{p}");
    assert!(p.contains("assert 1 == 2"), "{p}");
    let ci: Vec<_> = run_events(&e)
        .into_iter()
        .filter(|ev| ev["event"] == "ci_failed" || ev["event"] == "ci_passed")
        .map(|ev| (ev["event"].clone(), ev["where"].clone()))
        .collect();
    assert_eq!(
        ci,
        [
            ("ci_failed".into(), "github".into()),
            ("ci_passed".into(), "github".into())
        ]
    );
}

fn register_polls(e: &Env) -> Vec<String> {
    e.gh_calls()
        .lines()
        .filter(|l| l.ends_with("/check-runs --jq .total_count"))
        .map(String::from)
        .collect()
}

#[test]
fn checks_that_register_late_are_watched_then_merged() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "2");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    let third = calls
        .match_indices("/check-runs")
        .nth(2)
        .expect("3 polls")
        .0;
    let watch = calls.find("pr checks 12 --watch").unwrap();
    assert!(third < watch, "{calls}");
    assert_eq!(register_polls(&e).len(), 3, "{calls}");
}

#[test]
fn no_checks_registered_within_the_timeout_needs_a_human_merge() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "1000");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        v["reason"],
        "no CI checks registered within 3 min on PR #12; needs a human merge"
    );
    let calls = e.gh_calls();
    assert!(!calls.contains("--watch"), "{calls}");
    assert!(!calls.contains("pr merge"), "{calls}");
    assert_eq!(register_polls(&e).len(), 9, "{calls}");
}

#[test]
fn a_shorter_register_timeout_polls_on_the_capped_schedule() {
    let e = Env::new();
    e.factory(&format!("{AUTO}ci_register_timeout = 1\n"));
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "1000");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        v["reason"],
        "no CI checks registered within 1 min on PR #12; needs a human merge"
    );
    assert_eq!(register_polls(&e).len(), 5, "{}", e.gh_calls());
}

#[test]
fn status_only_ci_registers_and_is_watched() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "1000");
    e.gh_file("status.after", "1");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(calls.contains("pr checks 12 --watch"), "{calls}");
    assert_eq!(register_polls(&e).len(), 2, "{calls}");
}

#[test]
fn a_failing_check_query_is_reported_not_called_no_ci() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("check-runs.fail", "HTTP 403: rate limit exceeded");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("could not query CI checks on PR #12: "),
        "{reason}"
    );
    assert!(reason.contains("rate limit exceeded"), "{reason}");
    assert!(reason.ends_with("; needs a human merge"), "{reason}");
    assert!(!e.gh_calls().contains("--watch"), "{}", e.gh_calls());
}

#[test]
fn malformed_check_query_output_is_reported_not_called_no_ci() {
    for output in ["", "null", "<html>Bad Gateway</html>"] {
        let e = Env::new();
        e.factory(AUTO);
        e.ctl("pr", "12");
        e.queue("build", &["pass:commit"]);
        e.gh_file("check-runs.output", output);
        let v = e.run(&["run", "--issue", "7"], 0);
        assert_eq!(v["outcome"], "done", "{v}");
        let reason = v["reason"].as_str().unwrap();
        assert!(
            reason.starts_with("could not query CI checks on PR #12: "),
            "{output:?}: {reason}"
        );
        assert!(reason.ends_with("; needs a human merge"), "{reason}");
        assert!(!e.gh_calls().contains("--watch"), "{}", e.gh_calls());
    }
}

#[test]
fn zero_check_runs_and_a_failing_status_query_is_reported_not_called_no_ci() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("register.after", "1000");
    e.gh_file("status.fail", "HTTP 500: server error");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("could not query CI checks on PR #12: "),
        "{reason}"
    );
    assert!(reason.contains("server error"), "{reason}");
    assert!(!e.gh_calls().contains("--watch"), "{}", e.gh_calls());
}

#[test]
fn a_status_with_checks_proceeds_though_the_check_runs_query_fails() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("check-runs.fail", "HTTP 403: rate limit exceeded");
    e.gh_file("status.after", "0");
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(calls.contains("pr checks 12 --watch"), "{calls}");
    assert!(calls.contains("pr merge 12"), "{calls}");
}

const PRINT_HEAD_SHA_CMD: &str = "git rev-parse HEAD";

/// main gains an unrelated commit after review. `pass:script` in ship rebases the unit onto
/// it; `pass:commit` changes the unit's content instead. The PR head is the sha `print_pr_head_sha`
/// prints before the rebase; `setup` runs before the unit does.
fn rebased_after_review(
    ship: &[&str],
    print_pr_head_sha: &str,
    setup: impl FnOnce(&Env),
) -> (Env, Value, i32) {
    let rebase = "git update-ref refs/heads/main upstream\n\
                  git -c user.name=f -c user.email=f@f rebase -q main\n";
    main_moves_after_review(ship, print_pr_head_sha, rebase, setup)
}

/// The commit main gains after review is on branch `upstream`; `pass:script` in ship runs
/// `bring_in` to take it into the unit.
fn main_moves_after_review(
    ship: &[&str],
    print_pr_head_sha: &str,
    bring_in: &str,
    setup: impl FnOnce(&Env),
) -> (Env, Value, i32) {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", ship);
    git(&e.root, &["checkout", "-qb", "upstream"]);
    fs::write(e.root.join("UPSTREAM"), "x\n").unwrap();
    git(&e.root, &["add", "UPSTREAM"]);
    git(&e.root, &["commit", "-q", "-m", "upstream"]);
    git(&e.root, &["checkout", "-q", "main"]);
    e.ctl(
        "ship.sh",
        &format!(
            "({print_pr_head_sha}) | tee \"$FAKE_GH_DIR/pr-12.head\" > \"$FAKE_GH_DIR/pr-12.shipped\"\n\
             {bring_in}"
        ),
    );
    e.gh_file("checks-12.json", GREEN);
    setup(&e);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let v = serde_json::from_slice(&out.stdout).unwrap();
    (e, v, out.status.code().unwrap())
}

#[test]
fn a_rebase_after_review_still_merges() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    // The rebased branch carries the merged change, so it goes.
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
    assert!(!e.worktree(UNIT).exists());
}

/// The sha `ns run` merged at: the unit's HEAD when its merge step ran.
fn merged_sha(e: &Env) -> String {
    let ev = run_events(e);
    let merged = ev
        .iter()
        .find(|v| v["event"] == "merged")
        .expect("merged event");
    merged["sha"].as_str().unwrap().to_string()
}

#[test]
fn a_pr_head_from_before_the_rebase_merges_at_that_head() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let old = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    assert_ne!(old.trim(), merged_sha(&e));
    let want = format!("--match-head-commit {}", old.trim());
    assert!(e.gh_calls().contains(&want), "{}", e.gh_calls());
}

const UPDATED_HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn a_behind_pr_merges_at_the_head_update_branch_returns() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.gh_file("pr-12.merge", "BEHIND");
        e.gh_file("updated-12.head", UPDATED_HEAD);
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let calls = e.gh_calls();
    assert!(calls.contains("pr update-branch 12"), "{calls}");
    assert!(gate_events(&e).is_empty());
    let want = format!("--match-head-commit {UPDATED_HEAD}");
    assert!(calls.contains(&want), "{calls}");
    let polls = register_polls(&e);
    assert!(!polls.is_empty(), "{calls}");
    let update = calls.find("pr update-branch 12").unwrap();
    let first_poll = calls.find("/check-runs").unwrap();
    let watch = calls.find("pr checks 12 --watch").unwrap();
    assert!(update < first_poll && first_poll < watch, "{calls}");
    assert!(
        polls
            .iter()
            .all(|p| p.contains(&format!("/commits/{UPDATED_HEAD}/"))),
        "{calls}"
    );
}

#[test]
fn a_behind_pr_without_an_updated_head_merges_at_the_pre_update_head() {
    let (e, v, code) = rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| {
        e.gh_file("pr-12.merge", "BEHIND");
        e.gh_file("updated-12.head", "none");
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    let old = fs::read_to_string(e.ghd.join("pr-12.head")).unwrap();
    assert_eq!(old.trim(), "none");
    let calls = e.gh_calls();
    let merge = calls.lines().find(|l| l.starts_with("pr merge")).unwrap();
    let head = merge.rsplit(' ').next().unwrap();
    let shipped = fs::read_to_string(e.ghd.join("pr-12.shipped")).unwrap();
    assert_eq!(head, shipped.trim(), "{merge}");
    assert_ne!(head, merged_sha(&e));
    assert_ne!(head, UPDATED_HEAD);
}

#[test]
fn a_dry_run_on_a_rebased_unit_sees_its_reviewed_artifacts_as_current() {
    // A human merge keeps the worktree to look at.
    let (e, v, code) =
        rebased_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, |e| e.factory(""));
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    let v = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(v["decision"]["action"], "done", "{v}");
}

#[test]
fn a_pr_head_with_other_content_needs_a_human_merge() {
    let other = "echo other > other.txt; git add other.txt; \
                 git -c user.name=f -c user.email=f@f commit -qm other; \
                 git rev-parse HEAD; git reset -q --hard HEAD~1";
    let (e, v, code) = rebased_after_review(&["pass:script"], other, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"done".into()), "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.contains("is not current with HEAD") && reason.ends_with("needs a human merge"),
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

/// Ship merges main's new commit as `git pull origin main` would: `origin/main` moves, local main
/// stays behind.
const MERGE_UPSTREAM: &str =
    "git -c user.name=f -c user.email=f@f merge -q --no-ff --no-edit upstream\n\
     git update-ref refs/remotes/origin/main upstream\n";

#[test]
fn a_merge_of_main_after_review_still_merges() {
    let (e, v, code) =
        main_moves_after_review(&["pass:script"], PRINT_HEAD_SHA_CMD, MERGE_UPSTREAM, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
}

#[test]
fn a_merge_of_main_that_changes_the_units_diff_reverifies() {
    let resolve = "git merge -q --no-ff --no-commit upstream\n\
                   echo resolved >> work.txt && git add work.txt\n\
                   git -c user.name=f -c user.email=f@f commit -q --no-edit\n\
                   git rev-parse HEAD > \"$FAKE_GH_DIR/pr-12.head\"\n";
    let (e, v, code) = main_moves_after_review(
        &["pass:script", "pass"],
        PRINT_HEAD_SHA_CMD,
        resolve,
        |_| {},
    );
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "verify", "review", "ship"]
    );
}

#[test]
fn a_content_change_after_review_reverifies_reviews_and_ships() {
    let (e, v, code) = rebased_after_review(&["pass:commit", "pass"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "ship", "verify", "review", "ship"]
    );
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
}

#[test]
fn rerunning_a_stale_pass_uses_no_attempt() {
    // Review's fix commit, then ship's, each leave a passing evidence.md behind HEAD: verify runs
    // three times under max_attempts 2, as on #156.
    let (e, v, code) = rebased_after_review(&["pass:commit", "pass"], PRINT_HEAD_SHA_CMD, |e| {
        e.queue("review", &["pass:commit", "pass"]);
    });
    assert_eq!((code, &v["outcome"]), (0, &"merged".into()), "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "verify", "ship", "verify", "review", "ship"]
    );
}

#[test]
fn only_the_first_rerun_after_a_pass_is_free() {
    let e = Env::new();
    e.factory("[phases.build]\nmax_attempts = 5\n");
    e.queue("build", &["pass:commit", "pass:commit", "pass:commit"]);
    e.queue("verify", &["pass", "fail", "fail", "fail"]);
    e.queue("review", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["reason"], "verify is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "verify", "build", "verify", "build"]
    );
}

#[test]
fn a_content_change_after_every_review_runs_out_of_attempts() {
    let (e, v, code) =
        rebased_after_review(&["pass:commit", "pass:commit"], PRINT_HEAD_SHA_CMD, |_| {});
    assert_eq!(code, 1, "{v}");
    // Each ship moves HEAD; the reruns of verify and review are free, so ship's cap stops it.
    assert_eq!(v["reason"], "ship is out of attempts (2)", "{v}");
    assert_eq!(
        e.calls(),
        [
            "triage", "build", "verify", "review", "ship", "verify", "review", "ship", "verify",
            "review"
        ]
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn human_review_files_need_a_human_merge() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "src/x.rs\n.github/workflows/ci.yml\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert!(v["reason"]
        .as_str()
        .unwrap()
        .starts_with("changes files that need human review; needs a human merge"));
    assert!(!e.gh_calls().contains("pr merge"));
}

/// main holds work.txt with a human-review region; the build commit runs `edit` on it.
fn marked(edit: &str) -> Value {
    let e = Env::new();
    // Spelled out at run time so this file holds no real markers.
    let (start, end) = ("ns:human-review start", "ns:human-review end");
    fs::write(
        e.root.join("work.txt"),
        format!("top\n# {start}: brake torque limits\nlimit = 5\n# {end}\nbottom\n"),
    )
    .unwrap();
    git(&e.root, &["add", "work.txt"]);
    git(&e.root, &["commit", "-q", "-m", "marked"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.ctl("edit", edit);
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "work.txt\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(
        v["outcome"] == "merged",
        e.gh_calls().contains("pr merge"),
        "{v}"
    );
    v
}

#[test]
fn an_edit_in_a_human_review_region_needs_a_human_merge() {
    let v = marked("sed -i 's/limit = 5/limit = 9/' work.txt");
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        v["reason"],
        "changes code in a human-review region; needs a human merge (work.txt:2-4: brake torque limits)"
    );
}

#[test]
fn an_edit_outside_a_human_review_region_merges() {
    let v = marked("sed -i 's/bottom/BOTTOM/' work.txt");
    assert_eq!(v["outcome"], "merged", "{v}");
}

#[test]
fn a_failed_fetch_of_the_default_branch_needs_a_human_merge() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.ctl("edit", "git remote set-url origin /nonexistent");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.gh_file("diff-12.txt", "work.txt\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("cannot check human-review regions: git fetch -q origin main failed"),
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn a_pr_merged_by_a_phase_is_stuck() {
    let e = Env::new();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:merge"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("PR merged by run"),
        "{v}"
    );
    assert!(!e.gh_calls().contains("pr merge"));
}

#[test]
fn human_policy_never_merges() {
    let e = Env::new();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert!(!e.gh_calls().contains("pr merge"));
    assert!(!e.gh_calls().contains("pr checks"));
}

#[test]
fn factory_validate_reports_problems() {
    let e = Env::new();
    e.factory("gates = \"auto\"\n[phases.build]\nskill = \"ns-build\"\n");
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["phases"].as_array().unwrap().len(), 5);
    e.factory("[defaults]\nharnes = \"claude\"\n");
    e.ns().args(["factory", "validate"]).assert().code(1);
    e.factory("");
    fs::create_dir_all(e.root.join(".nightshift/agents/build")).unwrap();
    fs::write(
        e.root.join(".nightshift/agents/build/agent.md"),
        "---\nrole: build\n---\nRun {skill} for {unit}; {bogus}\n",
    )
    .unwrap();
    let out = e
        .ns()
        .args(["factory", "validate"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8_lossy(&out).contains("unknown placeholder {bogus}"));
}

#[test]
fn factory_validate_warns_when_build_only_has_no_fast_ci_local() {
    let e = Env::new();
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command | Takes |\n|---|---|---|\n| ci-local | `make ci` | 2 min |\n",
    )
    .unwrap();
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["errors"], serde_json::json!([]), "{v}");
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("ci-local") && warnings.contains("`make ci --fast`"),
        "{v}"
    );
    e.factory("");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
}

#[test]
fn factory_validate_build_only_fast_warning_edge_cases() {
    let e = Env::new();
    e.factory("[gate]\nlocal = \"build-only\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["warnings"], serde_json::json!([]), "{v}");
    fs::create_dir_all(e.root.join("docs/agents")).unwrap();
    fs::write(
        e.root.join("docs/agents/stack.md"),
        "| Task | Command | Takes |\n|---|---|---|\n| ci-local | `scripts/ci-local.sh --fast` | 1 min |\n",
    )
    .unwrap();
    e.factory("[gate]\nlocal = \"build-only\"\n[phases.build]\ngate = \"make ci\"\n");
    let v = e.run(&["factory", "validate"], 0);
    assert_eq!(v["ok"], true, "{v}");
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("[phases.build] gate") && warnings.contains("`make ci --fast`"),
        "{v}"
    );
}
