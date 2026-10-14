/// Six lines on main that a PR and a landing commit can both change without a conflict.
const SHARED: &str = "1\n2\n3\n4\n5\n6\n";

/// The build edits `work.txt`, and `shared.txt`'s first line when `touch_shared`.
const EDIT_WORK: &str = "echo build >> work.txt";
const EDIT_BOTH: &str = "echo build >> work.txt && sed -i 1s/.*/pr/ shared.txt && git add shared.txt";

/// Commits another actor lands on main, run in a clone of origin with `$c` a committing git.
const LAND_DISJOINT: &str = "echo up > UP && git add UP && $c commit -qm up";
const LAND_SHARED: &str = "sed -i 6s/.*/main/ shared.txt && git add shared.txt && $c commit -qm shared";
const LAND_RENAME: &str = "git mv shared.txt moved.txt && $c commit -qm rename";
const LAND_UNRELATED: &str = "git checkout -q --orphan other && echo x > x && git add -A && $c commit -qm unrelated && git branch -q -f main HEAD && git checkout -q main";

/// `ctrl/land.sh <command>`: land the command's commit on origin's main from a clone.
const LAND_SH: &str = r#"set -e
c="git -c user.name=f -c user.email=f@f"
o="$FAKE_CTRL/other"
[ -d "$o" ] || git clone -q "$(git remote get-url origin)" "$o"
cd "$o"
git fetch -q origin main && git checkout -q -B main FETCH_HEAD
eval "$1"
git push -q -f origin main
"#;

/// A unit on a repo holding `shared.txt` under `factory`, whose build runs `edit` and whose ship
/// pushes the branch, then lands `land` on main. GitHub reports the PR `BEHIND` and its checks
/// green.
fn behind(factory: &str, edit: &str, land: &str) -> Env {
    let e = Env::new();
    fs::write(e.root.join("shared.txt"), SHARED).unwrap();
    git(&e.root, &["add", "shared.txt"]);
    git(&e.root, &["commit", "-q", "-m", "shared"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    e.factory(factory);
    e.ctl("pr", "12");
    e.ctl("edit", edit);
    e.ctl("land.sh", LAND_SH);
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:script"]);
    e.ctl(
        "ship.sh",
        &format!("set -e\ngit push -q -f origin HEAD\nsh \"$FAKE_CTRL/land.sh\" '{land}'\n"),
    );
    e.gh_file("pr-12.merge", "BEHIND");
    e.gh_file("checks-12.json", GREEN);
    e
}

const BUILD_ONLY: &str = "[gate]\nlocal = \"build-only\"\n";

/// The head ship pushed, the PR head the fake gh reports until an update.
fn unit_head(e: &Env) -> String {
    git(&e.base.join("remote.git"), &["rev-parse", &format!("ns/{UNIT}")])
}

#[test]
fn a_base_that_moved_in_other_files_merges_the_pr_head_as_it_is() {
    let e = behind(&format!("{AUTO}{BUILD_ONLY}"), EDIT_WORK, LAND_DISJOINT);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(!calls.contains("pr update-branch"), "{calls}");
    let head = unit_head(&e);
    assert!(
        calls.contains(&format!("--match-head-commit {head}")),
        "{calls}"
    );
    let skipped = events_named(&e, "merge_update_skipped");
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    let s = &skipped[0];
    assert_eq!(
        (&s["pr"], &s["base"], &s["pr_files"], &s["base_files"]),
        (&json!(12), &json!(remote_main(&e)), &json!(["work.txt"]), &json!(["UP"])),
        "{s}"
    );
    assert!(events_named(&e, "merge_update_needed").is_empty());
}

#[test]
fn a_base_that_moved_in_a_file_the_pr_changes_updates_it() {
    let e = behind(&format!("{AUTO}{BUILD_ONLY}"), EDIT_BOTH, LAND_SHARED);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let calls = e.gh_calls();
    assert!(calls.contains("pr update-branch 12"), "{calls}");
    assert!(calls.contains(&format!("--match-head-commit {UPDATED}")), "{calls}");
    let needed = events_named(&e, "merge_update_needed");
    assert_eq!(needed.len(), 1, "{needed:?}");
    assert_eq!(needed[0]["overlap"], json!(["shared.txt"]), "{needed:?}");
    assert!(events_named(&e, "merge_update_skipped").is_empty());
}

#[test]
fn a_rename_of_a_file_the_pr_changes_overlaps() {
    let e = behind(&format!("{AUTO}{BUILD_ONLY}"), EDIT_BOTH, LAND_RENAME);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert!(e.gh_calls().contains("pr update-branch 12"));
    let needed = events_named(&e, "merge_update_needed");
    assert_eq!(needed[0]["overlap"], json!(["shared.txt"]), "{needed:?}");
}

/// Main replaced by unrelated history: no merge base, so no list of what the PR lacks. The
/// region check later needs the merge base too, so the unit ends needing a human merge.
#[test]
fn a_diff_that_cannot_be_read_updates_the_pr() {
    let e = behind(&format!("{AUTO}{BUILD_ONLY}"), EDIT_WORK, LAND_UNRELATED);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert!(e.gh_calls().contains("pr update-branch 12"));
    let needed = events_named(&e, "merge_update_needed");
    assert_eq!(needed.len(), 1, "{needed:?}");
    assert_eq!(needed[0]["overlap"], Value::Null, "{needed:?}");
    assert!(needed[0]["error"].as_str().is_some(), "{needed:?}");
}

#[test]
fn with_the_local_queue_off_a_base_that_moved_in_other_files_skips_the_update() {
    let e = behind(
        &format!("{AUTO}{LEGACY}[phases.build]\ngate = \"true\"\n"),
        EDIT_WORK,
        LAND_DISJOINT,
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert!(!e.gh_calls().contains("pr update-branch"));
    assert_eq!(events_named(&e, "merge_update_skipped").len(), 1);
}

#[test]
fn with_the_local_queue_on_a_behind_pr_gates_the_merge_result_and_checks_no_overlap() {
    let e = behind(
        &format!("{AUTO}[phases.build]\ngate = \"true\"\n"),
        EDIT_WORK,
        LAND_DISJOINT,
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert!(!e.gh_calls().contains("pr update-branch"));
    assert!(events_named(&e, "merge_update_skipped").is_empty());
    assert!(events_named(&e, "merge_update_needed").is_empty());
}

/// Land `land` on main once, while the merge step waits for the PR's CI.
fn land_during_ci(e: &Env, land: &str) {
    e.gh_file(
        "hook.sh",
        &format!(
            "case \"$*\" in \"pr checks 12 --watch\") [ -f \"$d/landed\" ] || {{ touch \"$d/landed\"; sh \"$FAKE_CTRL/land.sh\" '{land}' >&2; }} ;; esac\n"
        ),
    );
}

#[test]
fn a_base_that_moves_again_in_other_files_under_the_lock_merges_without_going_round() {
    let e = behind(&format!("{AUTO}{BUILD_ONLY}"), EDIT_WORK, LAND_DISJOINT);
    land_during_ci(&e, "echo two > TWO && git add TWO && $c commit -qm two");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert!(events_named(&e, "merge_base_moved").is_empty());
    let skipped = events_named(&e, "merge_update_skipped");
    assert_eq!(skipped.len(), 2, "{skipped:?}");
    assert_eq!(skipped[1]["base"], remote_main(&e), "{skipped:?}");
    assert_eq!(skipped[1]["base_files"], json!(["TWO", "UP"]), "{skipped:?}");
    let watches = e
        .gh_calls()
        .lines()
        .filter(|l| *l == "pr checks 12 --watch")
        .count();
    assert_eq!(watches, 1);
}

#[test]
fn a_base_that_moves_again_in_a_file_the_pr_changes_under_the_lock_goes_round() {
    let e = behind(&format!("{AUTO}{BUILD_ONLY}"), EDIT_BOTH, LAND_DISJOINT);
    land_during_ci(&e, LAND_SHARED);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let moved = events_named(&e, "merge_base_moved");
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(moved[0]["to"], remote_main(&e), "{moved:?}");
    let needed = events_named(&e, "merge_update_needed");
    assert_eq!(needed[0]["overlap"], json!(["shared.txt"]), "{needed:?}");
    assert!(e.gh_calls().contains("pr update-branch 12"));
}

const MAIN_TIP: &str = "abcdef0123456789abcdef0123456789abcdef01";

/// Main's CI at `MAIN_TIP`, its one check `test` finished with `conclusion`.
fn main_ci_test(conclusion: &str) -> String {
    main_ci_at(MAIN_TIP, &[], &[("test", "COMPLETED", conclusion)])
}

#[test]
fn a_red_main_holds_the_merge_until_it_goes_green() {
    let e = behind(&format!("{AUTO}{BUILD_ONLY}"), EDIT_WORK, LAND_DISJOINT);
    e.gh_file("main-ci-1.json", &main_ci_test("FAILURE"));
    e.gh_file("main-ci-2.json", &main_ci_test("FAILURE"));
    e.gh_file("main-ci.json", &main_ci_test("SUCCESS"));
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let held = events_named(&e, "merge_held_main_red");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!((&held[0]["pr"], &held[0]["sha"]), (&json!(12), &json!(MAIN_TIP)));
    let calls = e.gh_calls();
    let queries: Vec<usize> = calls
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with("api graphql"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(queries.len(), 3, "{calls}");
    let merge = calls.lines().position(|l| l.starts_with("pr merge")).unwrap();
    assert!(queries[2] < merge, "{calls}");
}

#[test]
fn a_main_still_red_after_the_ci_timeout_ends_the_unit_stuck() {
    let e = behind(&format!("{AUTO}{BUILD_ONLY}"), EDIT_WORK, LAND_DISJOINT);
    e.gh_file("main-ci.json", &main_ci_test("FAILURE"));
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(
        v["reason"],
        format!("main's CI is red at {MAIN_TIP}; not merging until it is fixed")
    );
    assert!(!e.gh_calls().contains("pr merge"));
    assert_eq!(events_named(&e, "merge_held_main_red").len(), 1);
}

#[test]
fn a_main_with_ci_still_running_or_unreadable_holds_nothing() {
    for (file, text) in [
        ("main-ci.json", main_ci_at(MAIN_TIP, &[], &[("test", "IN_PROGRESS", "")])),
        ("main-ci.fail", "HTTP 502".to_string()),
    ] {
        let e = behind(&format!("{AUTO}{BUILD_ONLY}"), EDIT_WORK, LAND_DISJOINT);
        e.gh_file(file, &text);
        let v = e.run(&["run", "--issue", "7"], 0);
        assert_eq!(v["outcome"], "merged", "{file}: {v}");
        assert!(events_named(&e, "merge_held_main_red").is_empty(), "{file}");
    }
}

/// The local merge queue gated the merge with the old base, so a move in other files still
/// sends the PR round to gate the merge with the new one.
#[test]
fn with_the_local_queue_on_a_base_that_moves_in_other_files_under_the_lock_goes_round() {
    let e = behind(
        &format!("{AUTO}[phases.build]\ngate = \"true\"\n"),
        EDIT_WORK,
        LAND_DISJOINT,
    );
    land_during_ci(&e, "echo two > TWO && git add TWO && $c commit -qm two");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let moved = events_named(&e, "merge_base_moved");
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(moved[0]["to"], remote_main(&e), "{moved:?}");
    assert!(events_named(&e, "merge_update_skipped").is_empty());
}

#[test]
fn with_the_local_queue_on_a_red_main_holds_nothing() {
    let e = behind(
        &format!("{AUTO}[phases.build]\ngate = \"true\"\n"),
        EDIT_WORK,
        LAND_DISJOINT,
    );
    e.gh_file("main-ci.json", &main_ci_test("FAILURE"));
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert!(events_named(&e, "merge_held_main_red").is_empty());
    assert!(!e.gh_calls().contains("api graphql"));
}
