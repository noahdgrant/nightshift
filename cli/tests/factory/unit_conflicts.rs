/// The build commits a change to README on the unit, and merges in a side branch that changed
/// README too, keeping the unit's side: one conflicting merge. It does so once, however many
/// attempts run.
fn build_merges_a_conflict(e: &Env, actions: &[&str]) {
    e.ctl(
        "build.sh",
        r#"set -e
c="git -c user.name=f -c user.email=f@f"
[ -z "$(git rev-list --merges main..HEAD)" ] || exit 0
git branch side HEAD
echo unit > README && $c commit -qam unit
git checkout -q side && echo side > README && $c commit -qam side
git checkout -q - && $c merge -q -s ours -m "merge side" side"#,
    );
    e.queue("build", actions);
}

/// The merge commit on the unit's branch.
fn unit_merge(e: &Env) -> String {
    git(
        &e.root,
        &["rev-list", "--merges", &format!("main..ns/{UNIT}")],
    )
}

fn conflict_events(e: &Env) -> Vec<Value> {
    run_events(e)
        .into_iter()
        .filter(|v| v["event"] == "conflict_resolved")
        .collect()
}

#[test]
fn a_unit_that_ends_done_records_the_conflicts_its_branch_resolved() {
    let e = Env::new();
    build_merges_a_conflict(&e, &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let merge = unit_merge(&e);
    let want = json!([{"merge": merge, "paths": ["README"]}]);
    let records = quality_records(&e);
    assert!(!records.is_empty());
    for r in &records {
        assert_eq!(r["conflicts"], want, "{r}");
    }
    let evs = conflict_events(&e);
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert_eq!(
        (&evs[0]["unit"], &evs[0]["merge"], &evs[0]["paths"]),
        (&json!(UNIT), &json!(merge), &json!(["README"]))
    );
    assert!(
        event_at(&e, |v| v["event"] == "conflict_resolved")
            < event_at(&e, |v| v["event"] == "quality_record")
    );
}

#[test]
fn a_unit_that_ends_merged_records_its_conflicts() {
    let e = auto_merge_env();
    build_merges_a_conflict(&e, &["pass:script"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "merged", "{v}");
    let r = &quality_records(&e)[0];
    assert_eq!(r["conflicts"][0]["paths"], json!(["README"]), "{r}");
    assert_eq!(conflict_events(&e).len(), 1);
}

#[test]
fn a_unit_that_ends_stuck_records_and_logs_its_conflicts() {
    let e = Env::new();
    let open = "schema: ns-artifact/2|cause: findings_open|next: review|reason: \"I1 is open\"|findings: [I1]";
    let fixed = "schema: ns-artifact/2|cause: none|next: review|findings: [I1]";
    e.factory("[phases.review]\nfull_cycles = 1\nfix_only_cycles = 0\n");
    build_merges_a_conflict(&e, &["pass:script"]);
    e.queue("review", &["fail", "fail"]);
    e.ctl("review.fronts", &format!("{open}\n{open}\n"));
    e.queue("review-fix", &["pass:commit"]);
    e.ctl("review-fix.fronts", &format!("{fixed}\n"));
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let evs = conflict_events(&e);
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert_eq!(evs[0]["paths"], json!(["README"]));
    let records = quality_records(&e);
    assert!(!records.is_empty());
    for r in &records {
        assert_eq!(r["conflicts"][0]["paths"], json!(["README"]), "{r}");
    }
}

#[test]
fn a_unit_with_no_merges_records_no_conflicts() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let records = quality_records(&e);
    assert!(!records.is_empty());
    assert!(records.iter().all(|r| r.get("conflicts").is_none()));
    assert!(run_events(&e)
        .iter()
        .all(|v| !v["event"].as_str().unwrap_or("").starts_with("conflict_")));
}

#[test]
fn clean_records_the_conflicts_of_a_unit_a_human_merged() {
    let e = Env::new();
    let wt = unit_with_pr(&e, 3, "3-done", 30, "CLOSED", "MERGED");
    git(&wt, &["branch", "side", "main"]);
    fs::write(wt.join("README"), "unit\n").unwrap();
    git(&wt, &["commit", "-qam", "unit"]);
    git(&wt, &["checkout", "-q", "side"]);
    fs::write(wt.join("README"), "side\n").unwrap();
    git(&wt, &["commit", "-qam", "side"]);
    git(&wt, &["checkout", "-q", "ns/3-done"]);
    git(&wt, &["merge", "-q", "-s", "ours", "-m", "merge side", "side"]);
    let merge = git(&wt, &["rev-parse", "HEAD"]);
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert_eq!(
        quality_records(&e)[0]["conflicts"],
        json!([{"merge": merge, "paths": ["README"]}])
    );
    assert!(run_events(&e)
        .iter()
        .all(|v| v["event"] != "conflict_resolved"));
}
