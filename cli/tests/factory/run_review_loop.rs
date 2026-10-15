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
            // Each ends its harness's wall time on, a second or so here.
            assert_eq!(r["started"].as_str(), Some(at), "{r}");
            assert!(r["ended"].as_str().is_some_and(|t| (at.."2026-10-09T00:01:00Z").contains(&t)), "{r}");
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
fn the_cycle_limit_in_phases_review_stops_the_loop_with_review_md_blocked_for_a_human() {
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
    let review = e.worktree(UNIT).join(format!(".ns/{UNIT}/review.md"));
    let text = fs::read_to_string(&review).unwrap();
    for line in ["status: blocked", "cause: findings_open", "next: human"] {
        assert!(text.contains(&format!("\n{line}\n")), "{text}");
    }
    assert!(text.contains(&format!("reason: {}", v["reason"])), "{text}");
    e.ns()
        .args(["artifact", "check", review.to_str().unwrap(), "--phase", "review"])
        .assert()
        .success();
    let records = quality_records(&e);
    let last = records.last().unwrap();
    assert_eq!((&last["status"], &last["cycles"]), (&json!("blocked"), &json!(1)), "{last}");
    let again = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(again["outcome"], "stuck", "{again}");
    assert_eq!(e.calls().len(), 7, "a rerun at the limit runs no phase");
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
        .env("NS_PHASE_TIMEOUT_MS", "review=4000")
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

#[test]
fn an_invalid_pass_inside_the_loop_is_rerun_with_the_loop_as_it_was() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail", "pass", "pass"]);
    let invalid = "schema: ns-artifact/2|cause: nonsense|next: review";
    e.ctl("review.fronts", &format!("{OPEN_I1}\n{invalid}\n"));
    e.queue("review-fix", &["pass:commit"]);
    e.ctl("review-fix.fronts", &format!("{FIXED_I1}\n"));
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "review", "review-fix", "verify", "review", "review", "ship"]
    );
    let got: Vec<(String, u64)> = review_record(&e)
        .iter()
        .map(|r| (r["phase"].as_str().unwrap().to_string(), r["cycle"].as_u64().unwrap()))
        .collect();
    let want = [("review", 0), ("review-fix", 1), ("review", 1)];
    let want: Vec<(String, u64)> = want.iter().map(|(p, c)| (p.to_string(), *c)).collect();
    assert_eq!(got, want, "the rerun was pass 1 of the same loop");
    let wt = e.worktree(UNIT).join(format!(".ns/{UNIT}"));
    assert!(wt.join("history/review-invalid-1.md").is_file());
}

#[test]
fn a_pass_inside_the_loop_that_crashes_or_writes_nothing_is_rerun_and_counts_as_no_pass() {
    for status in ["crash", "none"] {
        let e = Env::new();
        e.queue("build", &["pass:commit"]);
        e.queue("review", &["fail", status, "pass"]);
        e.ctl("review.fronts", &format!("{OPEN_I1}\n"));
        e.queue("review-fix", &["pass:commit"]);
        e.ctl("review-fix.fronts", &format!("{FIXED_I1}\n"));
        let v = e.run(&["run", "--issue", "7"], 0);
        assert_eq!(v["outcome"], "done", "{status}: {v}");
        assert_eq!(
            e.calls(),
            ["triage", "build", "verify", "review", "review-fix", "verify", "review", "review", "ship"],
            "{status}"
        );
        let got: Vec<(String, u64)> = review_record(&e)
            .iter()
            .map(|r| (r["phase"].as_str().unwrap().to_string(), r["cycle"].as_u64().unwrap()))
            .collect();
        let want = [("review", 0), ("review-fix", 1), ("review", 1)];
        let want: Vec<(String, u64)> = want.iter().map(|(p, c)| (p.to_string(), *c)).collect();
        assert_eq!(got, want, "{status}");
    }
}

#[test]
fn a_fix_cycle_that_breaks_ns_record_ends_the_unit_stuck_and_counts_as_no_cycle() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail"]);
    e.ctl("review.fronts", &format!("{OPEN_I1}\n"));
    e.queue("review-fix", &["pass:script"]);
    e.ctl("review-fix.fronts", &format!("{FIXED_I1}\n"));
    e.ctl("review-fix.sh", "echo garbage > \".ns/$NS_UNIT/review/cycles.json\"\n");
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("ns could not record the review-fix run in its review record: cannot parse"),
        "{reason}"
    );
    let rec = review_record(&e);
    assert_eq!(rec.len(), 1, "the record is put back as it was: {rec:?}");
    let wt = e.worktree(UNIT).join(format!(".ns/{UNIT}"));
    assert!(!wt.join("fix.md").exists() && wt.join("history/fix-unrecorded-1.md").is_file());
}

#[test]
fn a_fix_cycle_holds_the_locks_of_reviews_runner() {
    let e = Env::new();
    let locks = e.base.join("locks");
    e.factory("[phases.review]\nrunner = \"bench\"\n");
    e.runner("bench", "kind = \"bench\"\nlocks = [\"bench-1\"]\n");
    e.config(&format!("[runners]\nlock_dir = {:?}\n", locks.to_str().unwrap()));
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail", "pass"]);
    e.ctl("review.fronts", &format!("{OPEN_I1}\n"));
    e.queue("review-fix", &["pass:script"]);
    e.ctl("review-fix.fronts", &format!("{FIXED_I1}\n"));
    let seen = e.ctrl.join("seen");
    e.ctl(
        "review-fix.sh",
        &format!(
            "flock -n {l:?} true && echo free > {seen:?} || echo held > {seen:?}\n",
            l = locks.join("bench-1.lock"),
        ),
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(fs::read_to_string(&seen).unwrap(), "held\n");
    assert!(lock_is_free(&locks.join("bench-1.lock")));
}

#[test]
fn a_dry_run_on_a_unit_waiting_for_a_fix_cycle_shows_the_fix_cycle() {
    let e = Env::new();
    // Four phases at $0.50 each spend the budget before the fix cycle.
    e.factory("[limits]\nbudget_usd = 2.0\n");
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["fail"]);
    e.ctl("review.fronts", &format!("{OPEN_I1}\n"));
    let v = e.run(&["run", "--issue", "7"], 3);
    assert_eq!(v["outcome"], "budget", "{v}");
    e.factory("");
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    assert_eq!(d["decision"]["action"], "run", "{d}");
    assert_eq!(d["decision"]["phase"], "review-fix", "{d}");
    let prompt = d["prompt"].as_str().unwrap();
    assert!(prompt.contains("Phase: review-fix") && prompt.contains("Fix cycle 1 (full)"), "{prompt}");
    assert!(d["command"].as_array().is_some_and(|c| !c.is_empty()), "{d}");
}

#[test]
fn ns_records_a_run_as_ending_when_its_harness_did() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    e.queue("review", &["pass:script"]);
    e.ctl("review.sh", "sleep 2\n");
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let rec = review_record(&e);
    let (started, ended) = (rec[0]["started"].as_str().unwrap(), rec[0]["ended"].as_str().unwrap());
    assert_eq!(started, "2026-10-09T00:00:00Z", "{rec:?}");
    // The same format sorts by time: the 2s harness ended 2s to a minute after it started.
    assert!(("2026-10-09T00:00:02Z".."2026-10-09T00:01:00Z").contains(&ended), "{rec:?}");
}
