// The review loop `ns run` drives (#560): each review pass and fix cycle is its own phase run,
// with its own timeout, and `ns` records when each one ran.

/// A review pass's frontmatter that leaves I1 open for a fix cycle.
const OPEN_I1: &str = "schema: ns-artifact/2|cause: findings_open|next: review|reason: \"I1 is open\"|findings: [I1]";

/// A fix cycle's frontmatter that fixed I1.
const FIXED_I1: &str = "schema: ns-artifact/2|cause: none|next: review|findings: [I1]";

fn review_record(e: &Env) -> Vec<Value> {
    let p = e.worktree(UNIT).join(format!(".ns/{UNIT}/review/cycles.json"));
    serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap()
}

#[test]
fn a_review_that_leaves_an_important_open_runs_a_fix_cycle_then_a_second_pass() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail", "pass"]);
    e.ctl("review.fronts", &format!("{OPEN_I1}\n"));
    e.queue("review-fix", &["pass:commit"]);
    e.ctl("review-fix.fronts", &format!("{FIXED_I1}\n"));
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "review-fix", "verify", "review", "ship"]
    );
    let fix = e.prompt(5, "review-fix");
    assert!(fix.contains("ns-review") && fix.contains("Phase: review-fix (attempt 1)"), "{fix}");
    assert!(fix.contains("Fix cycle 1 (full)") && fix.contains(": I1."), "{fix}");
    let pass = e.prompt(7, "review");
    assert!(pass.contains("Review pass 1, after fix cycle 1 (full)"), "{pass}");
    let wt = e.worktree(UNIT).join(format!(".ns/{UNIT}"));
    assert!(wt.join("history/fix-1.md").is_file(), "the pass took up fix.md");
    assert!(!wt.join("history/review-1.md").exists(), "the pass updated review.md in place");
}

#[test]
fn ns_records_each_pass_and_fix_cycle_with_its_own_clock_and_the_quality_record_carries_them() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail", "pass"]);
    e.ctl("review.fronts", &format!("{OPEN_I1}\n"));
    e.queue("review-fix", &["pass:commit"]);
    e.ctl("review-fix.fronts", &format!("{FIXED_I1}\n"));
    e.run(&["run", "--issue", "7"], 0);
    let at = "2026-10-09T00:00:00Z";
    let rec = review_record(&e);
    let got: Vec<(&str, u64, &str)> = rec
        .iter()
        .map(|r| {
            assert_eq!((r["started"].as_str(), r["ended"].as_str()), (Some(at), Some(at)), "{r}");
            (r["phase"].as_str().unwrap(), r["cycle"].as_u64().unwrap(), r["kind"].as_str().unwrap())
        })
        .collect();
    assert_eq!(
        got,
        [("review", 0, "first"), ("review-fix", 1, "full"), ("review", 1, "full")]
    );
    let records = quality_records(&e);
    let last = records.last().unwrap();
    assert_eq!(last["cycles"], 1, "{last}");
    assert_eq!(last["review_runs"].as_array().unwrap().len(), 3, "{last}");
    assert_eq!(last["review_runs"][1]["started"], at);
    let phases = events_named(&e, "phase");
    let fix = phases.iter().find(|p| p["phase"] == "review-fix").unwrap();
    assert_eq!(fix["review_cycle"], json!({"cycle": 1, "kind": "full"}));
}

#[test]
fn the_cycle_limit_in_phases_review_stops_the_loop_stuck() {
    let e = Env::new();
    e.factory("[phases.review]\nfull_cycles = 1\nfix_only_cycles = 0\n");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail", "fail"]);
    e.ctl("review.fronts", &format!("{OPEN_I1}\n{OPEN_I1}\n"));
    e.queue("review-fix", &["pass:commit"]);
    e.ctl("review-fix.fronts", &format!("{FIXED_I1}\n"));
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(
        v["reason"],
        "review.md has findings open after 1 fix cycles (limit 1 full, 0 fix-only): I1"
    );
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "review-fix", "verify", "review"]
    );
}

#[test]
fn a_fix_cycle_that_hangs_is_cut_off_at_its_own_timeout_and_the_loop_goes_on() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail", "pass"]);
    e.ctl("review.fronts", &format!("{OPEN_I1}\n"));
    e.queue("review-fix", &["pass:commit+sleep", "pass:commit"]);
    e.ctl("review-fix.fronts", &format!("{FIXED_I1}\n{FIXED_I1}\n"));
    let out = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "review-fix=4000")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "done", "{v}");
    let runs: Vec<(String, u64, bool)> = events_named(&e, "phase")
        .iter()
        .filter(|p| p["phase"] == "review" || p["phase"] == "review-fix")
        .map(|p| {
            let phase = p["phase"].as_str().unwrap().to_string();
            (phase, p["attempt"].as_u64().unwrap(), p["timed_out"] == true)
        })
        .collect();
    let want = [("review", 1, false), ("review-fix", 1, true), ("review-fix", 2, false), ("review", 2, false)];
    let want: Vec<(String, u64, bool)> = want.iter().map(|(p, a, t)| (p.to_string(), *a, *t)).collect();
    assert_eq!(runs, want);
    let fixes: Vec<_> = review_record(&e).into_iter().filter(|r| r["phase"] == "review-fix").collect();
    assert_eq!(fixes.len(), 1, "the cut-off run is no cycle: {fixes:?}");
}

#[test]
fn an_invalid_fix_md_is_rewritten_and_counts_as_no_cycle() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail", "pass"]);
    e.ctl("review.fronts", &format!("{OPEN_I1}\n"));
    e.queue("review-fix", &["pass:commit", "pass"]);
    let invalid = "schema: ns-artifact/2|cause: nonsense|next: review";
    e.ctl("review-fix.fronts", &format!("{invalid}\n{FIXED_I1}\n"));
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let fixes: Vec<_> = review_record(&e)
        .into_iter()
        .filter(|r| r["phase"] == "review-fix")
        .collect();
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0]["cycle"], 1);
}
