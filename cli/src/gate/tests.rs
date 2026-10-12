use super::*;
use crate::factory::parse;

const STACK: &str = "| Task | Command |\n|---|---|\n| build | `cargo build` |\n| ci-local | `scripts/ci-local.sh` | 30 s |\n";

#[test]
fn ci_local_reads_the_table_row() {
    assert_eq!(ci_local(STACK).as_deref(), Some("scripts/ci-local.sh"));
    assert_eq!(
        ci_local("| `ci-local` | `make ci` (all of CI) |").as_deref(),
        Some("make ci")
    );
    assert_eq!(ci_local("| build | `cargo build` |"), None);
    assert_eq!(ci_local("ci-local: `make ci`"), None);
    assert_eq!(ci_local("| ci-local | none |"), None);
    assert_eq!(ci_local("| ci-local | `` |"), None);
    assert_eq!(ci_local("| ci-local | n/a | see `docs` |"), None);
}

#[test]
fn ci_local_keeps_pipes_inside_the_command() {
    assert_eq!(
        ci_local("| ci-local | `make ci | tee x` | 1 s |").as_deref(),
        Some("make ci | tee x")
    );
    assert_eq!(
        ci_local("| ci-local | `a || b` |").as_deref(),
        Some("a || b")
    );
}

#[test]
fn the_configured_gate_wins_over_stack_md_and_neither_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    let none = parse("").unwrap();
    assert_eq!(command(&none, tmp.path(), Path::new(NS_EXE)), None);
    fs::create_dir_all(tmp.path().join("docs/agents")).unwrap();
    fs::write(tmp.path().join("docs/agents/stack.md"), STACK).unwrap();
    assert_eq!(
        command(&none, tmp.path(), Path::new(NS_EXE)).as_deref(),
        Some("scripts/ci-local.sh")
    );
    let set = parse("[phases.build]\ngate = \"make ci\"\n").unwrap();
    assert_eq!(
        command(&set, tmp.path(), Path::new(NS_EXE)).as_deref(),
        Some("make ci")
    );
}

const NS_EXE: &str = "/opt/ns bin/ns";

#[test]
fn a_rules_row_adds_ns_rules_check_after_the_gate_or_alone() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("docs/agents")).unwrap();
    let stack = tmp.path().join("docs/agents/stack.md");
    fs::write(&stack, format!("{STACK}| rules | `ruff check .` |\n")).unwrap();
    let none = parse("").unwrap();
    assert_eq!(
        command(&none, tmp.path(), Path::new(NS_EXE)).as_deref(),
        Some("(scripts/ci-local.sh) && '/opt/ns bin/ns' rules check")
    );
    let set = parse("[phases.build]\ngate = \"make ci\"\n").unwrap();
    assert_eq!(
        command(&set, tmp.path(), Path::new(NS_EXE)).as_deref(),
        Some("(make ci) && '/opt/ns bin/ns' rules check")
    );
    fs::write(&stack, "| rules | `ruff check .` |\n").unwrap();
    assert_eq!(
        command(&none, tmp.path(), Path::new(NS_EXE)).as_deref(),
        Some("'/opt/ns bin/ns' rules check")
    );
    fs::write(&stack, "| rules | none yet |\n").unwrap();
    assert_eq!(command(&none, tmp.path(), Path::new(NS_EXE)), None);
}

#[test]
fn a_ratchet_adds_ns_ratchet_check_last_or_alone() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("docs/agents")).unwrap();
    let stack = tmp.path().join("docs/agents/stack.md");
    let fac = parse("[ratchets.flash]\ncommand = \"echo 1\"\nbetter = \"lower\"\n").unwrap();
    fs::write(&stack, format!("{STACK}| rules | `ruff check .` |\n")).unwrap();
    assert_eq!(
        command(&fac, tmp.path(), Path::new(NS_EXE)).as_deref(),
        Some("(scripts/ci-local.sh) && '/opt/ns bin/ns' rules check && '/opt/ns bin/ns' ratchet check --human")
    );
    fs::write(&stack, STACK).unwrap();
    assert_eq!(
        command(&fac, tmp.path(), Path::new(NS_EXE)).as_deref(),
        Some("(scripts/ci-local.sh) && '/opt/ns bin/ns' ratchet check --human")
    );
    fs::write(&stack, "").unwrap();
    assert_eq!(
        command(&fac, tmp.path(), Path::new(NS_EXE)).as_deref(),
        Some("'/opt/ns bin/ns' ratchet check --human")
    );
}

#[test]
fn every_step_runs_the_full_command_everywhere() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("docs/agents")).unwrap();
    fs::write(
        tmp.path().join("docs/agents/stack.md"),
        format!("{STACK}| ci-affected | `scripts/affected.sh` |\n"),
    )
    .unwrap();
    let c = Commands::read(&parse("").unwrap(), tmp.path(), Path::new(NS_EXE));
    assert_eq!(c.build.as_deref(), Some("scripts/ci-local.sh"));
    assert_eq!(c.rest, c.build);
    assert!(c.full("scripts/ci-local.sh"));
    assert_eq!(
        c.for_trigger(Trigger::BeforeMerge),
        Some("scripts/ci-local.sh")
    );
}

#[test]
fn build_only_runs_the_fast_command_and_affected_tests_after_any_step_but_build() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("docs/agents")).unwrap();
    let stack = tmp.path().join("docs/agents/stack.md");
    let fac = parse("[gate]\nlocal = \"build-only\"\n").unwrap();
    let read = |text: &str| {
        fs::write(&stack, text).unwrap();
        Commands::read(&fac, tmp.path(), Path::new(NS_EXE))
    };
    let c = read(&format!(
        "{STACK}| ci-affected | `scripts/affected.sh` |\n| rules | `ruff check .` |\n"
    ));
    let full = "(scripts/ci-local.sh) && '/opt/ns bin/ns' rules check";
    let fast = "(scripts/ci-local.sh --fast) && (scripts/affected.sh)";
    assert_eq!(c.build.as_deref(), Some(full));
    assert_eq!(c.rest.as_deref(), Some(fast));
    assert_eq!(c.for_trigger(Trigger::BuildPassed), Some(full));
    assert_eq!(c.for_trigger(Trigger::BuildWroteNothing), Some(full));
    assert_eq!(c.for_trigger(Trigger::PhaseMovedHead), Some(fast));
    assert_eq!(c.for_trigger(Trigger::BeforeMerge), Some(fast));
    assert!(c.full(full) && !c.full(fast));
    assert_eq!(
        read(STACK).rest.as_deref(),
        Some("(scripts/ci-local.sh --fast)")
    );
    assert_eq!(
        read("| ci-affected | `scripts/affected.sh` |\n")
            .rest
            .as_deref(),
        Some("(scripts/affected.sh)")
    );
    assert_eq!(read("").rest, None);
    let fac =
        parse("[gate]\nlocal = \"build-only\"\n[phases.build]\ngate = \"make ci\"\n").unwrap();
    fs::write(&stack, STACK).unwrap();
    let c = Commands::read(&fac, tmp.path(), Path::new(NS_EXE));
    assert_eq!(c.rest.as_deref(), Some("(make ci --fast)"));
}

#[test]
fn this_repos_gate_is_ci_local_sh_then_the_rule_pack() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let cmd = command(&parse("").unwrap(), repo, Path::new(NS_EXE)).unwrap();
    assert_eq!(cmd, "(scripts/ci-local.sh) && '/opt/ns bin/ns' rules check");
    assert!(repo.join("scripts/ci-local.sh").is_file());
}

#[test]
fn the_script_and_both_hooks_are_executable_and_pre_commit_runs_fast_ci_and_pre_push_ci() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for f in [
        "scripts/ci-local.sh",
        ".githooks/pre-commit",
        ".githooks/pre-push",
    ] {
        let mode = std::fs::metadata(repo.join(f))
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "{f} is not executable");
    }
    let text = std::fs::read_to_string(repo.join(".githooks/pre-commit")).unwrap();
    assert_eq!(text.lines().last(), Some("exec scripts/ci-local.sh --fast"));
    // pre-push runs it once per commit pushed; the cli tests pin how.
    let text = std::fs::read_to_string(repo.join(".githooks/pre-push")).unwrap();
    assert!(text.contains("exec scripts/ci-local.sh \"$@\" </dev/null"));
}

fn gate(cmd: &str, timeout: Duration) -> (GateRun, tempfile::TempDir) {
    gate_capped(cmd, timeout, None)
}

fn gate_capped(cmd: &str, timeout: Duration, cap: Option<&Cap>) -> (GateRun, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("logs/gate.log");
    let ratchets = Ratchets {
        factory: tmp.path(),
        night: None,
    };
    let r = run(cmd, tmp.path(), timeout, &log, cap, &ratchets, None).unwrap();
    (r, tmp)
}

const REPORT: &str = r#"{"ratchets": [{"name": "flash", "command": "s", "better": "lower", "tolerance": "0", "recorded": 1, "measured": 2, "runs": [2], "limit": 1, "status": "worse"}, {"name": "cov", "command": "c", "better": "higher", "tolerance": "0", "recorded": 1, "measured": 2, "runs": [2], "limit": 1, "status": "better"}]}"#;

#[test]
fn a_full_gate_sees_ns_gate_files_unset_and_a_fast_gate_exactly_the_files_given() {
    std::env::set_var("NS_GATE_FILES", "stale.txt");
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("gate.log");
    let ratchets = Ratchets {
        factory: tmp.path(),
        night: None,
    };
    let t = Duration::from_secs(30);
    let cmd = "echo \"[$NS_GATE_FILES]\"";
    let full = run(cmd, tmp.path(), t, &log, None, &ratchets, None).unwrap();
    assert_eq!(full.tail, "[]");
    let files = run(
        cmd,
        tmp.path(),
        t,
        &log,
        None,
        &ratchets,
        Some("a.rs\nb.rs"),
    )
    .unwrap();
    assert_eq!(files.tail, "[a.rs\nb.rs]");
    std::env::remove_var("NS_GATE_FILES");
}

#[test]
fn the_gate_tells_ns_ratchet_check_its_factory_night_and_report() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("gate-build-1.log");
    let night = tmp.path().join("night");
    let cmd = format!(
        "echo \"$NS_RATCHET_FACTORY|$NS_RATCHET_STEADY|$NS_RATCHET_REPORT\"; printf '%s' '{REPORT}' > \"$NS_RATCHET_REPORT\"; exit 1"
    );
    let with_night = Ratchets {
        factory: Path::new("fac"),
        night: Some(&night),
    };
    let t = Duration::from_secs(30);
    let r = run(&cmd, tmp.path(), t, &log, None, &with_night, None).unwrap();
    let report = tmp.path().join("gate-build-1.ratchets.json");
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(
        r.tail,
        format!(
            "{}|{}|{}",
            cwd.join("fac").display(),
            night.join("ratchets-steady.json").display(),
            report.display()
        )
    );
    assert_eq!(
        r.ratchets.as_deref(),
        Some(
            "ratchet flash is worse: measured 2, recorded 1, tolerance 0, limit 1, lower is better"
        )
    );
    // A report left by an earlier gate is gone before the next one runs.
    let no_night = Ratchets {
        factory: tmp.path(),
        night: None,
    };
    let r = run(
        "echo \"[$NS_RATCHET_STEADY]\"",
        tmp.path(),
        t,
        &log,
        None,
        &no_night,
        None,
    )
    .unwrap();
    assert_eq!((r.tail.as_str(), r.ratchets), ("[]", None));
    assert!(!report.exists());
    fs::create_dir(&report).unwrap();
    fs::write(report.join("x"), "").unwrap();
    let e = run("true", tmp.path(), t, &log, None, &no_night, None)
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("cannot remove"), "{e:#}");
}

#[test]
fn a_ratchet_failure_is_in_the_feedback_reason_and_stuck_reason() {
    let mut g = red(false, Some(1));
    g.ratchets = Some("ratchet flash is worse: measured 2, recorded 1".into());
    let f = feedback("make ci", &g, Duration::from_secs(60), "abc");
    assert!(
        f.contains("at abc. It failed on ratchet flash is worse: measured 2, recorded 1. Get each value back"),
        "{f}"
    );
    assert!(f.contains("never edit ratchets.json"), "{f}");
    let plain = feedback(
        "make ci",
        &red(false, Some(1)),
        Duration::from_secs(60),
        "abc",
    );
    assert!(plain.contains("at abc. Make it pass."), "{plain}");

    let mut red = Red {
        feedback: String::new(),
        reason: String::new(),
        killed: None,
        ratchets: Some("ratchet flash is worse".into()),
    };
    assert_eq!(
        red.stuck_reason().as_deref(),
        Some("the CI gate failed on ratchet flash is worse")
    );
    red.killed = Some("the CI gate was killed by SIGKILL".into());
    assert_eq!(
        red.stuck_reason().as_deref(),
        Some("the CI gate was killed by SIGKILL")
    );
    red.ratchets = None;
    red.killed = None;
    assert_eq!(red.stuck_reason(), None);
}

const HOG: &str = "python3 -c \"b = b'x' * (256 << 20)\"";

#[test]
fn a_gate_past_the_memory_cap_is_red_and_says_so() {
    let cap = memcap::detect(64).unwrap();
    let (r, _tmp) = gate_capped(HOG, Duration::from_secs(60), Some(&cap));
    assert!(!r.green());
    assert_eq!(r.cut, Some(Cut::Exceeded(64)));
    let f = feedback(HOG, &r, Duration::from_secs(60), "abc123");
    assert!(
        f.contains("exceeded the 64 MB memory limit at abc123"),
        "{f}"
    );
    let (r, _tmp) = gate_capped("echo fits", Duration::from_secs(60), Some(&cap));
    assert!(r.green(), "{}", r.tail);
    assert_eq!(r.cut, None);
}

#[test]
fn a_gate_past_the_rlimit_fallback_is_red_and_says_so() {
    let cap = Cap {
        mb: 64,
        via: memcap::Via::Rlimit,
        fallback: None,
    };
    let (r, _tmp) = gate_capped(HOG, Duration::from_secs(60), Some(&cap));
    assert!(!r.green());
    assert_eq!(r.cut, Some(Cut::Exceeded(64)));
    assert_eq!(
        r.killed().as_deref(),
        Some("exceeded the 64 MB memory limit")
    );
}

#[test]
fn a_capped_gate_that_times_out_is_a_timeout_whatever_it_printed() {
    let cap = Cap {
        mb: 64,
        via: memcap::Via::Rlimit,
        fallback: None,
    };
    let (r, _tmp) = gate_capped(
        "echo MemoryError; exec sleep 30",
        Duration::from_millis(300),
        Some(&cap),
    );
    assert!(r.timed_out);
    assert_eq!(r.cut, None);
}

#[test]
fn an_uncapped_gate_that_fails_with_an_allocation_message_is_an_ordinary_failure() {
    let (r, _tmp) = gate("echo MemoryError; exit 1", Duration::from_secs(30));
    assert_eq!(r.cut, None);
    assert_eq!(r.killed(), None);
    assert_eq!(r.exit, Some(1));
}

#[test]
fn a_zero_exit_is_green_and_runs_in_cwd() {
    let (r, tmp) = gate("touch here && echo ok", Duration::from_secs(30));
    assert!(r.green());
    assert_eq!(r.exit, Some(0));
    assert_eq!(r.tail, "ok");
    assert!(tmp.path().join("here").exists());
}

#[test]
fn the_tail_keeps_blank_lines_between_output_but_not_after_it() {
    let (r, _tmp) = gate("printf 'a\\n\\nb\\n\\n\\n'", Duration::from_secs(30));
    assert_eq!(r.tail, "a\n\nb");
}

#[test]
fn a_non_zero_exit_is_red_with_stdout_and_stderr_in_the_tail() {
    let (r, _tmp) = gate("echo out; echo err >&2; exit 3", Duration::from_secs(30));
    assert!(!r.green());
    assert_eq!(r.exit, Some(3));
    assert_eq!(r.tail, "out\nerr");
}

#[test]
fn a_timeout_is_red() {
    let (r, _tmp) = gate("sleep 30", Duration::from_millis(200));
    assert!(r.timed_out);
    assert!(!r.green());
    assert!(r.wall_s < 10.0, "{}", r.wall_s);
}

#[test]
fn the_tail_keeps_the_last_lines_within_the_byte_cap() {
    let many: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    let t = output_tail(many.as_bytes());
    assert_eq!(t.lines().count(), TAIL_LINES);
    assert!(t.ends_with("line 100"));
    assert!(t.starts_with("line 61"));
    let long = format!("x{}", "é".repeat(4000));
    let t = output_tail(long.as_bytes());
    assert!(t.len() <= TAIL_BYTES);
    assert!(t.chars().all(|c| c == 'é'));
    assert!(t.len() > TAIL_BYTES - 2);
}

#[test]
fn a_signal_kill_is_red_with_no_exit_code() {
    let (r, _tmp) = gate("kill -9 $$", Duration::from_secs(30));
    assert_eq!(r.exit, None);
    assert_eq!(r.cut, Some(Cut::Signal(libc::SIGKILL)));
    assert!(!r.timed_out);
    assert!(!r.green());
    assert_eq!(r.killed().as_deref(), Some("was killed by SIGKILL"));
    let f = feedback("x", &r, Duration::from_secs(30), "a");
    assert!(f.contains("`x` was killed by SIGKILL at a"), "{f}");
}

#[test]
fn a_timed_out_gate_is_not_called_killed() {
    let (r, _tmp) = gate("sleep 30", Duration::from_millis(200));
    assert!(r.timed_out);
    assert_eq!(r.killed(), None);
}

#[test]
fn the_tail_reads_only_the_end_of_a_huge_log() {
    let (r, _tmp) = gate("seq 1 100000", Duration::from_secs(30));
    assert!(r.green());
    assert_eq!(r.tail.lines().count(), TAIL_LINES);
    assert!(r.tail.starts_with("99961"), "{}", r.tail);
    assert!(r.tail.ends_with("100000"));
}

#[test]
fn the_window_drops_a_leading_partial_line() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("big.log");
    let text: String = (0..5000).map(|i| format!("line {i:05}\n")).collect();
    fs::write(&log, &text).unwrap();
    let w = String::from_utf8(read_window(&log)).unwrap();
    assert!((w.len() as u64) <= WINDOW_BYTES);
    assert!(w.starts_with("line "), "{w:?}");
    assert!(w.ends_with("line 04999\n"));
    fs::write(&log, "small\n").unwrap();
    assert_eq!(read_window(&log), b"small\n");
    assert!(read_window(&tmp.path().join("missing")).is_empty());
}

#[test]
fn a_long_final_line_without_a_newline_still_leaves_a_tail() {
    let (r, _tmp) = gate(
        "head -c 20000 /dev/zero | tr '\\0' x",
        Duration::from_secs(30),
    );
    assert!(r.green());
    assert!(!r.tail.is_empty());
    assert!(r.tail.chars().all(|c| c == 'x'));
}

#[test]
fn timed_out_after_uses_minutes_only_for_whole_minutes() {
    assert_eq!(
        timed_out_after(Duration::from_secs(60)),
        "timed out after 1 min"
    );
    assert_eq!(
        timed_out_after(Duration::from_secs(59)),
        "timed out after 59.0 s"
    );
    assert_eq!(
        timed_out_after(Duration::from_millis(60_500)),
        "timed out after 60.5 s"
    );
}

fn red(timed_out: bool, exit: Option<i32>) -> GateRun {
    GateRun {
        exit,
        cut: None,
        timed_out,
        wall_s: 0.0,
        tail: "last line".into(),
        ratchets: None,
    }
}

#[test]
fn feedback_says_how_the_gate_failed() {
    let t = Duration::from_secs(90 * 60);
    let f = feedback("make ci", &red(true, None), t, "abc123");
    assert!(
        f.contains("`make ci` timed out after 90 min at abc123"),
        "{f}"
    );
    assert!(f.ends_with("last line\n"));
    let f = feedback(
        "make ci",
        &red(true, None),
        Duration::from_millis(1500),
        "a",
    );
    assert!(f.contains("timed out after 1.5 s"), "{f}");
    let f = feedback("make ci", &red(false, Some(3)), t, "a");
    assert!(f.contains("exited 3"), "{f}");
    let f = feedback("make ci", &red(false, None), t, "a");
    assert!(f.contains("was killed by a signal"), "{f}");
    let mut r = red(false, Some(1));
    r.cut = Some(Cut::Exceeded(64));
    let f = feedback("make ci", &r, t, "a");
    assert!(
        f.contains("`make ci` exceeded the 64 MB memory limit at a"),
        "{f}"
    );
    r.timed_out = true;
    let f = feedback("make ci", &r, t, "a");
    assert!(f.contains("timed out after 90 min"), "{f}");
}

#[test]
fn a_zero_exit_past_the_cap_is_not_green() {
    let mut r = red(false, Some(0));
    assert!(r.green());
    r.cut = Some(Cut::Exceeded(64));
    assert!(!r.green());
}
