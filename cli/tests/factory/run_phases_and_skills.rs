#[test]
fn run_by_issue_without_a_unit_refuses_when_the_issue_is_unreadable() {
    let e = Env::new();
    e.ns()
        .args(["run", "--issue", "8"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "cannot read issue #8 to name the unit",
        ));
    assert!(e.calls().is_empty());
}

#[test]
fn claude_phases_wait_for_background_tasks() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    let seen = fs::read_to_string(e.ctrl.join("bgwait")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["0"; 5], "{seen}");
}

#[test]
fn a_non_claude_harness_gets_no_bg_wait_ceiling() {
    let e = Env::new();
    let agent = e.bin.join("agent");
    copy_exe(&e.bin.join("claude"), &agent);
    e.config(&format!(
        "[harness.other]\ncommand = [{a:?}]\ncommand_write = [{a:?}]\n",
        a = agent.to_str().unwrap()
    ));
    e.factory("[defaults]\nharness = \"other\"\n");
    e.run(&["run", "--issue", "7"], 0);
    let seen = fs::read_to_string(e.ctrl.join("bgwait")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["unset"; 5], "{seen}");
    let seen = fs::read_to_string(e.ctrl.join("automem")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["unset"; 5], "{seen}");
}

/// `ns run` on a codex harness with every OpenAI API variable set; the variables codex saw.
fn codex_phases_saw(factory: &str) -> String {
    let e = Env::new();
    let codex = e.bin.join("codex");
    copy_exe(&e.bin.join("claude"), &codex);
    e.config(&format!(
        "[harness.codex]\ncommand = [{c:?}]\ncommand_write = [{c:?}]\n",
        c = codex.to_str().unwrap()
    ));
    e.factory(&format!("[defaults]\nharness = \"codex\"\n{factory}"));
    e.ns()
        .env("OPENAI_API_KEY", "k")
        .env("OPENAI_BASE_URL", "u")
        .env("CODEX_API_KEY", "c")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    assert!(
        e.ctrl.join("leak").exists(),
        "the codex scrub removed ANTHROPIC_API_KEY"
    );
    fs::read_to_string(e.ctrl.join("openai")).unwrap_or_default()
}

#[test]
fn a_codex_phase_sees_the_snapshot_through_agents_skills_but_gets_no_claude_settings() {
    let e = Env::new();
    let codex = e.bin.join("codex");
    copy_exe(&e.bin.join("claude"), &codex);
    e.config(&format!(
        "[harness.codex]\ncommand = [{c:?}]\ncommand_write = [{c:?}]\n",
        c = codex.to_str().unwrap()
    ));
    e.factory("[defaults]\nharness = \"codex\"\n");
    e.run(&["run", "--issue", "7"], 0);
    let args = fs::read_to_string(e.ctrl.join("args")).unwrap();
    for flag in ["--settings", "--setting-sources", "--add-dir"] {
        assert!(!args.split_whitespace().any(|a| a == flag), "{args}");
    }
    let wt = e.worktree(UNIT);
    for link in [".claude/skills", ".agents/skills"] {
        let target = fs::read_link(wt.join(link)).unwrap();
        assert!(
            target.ends_with(format!(".ns/{UNIT}/skills")),
            "{link}: {target:?}"
        );
        assert!(wt.join(link).join("ns-demo").is_dir(), "{link}");
    }
}

#[test]
fn codex_phases_on_a_subscription_run_without_openai_api_keys() {
    assert_eq!(codex_phases_saw(""), "");
}

#[test]
fn codex_phases_under_api_billing_keep_openai_api_keys() {
    let seen = codex_phases_saw("billing = \"api\"\n");
    assert_eq!(
        seen.lines().collect::<Vec<_>>(),
        ["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"].repeat(5),
        "{seen}"
    );
}

#[test]
fn claude_phases_wait_for_background_tasks_under_api_billing() {
    let e = Env::new();
    e.factory("[defaults]\nbilling = \"api\"\n");
    e.run(&["run", "--issue", "7"], 0);
    let seen = fs::read_to_string(e.ctrl.join("bgwait")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["0"; 5], "{seen}");
}

#[test]
fn a_bg_wait_ceiling_the_user_set_wins() {
    let e = Env::new();
    e.ns()
        .env("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS", "900000")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let seen = fs::read_to_string(e.ctrl.join("bgwait")).unwrap();
    assert!(seen.lines().all(|l| l == "900000"), "{seen}");
}

#[test]
fn claude_phases_run_with_auto_memory_off_whatever_the_operator_set() {
    let e = Env::new();
    e.ns()
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "0")
        .args(["run", "--issue", "7"])
        .assert()
        .code(0);
    let seen = fs::read_to_string(e.ctrl.join("automem")).unwrap();
    assert_eq!(seen.lines().collect::<Vec<_>>(), ["1"; 5], "{seen}");
}

/// Run the `PreToolUse` hook from a claude phase's `--settings` on one Bash command; the
/// reason it denied the command, or `None` when it let it through.
fn hook_denies(settings: &Value, command: &str) -> Option<String> {
    let input = serde_json::json!({"tool_name": "Bash", "tool_input": {"command": command}});
    hook_says(settings, &input.to_string())
}

/// The deny reason the `PreToolUse` hook prints for hook input `input`, if any.
fn hook_says(settings: &Value, input: &str) -> Option<String> {
    let hook = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let out = common::command("sh")
        .args(["-c", hook])
        .write_stdin(input)
        .output();
    assert!(out.status.success(), "{input}: {out:?}");
    if out.stdout.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let o = &v["hookSpecificOutput"];
    assert_eq!(o["hookEventName"], "PreToolUse");
    assert_eq!(o["permissionDecision"], "deny");
    Some(o["permissionDecisionReason"].as_str().unwrap().to_string())
}

/// Run unit 7 through every phase and return the last phase's argv and the settings file its
/// `--settings` names.
fn phase_settings(e: &Env) -> (Vec<String>, Value) {
    e.run(&["run", "--issue", "7"], 0);
    let args = fs::read_to_string(e.ctrl.join("args")).unwrap();
    let argv: Vec<String> = args.split_whitespace().map(String::from).collect();
    let i = argv
        .iter()
        .position(|a| a == "--settings")
        .expect("--settings");
    let text = fs::read_to_string(&argv[i + 1]).unwrap();
    (argv, serde_json::from_str(&text).unwrap())
}

#[test]
fn a_claude_phase_hook_denies_every_git_hook_bypass() {
    let e = Env::new();
    let (_, settings) = phase_settings(&e);
    assert_eq!(
        settings["hooks"]["PreToolUse"][0]["matcher"],
        "Bash|Edit|Write|MultiEdit|NotebookEdit"
    );
    for bypass in [
        "git commit --no-verify -m x",
        "git commit -n -m x",
        "git push --no-verify",
        "git merge --no-verify main",
        "git rebase --no-verify origin/main",
        "git -c core.hooksPath=/dev/null commit -m x",
        "git config core.hooksPath /tmp/none",
        "rm .githooks/pre-commit",
        "echo 'exit 0' > .githooks/pre-push",
        "HUSKY=0 git commit -m x",
        "SKIP=ci git commit -m x",
    ] {
        let why = hook_denies(&settings, bypass).unwrap_or_else(|| panic!("allowed: {bypass}"));
        assert!(why.contains("skills/ns-contract"), "{bypass}: {why}");
        assert!(why.contains("Git hooks"), "{bypass}: {why}");
    }
    for ok in [
        "git commit -m x",
        "git push origin ns/x",
        "echo 'git commit --no-verify'",
    ] {
        assert_eq!(hook_denies(&settings, ok), None, "{ok}");
    }
    assert_eq!(hook_says(&settings, "not json"), None);
}

#[test]
fn claude_phases_read_only_project_settings_and_the_factory_settings_file() {
    let e = Env::new();
    fs::write(
        e.home.join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(git remote set-url:*)"]}}"#,
    )
    .unwrap();
    let d = e.run(&["run", "--issue", "7", "--dry-run"], 0);
    let file = e
        .root
        .join(".git/ns/units")
        .join(UNIT)
        .join("phase-settings.json");
    let skills = e.worktree(UNIT).join(".ns").join(UNIT).join("skills");
    let dry: Vec<&str> = d["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let tail = [
        "--setting-sources",
        "project,local",
        "--settings",
        file.to_str().unwrap(),
        "--add-dir",
        skills.to_str().unwrap(),
    ];
    assert!(dry.windows(6).any(|w| w == tail), "{dry:?}");

    let (argv, settings) = phase_settings(&e);
    assert!(argv.windows(6).any(|w| w == tail), "{argv:?}");
    let deny: Vec<&str> = settings["permissions"]["deny"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    for rule in [
        "Bash(sudo:*)",
        "Bash(terraform apply:*)",
        "Read(//**/.env)",
        "Read(//**/secrets/**)",
    ] {
        assert!(deny.contains(&rule), "{rule}: {deny:?}");
    }
    let sources: Vec<&String> = argv
        .iter()
        .enumerate()
        .filter(|(_, a)| a.starts_with("--setting-sources"))
        .map(|(i, _)| &argv[i + 1])
        .collect();
    assert_eq!(sources, ["project,local"], "{argv:?}");
}

/// Commit `files` (path, text) in the main checkout and push them to `origin/main`.
fn commit_files(e: &Env, files: &[(&str, &str)], msg: &str) {
    for (path, text) in files {
        let p = e.root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    git(&e.root, &["add", "-A"]);
    git(&e.root, &["commit", "-q", "-m", msg]);
    git(&e.root, &["push", "-q", "origin", "main"]);
}

#[test]
fn phases_run_the_base_commits_skills_whatever_the_worktree_or_main_hold_later() {
    let e = Env::new();
    commit_files(
        &e,
        &[
            ("skills/ns-x/SKILL.md", "v1\n"),
            ("skills/ns-x/references/r.md", "ref\n"),
            (".agents/skills/verify-x/SKILL.md", "verify\n"),
        ],
        "skills v1",
    );
    e.run(&["worktree", "new", UNIT], 0);
    let wt = e.worktree(UNIT);
    fs::write(wt.join("skills/ns-x/SKILL.md"), "worktree edit\n").unwrap();
    git(&wt, &["commit", "-qam", "edit a skill in the unit"]);
    commit_files(&e, &[("skills/ns-x/SKILL.md", "v2\n")], "skills v2");
    e.run(&["run", "--issue", "7"], 0);

    let seen = |p: &str| fs::read_to_string(wt.join(".claude/skills").join(p)).unwrap();
    assert_eq!(seen("ns-x/SKILL.md"), "v1\n");
    assert_eq!(seen("ns-x/references/r.md"), "ref\n");
    assert_eq!(seen("verify-x/SKILL.md"), "verify\n");
    let snap = wt.join(".ns").join(UNIT).join("skills/ns-x/SKILL.md");
    assert!(!fs::symlink_metadata(&snap).unwrap().is_symlink());
    // A resume keeps the snapshot it wrote first, even once the unit has merged main's v2.
    git(
        &wt,
        &["merge", "-q", "--no-edit", "origin/main", "-X", "ours"],
    );
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(seen("ns-x/SKILL.md"), "v1\n");
    // The repo tracks .agents/skills, so the worktree keeps its own copy there.
    assert!(!fs::symlink_metadata(wt.join(".agents/skills"))
        .unwrap()
        .is_symlink());
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
}

#[test]
fn a_skill_in_both_skills_and_agents_skills_pins_the_agents_copy() {
    let e = Env::new();
    commit_files(
        &e,
        &[
            ("skills/ns-x/SKILL.md", "from skills\n"),
            (".agents/skills/ns-x/SKILL.md", "from agents\n"),
        ],
        "clashing skill",
    );
    e.run(&["run", "--issue", "7"], 0);
    let snap = e.worktree(UNIT).join(".ns").join(UNIT).join("skills");
    assert_eq!(
        fs::read_to_string(snap.join("ns-x/SKILL.md")).unwrap(),
        "from agents\n"
    );
    assert!(!snap.with_extension("tmp").exists());
}

#[test]
fn the_snapshot_links_are_excluded_from_git() {
    let e = Env::new();
    e.run(&["worktree", "new", UNIT], 0);
    let wt = e.worktree(UNIT);
    let leftover = wt.join(".ns").join(UNIT).join("skills.tmp/ns-junk");
    fs::create_dir_all(&leftover).unwrap();
    e.run(&["run", "--issue", "7"], 0);
    assert!(!wt.join(".ns").join(UNIT).join("skills/ns-junk").exists());
    for link in [".claude/skills", ".agents/skills"] {
        let target = fs::read_link(wt.join(link)).unwrap();
        assert_eq!(
            wt.join(link)
                .parent()
                .unwrap()
                .join(&target)
                .canonicalize()
                .unwrap(),
            wt.join(".ns")
                .join(UNIT)
                .join("skills")
                .canonicalize()
                .unwrap(),
            "{link}"
        );
    }
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
}

/// Install an `ns-y` skill under the fake home's `~/.claude/skills`, as a symlink into a
/// checkout, and return the checkout's skill dir.
fn install_skill_y(e: &Env) -> PathBuf {
    let src = e.base.join("checkout/skills/ns-y");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), "installed\n").unwrap();
    fs::create_dir_all(e.home.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink(&src, e.home.join(".claude/skills/ns-y")).unwrap();
    src
}

/// Remove the repo's `skills/` from `main` and `origin/main`.
fn drop_repo_skills(e: &Env) {
    git(&e.root, &["rm", "-rq", "skills"]);
    git(&e.root, &["commit", "-q", "-m", "drop skills"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
}

fn run_failing(e: &Env, args: &[&str]) -> String {
    let out = e.ns().args(args).output();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn without_skills_in_the_repo_the_snapshot_copies_the_installed_ns_skills() {
    let e = Env::new();
    drop_repo_skills(&e);
    let src = install_skill_y(&e);
    e.run(&["run", "--issue", "7"], 0);
    fs::write(src.join("SKILL.md"), "changed later\n").unwrap();
    let snap = e.worktree(UNIT).join(".ns").join(UNIT).join("skills/ns-y");
    assert!(!fs::symlink_metadata(&snap).unwrap().is_symlink());
    assert_eq!(
        fs::read_to_string(snap.join("SKILL.md")).unwrap(),
        "installed\n"
    );
    let pinned = events_named(&e, "skills_pinned");
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0]["source"], "installed");
}

#[test]
fn a_skills_tree_without_ns_skills_falls_back_to_the_installed_ones() {
    let e = Env::new();
    drop_repo_skills(&e);
    commit_files(
        &e,
        &[("skills/other/SKILL.md", "not ours\n")],
        "other skills",
    );
    install_skill_y(&e);
    e.run(&["run", "--issue", "7"], 0);
    let skills = e.worktree(UNIT).join(".ns").join(UNIT).join("skills");
    assert_eq!(
        fs::read_to_string(skills.join("ns-y/SKILL.md")).unwrap(),
        "installed\n"
    );
    assert_eq!(events_named(&e, "skills_pinned")[0]["source"], "installed");
}

#[test]
fn a_repo_with_no_ns_skills_to_pin_is_refused_and_leaves_no_snapshot() {
    let e = Env::new();
    drop_repo_skills(&e);
    let err = run_failing(&e, &["run", "--issue", "7"]);
    assert!(err.contains("no ns-* skills to pin"), "{err}");
    let unit = e.worktree(UNIT).join(".ns").join(UNIT);
    assert!(!unit.join("skills").exists());
    assert!(!unit.join("skills.tmp").exists());
    assert!(e.calls().is_empty(), "{:?}", e.calls());
}

#[test]
fn an_empty_snapshot_from_an_older_run_is_rebuilt() {
    let e = Env::new();
    e.run(&["worktree", "new", UNIT], 0);
    let skills = e.worktree(UNIT).join(".ns").join(UNIT).join("skills");
    fs::create_dir_all(&skills).unwrap();
    e.run(&["run", "--issue", "7"], 0);
    assert!(skills.join("ns-demo/SKILL.md").is_file());
}

#[test]
fn an_unresolvable_base_errors_and_leaves_no_snapshot() {
    let e = Env::new();
    e.run(&["worktree", "new", UNIT], 0);
    let err = run_failing(&e, &["run", "--issue", "7", "--base", "no-such-base"]);
    assert!(err.contains("no-such-base"), "{err}");
    assert!(!e
        .worktree(UNIT)
        .join(".ns")
        .join(UNIT)
        .join("skills")
        .exists());
    assert!(!e
        .worktree(UNIT)
        .join(".ns")
        .join(UNIT)
        .join("skills.tmp")
        .exists());
}

#[test]
fn a_repo_with_skills_but_no_agents_skills_pins_both_links() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    let wt = e.worktree(UNIT);
    assert!(!wt.join("../myrepo/.agents/skills").exists());
    for link in [".claude/skills", ".agents/skills"] {
        assert!(
            wt.join(link).join("ns-demo/SKILL.md").is_file(),
            "{link} shows the snapshot"
        );
    }
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
}

#[test]
fn the_pin_logs_its_sha_source_and_skill_count_once_and_not_on_a_resume() {
    let e = Env::new();
    commit_files(
        &e,
        &[(".agents/skills/verify-x/SKILL.md", "v\n")],
        "local skill",
    );
    e.run(&["run", "--issue", "7"], 0);
    let pinned = events_named(&e, "skills_pinned");
    assert_eq!(pinned.len(), 1, "{pinned:?}");
    assert_eq!(pinned[0]["sha"], git(&e.root, &["rev-parse", "main"]));
    assert_eq!(pinned[0]["source"], "base");
    assert_eq!(pinned[0]["skills"], 2);
    let unpinned = events_named(&e, "skills_unpinned");
    assert_eq!(unpinned.len(), 1, "{unpinned:?}");
    assert_eq!(unpinned[0]["path"], ".agents/skills");
    assert!(unpinned[0]["why"].as_str().unwrap().contains("tracked"));
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(events_named(&e, "skills_pinned").len(), 1);
}

/// `ns run` stops with a usage error (2) before any phase.
fn run_unpinnable(e: &Env) -> String {
    let out = e.ns().args(["run", "--issue", "7"]).output();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_tracked_claude_skills_dir_or_symlink_stops_the_run() {
    let e = Env::new();
    commit_files(
        &e,
        &[(".claude/skills/own/SKILL.md", "own\n")],
        "tracked dir",
    );
    let err = run_unpinnable(&e);
    assert!(err.contains(".claude/skills"), "{err}");
    assert!(err.contains("real directory"), "{err}");
    assert!(e.calls().is_empty());

    let e = Env::new();
    fs::create_dir_all(e.root.join(".claude")).unwrap();
    std::os::unix::fs::symlink("../elsewhere", e.root.join(".claude/skills")).unwrap();
    git(&e.root, &["add", "-f", ".claude/skills"]);
    git(&e.root, &["commit", "-q", "-m", "tracked symlink"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let err = run_unpinnable(&e);
    assert!(err.contains("not the unit's snapshot"), "{err}");
    assert_eq!(
        fs::read_link(e.worktree(UNIT).join(".claude/skills")).unwrap(),
        Path::new("../elsewhere")
    );
}

#[test]
fn a_tracked_agents_skills_symlink_is_kept_and_reported() {
    let e = Env::new();
    fs::create_dir_all(e.root.join(".agents")).unwrap();
    std::os::unix::fs::symlink("../elsewhere", e.root.join(".agents/skills")).unwrap();
    git(&e.root, &["add", "-f", ".agents/skills"]);
    git(&e.root, &["commit", "-q", "-m", "tracked symlink"]);
    git(&e.root, &["push", "-q", "origin", "main"]);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("warning: .agents/skills is a symlink that is not the unit's snapshot"));
    let wt = e.worktree(UNIT);
    assert_eq!(
        fs::read_link(wt.join(".agents/skills")).unwrap(),
        Path::new("../elsewhere")
    );
    let ev = events_named(&e, "skills_unpinned");
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(ev[0]["path"], ".agents/skills");
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
}

#[test]
fn a_stale_snapshot_link_is_pointed_at_the_unit_again() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    let wt = e.worktree(UNIT);
    fs::remove_file(wt.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink("../.ns/other/skills", wt.join(".claude/skills")).unwrap();
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(
        fs::read_link(wt.join(".claude/skills")).unwrap(),
        Path::new("../.ns").join(UNIT).join("skills")
    );
    assert!(events_named(&e, "skills_unpinned").is_empty());
}

#[test]
fn the_phase_settings_file_is_written_again_on_a_later_run_and_before_each_phase() {
    let e = Env::new();
    e.run(&["run", "--issue", "7"], 0);
    let file = e
        .root
        .join(".git/ns/units")
        .join(UNIT)
        .join("phase-settings.json");
    assert!(!e
        .worktree(UNIT)
        .join(".ns")
        .join(UNIT)
        .join("phase-settings.json")
        .exists());
    let good = fs::read_to_string(&file).unwrap();
    fs::write(&file, "{}").unwrap();
    e.run(&["run", "--issue", "7"], 0);
    assert_eq!(fs::read_to_string(&file).unwrap(), good);

    // Build deletes the file; verify and review must still find it whole.
    let e = Env::new();
    let seen = e.ctrl.join("seen");
    let check =
        format!("cat \"$NS_PHASE_SETTINGS\" >> {seen:?} 2>/dev/null || echo MISSING >> {seen:?}\n");
    e.ctl("build.sh", "rm -f \"$NS_PHASE_SETTINGS\"\n");
    e.ctl("verify.sh", &check);
    e.ctl("review.sh", &check);
    e.queue("build", &["pass:script"]);
    e.queue("verify", &["pass:script"]);
    e.queue("review", &["pass:script"]);
    e.run(&["run", "--issue", "7"], 0);
    let seen = fs::read_to_string(seen).unwrap();
    assert!(!seen.contains("MISSING"), "{seen}");
    assert_eq!(seen.matches("PreToolUse").count(), 2, "{seen}");
}

#[test]
fn happy_path_triage_to_done() {
    let e = Env::new();
    e.queue("build", &["pass:commit"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["unit"], UNIT);
    assert_eq!(v["outcome"], "done");
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    assert_eq!(v["cost_usd"], 2.5);
    assert_eq!(v["phases"].as_array().unwrap().len(), 5);
    assert!(
        !e.ctrl.join("leak").exists(),
        "ANTHROPIC_API_KEY reached claude"
    );
    let args = fs::read_to_string(e.ctrl.join("args")).unwrap();
    assert!(args.contains("bypassPermissions"), "{args}");
    assert!(
        args.contains("--output-format stream-json --verbose"),
        "{args}"
    );
    let common = e.root.join(".git");
    let log = fs::read_to_string(common.join("ns/runs.jsonl")).unwrap();
    assert!(log.lines().count() >= 7);
    assert!(log.contains("\"outcome\":\"done\""));
    assert!(common
        .join(format!("ns/transcripts/{UNIT}/verify-1.jsonl"))
        .is_file());
    assert!(e.prompt(1, "triage").contains("ns-triage"));
    assert!(e
        .prompt(1, "triage")
        .contains("https://github.com/o/r/issues/7"));
    let lock = common.join(format!("ns-run-{UNIT}.lock"));
    assert_eq!(fs::read_to_string(&lock).unwrap(), "");
    assert!(lock_is_free(&lock));
}

/// Fail, naming the tool, rather than let a missing tool time out a phase or pass as a clean pack.
fn need_python_tools() {
    for tool in ["pytest", "ruff", "ast-grep"] {
        let found = StdCommand::new(tool).arg("--version").output();
        assert!(
            found.is_ok_and(|o| o.status.success()),
            "the py-inventory tests need `{tool}` on PATH; install it with scripts/ci-tools.sh install {tool}"
        );
    }
}

/// A copy of the py-inventory eval fixture on default branch `branch`, with the Python rule pack
/// installed as `ns-setup` installs it, the fixture's existing violations baselined, and a
/// `ci-local` row that runs its tests.
fn py_inventory(branch: &str) -> Env {
    need_python_tools();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    Env::with_repo(branch, |root| {
        common::copy_dir(&repo.join("evals/fixtures/py-inventory"), root);
        let pack = root.join(".nightshift/rules/python");
        fs::create_dir_all(&pack).unwrap();
        for f in ["ruff.toml", "sgconfig.yml", "file-size.sh"] {
            common::copy_exe(&repo.join("rules/python").join(f), &pack.join(f));
        }
        for d in ["rules", "utils"] {
            common::copy_dir(&repo.join("rules/python").join(d), &pack.join(d));
        }
        let allowed = pack.join("utils/allowed-package.yml");
        let text = fs::read_to_string(&allowed).unwrap();
        fs::write(
            &allowed,
            text.replace("(?:)", "(?:inventory|pytest|conftest)"),
        )
        .unwrap();
        let row = fs::read_to_string(repo.join("rules/python/stack-row.md")).unwrap();
        let row = row
            .replace("{pack}", ".nightshift/rules/python")
            .replace("{target}", ".");
        let stack = root.join("docs/agents/stack.md");
        let text = fs::read_to_string(&stack).unwrap();
        let typecheck = "| typecheck | none | |\n";
        assert!(text.contains(typecheck), "{text}");
        let rows = format!("{typecheck}{row}| ci-local | `pytest -q` | 1 s |\n");
        fs::write(&stack, text.replace(typecheck, &rows)).unwrap();
        common::ns()
            .current_dir(root)
            .args(["rules", "baseline", "--allow-growth"])
            .assert()
            .success();
        let baseline = fs::read_to_string(root.join(".nightshift/rules-baseline.json")).unwrap();
        let v: Value = serde_json::from_str(&baseline).unwrap();
        assert!(
            v["violations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["message"]
                    .as_str()
                    .unwrap()
                    .contains("[suppression-reason]")),
            "the baseline holds more than the fixture's unexplained suppressions: {baseline}"
        );
    })
}

/// The log of every gate run, for a failure message.
fn gate_logs(e: &Env) -> String {
    gate_events(e)
        .iter()
        .filter_map(|g| g["log"].as_str())
        .map(|l| format!("--- {l}\n{}\n", fs::read_to_string(l).unwrap_or_default()))
        .collect()
}

/// Every artifact the unit's phases wrote, for a failure message.
fn artifacts(e: &Env, unit: &str) -> String {
    let dir = e.worktree(unit).join(".ns").join(unit);
    let mut out = String::new();
    for f in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let text = fs::read_to_string(f.path()).unwrap_or_default();
        out.push_str(&format!("--- {}\n{text}\n", f.path().display()));
    }
    out
}

/// `ns run` takes one unit of py-inventory, on default branch `branch`, to done: the gate after
/// build runs the fixture's own `ci-local` row and the Python rule pack, and no step makes a
/// `main` branch.
fn py_inventory_reaches_done(branch: &str) {
    let e = py_inventory(branch);
    e.queue("build", &["pass:commit"]);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    assert!(
        out.status.success() && v["outcome"] == "done",
        "{}\nstdout={v}\nstderr={}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        artifacts(&e, UNIT) + &gate_logs(&e)
    );
    assert_eq!(e.calls(), ["triage", "build", "verify", "review", "ship"]);
    let g = gate_events(&e);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0]["green"], true, "{g:?}");
    let cmd = g[0]["command"].as_str().unwrap();
    assert!(
        cmd.starts_with("(pytest -q) && ") && cmd.ends_with(" rules check"),
        "{cmd}"
    );
    let log = fs::read_to_string(g[0]["log"].as_str().unwrap()).unwrap();
    for proof in [
        " passed in ",
        "ns rules: ruff check --config .nightshift/rules/python/ruff.toml",
        "ns rules: ast-grep scan",
        "ns rules: .nightshift/rules/python/file-size.sh .",
    ] {
        assert!(log.contains(proof), "no {proof:?} in the gate log:\n{log}");
    }
    let refs = git(&e.root, &["for-each-ref", "--format=%(refname)"]);
    assert!(!refs.lines().any(|r| r.ends_with("/main")), "{refs}");
}

#[test]
fn py_inventory_on_branch_trunk_reaches_done() {
    py_inventory_reaches_done("trunk");
}

#[test]
fn py_inventory_on_branch_develop_reaches_done() {
    py_inventory_reaches_done("develop");
}

#[test]
fn verify_fail_goes_back_to_build_with_feedback() {
    let e = Env::new();
    e.queue("build", &["pass:commit", "pass:commit"]);
    e.queue("verify", &["fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "verify", "review", "ship"]
    );
    let p = e.prompt(4, "build");
    assert!(p.contains("attempt 2"), "{p}");
    assert!(p.contains("verify said fail"), "{p}");
}

/// Shell that adds a commit to `origin`'s main, as another actor merging a PR would.
fn move_main_sh(e: &Env) -> String {
    let r = e.base.join("remote.git");
    format!(
        "r={r:?}; c=$(git --git-dir=\"$r\" commit-tree \"$(git --git-dir=\"$r\" rev-parse main^{{tree}})\" -p main -m other) && git --git-dir=\"$r\" update-ref refs/heads/main \"$c\"",
        r = r.display()
    )
}

/// Like [`move_main_sh`], but the new commit on main also sets `work.txt`, a file the fake
/// build changes, so a PR behind it overlaps it.
fn move_main_in_work_sh(e: &Env) -> String {
    let r = e.base.join("remote.git");
    format!(
        "r={r:?}; i=$(mktemp -u); b=$(echo main | git --git-dir=\"$r\" hash-object -w --stdin) \
         && GIT_INDEX_FILE=$i git --git-dir=\"$r\" read-tree main \
         && GIT_INDEX_FILE=$i git --git-dir=\"$r\" update-index --add --cacheinfo \"100644,$b,work.txt\" \
         && t=$(GIT_INDEX_FILE=$i git --git-dir=\"$r\" write-tree) && rm -f \"$i\" \
         && c=$(git --git-dir=\"$r\" commit-tree \"$t\" -p main -m other) && git --git-dir=\"$r\" update-ref refs/heads/main \"$c\"",
        r = r.display()
    )
}

fn remote_main(e: &Env) -> String {
    git(&e.base.join("remote.git"), &["rev-parse", "main"])
}

fn rebase_events(e: &Env) -> Vec<Value> {
    fs::read_to_string(e.root.join(".git/ns/runs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["event"] == "rebase_after_base_moved")
        .collect()
}

/// Build 1 commits and another actor moves main; verify gets five attempts, so only build's cap
/// can end the unit.
fn base_moves_during_build(e: &Env, extra: &str) {
    e.factory(&format!("[phases.verify]\nmax_attempts = 5\n{extra}"));
    e.ctl(
        "build.sh",
        &format!(
            "echo b >> work.txt && git add work.txt && git -c user.name=f -c user.email=f@f commit -qm b && {}",
            move_main_sh(e)
        ),
    );
}

#[test]
fn a_verify_no_go_for_base_movement_rebases_onto_the_newest_main_at_no_attempts_cost() {
    let e = Env::new();
    base_moves_during_build(&e, "");
    e.queue("build", &["pass:script", "pass:commit", "pass:commit"]);
    e.queue("verify", &["fail:base moved", "fail", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "verify", "build", "verify", "review", "ship"]
    );
    let main = remote_main(&e);
    let p = e.prompt(4, "build");
    assert!(p.contains("base moved"), "{p}");
    assert!(
        p.contains(&format!("rebase onto the newest origin/main ({main})")),
        "{p}"
    );
    assert!(!e.prompt(6, "build").contains("rebase onto the newest"));
    let wt = e.worktree(UNIT);
    assert_eq!(git(&wt, &["rev-parse", "origin/main"]), main, "fetched");
    let r = rebase_events(&e);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["attempt"], 2);
    assert_eq!(r[0]["onto"], main.as_str());
    assert_eq!(r[0]["cause"], "evidence.md failed");
}

#[test]
fn a_ship_no_go_for_base_movement_goes_to_build_at_no_attempts_cost() {
    let e = Env::new();
    base_moves_during_build(&e, "[phases.ship]\nmax_attempts = 5\n");
    e.queue("build", &["pass:script", "pass:commit", "pass:commit"]);
    e.queue("verify", &["pass", "fail", "pass"]);
    e.queue("ship", &["fail:base moved", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        &e.calls()[..6],
        ["triage", "build", "verify", "review", "ship", "build"]
    );
    let r = rebase_events(&e);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["cause"], "pr.md failed: base moved");
}

#[test]
fn the_ship_after_a_free_rebase_build_is_not_free() {
    let e = Env::new();
    base_moves_during_build(&e, "");
    e.queue("build", &["pass:script", "pass:commit"]);
    e.queue("verify", &["fail:base moved", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        e.calls(),
        ["triage", "build", "verify", "build", "verify", "review", "ship"]
    );
    assert!(!run_events(&e)
        .iter()
        .any(|ev| ev["event"] == "ship_after_base_moved"));
}

#[test]
fn a_free_rebase_build_that_fails_leaves_the_unit_stuck_at_the_normal_cap() {
    let e = Env::new();
    base_moves_during_build(&e, "");
    e.queue("build", &["pass:script", "fail", "pass:commit"]);
    e.queue("verify", &["fail:base moved"]);
    let v = e.run(&["run", "--issue", "7"], 1);
    assert_eq!(v["reason"], "build is out of attempts (2)", "{v}");
    assert_eq!(e.calls(), ["triage", "build", "verify", "build"]);
}

#[test]
fn a_review_no_go_for_base_movement_goes_to_build_at_no_attempts_cost() {
    let e = Env::new();
    base_moves_during_build(&e, "[phases.review]\nmax_attempts = 5\n");
    e.queue("build", &["pass:script", "pass:commit", "pass:commit"]);
    e.queue("verify", &["pass", "fail", "pass"]);
    e.queue("review", &["fail:base moved", "pass"]);
    let v = e.run(&["run", "--issue", "7"], 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(
        &e.calls()[..6],
        ["triage", "build", "verify", "review", "build", "verify"]
    );
    let r = rebase_events(&e);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["cause"], "review.md failed");
}

#[test]
fn a_free_rebase_build_that_times_out_leaves_the_unit_stuck_at_the_normal_cap() {
    let e = Env::new();
    base_moves_during_build(&e, "");
    e.queue("build", &["pass:script", "blocked:sleep", "pass:commit"]);
    e.queue("verify", &["fail:base moved"]);
    let out = e
        .ns()
        .env("NS_PHASE_TIMEOUT_MS", "build=1500")
        .args(["run", "--issue", "7"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .starts_with("build is out of attempts (2)"),
        "{v}"
    );
    assert_eq!(e.calls(), ["triage", "build", "verify", "build"]);
}
