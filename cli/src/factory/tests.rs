use super::*;

const FULL: &str = r#"
name = "nightshift"
gates = "auto"
[worktree]
setup = ["git submodule update --init"]
[defaults]
harness = "claude"
model = "opus"
timeout_minutes = 45
max_attempts = 2
[phases.triage]
skill = "ns-triage"
[phases.build]
skill = "ns-build"
timeout_minutes = 90
[phases.verify]
skill = "ns-verify"
[phases.review]
skill = "ns-review"
harness = "codex"
[phases.ship]
skill = "ns-ship"
[queue]
source = "github"
ready_label = "status:ready-for-agent"
in_progress_label = "status:in-progress"
done_label = "status:in-review"
stuck_label = "status:ready-for-human"
order = ["type:fix", "type:feat"]
[limits]
max_units = 4
budget_usd = 25.0
[merge]
policy = "auto"
human_review = [".github/**", ".nightshift/**", "cli/src/run.rs"]
ci_timeout_minutes = 30
"#;

#[test]
fn parses_the_spec_example() {
    let f = parse(FULL).unwrap();
    assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
    let b = f.phase("build");
    assert_eq!(b.timeout_minutes, 90);
    assert_eq!(b.model.as_deref(), Some("opus"));
    assert_eq!(f.phase("review").harness, "codex");
    assert_eq!(f.worktree.setup.len(), 1);
    assert_eq!(f.budget_usd(), Some(25.0));
    assert!(f.subscription());
    assert!(f.merge.auto());
    let hits = f.merge.human_review_hits(&[
        ".github/workflows/ci.yml".into(),
        "cli/src/run.rs".into(),
        "cli/src/main.rs".into(),
        ".nightshift/agents/build/agent.md".into(),
    ]);
    assert_eq!(hits.len(), 3, "{hits:?}");
    assert!(!parse("").unwrap().merge.auto());
}

#[test]
fn rejects_unknown_keys_everywhere() {
    for bad in [
        "nmae = \"x\"\n",
        "[defaults]\nharnes = \"claude\"\n",
        "[phases.build]\nskil = \"x\"\n",
        "[queue]\nready = \"x\"\n",
        "[limits]\nmax = 1\n",
        "[worktree]\nsetups = []\n",
        "[automations]\n",
        "[merge]\npolcy = \"auto\"\n",
        "[supervise]\nsilence = 45\n",
        "[quality]\nremotes = \"origin\"\n",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn memory_mb_is_off_by_default_and_must_be_positive() {
    assert_eq!(parse("").unwrap().limits.memory_mb, None);
    let f = parse("[limits]\nmemory_mb = 8192\n").unwrap();
    assert_eq!(f.limits.memory_mb, Some(8192));
    assert!(f.problems(None).is_empty());
    let f = parse("[limits]\nmemory_mb = 0\n").unwrap();
    assert_eq!(
        f.problems(None),
        ["limits.memory_mb must be at least 1; remove it for no cap"]
    );
    assert!(parse("[limits]\nmemory_mb = -1\n").is_err());
}

#[test]
fn disk_limits_default_to_20_and_10_gib_and_stop_must_not_exceed_wait() {
    let f = parse("").unwrap();
    assert_eq!((f.limits.disk_wait_gb(), f.limits.disk_stop_gb()), (20, 10));
    let f = parse("[limits]\ndisk_wait_gb = 50\ndisk_stop_gb = 50\n").unwrap();
    assert_eq!((f.limits.disk_wait_gb(), f.limits.disk_stop_gb()), (50, 50));
    assert!(f.problems(None).is_empty());
    let f = parse("[limits]\ndisk_wait_gb = 5\n").unwrap();
    assert_eq!(
        f.problems(None),
        ["limits.disk_stop_gb = 10 is above limits.disk_wait_gb = 5; the stop limit must not exceed the wait limit"]
    );
    assert!(parse("[limits]\ndisk_stop_gb = -1\n").is_err());
}

#[test]
fn a_wait_limit_of_zero_turns_the_wait_off_without_the_stop_limit_set() {
    for toml in [
        "[limits]\ndisk_wait_gb = 0\n",
        "[limits]\ndisk_wait_gb = 0\ndisk_stop_gb = 0\n",
    ] {
        assert!(parse(toml).unwrap().problems(None).is_empty(), "{toml}");
    }
}

#[test]
fn defaults_and_budget_by_billing() {
    let f = parse("").unwrap();
    assert_eq!(f.gates, "auto");
    assert_eq!(f.phase("verify").skill, "ns-verify");
    assert_eq!(f.phase("verify").max_attempts, 2);
    assert_eq!(f.budget_usd(), None);
    assert_eq!(f.merge.ci_register_timeout, 3);
    assert!(f.merge.local_queue);
    assert_eq!(f.queue.triage_label, "status:needs-triage");
    assert_eq!(f.queue.split_label, "status:split");
    let f = parse("[queue]\ntriage_label = \"triage\"\n").unwrap();
    assert_eq!(f.queue.triage_label, "triage");
    let f = parse("[merge]\nci_register_timeout = 7\n").unwrap();
    assert_eq!(f.merge.ci_register_timeout, 7);
    assert!(f.merge.local_queue);
    let f = parse("[merge]\nlocal_queue = false\n").unwrap();
    assert!(!f.merge.local_queue);
    let f = parse("[defaults]\nbilling = \"api\"\n").unwrap();
    assert_eq!(f.budget_usd(), Some(25.0));
}

#[test]
fn auto_merge_with_no_human_review_warns() {
    for quiet in [
        "[merge]\npolicy = \"human\"\n",
        "[merge]\npolicy = \"auto\"\nhuman_review = [\".github/**\"]\n",
    ] {
        assert!(parse(quiet).unwrap().warnings().is_empty(), "{quiet}");
    }
    let f = parse("[merge]\npolicy = \"auto\"\n").unwrap();
    let w = f.warnings();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(
        w[0].contains("merge.policy = \"auto\"") && w[0].contains("merge.human_review is empty"),
        "{w:?}"
    );
    assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
}

#[test]
fn a_removed_triage_per_night_parses_with_a_warning() {
    assert!(parse("").unwrap().warnings().is_empty());
    assert!(parse(FULL).unwrap().warnings().is_empty());
    for old in ["10", "0", "\"ten\""] {
        let f = parse(&format!("[queue]\ntriage_per_night = {old}\n")).unwrap();
        let w = f.warnings();
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].starts_with("[queue] triage_per_night is no longer used"),
            "{w:?}"
        );
        // A 0 meant "off", so its warning says the pass runs and names the real switch.
        let off = w[0].contains("0 no longer turns the triage pass off")
            && w[0].contains("gates = \"stop\"");
        assert_eq!(off, old == "0", "{w:?}");
        assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
    }
}

#[test]
fn a_build_gate_parses_and_is_unset_by_default() {
    assert_eq!(parse("").unwrap().phase("build").gate, None);
    let f = parse("[phases.build]\ngate = \"scripts/ci-local.sh\"\n").unwrap();
    assert_eq!(
        f.phase("build").gate.as_deref(),
        Some("scripts/ci-local.sh")
    );
    assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
}

#[test]
fn a_gate_off_build_or_empty_is_a_problem() {
    let f = parse("[phases.verify]\ngate = \"make ci\"\n").unwrap();
    let p = f.problems(None).join("\n");
    assert!(p.contains("phase verify: gate"), "{p}");
    let f = parse("[phases.build]\ngate = \" \"\n").unwrap();
    let p = f.problems(None).join("\n");
    assert!(p.contains("phase build: gate"), "{p}");
}

#[test]
fn supervise_defaults_to_no_notify_and_45_minutes_of_silence() {
    let f = parse("").unwrap();
    assert_eq!(f.supervise.notify, None);
    assert_eq!(f.supervise.silence_minutes, 45);
    let f = parse("[supervise]\nnotify = \"notify-send ns\"\nsilence_minutes = 30\n").unwrap();
    assert_eq!(f.supervise.notify.as_deref(), Some("notify-send ns"));
    assert_eq!(f.supervise.silence_minutes, 30);
    assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
}

#[test]
fn a_zero_silence_or_empty_notify_is_a_problem() {
    let p = parse("[supervise]\nsilence_minutes = 0\n")
        .unwrap()
        .problems(None);
    assert_eq!(p, ["[supervise] silence_minutes must be at least 1"]);
    let p = parse("[supervise]\nnotify = \" \"\n")
        .unwrap()
        .problems(None);
    assert_eq!(
        p,
        ["[supervise] notify is empty; remove it or name a command"]
    );
}

#[test]
fn a_quality_remote_that_is_not_a_remote_name_is_refused_where_it_is_used() {
    for bad in [
        "",
        "  ",
        "-x",
        "--upload-pack=touch x",
        "a b",
        "a..b",
        "a~1",
        "x/",
    ] {
        let f = parse(&format!("[quality]\nremote = {bad:?}\n")).unwrap();
        assert!(f.quality.target().is_err(), "{bad:?}");
    }
}

#[test]
fn quality_remote_defaults_to_origin_and_none_turns_the_push_off() {
    let f = parse("").unwrap();
    assert_eq!(f.quality.remote, "origin");
    assert_eq!(f.quality.target(), Ok(Some("origin")));
    let f = parse("[quality]\nremote = \"records\"\n").unwrap();
    assert_eq!(f.quality.target(), Ok(Some("records")));
    assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
    let f = parse("[quality]\nremote = \"none\"\n").unwrap();
    assert_eq!(f.quality.target(), Ok(None));
    assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
}

#[test]
fn an_empty_or_dash_quality_remote_is_a_problem() {
    for bad in ["", " ", "-x"] {
        let p = parse(&format!("[quality]\nremote = {bad:?}\n"))
            .unwrap()
            .problems(None);
        assert_eq!(p.len(), 1, "{bad:?}: {p:?}");
        assert!(p[0].starts_with("[quality] remote"), "{p:?}");
    }
}

#[test]
fn one_minute_of_silence_is_not_a_problem() {
    let f = parse("[supervise]\nsilence_minutes = 1\n").unwrap();
    assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
}

#[test]
fn queue_priority_defaults_and_overrides() {
    let f = parse("").unwrap();
    assert_eq!(
        f.queue.priority,
        ["priority:high", "priority:medium", "priority:low"]
    );
    let f = parse("[queue]\npriority = [\"p0\", \"p1\"]\n").unwrap();
    assert_eq!(f.queue.priority, ["p0", "p1"]);
    assert_eq!(f.queue.order[0], "type:fix");
}

#[test]
fn queue_source_is_unset_by_default_and_github_or_gitlab_is_no_problem() {
    assert_eq!(parse("").unwrap().queue.source, None);
    for source in ["github", "gitlab"] {
        let f = parse(&format!("[queue]\nsource = {source:?}\n")).unwrap();
        assert_eq!(f.queue.source.as_deref(), Some(source));
        assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
    }
    let p = parse("[queue]\nsource = \"svn\"\n").unwrap().problems(None);
    assert_eq!(
        p,
        ["[queue] source = \"svn\": unknown; use \"github\" or \"gitlab\""]
    );
}

#[test]
fn semantic_problems() {
    let f = parse(
        "gates = \"maybe\"\n[phases.deploy]\n[defaults]\nharness = \"nope\"\nmax_attempts = 0\n",
    )
    .unwrap();
    let p = f.problems(None).join("\n");
    assert!(p.contains("gates"));
    assert!(p.contains("unknown phase"));
    assert!(p.contains("\"nope\""));
    assert!(p.contains("max_attempts"));
}

fn runners(files: &[(&str, &str)]) -> (BTreeMap<String, Runner>, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("runners")).unwrap();
    for (name, text) in files {
        fs::write(tmp.path().join("runners").join(name), text).unwrap();
    }
    load_runners(tmp.path())
}

#[test]
fn runner_files_load_with_local_as_the_default_kind() {
    let (r, bad) = runners(&[
        ("zephyr.toml", "locks = [\"zephyr-workspace\"]\n"),
        (
            "bench.toml",
            "kind = \"bench\"\nlocks = [\"bench-1\", \"zephyr-workspace\"]\n",
        ),
        ("README.md", "not a runner"),
    ]);
    assert!(bad.is_empty(), "{bad:?}");
    assert_eq!(r.len(), 2);
    assert_eq!(r["zephyr"].kind, RunnerKind::Local);
    assert_eq!(r["bench"].kind, RunnerKind::Bench);
}

#[test]
fn a_bad_runner_file_is_a_problem() {
    let (r, bad) = runners(&[
        ("remote.toml", "kind = \"remote\"\n"),
        ("typo.toml", "lock = [\"a\"]\n"),
        ("notalist.toml", "locks = \"a\"\n"),
    ]);
    assert!(r.is_empty());
    assert_eq!(bad.len(), 3, "{bad:?}");
    assert!(bad[1].starts_with("runners/remote.toml"), "{bad:?}");
}

#[test]
fn runner_references_and_lock_names_are_checked() {
    let mut f = parse("[phases.verify]\nrunner = \"bench\"\n").unwrap();
    let p = f.problems(None).join("\n");
    assert!(
        p.contains("phase verify: runner \"bench\" has no runners/bench.toml"),
        "{p}"
    );
    f.runners.insert(
        "bench".into(),
        Runner {
            kind: RunnerKind::Bench,
            locks: vec![
                "../x".into(),
                "bench-1".into(),
                "bench-1".into(),
                "a/b".into(),
                ".hidden".into(),
                "..".into(),
            ],
        },
    );
    let p = f.problems(None);
    assert_eq!(p.len(), 4, "{p:?}");
    assert!(p[0].contains("\"../x\""), "{p:?}");
    assert!(f.locks(&f.phase("build")).is_empty());
}

#[test]
fn lock_names_that_differ_only_by_case_are_a_problem() {
    let mut f = parse("").unwrap();
    for (runner, lock) in [("a", "Bench"), ("b", "bench"), ("c", "Bench")] {
        f.runners.insert(
            runner.into(),
            Runner {
                kind: RunnerKind::Local,
                locks: vec![lock.into()],
            },
        );
    }
    let p = f.problems(None);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("runners/b.toml"), "{p:?}");
    assert!(p[0].contains("only by case"), "{p:?}");
}

#[test]
fn render_is_single_pass() {
    let v = PromptVars {
        skill: "ns-build",
        unit: "7-x",
        issue: "7",
        issue_url: "https://x/7",
        worktree: "/w",
        gates: "auto",
        phase: "build",
        attempt: 2,
        feedback: "keep {unit} literal",
    };
    let s = render("{skill} {unit} {attempt} {nope} {feedback}", &v);
    assert_eq!(s, "ns-build 7-x 2 {nope} keep {unit} literal");
}

#[test]
fn queue_overlap_ignore_is_empty_by_default_and_takes_globs() {
    assert!(parse("").unwrap().queue.overlap_ignore.is_empty());
    let f = parse("[queue]\noverlap_ignore = [\"docs/DESIGN.md\", \"docs/**\"]\n").unwrap();
    assert_eq!(f.queue.overlap_ignore, ["docs/DESIGN.md", "docs/**"]);
    assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
    assert!(parse("[queue]\noverlap_ignores = []\n").is_err());
    let p = parse("[queue]\noverlap_ignore = [\"a[\"]\n")
        .unwrap()
        .problems(None);
    assert_eq!(p, ["queue.overlap_ignore: \"a[\" is not a valid glob"]);
}

#[test]
fn gate_local_defaults_to_every_step_and_takes_build_only() {
    assert_eq!(parse("").unwrap().gate.local, LocalGate::EveryStep);
    let f = parse("[gate]\nlocal = \"every-step\"\n").unwrap();
    assert_eq!(f.gate.local, LocalGate::EveryStep);
    let f = parse("[gate]\nlocal = \"build-only\"\n").unwrap();
    assert_eq!(f.gate.local, LocalGate::BuildOnly);
    assert!(f.problems(None).is_empty(), "{:?}", f.problems(None));
}

#[test]
fn gate_local_rejects_other_values_and_keys() {
    let e = parse("[gate]\nlocal = \"never\"\n")
        .unwrap_err()
        .to_string();
    assert!(e.contains("build-only") && e.contains("every-step"), "{e}");
    assert!(parse("[gate]\nremote = \"github\"\n").is_err());
}

#[test]
fn build_only_warns_when_the_ci_local_row_shows_no_fast_support() {
    let build_only = parse("[gate]\nlocal = \"build-only\"\n").unwrap();
    let make = "| ci-local | `make ci` | 2 min |\n";
    let w = build_only.fast_gate_warning(make).expect("a warning");
    assert!(w.contains("ci-local") && w.contains("--fast"), "{w}");
    assert!(w.contains("`make ci --fast`"), "{w}");
    for shows_fast in [
        "| ci-local | `scripts/ci-local.sh` (`--fast` runs the quick steps) | 1 min |\n",
        "| ci-local | `make ci --fast` | 2 min |\n",
    ] {
        assert_eq!(
            build_only.fast_gate_warning(shows_fast),
            None,
            "{shows_fast}"
        );
    }
    for no_row in ["", "| ci-local | none | |\n", "| build | `make` | 1 s |\n"] {
        assert_eq!(build_only.fast_gate_warning(no_row), None, "{no_row:?}");
    }
    assert_eq!(parse("").unwrap().fast_gate_warning(make), None);
    let every = parse("[gate]\nlocal = \"every-step\"\n").unwrap();
    assert_eq!(every.fast_gate_warning(make), None);
}

#[test]
fn build_only_accepts_any_ci_local_row_showing_fast() {
    let build_only = parse("[gate]\nlocal = \"build-only\"\n").unwrap();
    let mixed =
        "| ci-local | `make ci` | 2 min |\n| ci-local | `scripts/ci-local.sh --fast` | 1 min |\n";
    assert_eq!(build_only.fast_gate_warning(mixed), None);
    let neither = "| ci-local | `make ci` | 2 min |\n| ci-local | `make check` | 1 min |\n";
    let w = build_only.fast_gate_warning(neither).expect("a warning");
    assert!(w.contains("`make ci --fast`"), "{w}");
}

#[test]
fn build_only_checks_a_build_gate_override_in_place_of_the_row() {
    let gate = |cmd: &str| {
        parse(&format!(
            "[gate]\nlocal = \"build-only\"\n[phases.build]\ngate = \"{cmd}\"\n"
        ))
        .unwrap()
    };
    let fast_row = "| ci-local | `scripts/ci-local.sh --fast` | 1 min |\n";
    let w = gate("make ci")
        .fast_gate_warning(fast_row)
        .expect("a warning");
    assert!(
        w.contains("[phases.build] gate") && w.contains("`make ci --fast`"),
        "{w}"
    );
    assert!(gate("make ci").fast_gate_warning("").is_some());
    let plain_row = "| ci-local | `make ci` | 2 min |\n";
    assert_eq!(gate("ci.sh --fast").fast_gate_warning(plain_row), None);
}

#[test]
fn build_only_warns_only_when_the_owner_is_a_personal_account() {
    let build_only = parse("[gate]\nlocal = \"build-only\"\n").unwrap();
    let w = build_only.owner_warning(Some("User")).expect("a warning");
    assert!(
        w.contains("no GitHub merge queue") && w.contains("local = \"every-step\""),
        "{w}"
    );
    for owner in [Some("Organization"), Some(""), Some("user"), None] {
        assert_eq!(build_only.owner_warning(owner), None, "{owner:?}");
    }
    assert_eq!(parse("").unwrap().owner_warning(Some("User")), None);
    let every = parse("[gate]\nlocal = \"every-step\"\n").unwrap();
    assert_eq!(every.owner_warning(Some("User")), None);
}

#[test]
fn review_cycles_come_from_phases_review_and_fall_back_to_three_full_and_two_fix_only() {
    let d = Cycles {
        full: 3,
        fix_only: 2,
    };
    assert_eq!(parse("").unwrap().cycles(), d);
    let set = parse("[phases.review]\nfull_cycles = 1\nfix_only_cycles = 0\n").unwrap();
    assert_eq!(
        set.cycles(),
        Cycles {
            full: 1,
            fix_only: 0
        }
    );
    assert!(set.warnings().is_empty(), "{:?}", set.warnings());
    assert!(set.problems(None).is_empty(), "{:?}", set.problems(None));
    let bad = parse("[phases.review]\nfull_cycles = -1\nfix_only_cycles = \"two\"\n").unwrap();
    assert_eq!(bad.cycles(), d);
    let w = bad.warnings();
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(
        w[0].contains("full_cycles = -1") && w[0].contains("default, 3"),
        "{w:?}"
    );
    assert!(
        w[1].contains("fix_only_cycles = \"two\"") && w[1].contains("default, 2"),
        "{w:?}"
    );
    assert!(bad.problems(None).is_empty(), "{:?}", bad.problems(None));
}

#[test]
fn cycle_limits_outside_phases_review_are_problems() {
    let f = parse("[phases.build]\nfull_cycles = 2\n[phases.ship]\nfix_only_cycles = 1\n").unwrap();
    let p = f.problems(None);
    assert!(
        p.iter()
            .any(|x| x
                == "phase build: full_cycles is only read for review; move it to [phases.review]"),
        "{p:?}"
    );
    assert!(
        p.iter().any(|x| x
            == "phase ship: fix_only_cycles is only read for review; move it to [phases.review]"),
        "{p:?}"
    );
}

#[test]
fn review_fix_runs_with_reviews_settings_and_writes_fix_md() {
    assert_eq!(config_phase("review-fix"), "review");
    assert_eq!(config_phase("build"), "build");
    assert_eq!(artifact_of("review-fix"), "fix.md");
    assert!(RUN_PHASES.iter().all(|p| PHASES.contains(&config_phase(p))));
}

#[test]
fn a_fix_cycle_takes_reviews_settings_under_its_own_name() {
    let f = parse("[phases.review]\nrunner = \"bench\"\nmax_attempts = 7\ntimeout_minutes = 13\n")
        .unwrap();
    let p = f.phase("review-fix");
    assert_eq!(p.name, "review-fix");
    assert_eq!(p.skill, "ns-review");
    assert_eq!((p.runner.as_deref(), p.max_attempts), (Some("bench"), 7));
    assert_eq!(p.timeout_minutes, 13);
}
