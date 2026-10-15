/// An env whose `stack.md` names `target` as build output, and whose build phase writes
/// `target/out` (6 bytes) beside its commit.
fn build_output_env() -> Env {
    let e = Env::with_repo("main", |root| {
        fs::write(root.join("README"), "hi\n").unwrap();
        fs::create_dir_all(root.join("docs/agents")).unwrap();
        fs::write(
            root.join("docs/agents/stack.md"),
            "| Task | Command | Takes |\n|---|---|---|\n| build-output | `target` | |\n",
        )
        .unwrap();
    });
    e.ctl(
        "edit",
        "mkdir -p target && echo built > target/out && echo w >> work.txt\n",
    );
    e
}

fn build_output_events(e: &Env) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|v| v["event"] == "build_output_removed")
        .collect()
}

/// The unit's worktree is kept, its build output gone, and one event says so.
fn assert_build_output_removed(e: &Env) {
    let wt = e.worktree(UNIT);
    assert!(wt.join("work.txt").is_file());
    assert!(!wt.join("target").exists());
    let evs = build_output_events(e);
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert_eq!(
        (evs[0]["unit"].as_str(), &evs[0]["paths"], &evs[0]["bytes"]),
        (Some(UNIT), &json!(["target"]), &json!(6))
    );
}

#[test]
fn a_stuck_run_deletes_the_build_output_of_the_worktree_it_keeps() {
    let e = build_output_env();
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["blocked:needs hardware"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_build_output_removed(&e);
    let end = event_at(&e, |v| v["event"] == "end");
    assert!(end < event_at(&e, |v| v["event"] == "build_output_removed"));
}

#[test]
fn a_done_run_deletes_the_build_output_of_the_worktree_it_keeps() {
    let e = build_output_env();
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_build_output_removed(&e);
}

#[test]
fn a_merged_run_that_keeps_its_worktree_deletes_its_build_output() {
    let e = build_output_env();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.ctl("review.body", FINDING);
    e.queue("ship", &["pass:script"]);
    e.ctl("ship.sh", "git checkout -q --detach\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["cleanup"]["removed"], false, "{v}");
    assert_build_output_removed(&e);
}

#[test]
fn a_paused_run_keeps_its_build_output_for_the_resume() {
    let e = build_output_env();
    e.ctl("reset", &(NOW + 3600).to_string());
    e.queue("build", &["pass:commit"]);
    e.queue("verify", &["limit"]);
    let v = e.run(&["run", "--issue", "7"], 4);
    assert_eq!(v["outcome"], "paused", "{v}");
    assert!(e.worktree(UNIT).join("target/out").is_file());
    assert!(build_output_events(&e).is_empty());
}
