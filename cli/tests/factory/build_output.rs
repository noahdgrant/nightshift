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

/// Unit `3-open`'s worktree, kept by `ns clean` since its issue is open, with `target/out`.
fn open_unit_with_build_output(e: &Env) -> PathBuf {
    let wt = unit_with_pr(e, 3, "3-open", 30, "OPEN", "OPEN");
    fs::create_dir_all(wt.join("target")).unwrap();
    fs::write(wt.join("target/out"), "built\n").unwrap();
    wt
}

#[test]
fn a_budget_run_keeps_its_build_output_for_the_resume() {
    let e = build_output_env();
    e.factory("[limits]\nbudget_usd = 1.0\n[phases.build]\ngate = \"exit 1\"\n");
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 3);
    assert_eq!(v["outcome"], "budget", "{v}");
    assert!(e.worktree(UNIT).join("target/out").is_file());
    assert!(build_output_events(&e).is_empty());
}

#[test]
fn a_split_run_keeps_its_build_output() {
    let e = build_output_env();
    e.factory("[worktree]\nsetup = [\"mkdir -p target && echo built > target/out\"]\n");
    e.queue(
        "triage",
        &["split:into #8 and #9.", "split:into #8 and #9."],
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "split", "{v}");
    assert!(e.worktree(UNIT).join("target/out").is_file());
    assert!(build_output_events(&e).is_empty());
}

#[test]
fn a_merged_run_that_removes_its_worktree_reports_no_build_output() {
    let e = build_output_env();
    e.factory(AUTO);
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("checks-12.json", GREEN);
    e.ctl("review.body", FINDING);
    e.queue("ship", &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    assert_eq!(v["cleanup"]["removed"], true, "{v}");
    assert!(!e.worktree(UNIT).exists());
    assert!(build_output_events(&e).is_empty());
}

/// A worktree whose `build-output` row names `target`, `../x` and a symlink out, with the file
/// each would reach outside the worktree.
fn plant_refusals(wt: &Path) -> PathBuf {
    let outside = wt.parent().unwrap().join("x");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("f"), "keep").unwrap();
    std::os::unix::fs::symlink(&outside, wt.join("link")).unwrap();
    fs::create_dir_all(wt.join("target")).unwrap();
    fs::write(wt.join("target/out"), "built\n").unwrap();
    outside.join("f")
}

fn assert_refused(v: &Value, outside_file: &Path) {
    let refused: Vec<(&str, &str)> = v["refused"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["path"].as_str().unwrap(), r["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(refused.len(), 2, "{v}");
    assert_eq!(refused[0], ("../x", "climbs out with .."), "{v}");
    assert_eq!(refused[1].0, "link", "{v}");
    assert!(refused[1].1.starts_with("resolves outside the worktree"), "{v}");
    assert_eq!(v["paths"], json!(["target"]), "{v}");
    assert_eq!(fs::read_to_string(outside_file).unwrap(), "keep");
}

const REFUSING_ROW: &str =
    "| Task | Command | Takes |\n|---|---|---|\n| build-output | `target`, `../x`, `link` | |\n";

#[test]
fn a_run_reports_the_build_output_paths_it_refuses() {
    let e = build_output_env();
    e.ctl(
        "edit",
        "sed -i 's#.*build-output.*#| build-output | `target`, `../x`, `link` | |#' docs/agents/stack.md\n\
             ln -s \"$(cd .. && pwd)/x\" link\n\
             mkdir -p ../x target && echo keep > ../x/f && echo built > target/out && echo w >> work.txt\n",
    );
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let evs = build_output_events(&e);
    assert_eq!(evs.len(), 1, "{evs:?}");
    let outside = e.worktree(UNIT).parent().unwrap().join("x/f");
    assert_eq!(fs::read_to_string(&outside).unwrap().trim(), "keep");
    assert_eq!(evs[0]["refused"].as_array().unwrap().len(), 2, "{evs:?}");
    assert_eq!(evs[0]["paths"], json!(["target"]), "{evs:?}");
}

#[test]
fn clean_reports_the_build_output_paths_it_refuses() {
    let e = build_output_env();
    let wt = open_unit_with_build_output(&e);
    fs::write(wt.join("docs/agents/stack.md"), REFUSING_ROW).unwrap();
    let outside = plant_refusals(&wt);
    let v = e.run(&["clean"], 0);
    assert_refused(&v["kept"][0]["build_output"], &outside);
    assert!(!wt.join("target").exists());
    assert!(fs::symlink_metadata(wt.join("link")).is_ok());
    assert_eq!(build_output_events(&e)[0]["refused"].as_array().unwrap().len(), 2);
}

#[test]
fn clean_deletes_the_build_output_of_each_worktree_it_keeps() {
    let e = build_output_env();
    let wt = open_unit_with_build_output(&e);
    let v = e.run(&["clean"], 0);
    assert_eq!(reasons(&v)["3-open"], "issue #3 is open", "{v}");
    assert_eq!(
        v["kept"][0]["build_output"],
        json!({"paths": ["target"], "bytes": 6, "refused": [], "failed": []}),
        "{v}"
    );
    assert!(wt.join("3-open.txt").is_file());
    assert!(!wt.join("target").exists());
    let evs = build_output_events(&e);
    assert_eq!(
        (evs.len(), evs[0]["unit"].as_str(), evs[0]["by"].as_str()),
        (1, Some("3-open"), Some("clean"))
    );
    assert_eq!(evs[0]["bytes"], 6);
}

#[test]
fn clean_leaves_the_build_output_of_a_unit_a_live_run_holds_or_a_dry_run() {
    let e = build_output_env();
    let wt = open_unit_with_build_output(&e);
    let v = e.run(&["clean", "--dry-run"], 0);
    assert!(v["kept"][0].get("build_output").is_none(), "{v}");
    let _held = hold(
        &e.root.join(".git/ns-run-3-open.lock"),
        r#"{"pid":42,"unit":"3-open","issue":3}"#,
    );
    let v = e.run(&["clean"], 0);
    assert!(v["kept"][0].get("build_output").is_none(), "{v}");
    assert!(wt.join("target/out").is_file());
    assert!(build_output_events(&e).is_empty());
}
