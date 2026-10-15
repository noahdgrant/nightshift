/// An env whose repo tracks a `.githooks/pre-commit`, with build's script set to `script`.
fn tamper_env(script: &str) -> Env {
    let e = Env::with_repo("main", |root| {
        fs::write(root.join("README"), "hi\n").unwrap();
        fs::create_dir(root.join(".githooks")).unwrap();
        fs::write(root.join(".githooks/pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
    });
    e.queue("build", &["pass:script"]);
    e.ctl("build.sh", script);
    e
}

/// Run the unit until build's tamper check stops it, and return the run's reason.
fn run_tampered(e: &Env) -> String {
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["outcome"], "stuck", "{v}");
    assert_eq!(v["phase"], "build", "{v}");
    let log = fs::read_to_string(e.root.join(".git/ns/runs.jsonl")).unwrap();
    let ev = log
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .rfind(|ev| ev["event"] == "phase" && ev["phase"] == "build")
        .unwrap();
    assert_eq!(ev["status"], "tampered", "{ev}");
    v["reason"].as_str().unwrap().to_string()
}

#[test]
fn a_phase_that_changes_a_protected_path_is_failed_as_tampered_and_the_path_put_back() {
    let snapshot_skill = format!(".ns/{UNIT}/skills/ns-demo/SKILL.md");
    let cases: [(&str, String, &str); 9] = [
        (
            "common config",
            "git config core.hooksPath /dev/null\n".into(),
            ".git/config",
        ),
        (
            "worktree config",
            "printf '[core]\\n\\thooksPath = /dev/null\\n' > \"$(git rev-parse --git-dir)/config.worktree\"\n".into(),
            "config.worktree",
        ),
        (
            "git hooks",
            "printf 'exit 0\\n' > \"$(git rev-parse --git-common-dir)/hooks/pre-commit\"\n".into(),
            ".git/hooks/pre-commit",
        ),
        (
            "githooks edit",
            "printf 'exit 1\\n' > .githooks/pre-commit\n".into(),
            ".githooks/pre-commit",
        ),
        ("githooks removal", "rm -r .githooks\n".into(), ".githooks"),
        (
            "skill snapshot",
            format!("echo evil > {snapshot_skill}\n"),
            "skills/ns-demo/SKILL.md",
        ),
        (
            "phase settings",
            "echo '{}' > \"$NS_PHASE_SETTINGS\"\n".into(),
            "phase-settings.json",
        ),
        (
            "claude skills link",
            "rm .claude/skills && mkdir \"$TMPDIR/elsewhere\" && ln -s \"$TMPDIR/elsewhere\" .claude/skills\n".into(),
            ".claude/skills",
        ),
        (
            "agents skills link",
            "rm .agents/skills && mkdir .agents/skills\n".into(),
            ".agents/skills",
        ),
    ];
    for (name, script, shown) in cases {
        let e = tamper_env(&script);
        let settings = e.root.join(format!(".git/ns/units/{UNIT}/phase-settings.json"));
        let reason = run_tampered(&e);
        assert!(reason.contains("tampered"), "{name}: {reason}");
        assert!(reason.contains(shown), "{name}: {reason}");
        assert!(reason.contains("put it back"), "{name}: {reason}");
        let wt = e.worktree(UNIT);
        let archived = wt.join(format!(".ns/{UNIT}/history/build-tampered-1.md"));
        assert!(archived.is_file(), "{name}: what the phase wrote is set aside");
        assert!(!wt.join(format!(".ns/{UNIT}/build.md")).exists(), "{name}");
        let config = fs::read_to_string(e.root.join(".git/config")).unwrap();
        assert!(!config.contains("hooksPath"), "{name}: {config}");
        let wt_git = PathBuf::from(git(&wt, &["rev-parse", "--absolute-git-dir"]));
        assert!(!wt_git.join("config.worktree").exists(), "{name}");
        assert!(!e.root.join(".git/hooks/pre-commit").exists(), "{name}");
        assert_eq!(
            fs::read_to_string(wt.join(".githooks/pre-commit")).unwrap(),
            "#!/bin/sh\nexit 0\n",
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(wt.join(&snapshot_skill)).unwrap(),
            "demo\n",
            "{name}"
        );
        assert!(
            fs::read_to_string(&settings).unwrap().contains("guard-hooks"),
            "{name}"
        );
        for link in [".claude/skills", ".agents/skills"] {
            let target = fs::read_link(wt.join(link)).unwrap();
            assert_eq!(target, Path::new("..").join(".ns").join(UNIT).join("skills"), "{name}: {link}");
        }
    }
}

#[test]
fn a_phase_that_tampers_and_writes_no_artifact_still_fails_as_tampered() {
    let e = tamper_env("printf 'exit 1\\n' > .githooks/pre-commit\n");
    e.queue("build", &["none:script"]);
    let reason = run_tampered(&e);
    assert!(reason.contains(".githooks/pre-commit"), "{reason}");
}

#[test]
fn a_phase_that_sets_its_branch_upstream_and_writes_artifacts_is_not_tampering() {
    let e = tamper_env(
        "git push -q -u origin \"ns/$NS_UNIT\"\necho notes > .ns/$NS_UNIT/notes.md\n",
    );
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let config = fs::read_to_string(e.root.join(".git/config")).unwrap();
    assert!(config.contains(&format!("[branch \"ns/{UNIT}\"]")), "{config}");
}

#[test]
fn a_phase_that_tampers_and_times_out_is_still_checked() {
    let e = tamper_env("printf 'exit 1\\n' > .githooks/pre-commit\n");
    e.queue("build", &["pass:script+sleep"]);
    let out = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "build=2000")
        .args(["run", "--issue", "7"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["outcome"], "stuck", "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("build tampered") && reason.contains(".githooks/pre-commit"), "{reason}");
    let hook = fs::read_to_string(e.worktree(UNIT).join(".githooks/pre-commit")).unwrap();
    assert_eq!(hook, "#!/bin/sh\nexit 0\n");
}

#[test]
fn a_phase_that_tampers_and_is_stopped_is_put_back_and_says_so() {
    let e = Env::new();
    e.ready(2, "Fix a", &["type:fix"], "");
    let f = fifo(&e);
    e.ctl(
        "build.sh",
        &format!(
            "printf 'exit 0\\n' > \"$(git rev-parse --git-common-dir)/hooks/pre-commit\"\n{}\n",
            blocks(&f)
        ),
    );
    e.queue("build", &["pass:script"]);
    let r = interrupt(&e, f, libc::SIGTERM, false);
    assert_interrupted(&e, &r, "SIGTERM", 143);
    let ev = run_events(&e)
        .into_iter()
        .find(|ev| ev["event"] == "phase" && ev["phase"] == "build" && ev["interrupted"] == "SIGTERM")
        .unwrap();
    let why = ev["tampered"].as_str().unwrap();
    assert!(why.contains(".git/hooks/pre-commit"), "{why}");
    assert!(!e.root.join(".git/hooks/pre-commit").exists());
}

#[test]
fn a_rerun_that_tampers_puts_back_the_artifact_it_replaced() {
    let e = tamper_env("printf 'exit 1\\n' > .githooks/pre-commit\n");
    e.queue("build", &["fail", "pass:script"]);
    let reason = run_tampered(&e);
    assert!(reason.contains(".githooks/pre-commit"), "{reason}");
    let unit = e.worktree(UNIT).join(format!(".ns/{UNIT}"));
    let build = fs::read_to_string(unit.join("build.md")).unwrap();
    assert!(build.contains("status: fail"), "{build}");
    assert!(unit.join("history/build-tampered-1.md").is_file());
}

/// Every phase on the built-in `codex` command, run by the fake harness under that name.
fn builtin_codex(e: &Env) {
    copy_exe(&e.bin.join("claude"), &e.bin.join("codex"));
    e.factory("[defaults]\nharness = \"codex\"\n");
}

#[test]
fn a_codex_phase_with_full_access_that_changes_a_protected_path_is_failed_as_tampered() {
    let e = tamper_env("printf 'exit 1\\n' > .githooks/pre-commit\n");
    builtin_codex(&e);
    let reason = run_tampered(&e);
    assert!(reason.contains(".githooks/pre-commit"), "{reason}");
    let args = fs::read_to_string(e.ctrl.join("args")).unwrap();
    assert!(args.starts_with("exec --sandbox danger-full-access"), "{args}");
    let hook = fs::read_to_string(e.worktree(UNIT).join(".githooks/pre-commit")).unwrap();
    assert_eq!(hook, "#!/bin/sh\nexit 0\n");
}
