use crate::common;

use assert_cmd::Command;
use common::{git, script, Repo};

/// How the fake `claude` behaves. It builds, and when asked to verify it checks and ends with a
/// verdict.
pub struct Fakes {
    /// The verdict the first check ends with.
    pub first_verdict: &'static str,
    /// The verdict every later check ends with.
    pub verdict: &'static str,
    pub check_edits: CheckEdits,
    /// The exit code of `claude`.
    pub exit: i32,
    /// Whether the fake dies without a word in a check, as a crashed harness would.
    pub crash_in_first_check: bool,
    pub build_commits: BuildCommits,
    pub ci: Ci,
    /// Whether the build runs the checks itself, the way its prompt tells it to.
    pub agent_runs_checks: bool,
    /// The verdict the first review ends with.
    pub first_review: &'static str,
    /// The verdict every later review ends with.
    pub review: &'static str,
    /// The labels the unit has before anything is done to it.
    pub labels: &'static str,
    /// Whether the unit's labels change when `ns` edits them. Without it the unit stays as it began.
    pub labels_follow_edits: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Ci {
    Passes,
    /// The first look at the checks finds one failed; later looks find them passed.
    FailsOnce,
    /// The first look finds them pending; later looks find them passed.
    PendingOnce,
    NeverFinishes,
    /// No check is reported, as before any has started.
    NoneReported,
}

#[derive(Clone, Copy, PartialEq)]
pub enum BuildCommits {
    Always,
    /// The first build leaves its work uncommitted; the next one, told so, commits it.
    OnlyWhenTold,
    Never,
    /// The first build leaves two commits; the next one, told so, squashes them.
    TwoUntilTold,
    /// Every build leaves two commits.
    TwoAlways,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CheckEdits {
    Never,
    /// The first check writes a file into the workspace; later checks remove it again.
    FirstThenFixes,
    /// Every check writes to the workspace.
    Always,
}

const PASS: &str = "VERDICT: PASS";
const FAIL: &str = "VERDICT: FAIL";

impl Fakes {
    pub fn passing() -> Fakes {
        Fakes {
            first_verdict: PASS,
            verdict: PASS,
            check_edits: CheckEdits::Never,
            exit: 0,
            crash_in_first_check: false,
            build_commits: BuildCommits::Always,
            ci: Ci::Passes,
            agent_runs_checks: false,
            first_review: PASS,
            review: PASS,
            labels: r#"[{"name":"status:ready-for-agent"}]"#,
            labels_follow_edits: true,
        }
    }
}

/// A `bin` directory holding a `gh` that serves one unit and the fake `claude`. Returns the
/// `PATH` to start `ns` with, that directory first.
pub fn tools(repo: &Repo, fakes: &Fakes) -> String {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pass = r#"echo '[{"name":"ci","bucket":"pass"}]'"#;
    let first_then_pass = |first: &str| {
        format!(
            r#"if [ -e "{bin}/looked" ]; then {pass}; else touch "{bin}/looked"; {first}; fi"#,
            bin = bin.display()
        )
    };
    let checks = match fakes.ci {
        Ci::Passes => pass.to_string(),
        Ci::FailsOnce => first_then_pass(r#"echo '[{"name":"lint","bucket":"fail"}]'; exit 1"#),
        Ci::PendingOnce => first_then_pass(r#"echo '[{"name":"ci","bucket":"pending"}]'; exit 8"#),
        Ci::NeverFinishes => r#"echo '[{"name":"ci","bucket":"pending"}]'; exit 8"#.to_string(),
        Ci::NoneReported => "echo 'no checks reported' >&2; exit 1".to_string(),
    };
    script(
        &bin,
        "gh",
        &format!(
            r#"echo "$@" >> "{bin}/gh.log"
labels() {{
  l='{initial}'
  [ "{follow}" = yes ] && case "$(grep 'issue edit' "{bin}/gh.log" | tail -1)" in
  *"--add-label type:feat,status:ready-for-agent"*) l='{ready}';;
  *"--add-label status:in-progress"*) l='{progress}';;
  *"--add-label status:ready-for-human"*) l='{human}';;
  *"--remove-label status:in-progress"*) l='[]';;
  esac
}}
case "$1 $2" in
"api repos/{{owner}}/{{repo}}/issues/5") labels; echo '{{"number":5,"title":"Add a thing","body":"Make the thing.","labels":'"$l"',"author_association":"OWNER"}}';;
"api --paginate") labels; echo '[[{{"number":5,"title":"Add a thing","body":"Make the thing.","labels":'"$l"',"author_association":"OWNER"}}]]';;
"pr list") if [ -e "{bin}/merged" ]; then state=MERGED; else state=OPEN; fi
  if [ -e "{bin}/opened" ]; then echo '[{{"number":9,"url":"https://forge.example/9","state":"'$state'"}}]'; else echo '[]'; fi;;
"pr create") touch "{bin}/opened"; echo https://forge.example/9;;
"pr checks") {checks};;
"pr merge") touch "{bin}/merged";;
esac"#,
            bin = bin.display(),
            checks = checks,
            initial = fakes.labels,
            follow = if fakes.labels_follow_edits {
                "yes"
            } else {
                "no"
            },
            ready = r#"[{"name":"status:ready-for-agent"}]"#,
            progress = r#"[{"name":"status:in-progress"}]"#,
            human = r#"[{"name":"status:ready-for-human"}]"#,
        ),
    );
    let edit = match fakes.check_edits {
        CheckEdits::Never => "",
        CheckEdits::FirstThenFixes => {
            r#"if [ -e "$seen" ]; then rm -f touched.txt; else echo x > touched.txt; fi; "#
        }
        CheckEdits::Always => "echo x >> touched.txt; ",
    };
    let run_checks = if fakes.agent_runs_checks {
        r#"ns=$(echo "$p" | sed -n 's/.*run `\(.*\) check` in your workspace.*/\1/p'); [ -n "$ns" ] && "$ns" check 2>/dev/null; "#
    } else {
        ""
    };
    let commit = match fakes.build_commits {
        BuildCommits::Always => r#"git add -A; git commit -q -m "Add a thing"; "#,
        BuildCommits::TwoUntilTold | BuildCommits::TwoAlways => {
            r#"git add -A; git commit -q -m one; echo more > more.txt; git add -A; git commit -q -m two; "#
        }
        BuildCommits::OnlyWhenTold | BuildCommits::Never => "",
    };
    let squash = match fakes.build_commits {
        BuildCommits::TwoUntilTold => {
            r#"git reset -q --soft HEAD~2; git commit -q -m "Add a thing"; "#
        }
        _ => "",
    };
    let when_told = match fakes.build_commits {
        BuildCommits::OnlyWhenTold => r#"git add -A; git commit -q -m "Add a thing"; "#,
        _ => "",
    };
    let crash = if fakes.crash_in_first_check {
        r#"if [ ! -e "$seen" ]; then touch "$seen"; exit 9; fi; "#
    } else {
        ""
    };
    script(
        &bin,
        "claude",
        &format!(
            r#"seen="{bin}/checked"
reviewed="{bin}/reviewed"
p="$(cat)"
case "$p" in
*"decide what happens to one unit"*) printf 'Small.\nLABELS: type:feat\nROUTE: build\n';;
*"Find what must be fixed before this change merges"*)
  if echo "$p" | grep -q thing.txt; then echo reviewed the diff; else echo saw no diff; fi
  if [ -e "$reviewed" ]; then echo '{review}'; else touch "$reviewed"; echo '{first_review}'; fi;;
*"Run the real thing and record what you saw"*)
  {crash}{edit}echo checked it
  if [ -e "$seen" ]; then echo '{verdict}'; else touch "$seen"; echo '{first}'; fi;;
*"VERDICT: FAIL"*) echo rebuilt with the evidence;;
*"The checks failed:"*) touch "{bin}/fixed"; echo fixed it;;
*"left uncommitted changes"*) {when_told}echo committed when told;;
*"commits on the branch"*) {squash}echo squashed when told;;
*) printf %s "$p" > "{bin}/build.prompt"; echo thing > thing.txt; {run_checks}{commit}echo built the thing;;
esac
exit {exit}"#,
            bin = bin.display(),
            verdict = fakes.verdict,
            first = fakes.first_verdict,
            review = fakes.review,
            first_review = fakes.first_review,
            exit = fakes.exit,
        ),
    );
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap())
}

fn run(repo: &Repo, path: &str) -> assert_cmd::assert::Assert {
    run_with(repo, path, &[])
}

/// Runs `ns run 5` with `extra` arguments. It looks at the checks without waiting between looks.
fn run_with(repo: &Repo, path: &str, extra: &[&str]) -> assert_cmd::assert::Assert {
    let mut args = vec!["run", "5", "--gates", "auto", "--ci-poll-seconds", "0"];
    args.extend(extra);
    run_args(repo, path, &args)
}

/// Runs `ns` with exactly `args`.
fn run_args(repo: &Repo, path: &str, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .args(args)
        .env("PATH", path)
        .env("NS_CONFIG_DIR", &repo.config)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .current_dir(&repo.path)
        .assert()
}

fn moves(out: Vec<u8>) -> Vec<String> {
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            match v.get("to") {
                Some(to) => format!("{}>{}", v["from"].as_str().unwrap(), to.as_str().unwrap()),
                None => format!("start {}", v["state"].as_str().unwrap()),
            }
        })
        .collect()
}

fn read(repo: &Repo, file: &str) -> String {
    std::fs::read_to_string(repo.workspace("5").join(".ns/5").join(file)).unwrap()
}

#[test]
fn run_sets_up_builds_verifies_and_prints_each_move() {
    let repo = Repo::new();
    let out = run(&repo, &tools(&repo, &Fakes::passing()))
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        moves(out),
        [
            "start setup",
            "setup>build",
            "build>verify",
            "verify>review",
            "review>ship",
            "ship>clean_up",
            "clean_up>done",
        ]
    );
    assert!(repo.branches().contains(&"ns/5".to_string()));
    assert_eq!(read(&repo, "build.md"), "built the thing\n");
    assert_eq!(
        read(&repo, "evidence.md"),
        "reviewed the diff\nVERDICT: PASS\n"
    );
}

#[test]
fn ship_publishes_what_the_build_committed_and_opens_one_change_request() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    let origin = repo.path.parent().unwrap().join("origin.git");
    assert!(common::git(&origin, &["branch", "--list", "ns/5"]).contains("ns/5"));
    let log = std::fs::read_to_string(repo.path.parent().unwrap().join("bin/gh.log")).unwrap();
    assert_eq!(
        log.matches("pr create --head ns/5 --title Add a thing")
            .count(),
        1,
        "{log}"
    );
    assert!(log.contains("Posted by nightshift, an AI agent"), "{log}");
    assert!(log.contains("Closes #5"), "{log}");
    assert_eq!(read(&repo, "ship.md"), "https://forge.example/9\n");
    let subject = common::git(&repo.workspace("5"), &["log", "-1", "--format=%s"]);
    assert_eq!(subject.trim(), "Add a thing");
    let commits = common::git(&repo.path, &["rev-list", "--count", "main..ns/5"]);
    assert_eq!(
        commits.trim(),
        "1",
        "the harness commits once and ship adds none"
    );
}

fn gh_log(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/gh.log")).unwrap()
}

#[test]
fn review_sees_the_diff_and_a_pass_lets_the_unit_ship() {
    let repo = Repo::new();
    let out = run(&repo, &tools(&repo, &Fakes::passing()))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[3..5], ["verify>review", "review>ship"]);
    assert!(read(&repo, "evidence.md").starts_with("reviewed the diff"));
}

#[test]
fn a_failed_review_sends_the_unit_back_to_build_and_review_looks_again() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_review: FAIL,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    let at = moves
        .iter()
        .position(|m| m == "review>build")
        .expect("review goes back");
    assert_eq!(
        &moves[at..at + 4],
        [
            "review>build",
            "build>verify",
            "verify>review",
            "review>ship"
        ]
    );
    assert_eq!(read(&repo, "build.md"), "rebuilt with the evidence\n");
}

#[test]
fn a_review_that_keeps_failing_sticks_the_unit() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_review: FAIL,
        review: FAIL,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(moves.last().unwrap(), "review>stuck");
    assert!(!moves.iter().any(|m| m.starts_with("ship")), "{moves:?}");
}

/// Makes the repository's checks log each run to `bin/checks.log`, and pass once `bin/fixed` exists
/// when `needs_fix`.
fn set_checks(repo: &Repo, needs_fix: bool) -> std::path::PathBuf {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = bin.join("checks.log");
    let body = if needs_fix {
        format!(
            "if [ -e {}/fixed ]; then exit 0; else echo 'lint: unused thing'; exit 1; fi",
            bin.display()
        )
    } else {
        "exit 0".to_string()
    };
    repo.set_check(&format!("echo ran >> {}\n{body}", log.display()));
    log
}

fn runs(log: &std::path::Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |t| t.lines().count())
}

#[test]
fn checks_the_agent_ran_itself_are_not_run_again_after_the_build() {
    let repo = Repo::new();
    let log = set_checks(&repo, false);
    let fakes = Fakes {
        agent_runs_checks: true,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    assert_eq!(runs(&log), 1);
}

#[test]
fn checks_the_agent_did_not_run_are_run_after_the_build() {
    let repo = Repo::new();
    let log = set_checks(&repo, false);
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(runs(&log), 1);
}

#[test]
fn failing_checks_send_the_build_around_again_told_what_failed() {
    let repo = Repo::new();
    set_checks(&repo, true);
    let out = run(&repo, &tools(&repo, &Fakes::passing()))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert!(moves.contains(&"build>build".to_string()), "{moves:?}");
    assert_eq!(moves.last().unwrap(), "clean_up>done");
    assert_eq!(read(&repo, "build.md"), "fixed it\n");
}

#[test]
fn checks_that_never_pass_stick_the_unit_before_verify() {
    let repo = Repo::new();
    repo.set_check("echo boom; exit 1");
    let out = run(&repo, &tools(&repo, &Fakes::passing()))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["build>build", "build>stuck"]);
    assert!(!moves.iter().any(|m| m.starts_with("verify")), "{moves:?}");
    assert!(read(&repo, "build.problem").contains("boom"));
}

fn build_prompt(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/build.prompt")).unwrap()
}

#[test]
fn the_build_prompt_is_the_installed_skill_and_the_unit() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    let prompt = build_prompt(&repo);
    assert!(prompt.starts_with("# Build"), "{prompt}");
    assert!(
        prompt.contains("## Add a thing\n\nMake the thing."),
        "{prompt}"
    );
    assert!(prompt.contains("The branch started at commit "), "{prompt}");
}

#[test]
fn a_skill_changed_after_install_is_used_on_the_next_run() {
    let repo = Repo::new();
    let source = repo.path.parent().unwrap().join("my-skills");
    std::fs::create_dir_all(source.join("ns-build")).unwrap();
    for name in ["ns-build", "ns-verify", "ns-review"] {
        std::fs::create_dir_all(source.join(name)).unwrap();
        let from = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../skills")
            .join(name);
        std::fs::copy(from.join("SKILL.md"), source.join(name).join("SKILL.md")).unwrap();
    }
    repo.install_skills(&source);
    std::fs::write(
        source.join("ns-build/SKILL.md"),
        "# Build\n\nMY OWN BUILD STEPS\n",
    )
    .unwrap();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(build_prompt(&repo).contains("MY OWN BUILD STEPS"));
}

#[test]
fn a_skill_in_the_repository_wins_over_an_installed_one() {
    let repo = Repo::new();
    std::fs::create_dir_all(repo.path.join(".nightshift/skills/ns-build")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/skills/ns-build/SKILL.md"),
        "---\nname: ns-build\n---\n# Build\n\nTHIS REPOSITORY BUILDS ITS OWN WAY\n",
    )
    .unwrap();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    let prompt = build_prompt(&repo);
    assert!(prompt.starts_with("# Build\n\nTHIS REPOSITORY"), "{prompt}");
}

#[test]
fn a_skill_that_is_not_installed_goes_wrong_and_says_how_to_install_it() {
    let repo = Repo::new();
    std::fs::remove_dir_all(repo.config.join("skills")).unwrap();
    let out = run(&repo, &tools(&repo, &Fakes::passing())).failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(
        stderr.contains("ns-build is not installed; run `ns install`"),
        "{stderr}"
    );
}

#[test]
fn ship_stops_at_ready_to_merge_unless_asked_to_merge() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(!gh_log(&repo).contains("pr merge"));
}

#[test]
fn ship_merges_once_the_checks_pass_when_asked() {
    let repo = Repo::new();
    run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"]).success();
    assert_eq!(gh_log(&repo).matches("pr merge 9 --squash").count(), 1);
}

#[test]
fn clean_up_removes_the_workspace_and_branch_of_a_merged_unit() {
    let repo = Repo::new();
    let out = run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"])
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(moves(out).last().unwrap(), "clean_up>done");
    assert!(!repo.workspace("5").exists());
    assert!(!repo.branches().contains(&"ns/5".to_string()));
}

#[test]
fn clean_up_brings_the_main_checkout_up_to_date() {
    let repo = Repo::new();
    git(
        &repo.path,
        &["push", "-q", "--set-upstream", "origin", "main"],
    );
    let other = repo.path.parent().unwrap().join("other");
    git(
        repo.path.parent().unwrap(),
        &["clone", "-q", "origin.git", "other"],
    );
    git(
        &other,
        &["commit", "-q", "--allow-empty", "-m", "merged elsewhere"],
    );
    git(&other, &["push", "-q", "origin", "main"]);
    let path = tools(&repo, &Fakes::passing());
    run_with(&repo, &path, &["--merge"]).success();
    assert_eq!(
        git(&repo.path, &["log", "-1", "--format=%s", "main"]),
        "merged elsewhere\n"
    );
}

#[test]
fn clean_up_leaves_the_workspace_of_a_change_request_still_open() {
    let repo = Repo::new();
    let out = run(&repo, &tools(&repo, &Fakes::passing()))
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(moves(out).last().unwrap(), "clean_up>done");
    assert!(repo.workspace("5").is_dir());
    assert!(repo.branches().contains(&"ns/5".to_string()));
}

#[test]
fn ship_waits_for_pending_checks() {
    let repo = Repo::new();
    let fakes = Fakes {
        ci: Ci::PendingOnce,
        ..Fakes::passing()
    };
    run_with(&repo, &tools(&repo, &fakes), &["--merge"]).success();
    let log = gh_log(&repo);
    assert_eq!(log.matches("pr checks").count(), 2, "{log}");
    assert_eq!(log.matches("pr merge").count(), 1, "{log}");
}

#[test]
fn checks_that_never_finish_go_wrong_and_nothing_is_merged() {
    let repo = Repo::new();
    let fakes = Fakes {
        ci: Ci::NeverFinishes,
        ..Fakes::passing()
    };
    let out = run_with(
        &repo,
        &tools(&repo, &fakes),
        &["--merge", "--ci-timeout-seconds", "0"],
    )
    .failure()
    .get_output()
    .stdout
    .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["ship>ship", "ship>stuck"]);
    assert!(!gh_log(&repo).contains("pr merge"));
}

#[test]
fn a_repository_with_no_checks_has_none_to_wait_for() {
    let repo = Repo::new();
    let fakes = Fakes {
        ci: Ci::NoneReported,
        ..Fakes::passing()
    };
    let path = tools(&repo, &fakes);
    run_with(&repo, &path, &["--merge", "--ci-start-seconds", "0"]).success();
    assert!(gh_log(&repo).contains("pr merge"));
}

#[test]
fn checks_not_yet_reported_are_waited_for() {
    let repo = Repo::new();
    let fakes = Fakes {
        ci: Ci::NoneReported,
        ..Fakes::passing()
    };
    let out = run_with(
        &repo,
        &tools(&repo, &fakes),
        &["--merge", "--ci-timeout-seconds", "0"],
    )
    .failure()
    .get_output()
    .stdout
    .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["ship>ship", "ship>stuck"]);
    assert!(!gh_log(&repo).contains("pr merge"));
}

#[test]
fn failed_checks_send_the_unit_back_to_build_with_their_names() {
    let repo = Repo::new();
    let fakes = Fakes {
        ci: Ci::FailsOnce,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert!(moves.contains(&"ship>build".to_string()), "{moves:?}");
    assert_eq!(moves.last().unwrap(), "clean_up>done");
    assert_eq!(read(&repo, "build.md"), "rebuilt with the evidence\n");
    let log = gh_log(&repo);
    assert_eq!(
        log.matches("pr create").count(),
        1,
        "the change request is kept: {log}"
    );
}

fn lines(out: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn run_log(repo: &Repo) -> Vec<serde_json::Value> {
    let path = repo.path.join(".git/ns/runs.jsonl");
    lines(&std::fs::read(path).unwrap_or_default())
}

#[test]
fn gates_stop_the_unit_before_each_state_and_running_again_goes_on() {
    let repo = Repo::new();
    let path = tools(&repo, &Fakes::passing());
    let first = run_args(&repo, &path, &["run", "5", "--ci-poll-seconds", "0"])
        .success()
        .get_output()
        .stdout
        .clone();
    let first = lines(&first);
    assert_eq!(first.last().unwrap()["waiting"], "build");
    assert!(!repo.workspace("5").join(".ns/5/build.md").exists());
    let mut runs = 1;
    loop {
        let out = run_args(&repo, &path, &["run", "5", "--ci-poll-seconds", "0"])
            .success()
            .get_output()
            .stdout
            .clone();
        runs += 1;
        let last = lines(&out).last().unwrap().clone();
        if last.get("waiting").is_none() {
            assert_eq!(last["to"], "done");
            break;
        }
        assert!(runs < 12, "never finished");
    }
    assert_eq!(read(&repo, "build.md"), "built the thing\n");
}

#[test]
fn a_stuck_unit_resumes_at_the_state_it_stuck_in_with_fresh_attempts() {
    let repo = Repo::new();
    let broken = Fakes {
        exit: 1,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &broken)).failure();
    let last = run_log(&repo).last().unwrap().clone();
    assert_eq!(
        (last["from"].as_str(), last["to"].as_str()),
        (Some("build"), Some("stuck"))
    );
    let out = run(&repo, &tools(&repo, &Fakes::passing()))
        .success()
        .get_output()
        .stdout
        .clone();
    let events = lines(&out);
    assert_eq!(events[0]["state"], "build", "resumed at build, not setup");
    assert_eq!(events.last().unwrap()["to"], "done");
}

#[test]
fn fresh_deletes_the_workspace_and_branch_and_starts_over_from_setup() {
    let repo = Repo::new();
    let broken = Fakes {
        exit: 1,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &broken)).failure();
    let leftover = repo.workspace("5").join("leftover.txt");
    std::fs::write(&leftover, "from the first try").unwrap();
    let out = run_with(&repo, &tools(&repo, &Fakes::passing()), &["--fresh"])
        .success()
        .get_output()
        .stdout
        .clone();
    let events = lines(&out);
    assert_eq!(events[0]["state"], "setup");
    assert_eq!(events.last().unwrap()["to"], "done");
    assert!(!leftover.exists(), "the first try's files are gone");
    let commits = common::git(&repo.path, &["rev-list", "--count", "main..ns/5"]);
    assert_eq!(commits.trim(), "1");
}

#[test]
fn a_finished_unit_is_not_run_again() {
    let repo = Repo::new();
    run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"]).success();
    let before = run_log(&repo).len();
    let out = run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"])
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        lines(&out),
        [serde_json::json!({"unit": "5", "state": "done"})]
    );
    assert_eq!(run_log(&repo).len(), before);
}

#[test]
fn the_run_log_holds_every_move_with_its_time() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    let log = run_log(&repo);
    let moves: Vec<String> = log
        .iter()
        .map(|e| {
            format!(
                "{}>{}",
                e["from"].as_str().unwrap(),
                e["to"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        moves,
        [
            "setup>build",
            "build>verify",
            "verify>review",
            "review>ship",
            "ship>clean_up",
            "clean_up>done"
        ]
    );
    assert!(log.iter().all(|e| e["at"].as_u64().is_some_and(|t| t > 0)));
}

#[test]
fn a_build_that_leaves_changes_uncommitted_is_run_again_and_told_which() {
    let repo = Repo::new();
    let fakes = Fakes {
        build_commits: BuildCommits::OnlyWhenTold,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert!(moves.contains(&"build>build".to_string()), "{moves:?}");
    assert_eq!(moves.last().unwrap(), "clean_up>done");
    assert_eq!(read(&repo, "build.md"), "committed when told\n");
}

#[test]
fn a_build_that_leaves_two_commits_is_told_to_squash_them() {
    let repo = Repo::new();
    let fakes = Fakes {
        build_commits: BuildCommits::TwoUntilTold,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert!(moves.contains(&"build>build".to_string()), "{moves:?}");
    assert_eq!(read(&repo, "build.md"), "squashed when told\n");
    let commits = common::git(&repo.path, &["rev-list", "--count", "main..ns/5"]);
    assert_eq!(commits.trim(), "1");
}

#[test]
fn a_build_that_keeps_two_commits_sticks_the_unit_before_verify() {
    let repo = Repo::new();
    let fakes = Fakes {
        build_commits: BuildCommits::TwoAlways,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["build>build", "build>stuck"]);
    assert!(!moves.iter().any(|m| m.starts_with("verify")), "{moves:?}");
}

#[test]
fn a_build_that_never_commits_sticks_the_unit_before_verify() {
    let repo = Repo::new();
    let fakes = Fakes {
        build_commits: BuildCommits::Never,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["build>build", "build>stuck"]);
    assert!(!moves.iter().any(|m| m.starts_with("verify")), "{moves:?}");
}

#[test]
fn a_build_that_keeps_failing_sticks_the_unit_and_keeps_its_output() {
    let repo = Repo::new();
    let fakes = Fakes {
        exit: 1,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    assert_eq!(moves(out).last().unwrap(), "build>stuck");
    assert!(repo.workspace("5").join(".ns/5/build.md").is_file());
}

#[test]
fn a_failed_check_sends_the_unit_back_to_build_with_the_evidence() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: FAIL,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        moves(out),
        [
            "start setup",
            "setup>build",
            "build>verify",
            "verify>build",
            "build>verify",
            "verify>review",
            "review>ship",
            "ship>clean_up",
            "clean_up>done",
        ]
    );
    assert_eq!(read(&repo, "build.md"), "rebuilt with the evidence\n");
    assert_eq!(
        read(&repo, "evidence.md"),
        "reviewed the diff\nVERDICT: PASS\n"
    );
}

#[test]
fn a_check_that_keeps_failing_sticks_the_unit_after_going_back_to_build_once() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: FAIL,
        verdict: FAIL,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(
        &moves[moves.len() - 3..],
        ["verify>build", "build>verify", "verify>stuck"]
    );
}

#[test]
fn evidence_with_no_verdict_is_a_broken_check_that_is_run_again_not_a_failed_build() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: "all fine",
        verdict: "all fine",
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["verify>verify", "verify>stuck"]);
    assert!(!moves.contains(&"verify>build".to_string()), "{moves:?}");
    let evidence = read(&repo, "evidence.md");
    assert!(
        evidence.contains("does not end with a verdict"),
        "{evidence}"
    );
    assert!(evidence.ends_with("VERDICT: ERROR\n"), "{evidence}");
}

#[test]
fn a_check_that_changes_the_workspace_is_run_again_and_must_put_it_right() {
    let repo = Repo::new();
    let fakes = Fakes {
        check_edits: CheckEdits::FirstThenFixes,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert!(moves.contains(&"verify>verify".to_string()), "{moves:?}");
    assert!(!moves.contains(&"verify>build".to_string()), "{moves:?}");
    assert_eq!(moves.last().unwrap(), "clean_up>done");
    assert!(!repo.workspace("5").join("touched.txt").exists());
    assert_eq!(read(&repo, "build.md"), "built the thing\n");
}

#[test]
fn a_check_that_keeps_changing_the_workspace_sticks_the_unit_without_going_back_to_build() {
    let repo = Repo::new();
    let fakes = Fakes {
        check_edits: CheckEdits::Always,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["verify>verify", "verify>stuck"]);
    assert!(!moves.contains(&"verify>build".to_string()), "{moves:?}");
    let evidence = read(&repo, "evidence.md");
    assert!(evidence.contains("not as the build left it"), "{evidence}");
    assert!(evidence.ends_with("VERDICT: ERROR\n"), "{evidence}");
}

#[test]
fn a_crashed_harness_in_a_check_runs_the_check_again_not_the_build() {
    let repo = Repo::new();
    let fakes = Fakes {
        crash_in_first_check: true,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert!(moves.contains(&"verify>verify".to_string()), "{moves:?}");
    assert!(!moves.contains(&"verify>build".to_string()), "{moves:?}");
    assert_eq!(read(&repo, "build.md"), "built the thing\n");
}

#[test]
fn artifacts_do_not_block_removing_the_workspace() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    Command::cargo_bin("ns")
        .unwrap()
        .args(["workspace", "remove", "5"])
        .current_dir(&repo.path)
        .assert()
        .success();
    assert!(!repo.workspace("5").exists());
}

#[test]
fn a_session_that_runs_out_of_time_goes_wrong_like_any_other_failure() {
    let repo = Repo::new();
    let out = run_with(
        &repo,
        &tools(&repo, &Fakes::passing()),
        &["--harness-timeout-seconds", "0"],
    )
    .failure()
    .get_output()
    .stdout
    .clone();
    assert_eq!(
        &moves(out)[..],
        ["start setup", "setup>build", "build>build", "build>stuck"]
    );
}

/// The last label change the fake `gh` was asked for.
fn last_label_edit(repo: &Repo) -> String {
    gh_log(repo)
        .lines()
        .rfind(|l| l.starts_with("issue edit"))
        .unwrap_or_default()
        .to_string()
}

const STARTED: &str =
    "issue edit 5 --add-label status:in-progress --remove-label status:ready-for-agent";
const TO_HUMAN: &str =
    "issue edit 5 --add-label status:ready-for-human --remove-label status:in-progress";

#[test]
fn a_unit_is_in_progress_from_the_start_and_its_label_goes_when_its_change_merges() {
    let repo = Repo::new();
    run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"]).success();
    let edits: Vec<_> = gh_log(&repo)
        .lines()
        .filter(|l| l.starts_with("issue edit"))
        .map(String::from)
        .collect();
    assert_eq!(
        edits,
        [STARTED, "issue edit 5 --remove-label status:in-progress"]
    );
}

#[test]
fn a_change_ready_to_merge_leaves_the_unit_for_a_human() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(last_label_edit(&repo), TO_HUMAN);
}

#[test]
fn a_stuck_unit_is_left_for_a_human() {
    let repo = Repo::new();
    let broken = Fakes {
        exit: 1,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &broken)).failure();
    assert_eq!(last_label_edit(&repo), TO_HUMAN);
}

#[test]
fn a_unit_waiting_at_a_gate_is_left_for_a_human() {
    let repo = Repo::new();
    let path = tools(&repo, &Fakes::passing());
    run_args(&repo, &path, &["run", "5", "--ci-poll-seconds", "0"]).success();
    assert_eq!(last_label_edit(&repo), TO_HUMAN);
}

#[test]
fn a_human_resuming_a_unit_marks_it_in_progress_again() {
    let repo = Repo::new();
    let broken = Fakes {
        exit: 1,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &broken)).failure();
    run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"]).success();
    let log = gh_log(&repo);
    assert_eq!(
        log.matches("--add-label status:in-progress").count(),
        2,
        "{log}"
    );
    assert_eq!(
        last_label_edit(&repo),
        "issue edit 5 --remove-label status:in-progress"
    );
}
