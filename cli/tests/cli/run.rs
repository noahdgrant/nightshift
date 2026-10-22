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
    /// Whether the first build session ends with an overloaded service, as one the service dropped.
    pub first_build_dropped: bool,
    /// The verdict the first review ends with.
    pub first_review: &'static str,
    /// The verdict every later review ends with.
    pub review: &'static str,
    /// The name of the review session that always ends FAIL, whatever the other reviews say. Empty
    /// for none.
    pub review_fails_in: &'static str,
    /// The name of the review session that dies without a word the first time it runs. Empty for
    /// none.
    pub review_dies_in: &'static str,
    /// The labels the unit has before anything is done to it.
    pub labels: &'static str,
    /// Whether the unit's labels change when `ns` edits them. Without it the unit stays as it began.
    pub labels_follow_edits: bool,
    /// The units the forge lists, all with `labels`. With more than one, `labels_follow_edits`
    /// should be off, for the fake cannot tell whose labels an edit changed.
    pub units: &'static [u64],
    /// Whether the forge refuses to assign anyone.
    pub refuses_assignee: bool,
    /// Whether the forge refuses to add a comment to a unit.
    pub refuses_comment: bool,
    /// The file the build writes, relative to the workspace.
    pub builds: &'static str,
    /// How each triage or troubleshoot session ends, one letter per session in the order they run:
    /// `s` answers, `f` fails as a harness that is down, `b` exits cleanly with an answer `ns`
    /// cannot use. Sessions past its end answer.
    pub sessions: &'static str,
    /// What the forge answers when asked which units block a unit, as the pages of its list.
    /// `None` is a forge that cannot say.
    pub blocked_by: Option<&'static str>,
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
    /// Only the checks after the first write to the workspace.
    AfterFirst,
}

const PASS: &str = "VERDICT: PASS";
pub const INCONCLUSIVE: &str = "VERDICT: INCONCLUSIVE";
pub const FAIL: &str = "VERDICT: FAIL";

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
            first_build_dropped: false,
            first_review: PASS,
            review: PASS,
            review_fails_in: "",
            review_dies_in: "",
            labels: r#"[{"name":"status:ready-for-agent"}]"#,
            labels_follow_edits: true,
            units: &[5],
            refuses_assignee: false,
            refuses_comment: false,
            builds: "thing.txt",
            sessions: "",
            blocked_by: Some("[[]]"),
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
  [ "{follow}" = yes ] && case "$(grep 'issue edit' "{bin}/gh.log" | grep -v assignee | tail -1)" in
  *"--add-label status:in-progress"*) l='{progress}';;
  *"--add-label status:ready-for-human"*) l='{human}';;
  *"--remove-label status:in-progress"*|*"--remove-label status:ready-for-human"*) l='[]';;
  *"--add-label "*"status:ready-for-agent"*) l='{ready}';;
  esac
}}
[ -x "{bin}/on-$1-$2" ] && "{bin}/on-$1-$2"
case "$*" in *dependencies/blocked_by*) {blocked_by};; esac
case "$1 $2" in
"issue comment") [ "{refuses_comment}" = yes ] && {{ echo "cannot comment" >&2; exit 1; }};;
"issue edit"|"pr edit") case "$*" in *--add-assignee*) [ "{refuses}" = yes ] && {{ echo "cannot assign" >&2; exit 1; }};; esac;;
"api repos/{{owner}}/{{repo}}/issues/"*) labels; echo '{{"number":'"${{2##*/}}"',"title":"Add a thing","body":"Make the thing.","labels":'"$l"',"author_association":"OWNER"}}';;
"api --paginate") labels; echo '[[{listed}]]';;
"pr list") if [ -e "{bin}/merged" ]; then state=MERGED; elif [ -e "{bin}/closed" ]; then state=CLOSED; else state=OPEN; fi
  if [ -e "{bin}/opened" ]; then echo '[{{"number":9,"url":"https://forge.example/9","state":"'$state'"}}]'; else echo '[]'; fi;;
"pr create") touch "{bin}/opened"; echo https://forge.example/9;;
"pr checks") {checks};;
"pr merge") touch "{bin}/merged";;
esac"#,
            bin = bin.display(),
            checks = checks,
            blocked_by = match fakes.blocked_by {
                Some(pages) => format!("echo '{pages}'; exit 0"),
                None => "echo 'HTTP 404: Not Found' >&2; exit 1".to_string(),
            },
            initial = fakes.labels,
            listed = fakes
                .units
                .iter()
                .map(|n| format!(
                    r#"{{"number":{n},"title":"Add a thing","body":"Make the thing.","labels":'"$l"',"author_association":"OWNER"}}"#
                ))
                .collect::<Vec<_>>()
                .join(","),
            refuses_comment = if fakes.refuses_comment { "yes" } else { "no" },
            refuses = if fakes.refuses_assignee { "yes" } else { "no" },
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
        CheckEdits::AfterFirst => r#"if [ -e "$seen" ]; then echo x >> touched.txt; fi; "#,
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
kind=build
case "$p" in *"You diagnose one bug"*) kind=troubleshoot;; *"decide what happens to one unit"*) kind=triage;; *"Find what must be fixed before this change merges"*) kind=review;; *"Run the real thing and record what you saw"*) kind=verify;; esac
echo "$kind $*" >> "{bin}/claude.calls"
case "$p" in
*"You diagnose one bug"*) {session}*) printf '### Repro\nrun it\n\nROUTE: build\n';; esac;;
*"decide what happens to one unit"*) {session}*) printf 'Small.\nLABELS: type:feat\nROUTE: build\n';; esac;;
*"Find what must be fixed before this change merges"*)
  n=$(ls "{bin}"/review.prompt.* 2>/dev/null | wc -l); printf %s "$p" > "{bin}/review.prompt.$((n+1))"
  if [ -n "{review_dies_in}" ] && [ ! -e "{bin}/review.died" ] && printf %s "$p" | grep -q "^{review_dies_in}: review only"; then touch "{bin}/review.died"; exit 9; fi
  if echo "$p" | grep -q thing.txt; then echo reviewed the diff; else echo saw no diff; fi
  if [ -n "{review_fails_in}" ] && printf %s "$p" | grep -q "^{review_fails_in}: review only"; then echo '{FAIL}'
  elif [ -e "$reviewed" ]; then echo '{review}'; else touch "$reviewed"; echo '{first_review}'; fi;;
*"Run the real thing and record what you saw"*)
  n=$(ls "{bin}"/verify.prompt.* 2>/dev/null | wc -l); printf %s "$p" > "{bin}/verify.prompt.$((n+1))"
  {crash}{edit}echo checked it
  if [ -e "$seen" ]; then echo '{verdict}'; else touch "$seen"; echo '{first}'; fi;;
*"VERDICT: FAIL"*) [ -x "{bin}/on-rebuild" ] && "{bin}/on-rebuild"; echo rebuilt with the evidence;;
*"The checks failed:"*) touch "{bin}/fixed"; echo fixed it;;
*"left uncommitted changes"*) {when_told}echo committed when told;;
*"commits on the branch"*) {squash}echo squashed when told;;
*) {drop}printf %s "$p" > "{bin}/build.prompt"; mkdir -p "$(dirname '{builds}')"; echo thing > '{builds}'; {run_checks}{commit}echo built the thing;;
esac
exit {exit}"#,
            bin = bin.display(),
            verdict = fakes.verdict,
            first = fakes.first_verdict,
            review = fakes.review,
            first_review = fakes.first_review,
            review_fails_in = fakes.review_fails_in,
            review_dies_in = fakes.review_dies_in,
            drop = if fakes.first_build_dropped {
                format!(
                    r#"if [ ! -e "{bin}/dropped" ]; then touch "{bin}/dropped"; echo 'API Error: 529 Overloaded'; exit 1; fi; "#,
                    bin = bin.display()
                )
            } else {
                String::new()
            },
            exit = fakes.exit,
            builds = fakes.builds,
            session = format!(
                r#"n=$(cat "{bin}/sessions" 2>/dev/null || echo 0); echo $((n+1)) > "{bin}/sessions"; case "$(printf %s '{pattern}' | cut -c$((n+1)))" in f) exit 1;; b) echo 'Not an answer.';; "#,
                bin = bin.display(),
                pattern = fakes.sessions
            ),
        ),
    );
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap())
}

fn run(repo: &Repo, path: &str) -> assert_cmd::assert::Assert {
    run_with(repo, path, &[])
}

/// Runs `ns run 5` with `extra` arguments, two attempts a state unless `extra` says otherwise. It
/// looks at the checks without waiting between looks.
fn run_with(repo: &Repo, path: &str, extra: &[&str]) -> assert_cmd::assert::Assert {
    let mut args = vec!["run", "5", "--gates", "auto", "--ci-poll-seconds", "0"];
    if !extra.contains(&"--max-attempts") {
        args.extend(["--max-attempts", "2"]);
    }
    args.extend(extra);
    run_args(repo, path, &args)
}

/// Runs `ns run 5` as it is by default, with no option for the attempts.
fn run_unset(repo: &Repo, path: &str) -> assert_cmd::assert::Assert {
    run_args(
        repo,
        path,
        &["run", "5", "--gates", "auto", "--ci-poll-seconds", "0"],
    )
}

/// The states a failing review ran, in order, as `ns run` left them in the log.
fn review_runs(repo: &Repo) -> usize {
    run_log(repo)
        .iter()
        .filter(|e| e["from"] == "review")
        .count()
}

#[test]
fn with_no_option_and_no_setting_a_state_runs_three_times_before_the_unit_is_stuck() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_review: FAIL,
        review: FAIL,
        ..Fakes::passing()
    };
    run_unset(&repo, &tools(&repo, &fakes)).failure();
    assert_eq!(review_runs(&repo), 3);
}

#[test]
fn a_setting_of_one_runs_a_state_once() {
    let repo = Repo::new();
    set_manifest(&repo, "[run]\nmax_attempts = 1\n");
    let fakes = Fakes {
        first_review: FAIL,
        review: FAIL,
        ..Fakes::passing()
    };
    run_unset(&repo, &tools(&repo, &fakes)).failure();
    assert_eq!(review_runs(&repo), 1);
}

#[test]
fn the_option_wins_over_the_setting() {
    let repo = Repo::new();
    set_manifest(&repo, "[run]\nmax_attempts = 5\n");
    let fakes = Fakes {
        first_review: FAIL,
        review: FAIL,
        ..Fakes::passing()
    };
    run_with(&repo, &tools(&repo, &fakes), &["--max-attempts", "2"]).failure();
    assert_eq!(review_runs(&repo), 2);
}

#[test]
fn a_setting_of_five_is_used_when_there_is_no_option() {
    let repo = Repo::new();
    set_manifest(&repo, "[run]\nmax_attempts = 5\n");
    let fakes = Fakes {
        first_review: FAIL,
        review: FAIL,
        ..Fakes::passing()
    };
    run_unset(&repo, &tools(&repo, &fakes)).failure();
    assert_eq!(review_runs(&repo), 5);
}

#[test]
fn a_bad_setting_is_an_error_that_names_the_key_and_nothing_runs() {
    for bad in ["0", "-1", "\"three\""] {
        let repo = Repo::new();
        set_manifest(&repo, &format!("[run]\nmax_attempts = {bad}\n"));
        let out = run_unset(&repo, &tools(&repo, &Fakes::passing())).failure();
        let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
        assert!(err.contains("run.max_attempts"), "{bad}: {err}");
        assert!(out.get_output().stdout.is_empty(), "{bad}");
    }
}

/// Writes the repository's `.nightshift/nightshift.toml`.
pub fn set_manifest(repo: &Repo, text: &str) {
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(repo.path.join(".nightshift/nightshift.toml"), text).unwrap();
}

/// Saves a transcript of unit 9, which no run here touches, last modified `days` days ago.
pub fn transcript_keep(repo: &Repo, name: &str, days: u64) -> std::path::PathBuf {
    let dir = repo.path.join(".git/ns/transcripts/9");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, "{}\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(
            std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400 + 3_600),
        )
        .unwrap();
    path
}

#[test]
fn run_deletes_transcripts_older_than_a_week_by_default_and_keeps_newer_ones() {
    let repo = Repo::new();
    let old = transcript_keep(&repo, "build-1.jsonl", 8);
    let new = transcript_keep(&repo, "build-2.jsonl", 6);
    run_unset(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(!old.exists());
    assert!(new.exists());
}

#[test]
fn log_still_shows_a_transcript_that_a_run_kept() {
    let repo = Repo::new();
    let dir = repo.path.join(".git/ns/transcripts/5");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("build-1.jsonl"),
        r#"{"type":"result","result":"Kept."}"#,
    )
    .unwrap();
    run_unset(&repo, &tools(&repo, &Fakes::passing())).success();
    let out = Command::cargo_bin("ns")
        .unwrap()
        .args(["log", "5", "--state", "build", "--last"])
        .current_dir(&repo.path)
        .assert()
        .success();
    let out = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(out.starts_with("== build"), "{out}");
}

#[test]
fn run_removes_the_directory_of_a_unit_whose_transcripts_are_all_deleted() {
    let repo = Repo::new();
    let old = transcript_keep(&repo, "build-1.jsonl", 8);
    run_unset(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(!old.parent().unwrap().exists());
}

#[test]
fn a_transcript_days_setting_of_zero_deletes_nothing() {
    let repo = Repo::new();
    set_manifest(&repo, "[run]\ntranscript_days = 0\n");
    let old = transcript_keep(&repo, "build-1.jsonl", 400);
    run_unset(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(old.exists());
}

#[test]
fn a_transcript_days_setting_of_thirty_keeps_a_transcript_of_twenty_days() {
    let repo = Repo::new();
    set_manifest(&repo, "[run]\ntranscript_days = 30\n");
    let kept = transcript_keep(&repo, "build-1.jsonl", 20);
    let gone = transcript_keep(&repo, "build-2.jsonl", 31);
    run_unset(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(kept.exists());
    assert!(!gone.exists());
}

#[test]
fn a_transcript_that_cannot_be_deleted_does_not_stop_the_run() {
    let repo = Repo::new();
    // A directory named like a session: old, and not removable as a file.
    let stuck = transcript_keep(&repo, "build-1.jsonl", 8);
    std::fs::remove_file(&stuck).unwrap();
    std::fs::create_dir(&stuck).unwrap();
    std::fs::File::open(&stuck)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(9 * 86_400))
        .unwrap();
    let other = transcript_keep(&repo, "verify-1.jsonl", 8);
    run_unset(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(stuck.exists());
    assert!(!other.exists());
}

#[test]
fn a_bad_transcript_days_setting_is_an_error_that_names_the_key_and_nothing_runs() {
    for bad in ["-1", "\"7\"", "2.5"] {
        let repo = Repo::new();
        set_manifest(&repo, &format!("[run]\ntranscript_days = {bad}\n"));
        let old = transcript_keep(&repo, "build-1.jsonl", 40);
        let out = run_unset(&repo, &tools(&repo, &Fakes::passing())).failure();
        let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
        assert!(err.contains("run.transcript_days"), "{bad}: {err}");
        assert!(out.get_output().stdout.is_empty(), "{bad}");
        assert!(old.exists(), "{bad}");
    }
}

/// Runs `ns` with exactly `args`.
fn run_args(repo: &Repo, path: &str, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .args(args)
        .env("PATH", path)
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

/// The model each session of `kind` (build, verify, review, triage or troubleshoot) was started
/// with, in order: the name after `--model`, or `-` when the session was given none.
pub fn models_used(repo: &Repo, kind: &str) -> Vec<String> {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/claude.calls"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix(kind)?.strip_prefix(' '))
        .map(|args| match args.split_once("--model ") {
            Some((_, rest)) => rest.split(' ').next().unwrap().to_string(),
            None => "-".to_string(),
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
    assert_eq!(read(&repo, "evidence.md"), merged_review([PASS; 3]));
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

#[test]
fn ship_gives_the_change_request_the_units_type_area_and_priority_but_not_its_status() {
    let repo = Repo::new();
    let fakes = Fakes {
        labels: r#"[{"name":"status:ready-for-agent"},{"name":"type:fix"},{"name":"area:cli"},{"name":"priority:high"},{"name":"bug"},{"name":"status:in-progress"}]"#,
        labels_follow_edits: false,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    let log = gh_log(&repo);
    let labels: Vec<_> = log
        .split("--label ")
        .skip(1)
        .map(|rest| rest.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(labels, ["type:fix", "area:cli", "priority:high"], "{log}");
}

#[test]
fn ship_opens_the_change_request_of_a_unit_with_none_of_those_labels_without_labels() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    let log = gh_log(&repo);
    assert_eq!(log.matches("pr create").count(), 1, "{log}");
    assert!(!log.contains("--label"), "{log}");
}

#[test]
fn the_unit_and_the_change_request_are_assigned_to_the_account_ns_acts_as() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    let log = gh_log(&repo);
    assert_eq!(
        log.matches("issue edit 5 --add-assignee @me").count(),
        1,
        "{log}"
    );
    assert_eq!(
        log.matches("pr edit 9 --add-assignee @me").count(),
        1,
        "{log}"
    );
}

#[test]
fn a_second_ship_assigns_the_change_request_that_already_exists() {
    let repo = Repo::new();
    let fakes = Fakes {
        ci: Ci::FailsOnce,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    let log = gh_log(&repo);
    assert_eq!(log.matches("pr create").count(), 1, "{log}");
    assert_eq!(
        log.matches("pr edit 9 --add-assignee @me").count(),
        2,
        "{log}"
    );
}

#[test]
fn assignees_are_only_added_never_replaced() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    let log = gh_log(&repo);
    assert!(!log.contains("--remove-assignee"), "{log}");
    assert!(!log.contains("--assignee"), "{log}");
}

#[test]
fn a_refused_assignment_is_said_on_stderr_and_does_not_stop_the_unit() {
    let repo = Repo::new();
    let fakes = Fakes {
        refuses_assignee: true,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes)).success();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("cannot assign the unit"), "{err}");
    assert!(err.contains("cannot assign the change request"), "{err}");
    assert_eq!(read(&repo, "ship.md"), "https://forge.example/9\n");
}

/// What the fake `gh` was asked, empty when it was not called at all.
fn gh_log(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/gh.log")).unwrap_or_default()
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
    assert!(read(&repo, "evidence.md")
        .starts_with("# Review session: Behavior\n\nreviewed the diff\nVERDICT: PASS\n"));
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

/// The review the three default sessions keep when they end with `verdicts`, the reviewed diff
/// their findings.
fn merged_review(verdicts: [&str; 3]) -> String {
    let names = ["Behavior", "Requirements and tests", "Maintainability"];
    let mut text = String::new();
    for (name, verdict) in names.iter().zip(verdicts) {
        text.push_str(&format!(
            "# Review session: {name}\n\nreviewed the diff\n{verdict}\n\n"
        ));
    }
    text.push_str(if verdicts.iter().all(|v| *v == PASS) {
        PASS
    } else {
        FAIL
    });
    text.push('\n');
    text
}

fn review_prompt(repo: &Repo, n: usize) -> String {
    prompt_of(repo, "review", n)
}

fn verify_prompt(repo: &Repo, n: usize) -> String {
    prompt_of(repo, "verify", n)
}

fn prompt_of(repo: &Repo, kind: &str, n: usize) -> String {
    std::fs::read_to_string(
        repo.path
            .parent()
            .unwrap()
            .join(format!("bin/{kind}.prompt.{n}")),
    )
    .unwrap()
}

const PREVIOUS: &str = "\n# Findings from the previous review";

#[test]
fn a_first_review_has_no_previous_findings_and_a_review_after_a_failure_has_them() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_review: FAIL,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    for n in 1..=3 {
        assert!(!review_prompt(&repo, n).contains(PREVIOUS));
    }
    // Only the session that failed is shown its findings, and not the verdict of the review.
    assert!(review_prompt(&repo, 4).contains(&format!(
        "{PREVIOUS}\n\nreviewed the diff\nVERDICT: FAIL\n\n# The change"
    )));
    assert!(!review_prompt(&repo, 5).contains(PREVIOUS));
    assert!(!review_prompt(&repo, 6).contains(PREVIOUS));
}

#[test]
fn every_review_is_kept_as_review_n() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_review: FAIL,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    assert_eq!(
        read(&repo, "review-1.md"),
        merged_review([FAIL, PASS, PASS])
    );
    assert_eq!(read(&repo, "review-2.md"), merged_review([PASS; 3]));
}

#[test]
fn a_unit_resumed_after_stuck_gets_the_last_reviews_findings() {
    let repo = Repo::new();
    let failing = Fakes {
        first_review: FAIL,
        review: FAIL,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &failing)).failure();
    let before = std::fs::read_dir(repo.path.parent().unwrap().join("bin"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("review.prompt.")
        })
        .count();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(review_prompt(&repo, before + 1).contains(PREVIOUS));
}

const PREVIOUS_VERIFICATION: &str = "\n# Findings from the previous verification";

#[test]
fn a_first_verification_has_no_previous_findings_and_one_after_a_failure_has_them() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: FAIL,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    assert!(!verify_prompt(&repo, 1).contains(PREVIOUS_VERIFICATION));
    assert!(verify_prompt(&repo, 2).contains(&format!(
        "{PREVIOUS_VERIFICATION}\n\nchecked it\nVERDICT: FAIL"
    )));
}

#[test]
fn a_verification_after_a_pass_has_no_previous_findings() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_review: FAIL,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    assert!(!verify_prompt(&repo, 2).contains(PREVIOUS_VERIFICATION));
}

#[test]
fn every_verification_is_kept_as_verify_n() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: FAIL,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    assert_eq!(read(&repo, "verify-1.md"), "checked it\nVERDICT: FAIL\n");
    assert_eq!(read(&repo, "verify-2.md"), "checked it\nVERDICT: PASS\n");
}

#[test]
fn a_unit_resumed_after_stuck_gets_the_last_verifications_findings() {
    let repo = Repo::new();
    let failing = Fakes {
        first_verdict: FAIL,
        verdict: FAIL,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &failing)).failure();
    let before = std::fs::read_dir(repo.path.parent().unwrap().join("bin"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("verify.prompt.")
        })
        .count();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(verify_prompt(&repo, before + 1).contains(PREVIOUS_VERIFICATION));
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
fn the_build_prompt_is_the_embedded_skill_and_the_unit() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    let prompt = build_prompt(&repo);
    assert!(prompt.starts_with("# Build"), "{prompt}");
    assert!(prompt.contains("# Test-Driven Development"), "{prompt}");
    assert!(
        prompt.contains("## Add a thing\n\nMake the thing."),
        "{prompt}"
    );
    assert!(prompt.contains("The branch started at commit "), "{prompt}");
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

/// Commits `.nightshift/nightshift.toml` with `text` to the repository, before a unit starts.
fn manifest_set(repo: &Repo, text: &str) {
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(repo.path.join(".nightshift/nightshift.toml"), text).unwrap();
}

#[test]
fn a_change_touching_a_human_review_file_is_not_merged_and_is_labeled() {
    let repo = Repo::new();
    manifest_set(&repo, "[merge]\nhuman_review = [\"*.txt\"]\n");
    let out = run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"])
        .success()
        .get_output()
        .stdout
        .clone();
    let log = gh_log(&repo);
    assert!(!log.contains("pr merge"), "{log}");
    assert_eq!(
        log.matches("pr edit 9 --add-label needs-human-merge")
            .count(),
        1,
        "{log}"
    );
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("thing.txt matches *.txt in merge.human_review"),
        "{out}"
    );
    assert!(
        log.contains("issue edit 5 --add-label status:ready-for-human"),
        "{log}"
    );
}

#[test]
fn a_workflow_change_is_not_merged_under_the_default_list() {
    let repo = Repo::new();
    let fakes = Fakes {
        builds: ".github/workflows/ci.yml",
        ..Fakes::passing()
    };
    let out = run_with(&repo, &tools(&repo, &fakes), &["--merge"])
        .success()
        .get_output()
        .stdout
        .clone();
    let log = gh_log(&repo);
    assert!(!log.contains("pr merge"), "{log}");
    assert!(log.contains("--add-label needs-human-merge"), "{log}");
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains(".github/workflows/ci.yml matches .github/** in merge.human_review"),
        "{out}"
    );
}

#[test]
fn a_configured_list_replaces_the_default_and_an_empty_one_merges_everything() {
    for text in [
        "[merge]\nhuman_review = [\".github/**\"]\n",
        "[merge]\nhuman_review = []\n",
    ] {
        let repo = Repo::new();
        manifest_set(&repo, text);
        run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"]).success();
        let log = gh_log(&repo);
        assert_eq!(log.matches("pr merge 9 --squash").count(), 1, "{log}");
        assert!(!log.contains("needs-human-merge"), "{log}");
    }
}

#[test]
fn a_change_touching_only_source_is_merged_under_the_default_list() {
    let repo = Repo::new();
    let fakes = Fakes {
        builds: "cli/src/x.rs",
        ..Fakes::passing()
    };
    run_with(&repo, &tools(&repo, &fakes), &["--merge"]).success();
    let log = gh_log(&repo);
    assert_eq!(log.matches("pr merge 9 --squash").count(), 1, "{log}");
    assert!(!log.contains("needs-human-merge"), "{log}");
}

#[test]
fn a_human_review_list_that_cannot_be_read_stops_the_unit_before_any_merge() {
    let repo = Repo::new();
    manifest_set(&repo, "[merge]\nhuman_review = \"a\"\n");
    run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"]);
    let log = gh_log(&repo);
    assert!(!log.contains("pr merge"), "{log}");
    assert!(!log.contains("needs-human-merge"), "{log}");
}

#[test]
fn without_merge_nothing_is_labeled_even_for_a_human_review_file() {
    let repo = Repo::new();
    manifest_set(&repo, "[merge]\nhuman_review = [\"*.txt\"]\n");
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert!(!gh_log(&repo).contains("needs-human-merge"));
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

/// Sticks unit 5 in `state` by failing every check of it, then returns the `PATH` of fakes that pass.
fn stuck_in(repo: &Repo, state: &str) -> String {
    let failing = match state {
        "review" => Fakes {
            first_review: FAIL,
            review: FAIL,
            ..Fakes::passing()
        },
        _ => Fakes {
            first_verdict: FAIL,
            verdict: FAIL,
            ..Fakes::passing()
        },
    };
    run(repo, &tools(repo, &failing)).failure();
    let last = run_log(repo).last().unwrap().clone();
    assert_eq!(
        (last["from"].as_str(), last["to"].as_str()),
        (Some(state), Some("stuck"))
    );
    tools(repo, &Fakes::passing())
}

#[test]
fn a_unit_stuck_in_a_check_with_nothing_changed_resumes_at_build_and_says_why() {
    for state in ["review", "verify"] {
        let repo = Repo::new();
        let path = stuck_in(&repo, state);
        let out = run(&repo, &path).success().get_output().stdout.clone();
        let events = lines(&out);
        assert_eq!(
            events[0],
            serde_json::json!({
                "unit": "5",
                "from": "stuck",
                "to": "build",
                "reason": format!("nothing changed since {state} failed"),
            }),
            "{state}"
        );
        assert_eq!(events[1]["state"], "build", "{state}");
        // The fake build says this only when its prompt carries the failing evidence.
        assert_eq!(read(&repo, "build.md"), "rebuilt with the evidence\n");
        let logged = run_log(&repo);
        assert!(
            logged.iter().any(|e| e["from"] == "stuck"
                && e["to"] == "build"
                && e["reason"] == format!("nothing changed since {state} failed")),
            "{state}: {logged:?}"
        );
    }
}

#[test]
fn a_unit_stuck_in_a_check_whose_workspace_was_edited_resumes_at_that_check() {
    for state in ["review", "verify"] {
        let repo = Repo::new();
        let path = stuck_in(&repo, state);
        std::fs::write(repo.workspace("5").join("by-a-human.txt"), "fixed\n").unwrap();
        // The check itself refuses a workspace the build did not leave, so the run may end stuck.
        let out = run(&repo, &path).get_output().stdout.clone();
        let events = lines(&out);
        assert_eq!(events[0]["state"], state);
        assert!(
            !events.iter().any(|e| e["from"] == "stuck"),
            "{state}: {events:?}"
        );
    }
}

#[test]
fn a_unit_stuck_in_a_check_with_no_snapshot_of_its_build_resumes_at_that_check() {
    let repo = Repo::new();
    let path = stuck_in(&repo, "review");
    std::fs::remove_file(repo.workspace("5").join(".ns/5/built.snapshot")).unwrap();
    let out = run(&repo, &path).get_output().stdout.clone();
    let events = lines(&out);
    assert_eq!(events[0]["state"], "review");
    assert!(!events.iter().any(|e| e["from"] == "stuck"), "{events:?}");
}

#[test]
fn a_unit_stuck_in_ship_resumes_at_ship_whatever_the_workspace_holds() {
    let repo = Repo::new();
    let never = Fakes {
        ci: Ci::NeverFinishes,
        ..Fakes::passing()
    };
    run_with(
        &repo,
        &tools(&repo, &never),
        &["--merge", "--ci-timeout-seconds", "0"],
    )
    .failure();
    let last = run_log(&repo).last().unwrap().clone();
    assert_eq!(last["from"], "ship");
    let out = run(&repo, &tools(&repo, &Fakes::passing()))
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(lines(&out)[0]["state"], "ship");
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

/// Ends a run with the change request open, then marks it as `how` on the forge.
fn finished_then(repo: &Repo, how: &str) -> String {
    let path = tools(repo, &Fakes::passing());
    run(repo, &path).success();
    std::fs::write(repo.path.parent().unwrap().join("bin").join(how), "").unwrap();
    path
}

#[test]
fn a_finished_unit_whose_change_request_merged_since_is_cleaned_up() {
    let repo = Repo::new();
    let path = finished_then(&repo, "merged");
    let before = run_log(&repo).len();
    let out = run(&repo, &path).success().get_output().stdout.clone();
    assert_eq!(
        lines(&out),
        [serde_json::json!({"unit": "5", "state": "done"})]
    );
    assert!(!repo.workspace("5").exists());
    assert!(!repo.branches().contains(&"ns/5".to_string()));
    assert!(gh_log(&repo).contains("--remove-label status:ready-for-human"));
    assert_eq!(run_log(&repo).len(), before);
}

#[test]
fn cleaning_up_a_finished_unit_again_changes_nothing() {
    let repo = Repo::new();
    let path = finished_then(&repo, "merged");
    run(&repo, &path).success();
    let log = gh_log(&repo);
    run(&repo, &path).success();
    assert_eq!(
        gh_log(&repo).matches("issue edit").count(),
        log.matches("issue edit").count()
    );
}

#[test]
fn a_finished_unit_whose_change_request_is_still_open_is_left_alone() {
    let repo = Repo::new();
    let path = finished_then(&repo, "opened");
    let edits = gh_log(&repo).matches("issue edit").count();
    let out = run(&repo, &path).success().get_output().stdout.clone();
    assert_eq!(
        lines(&out),
        [serde_json::json!({"unit": "5", "state": "done"})]
    );
    assert!(repo.workspace("5").is_dir());
    assert!(repo.branches().contains(&"ns/5".to_string()));
    assert_eq!(gh_log(&repo).matches("issue edit").count(), edits);
}

#[test]
fn a_finished_unit_whose_change_request_was_closed_is_reported_and_kept() {
    let repo = Repo::new();
    let path = finished_then(&repo, "closed");
    let err = run(&repo, &path).failure().get_output().stderr.clone();
    assert!(String::from_utf8_lossy(&err).contains("closed without merging"));
    assert!(repo.workspace("5").is_dir());
    assert!(repo.branches().contains(&"ns/5".to_string()));
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
    assert_eq!(read(&repo, "evidence.md"), merged_review([PASS; 3]));
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

fn transcripts(repo: &Repo) -> std::path::PathBuf {
    repo.path.join(".git/ns/transcripts/5")
}

#[test]
fn each_session_of_a_state_leaves_a_numbered_transcript_in_the_state_directory() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_review: FAIL,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    let dir = transcripts(&repo);
    // The fake prints plain text, which is all a session that never printed a result leaves.
    assert_eq!(
        std::fs::read_to_string(dir.join("build-1.jsonl")).unwrap(),
        "built the thing\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("build-2.jsonl")).unwrap(),
        "rebuilt with the evidence\n"
    );
    assert!(dir.join("verify-1.jsonl").is_file());
    assert!(dir.join("verify-2.jsonl").is_file());
    // Three sessions to a review, two reviews.
    for n in 1..=6 {
        assert!(dir.join(format!("review-{n}.jsonl")).is_file(), "{n}");
    }
    assert!(!dir.join("review-7.jsonl").exists());
}

#[test]
fn a_session_started_again_after_a_dropped_service_leaves_both_transcripts() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_build_dropped: true,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    let dir = transcripts(&repo);
    assert_eq!(
        std::fs::read_to_string(dir.join("build-1.jsonl")).unwrap(),
        "API Error: 529 Overloaded\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("build-2.jsonl")).unwrap(),
        "built the thing\n"
    );
}

#[test]
fn a_session_killed_on_timeout_still_leaves_a_transcript() {
    let repo = Repo::new();
    run_with(
        &repo,
        &tools(&repo, &Fakes::passing()),
        &["--harness-timeout-seconds", "0"],
    )
    .failure();
    let dir = transcripts(&repo);
    assert!(dir.join("build-1.jsonl").is_file());
    assert!(dir.join("build-2.jsonl").is_file());
}

/// The last label change the fake `gh` was asked for.
fn last_label_edit(repo: &Repo) -> String {
    gh_log(repo)
        .lines()
        .rfind(|l| l.starts_with("issue edit") && !l.contains("assignee"))
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
        .filter(|l| l.starts_with("issue edit") && !l.contains("assignee"))
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

/// Holds the lock `ns run` takes for unit 5, as another `ns run` of it would.
fn unit_lock_held(repo: &Repo, unit: &str) -> std::fs::File {
    let path = repo.path.join(format!(".git/ns/unit-{unit}.lock"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    file.lock().unwrap();
    file
}

#[test]
fn a_unit_already_being_run_is_not_run_again_and_nothing_of_it_is_touched() {
    let repo = Repo::new();
    let path = tools(&repo, &Fakes::passing());
    let held = unit_lock_held(&repo, "5");
    let out = run_with(&repo, &path, &["--fresh"]).failure();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("unit 5 is already being run"), "{err}");
    assert!(!repo.workspace("5").exists());
    assert!(!gh_log(&repo).contains("issue edit"));
    drop(held);
    run(&repo, &path).success();
}

#[test]
fn another_unit_being_run_does_not_block_this_one() {
    let repo = Repo::new();
    let _other = unit_lock_held(&repo, "6");
    run(&repo, &tools(&repo, &Fakes::passing())).success();
}

/// A verification that proved R1 and could not prove R2.
const UNPROVEN: &str = "### R1\nPASS: ran it\n\n### R2\nINCONCLUSIVE: needs a live service; \
                        run it against staging\nVERDICT: INCONCLUSIVE";

fn inconclusive_twice() -> Fakes {
    Fakes {
        first_verdict: UNPROVEN,
        verdict: UNPROVEN,
        ..Fakes::passing()
    }
}

#[test]
fn an_inconclusive_verification_runs_again_and_a_pass_goes_on_to_review() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: UNPROVEN,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(
        &moves[2..5],
        ["build>verify", "verify>verify", "verify>review"]
    );
    assert!(!moves.contains(&"verify>build".to_string()), "{moves:?}");
    assert!(!moves.contains(&"build>build".to_string()), "{moves:?}");
    let log = run_log(&repo);
    assert_eq!(log[2]["reason"], "inconclusive");
    assert_eq!(log[2]["attempt"], 1);
}

#[test]
fn the_second_verification_is_shown_the_unproven_requirements_of_the_first() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: UNPROVEN,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    assert!(!verify_prompt(&repo, 1).contains(PREVIOUS_VERIFICATION));
    assert!(
        verify_prompt(&repo, 2).contains(&format!("{PREVIOUS_VERIFICATION}\n\nchecked it\n### R1"))
    );
    assert!(verify_prompt(&repo, 2).contains("INCONCLUSIVE: needs a live service"));
    assert!(read(&repo, "verify-1.md").ends_with("VERDICT: INCONCLUSIVE\n"));
    assert_eq!(read(&repo, "verify-2.md"), "checked it\nVERDICT: PASS\n");
}

#[test]
fn an_inconclusive_verification_followed_by_a_fail_sends_the_unit_to_build() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: UNPROVEN,
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
        &moves[2..6],
        [
            "build>verify",
            "verify>verify",
            "verify>build",
            "build>verify"
        ]
    );
}

#[test]
fn two_inconclusive_verifications_stick_the_unit_for_a_human_and_say_what_is_unproven() {
    let repo = Repo::new();
    let out = run_with(
        &repo,
        &tools(&repo, &inconclusive_twice()),
        &["--max-attempts", "3"],
    )
    .failure()
    .get_output()
    .stdout
    .clone();
    let moves = moves(out);
    assert_eq!(
        &moves[2..],
        ["build>verify", "verify>verify", "verify>stuck"]
    );
    let last = run_log(&repo).last().unwrap().clone();
    assert_eq!(last["reason"], "inconclusive");
    let gh = gh_log(&repo);
    assert!(gh.contains("--add-label status:ready-for-human"), "{gh}");
    let comment = gh
        .lines()
        .skip_while(|l| !l.starts_with("issue comment 5"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        comment.contains("INCONCLUSIVE: needs a live service; run it against staging"),
        "{gh}"
    );
    assert!(!comment.contains("PASS: ran it"), "{gh}");
    assert!(!comment.contains("VERDICT"), "{gh}");
}

#[test]
fn an_inconclusive_verification_with_one_attempt_sticks_the_unit_after_one() {
    let repo = Repo::new();
    run_with(
        &repo,
        &tools(&repo, &inconclusive_twice()),
        &["--max-attempts", "1"],
    )
    .failure();
    assert!(repo.workspace("5").join(".ns/5/verify-1.md").is_file());
    assert!(!repo.workspace("5").join(".ns/5/verify-2.md").exists());
    let last = run_log(&repo).last().unwrap().clone();
    assert_eq!(
        (
            last["from"].as_str(),
            last["to"].as_str(),
            last["reason"].as_str()
        ),
        (Some("verify"), Some("stuck"), Some("inconclusive"))
    );
    assert!(gh_log(&repo).contains("issue comment 5 --body"));
}

#[test]
fn a_verification_without_an_inconclusive_section_is_commented_whole() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: "could not run it\nVERDICT: INCONCLUSIVE",
        verdict: "could not run it\nVERDICT: INCONCLUSIVE",
        ..Fakes::passing()
    };
    run_with(&repo, &tools(&repo, &fakes), &["--max-attempts", "1"]).failure();
    assert!(gh_log(&repo).contains("could not run it"));
}

#[test]
fn a_comment_the_forge_refuses_is_said_and_the_unit_is_still_stuck_for_a_human() {
    let repo = Repo::new();
    let fakes = Fakes {
        refuses_comment: true,
        ..inconclusive_twice()
    };
    let assert = run_with(&repo, &tools(&repo, &fakes), &["--max-attempts", "1"]).failure();
    let err = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        err.contains("cannot say what verification left unproven"),
        "{err}"
    );
    assert!(gh_log(&repo).contains("--add-label status:ready-for-human"));
}

#[test]
fn a_unit_stuck_for_another_reason_gets_no_comment() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: FAIL,
        verdict: FAIL,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).failure();
    assert!(!gh_log(&repo).contains("issue comment"));
}

#[test]
fn an_inconclusive_line_followed_by_more_text_is_a_broken_check_not_an_inconclusive_one() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: "VERDICT: INCONCLUSIVE\nbut also this",
        verdict: PASS,
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).success();
    let log = run_log(&repo);
    assert_eq!(log[2]["reason"], "went_wrong");
    assert!(!verify_prompt(&repo, 2).contains(PREVIOUS_VERIFICATION));
}

#[test]
fn a_fail_requirement_makes_the_verdict_fail_whatever_else_is_inconclusive() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: "### R1\nFAIL\n### R2\nINCONCLUSIVE\nVERDICT: FAIL",
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(&moves(out)[3], "verify>build");
}

#[test]
fn the_second_verification_must_leave_the_workspace_as_the_build_did() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_verdict: UNPROVEN,
        verdict: PASS,
        check_edits: CheckEdits::AfterFirst,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .failure()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[moves.len() - 2..], ["verify>verify", "verify>stuck"]);
    assert!(!moves.contains(&"verify>review".to_string()), "{moves:?}");
    assert!(read(&repo, "evidence.md").contains("not as the build left it"));
}

#[test]
fn a_review_that_is_inconclusive_does_not_let_the_unit_ship() {
    let repo = Repo::new();
    let fakes = Fakes {
        first_review: INCONCLUSIVE,
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    assert_eq!(&moves[4..6], ["review>build", "build>verify"]);
}

#[test]
fn a_unit_stuck_unproven_resumes_at_verify_not_build_though_nothing_changed() {
    let repo = Repo::new();
    run_with(
        &repo,
        &tools(&repo, &inconclusive_twice()),
        &["--max-attempts", "1"],
    )
    .failure();
    let out = run(&repo, &tools(&repo, &Fakes::passing()))
        .success()
        .get_output()
        .stdout
        .clone();
    let events = lines(&out);
    assert_eq!(events[0]["state"], "verify");
    assert_eq!(events.last().unwrap()["to"], "done");
}

const OPEN_BLOCKER: &str = r#"[[{"number":3,"state":"open"}]]"#;
const CLOSED_BLOCKER: &str = r#"[[{"number":3,"state":"closed"}]]"#;

#[test]
fn a_unit_with_an_open_blocker_is_refused_with_a_line_naming_it() {
    let repo = Repo::new();
    let fakes = Fakes {
        blocked_by: Some(OPEN_BLOCKER),
        ..Fakes::passing()
    };
    let out = run_with(&repo, &tools(&repo, &fakes), &[]).failure();
    let said = lines(&out.get_output().stdout);
    assert_eq!(said, [serde_json::json!({ "unit": 5, "blocked_by": [3] })]);
    assert!(run_log(&repo).is_empty());
    let log = std::fs::read_to_string(repo.path.parent().unwrap().join("bin/gh.log")).unwrap();
    assert!(!log.contains("issue edit"), "nothing was changed: {log}");
}

#[test]
fn a_closed_blocker_does_not_stop_a_unit() {
    let repo = Repo::new();
    let fakes = Fakes {
        blocked_by: Some(CLOSED_BLOCKER),
        ..Fakes::passing()
    };
    run_with(&repo, &tools(&repo, &fakes), &[]).success();
}

#[test]
fn ignoring_blockers_runs_a_blocked_unit() {
    let repo = Repo::new();
    let fakes = Fakes {
        blocked_by: Some(OPEN_BLOCKER),
        ..Fakes::passing()
    };
    run_with(&repo, &tools(&repo, &fakes), &["--ignore-blockers"]).success();
}

#[test]
fn a_unit_that_has_begun_is_not_refused_when_a_blocker_appears() {
    let repo = Repo::new();
    let path = tools(&repo, &Fakes::passing());
    run_args(&repo, &path, &["run", "5", "--ci-poll-seconds", "0"]).success();
    assert!(!run_log(&repo).is_empty());
    let blocked = Fakes {
        blocked_by: Some(OPEN_BLOCKER),
        ..Fakes::passing()
    };
    let out = run_args(
        &repo,
        &tools(&repo, &blocked),
        &["run", "5", "--ci-poll-seconds", "0"],
    )
    .success();
    assert!(!String::from_utf8_lossy(&out.get_output().stdout).contains("blocked_by"));
}

#[test]
fn a_unit_with_a_workspace_but_no_run_log_is_not_refused() {
    let repo = Repo::new();
    let workspace = repo.path.parent().unwrap().join(format!(
        "{}.workspaces/5",
        repo.path.file_name().unwrap().to_str().unwrap()
    ));
    std::fs::create_dir_all(&workspace).unwrap();
    let fakes = Fakes {
        blocked_by: Some(OPEN_BLOCKER),
        ..Fakes::passing()
    };
    let out = run_with(&repo, &tools(&repo, &fakes), &[]);
    assert!(!String::from_utf8_lossy(&out.get_output().stdout).contains("blocked_by"));
}

#[test]
fn a_fresh_start_of_a_blocked_unit_is_refused() {
    let repo = Repo::new();
    let path = tools(&repo, &Fakes::passing());
    run_args(&repo, &path, &["run", "5", "--ci-poll-seconds", "0"]).success();
    let blocked = Fakes {
        blocked_by: Some(OPEN_BLOCKER),
        ..Fakes::passing()
    };
    let out = run_args(&repo, &tools(&repo, &blocked), &["run", "5", "--fresh"]).failure();
    assert!(String::from_utf8_lossy(&out.get_output().stdout).contains(r#""blocked_by":[3]"#));
}

#[test]
fn a_forge_that_cannot_say_what_blocks_a_unit_blocks_nothing() {
    let repo = Repo::new();
    let fakes = Fakes {
        blocked_by: None,
        ..Fakes::passing()
    };
    let out = run_with(&repo, &tools(&repo, &fakes), &[]).success();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("cannot read what blocks unit 5"), "{err}");
}

#[test]
fn a_unit_the_forge_does_not_list_as_open_is_not_refused() {
    let repo = Repo::new();
    let fakes = Fakes {
        blocked_by: Some(OPEN_BLOCKER),
        units: &[],
        ..Fakes::passing()
    };
    run_with(&repo, &tools(&repo, &fakes), &[]).success();
}

/// Sets `[review] sessions` in the repository's `nightshift.toml`, as committed before the unit
/// starts.
fn review_sessions_set(repo: &Repo, value: &str) {
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        format!("[review]\nsessions = {value}\n"),
    )
    .unwrap();
    git(&repo.path, &["add", ".nightshift/nightshift.toml"]);
    git(&repo.path, &["commit", "-q", "-m", "set review sessions"]);
}

fn review_prompts(repo: &Repo) -> usize {
    std::fs::read_dir(repo.path.parent().unwrap().join("bin"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("review.prompt.")
        })
        .count()
}

fn focus_line(prompt: &str) -> &str {
    prompt
        .split("# Your focus\n\n")
        .nth(1)
        .and_then(|rest| rest.lines().next())
        .unwrap()
}

#[test]
fn three_sessions_run_one_after_another_each_naming_its_own_group() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(review_prompts(&repo), 3);
    let focus: Vec<_> = (1..=3)
        .map(|n| focus_line(&review_prompt(&repo, n)).to_string())
        .collect();
    assert_eq!(
        focus,
        [
            "Behavior: review only these areas: correctness, security. Leave the others out.",
            "Requirements and tests: review only these areas: spec, tests. Leave the others out.",
            "Maintainability: review only these areas: architecture, docs and comments. Leave the others out.",
        ]
    );
}

#[test]
fn six_sessions_run_each_naming_one_area() {
    let repo = Repo::new();
    review_sessions_set(&repo, "6");
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(review_prompts(&repo), 6);
    let focus: Vec<_> = (1..=6)
        .map(|n| focus_line(&review_prompt(&repo, n)).to_string())
        .collect();
    assert_eq!(
        focus,
        [
            "Correctness: review only these areas: correctness. Leave the others out.",
            "Security: review only these areas: security. Leave the others out.",
            "Spec: review only these areas: spec. Leave the others out.",
            "Tests: review only these areas: tests. Leave the others out.",
            "Architecture: review only these areas: architecture. Leave the others out.",
            "Docs and comments: review only these areas: docs and comments. Leave the others out.",
        ]
    );
}

#[test]
fn one_session_reviews_all_six_areas() {
    let repo = Repo::new();
    review_sessions_set(&repo, "1");
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(review_prompts(&repo), 1);
    assert_eq!(
        focus_line(&review_prompt(&repo, 1)),
        "All areas: review only these areas: correctness, security, spec, tests, architecture, \
         docs and comments. Leave the others out."
    );
}

#[test]
fn one_failing_session_fails_the_review_and_the_kept_review_names_it() {
    let repo = Repo::new();
    let fakes = Fakes {
        review_fails_in: "Requirements and tests",
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).failure();
    // All three ran, then the review failed; it is not the failing session alone that is kept.
    assert_eq!(
        read(&repo, "review-1.md"),
        merged_review([PASS, FAIL, PASS])
    );
    assert!(read(&repo, "review-1.md")
        .contains("# Review session: Requirements and tests\n\nreviewed the diff\nVERDICT: FAIL"));
}

#[test]
fn the_next_review_shows_a_session_only_its_own_unmet_findings() {
    let repo = Repo::new();
    let fakes = Fakes {
        review_fails_in: "Maintainability",
        ..Fakes::passing()
    };
    run(&repo, &tools(&repo, &fakes)).failure();
    // Two attempts: round 1 is prompts 1-3, round 2 is prompts 4-6.
    assert!(!review_prompt(&repo, 4).contains(PREVIOUS));
    assert!(!review_prompt(&repo, 5).contains(PREVIOUS));
    assert!(review_prompt(&repo, 6).contains(PREVIOUS));
}

#[test]
fn a_session_that_goes_wrong_makes_the_review_go_wrong_and_the_next_run_repeats_all() {
    let repo = Repo::new();
    let fakes = Fakes {
        review_dies_in: "Requirements and tests",
        ..Fakes::passing()
    };
    let out = run(&repo, &tools(&repo, &fakes))
        .success()
        .get_output()
        .stdout
        .clone();
    let moves = moves(out);
    let at = moves.iter().position(|m| m == "verify>review").unwrap();
    // Neither PASS nor FAIL: the review runs again and then the unit ships.
    assert_eq!(
        &moves[at..at + 3],
        ["verify>review", "review>review", "review>ship"]
    );
    // Behavior ran, Requirements and tests died, then all three again.
    assert_eq!(review_prompts(&repo), 5);
    assert_eq!(read(&repo, "review-1.md"), merged_review([PASS; 3]));
    assert!(!repo.workspace("5").join(".ns/5/review-2.md").exists());
}

#[test]
fn a_review_sessions_setting_that_is_not_1_3_or_6_stops_ns_run_before_anything_runs() {
    for value in ["0", "2", "4", "7", "\"3\"", "2.5"] {
        let repo = Repo::new();
        review_sessions_set(&repo, value);
        let out = run(&repo, &tools(&repo, &Fakes::passing())).failure();
        let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
        assert!(err.contains("review.sessions"), "{value}: {err}");
        assert!(out.get_output().stdout.is_empty(), "{value}");
        assert!(!repo.workspace("5").exists(), "{value}");
    }
}

fn models_all(repo: &Repo) -> [Vec<String>; 3] {
    ["build", "verify", "review"].map(|kind| models_used(repo, kind))
}

#[test]
fn with_no_model_table_no_session_is_given_a_model() {
    let repo = Repo::new();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(models_all(&repo), [vec!["-"], vec!["-"], vec!["-"; 3]]);
}

#[test]
fn the_default_model_applies_to_every_state() {
    let repo = Repo::new();
    set_manifest(&repo, "[model]\ndefault = \"sonnet\"\n");
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(
        models_all(&repo),
        [vec!["sonnet"], vec!["sonnet"], vec!["sonnet"; 3]]
    );
}

#[test]
fn a_states_own_model_beats_the_default_and_every_review_session_uses_the_review_model() {
    let repo = Repo::new();
    set_manifest(
        &repo,
        "[model]\ndefault = \"sonnet\"\nbuild = \"opus\"\nreview = \"haiku\"\n",
    );
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(
        models_all(&repo),
        [vec!["opus"], vec!["sonnet"], vec!["haiku"; 3]]
    );
}

#[test]
fn the_model_option_beats_every_key() {
    let repo = Repo::new();
    set_manifest(
        &repo,
        "[model]\ndefault = \"sonnet\"\nbuild = \"opus\"\nreview = \"haiku\"\n",
    );
    run_with(
        &repo,
        &tools(&repo, &Fakes::passing()),
        &["--model", "fable"],
    )
    .success();
    assert_eq!(
        models_all(&repo),
        [vec!["fable"], vec!["fable"], vec!["fable"; 3]]
    );
}

fn origin(repo: &Repo) -> std::path::PathBuf {
    repo.path.parent().unwrap().join("origin.git")
}

/// Publishes the repository's `main` to its origin, as a repository that has one.
fn main_published(repo: &Repo) {
    git(&repo.path, &["push", "-q", "origin", "main"]);
}

/// A shell snippet that adds a commit with the file `name` to the origin's `main`, as another
/// unit merging would, through a clone of its own.
fn base_advance_script(repo: &Repo, name: &str, text: &str) -> String {
    let other = repo.path.parent().unwrap().join("other");
    format!(
        r#"[ -d "{other}" ] || git clone -q "{origin}" "{other}"
cd "{other}" && git pull -q origin main
echo "{text}" > '{name}' && git add -A && git -c user.name=t -c user.email=t@t commit -q -m "other {name}" && git push -q origin main"#,
        other = other.display(),
        origin = origin(repo).display(),
    )
}

/// Moves the origin's `main` once now, before the unit starts, and returns its new tip.
fn base_advanced(repo: &Repo, name: &str, text: &str) -> String {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(base_advance_script(repo, name, text))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    git(&origin(repo), &["rev-parse", "main"])
        .trim()
        .to_string()
}

/// Has the base branch move, by a new file each time, on each look at the change request's
/// checks, up to `times` times.
fn base_moves_on_checks(repo: &Repo, times: u32) {
    let bin = repo.path.parent().unwrap().join("bin");
    script(
        &bin,
        "on-pr-checks",
        &format!(
            r#"n=$(cat "{bin}/moved" 2>/dev/null || echo 0)
[ "$n" -ge {times} ] && exit 0
echo $((n+1)) > "{bin}/moved"
{mover}"#,
            bin = bin.display(),
            mover = base_advance_script(repo, "moved.txt", "moved $n"),
        ),
    );
}

/// Has the repository's checks append a line to a log, and fail while `other.txt` is present when
/// `fail_with_other` is set.
fn counted_check(repo: &Repo, fail_with_other: bool) {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fail = if fail_with_other {
        "; if [ -e other.txt ]; then echo other file present; exit 1; fi"
    } else {
        ""
    };
    repo.set_check(&format!("echo x >> '{}/check.log'{fail}", bin.display()));
}

fn check_runs(repo: &Repo) -> usize {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/check.log"))
        .unwrap_or_default()
        .lines()
        .count()
}

fn branch_parent(repo: &Repo) -> String {
    git(&repo.workspace("5"), &["rev-parse", "HEAD~1"])
        .trim()
        .to_string()
}

#[test]
fn a_base_that_has_not_moved_rebases_nothing_and_runs_no_extra_check() {
    let repo = Repo::new();
    counted_check(&repo, false);
    main_published(&repo);
    let start = git(&repo.path, &["rev-parse", "main"]).trim().to_string();
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(branch_parent(&repo), start);
    assert_eq!(check_runs(&repo), 1, "only the build's check ran");
    assert_eq!(
        git(&repo.path, &["rev-parse", "refs/ns/base/ns/5"]).trim(),
        start
    );
}

#[test]
fn a_bad_model_table_stops_ns_run_before_anything_runs() {
    for (text, key) in [
        ("[model]\nbuidl = \"opus\"\n", "model.buidl"),
        ("[model]\nbuild = \"\"\n", "model.build"),
        ("[model]\nreview = 3\n", "model.review"),
    ] {
        let repo = Repo::new();
        set_manifest(&repo, text);
        let out = run(&repo, &tools(&repo, &Fakes::passing())).failure();
        let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
        assert!(err.contains(key), "{text}: {err}");
        assert!(err.contains("nightshift.toml"), "{text}: {err}");
        assert!(out.get_output().stdout.is_empty(), "{text}");
        assert!(!repo.workspace("5").exists(), "{text}");
    }
}

#[test]
fn an_empty_model_option_is_refused() {
    let repo = Repo::new();
    run_with(&repo, &tools(&repo, &Fakes::passing()), &["--model", ""]).failure();
    assert!(!repo.workspace("5").exists());
}

fn move_events(out: &assert_cmd::assert::Assert) -> Vec<serde_json::Value> {
    String::from_utf8(out.get_output().stdout.clone())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|e| e.get("to").is_some())
        .collect()
}

#[test]
fn the_moves_out_of_a_session_state_say_which_model_it_used_and_the_run_log_keeps_it() {
    let repo = Repo::new();
    set_manifest(&repo, "[model]\nbuild = \"opus\"\n");
    let out = run(&repo, &tools(&repo, &Fakes::passing())).success();
    let model_of = |from: &str| {
        let events = move_events(&out);
        let event = events.iter().find(|e| e["from"] == from).unwrap();
        event.get("model").cloned()
    };
    assert_eq!(model_of("build"), Some("opus".into()));
    assert_eq!(model_of("verify"), Some(serde_json::Value::Null));
    assert_eq!(model_of("review"), Some(serde_json::Value::Null));
    assert_eq!(model_of("setup"), None);
    let log = std::fs::read_to_string(repo.path.join(".git/ns/runs.jsonl")).unwrap();
    assert!(
        log.lines()
            .any(|l| l.contains(r#""from":"build""#) && l.contains(r#""model":"opus""#)),
        "{log}"
    );
}

#[test]
fn log_shows_the_model_each_session_used() {
    let repo = Repo::new();
    set_manifest(&repo, "[model]\nbuild = \"opus\"\n");
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    let out = Command::cargo_bin("ns")
        .unwrap()
        .args(["log", "5"])
        .current_dir(&repo.path)
        .assert()
        .success();
    let out = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(out.contains("== build 1 (opus) =="), "{out}");
    assert!(
        out.contains("== verify 1 (harness default model) =="),
        "{out}"
    );
}

#[test]
fn a_recorded_base_left_behind_by_a_stopped_rebase_is_moved_without_another_check() {
    let repo = Repo::new();
    main_published(&repo);
    let start = git(&repo.path, &["rev-parse", "main"]).trim().to_string();
    let tip = base_advanced(&repo, "other.txt", "other");
    git(&repo.path, &["pull", "-q", "origin", "main"]);
    // The branch already holds the base tip, but the build's check leaves the recorded base on
    // the old commit, as a run stopped between the rebase and the move would.
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    repo.set_check(&format!(
        "echo x >> '{}/check.log'; git update-ref refs/ns/base/ns/5 {start}",
        bin.display()
    ));
    run(&repo, &tools(&repo, &Fakes::passing())).success();
    assert_eq!(check_runs(&repo), 1, "only the build's check ran");
    assert_eq!(
        git(&repo.path, &["rev-parse", "refs/ns/base/ns/5"]).trim(),
        tip
    );
}

#[test]
fn a_base_that_moved_in_other_files_is_rebased_onto_and_the_result_checked() {
    let repo = Repo::new();
    counted_check(&repo, false);
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    let tip = {
        // The unit starts from the old main; the origin moves on before it ships.
        let moved = base_advanced(&repo, "other.txt", "other");
        assert_ne!(moved, git(&repo.path, &["rev-parse", "main"]).trim());
        moved
    };
    run(&repo, &path).success();
    assert_eq!(branch_parent(&repo), tip);
    assert_eq!(
        check_runs(&repo),
        2,
        "the build's check and the rebased result's"
    );
    assert_eq!(
        git(&repo.path, &["rev-parse", "refs/ns/base/ns/5"]).trim(),
        tip
    );
    // The unit's change is only its own file, so what is read from the recorded base is.
    assert_eq!(
        git(
            &repo.workspace("5"),
            &["diff", "--name-only", &format!("{tip}..HEAD")]
        )
        .trim(),
        "thing.txt"
    );
    assert_eq!(git(&origin(&repo), &["rev-parse", "ns/5~1"]).trim(), tip);
}

#[test]
fn a_rebase_does_not_run_verify_or_review_again() {
    let repo = Repo::new();
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    base_advanced(&repo, "other.txt", "other");
    let out = run(&repo, &path).success().get_output().stdout.clone();
    let moves = moves(out);
    assert_eq!(
        moves.iter().filter(|m| *m == "build>verify").count(),
        1,
        "{moves:?}"
    );
    assert_eq!(
        moves.iter().filter(|m| *m == "verify>review").count(),
        1,
        "{moves:?}"
    );
    assert!(!repo
        .path
        .parent()
        .unwrap()
        .join("bin/verify.prompt.2")
        .exists());
    assert_eq!(review_prompts(&repo), 3, "one review of three sessions");
}

#[test]
fn a_base_that_moved_is_brought_in_without_a_check_when_the_repository_has_none() {
    let repo = Repo::new();
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    let tip = base_advanced(&repo, "other.txt", "other");
    run(&repo, &path).success();
    assert_eq!(branch_parent(&repo), tip);
}

#[test]
fn a_failing_check_on_the_rebased_result_sends_the_unit_back_to_build_with_its_output() {
    let repo = Repo::new();
    counted_check(&repo, true);
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    base_advanced(&repo, "other.txt", "other");
    let out = run(&repo, &path).failure().get_output().stdout.clone();
    let moves = moves(out);
    assert!(moves.contains(&"ship>build".to_string()), "{moves:?}");
    let evidence = read(&repo, "evidence.md");
    assert!(evidence.contains("other file present"), "{evidence}");
    assert!(evidence.ends_with("VERDICT: FAIL\n"), "{evidence}");
}

#[test]
fn a_conflict_sends_the_unit_back_to_build_naming_the_files_and_leaves_the_workspace_as_it_was() {
    let repo = Repo::new();
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    base_advanced(&repo, "thing.txt", "theirs");
    let before = git(&repo.path, &["rev-parse", "main"]).trim().to_string();
    let out = run(&repo, &path).failure().get_output().stdout.clone();
    let moves = moves(out);
    assert!(moves.contains(&"ship>build".to_string()), "{moves:?}");
    let evidence = read(&repo, "evidence.md");
    assert!(evidence.contains("thing.txt"), "{evidence}");
    assert!(
        evidence.contains("Rebase onto the base branch and resolve them"),
        "{evidence}"
    );
    assert!(evidence.ends_with("VERDICT: FAIL\n"), "{evidence}");
    let ws = repo.workspace("5");
    assert_eq!(git(&ws, &["status", "--porcelain"]).trim(), "");
    assert_eq!(git(&ws, &["rev-parse", "HEAD~1"]).trim(), before);
    assert!(!ws.join(".git").is_dir());
    assert_eq!(
        std::fs::read_to_string(ws.join("thing.txt")).unwrap(),
        "thing\n"
    );
}

#[test]
fn with_the_queue_off_nothing_is_rebased_no_lock_is_taken_and_the_merge_names_the_head() {
    let repo = Repo::new();
    counted_check(&repo, false);
    manifest_set(&repo, "[merge]\nqueue = false\n");
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    let old = base_advanced(&repo, "other.txt", "other");
    // A held lock would stop the run, were it asked for.
    let lock = repo.path.join(".git/ns/merge.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = std::fs::File::create(&lock).unwrap();
    held.lock().unwrap();
    run_with(&repo, &path, &["--merge"]).success();
    assert_ne!(git(&origin(&repo), &["rev-parse", "ns/5~1"]).trim(), old);
    assert_eq!(check_runs(&repo), 1);
    let head = git(&origin(&repo), &["rev-parse", "ns/5"])
        .trim()
        .to_string();
    assert!(gh_log(&repo).contains(&format!("pr merge 9 --squash --match-head-commit {head}")));
}

#[test]
fn a_base_that_moves_after_the_checks_passed_sends_the_unit_round_again_and_the_new_head_merges() {
    let repo = Repo::new();
    counted_check(&repo, false);
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    base_moves_on_checks(&repo, 1);
    run_with(&repo, &path, &["--merge"]).success();
    let log = gh_log(&repo);
    assert_eq!(log.matches("pr create").count(), 1, "{log}");
    assert_eq!(log.matches("pr merge").count(), 1, "{log}");
    let head = git(&origin(&repo), &["rev-parse", "ns/5"])
        .trim()
        .to_string();
    let tip = git(&origin(&repo), &["rev-parse", "main"])
        .trim()
        .to_string();
    assert_eq!(git(&origin(&repo), &["rev-parse", "ns/5~1"]).trim(), tip);
    assert!(
        log.contains(&format!("--match-head-commit {head}")),
        "{log}"
    );
    assert_eq!(check_runs(&repo), 2);
}

#[test]
fn a_base_that_keeps_moving_sticks_the_unit_saying_so() {
    let repo = Repo::new();
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    base_moves_on_checks(&repo, 99);
    let out = run_with(&repo, &path, &["--merge"]).failure();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("the base branch kept moving"), "{stdout}");
    assert!(
        stdout.contains("\"ship\",\"to\":\"stuck\"")
            || stdout.contains("ship>stuck")
            || moves(stdout.clone().into_bytes()).contains(&"ship>stuck".to_string())
    );
    assert!(!gh_log(&repo).contains("pr merge"));
    // The default allows three moves, so the fourth is one too many.
    assert_eq!(
        std::fs::read_to_string(repo.path.parent().unwrap().join("bin/moved"))
            .unwrap()
            .trim(),
        "4"
    );
    let last = run_log(&repo).pop().unwrap();
    assert_eq!(last["reason"], "base_moved");
}

#[test]
fn a_setting_of_one_move_sticks_the_unit_at_the_second() {
    let repo = Repo::new();
    manifest_set(&repo, "[merge]\nmax_base_moves = 1\n");
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    base_moves_on_checks(&repo, 99);
    let out = run_with(&repo, &path, &["--merge"]).failure();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("the base branch kept moving"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(repo.path.parent().unwrap().join("bin/moved"))
            .unwrap()
            .trim(),
        "2"
    );
    assert!(!gh_log(&repo).contains("pr merge"));
}

#[test]
fn a_unit_left_for_a_person_is_also_brought_up_to_date_and_checked() {
    let repo = Repo::new();
    counted_check(&repo, false);
    manifest_set(&repo, "[merge]\nhuman_review = [\"*.txt\"]\n");
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    let tip = base_advanced(&repo, "other.txt", "other");
    run_with(&repo, &path, &["--merge"]).success();
    assert!(!gh_log(&repo).contains("pr merge"));
    assert!(gh_log(&repo).contains("needs-human-merge"));
    assert_eq!(branch_parent(&repo), tip);
    assert_eq!(check_runs(&repo), 2);
}

#[test]
fn the_merge_waits_for_the_lock_another_unit_holds() {
    let repo = Repo::new();
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    let lock = repo.path.join(".git/ns/merge.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = std::fs::File::create(&lock).unwrap();
    held.lock().unwrap();
    let log = repo.path.parent().unwrap().join("bin/gh.log");
    let watcher = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let merged_early = std::fs::read_to_string(log)
            .unwrap_or_default()
            .contains("pr merge");
        drop(held);
        merged_early
    });
    run_with(&repo, &path, &["--merge"]).success();
    assert!(!watcher.join().unwrap(), "merged while the lock was held");
    assert!(gh_log(&repo).contains("pr merge"));
}

/// Has the repository's checks and a build that is sent back to `build` log, on each run, whether
/// the merge lock was held. The checks fail while `other.txt` is present when `fail_with_other`.
fn lock_watching_check(repo: &Repo, fail_with_other: bool) {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let lock = repo.path.join(".git/ns/merge.lock");
    let probe = format!(
        "if [ ! -e '{lock}' ] || flock -n '{lock}' true; then echo $1-free; else echo $1-held; fi >> '{bin}/lock.log'",
        lock = lock.display(),
        bin = bin.display()
    );
    script(&bin, "on-rebuild", &format!("set -- rebuild\n{probe}"));
    let fail = if fail_with_other {
        "; if [ -e other.txt ]; then exit 1; fi"
    } else {
        ""
    };
    repo.set_check(&format!("set -- check; {probe}{fail}"));
}

fn lock_log(repo: &Repo) -> Vec<String> {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/lock.log"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn a_unit_to_be_merged_does_not_rebase_until_the_lock_is_free_and_then_rebases_onto_the_new_base() {
    let repo = Repo::new();
    counted_check(&repo, false);
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    let lock = repo.path.join(".git/ns/merge.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = std::fs::File::create(&lock).unwrap();
    held.lock().unwrap();
    let (log, origin_dir) = (repo.path.parent().unwrap().join("bin"), origin(&repo));
    let script = base_advance_script(&repo, "first.txt", "first");
    let watcher = std::thread::spawn(move || {
        // Wait until the unit is at the lock, then let the unit ahead merge into the base.
        std::thread::sleep(std::time::Duration::from_secs(2));
        let checks_while_held = std::fs::read_to_string(log.join("check.log"))
            .unwrap_or_default()
            .lines()
            .count();
        let pushed_while_held = origin_dir.join("refs/heads/ns/5").exists();
        assert!(std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .status()
            .unwrap()
            .success());
        let tip = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "main"])
                .current_dir(&origin_dir)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        drop(held);
        (checks_while_held, pushed_while_held, tip.trim().to_string())
    });
    run_with(&repo, &path, &["--merge"]).success();
    let (checks_while_held, pushed_while_held, tip) = watcher.join().unwrap();
    assert_eq!(checks_while_held, 1, "only the build's check ran");
    assert!(!pushed_while_held);
    assert_eq!(git(&origin(&repo), &["rev-parse", "ns/5~1"]).trim(), tip);
    assert_eq!(check_runs(&repo), 2, "the build's, and one on the rebase");
    assert_eq!(gh_log(&repo).matches("pr merge").count(), 1);
}

#[test]
fn a_unit_sent_back_to_build_by_a_conflict_lets_go_of_the_lock() {
    let repo = Repo::new();
    lock_watching_check(&repo, false);
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    base_advanced(&repo, "thing.txt", "theirs");
    run_with(&repo, &path, &["--merge"]).failure();
    // The conflict is found under the lock; the build it sends the unit back to has let go.
    assert_eq!(lock_log(&repo), ["check-free", "rebuild-free"]);
}

#[test]
fn a_failing_check_on_the_rebase_lets_go_of_the_lock() {
    let repo = Repo::new();
    lock_watching_check(&repo, true);
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    base_advanced(&repo, "other.txt", "other");
    run_with(&repo, &path, &["--merge"]).failure();
    let log = lock_log(&repo);
    // The build's, the rebase's under the lock, then the build the unit went back to.
    assert_eq!(
        &log[..3],
        ["check-free", "check-held", "rebuild-free"],
        "{log:?}"
    );
}

#[test]
fn failing_checks_on_the_change_request_let_go_of_the_lock() {
    let repo = Repo::new();
    lock_watching_check(&repo, false);
    main_published(&repo);
    let fakes = Fakes {
        ci: Ci::FailsOnce,
        ..Fakes::passing()
    };
    let path = tools(&repo, &fakes);
    run_with(&repo, &path, &["--merge"]).success();
    assert_eq!(lock_log(&repo), ["check-free", "rebuild-free"]);
}

#[test]
fn a_unit_that_needs_a_human_merge_never_waits_for_the_lock() {
    let repo = Repo::new();
    manifest_set(&repo, "[merge]\nhuman_review = [\"*.txt\"]\n");
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    let lock = repo.path.join(".git/ns/merge.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = std::fs::File::create(&lock).unwrap();
    held.lock().unwrap();
    run_with(&repo, &path, &["--merge"]).success();
    assert!(gh_log(&repo).contains("needs-human-merge"));
    assert!(!gh_log(&repo).contains("pr merge"));
    drop(held);
}

#[test]
fn a_unit_not_asked_to_merge_never_waits_for_the_lock() {
    let repo = Repo::new();
    main_published(&repo);
    let path = tools(&repo, &Fakes::passing());
    let lock = repo.path.join(".git/ns/merge.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = std::fs::File::create(&lock).unwrap();
    held.lock().unwrap();
    run(&repo, &path).success();
    assert!(gh_log(&repo).contains("pr create"));
    drop(held);
}

#[test]
fn a_bad_merge_setting_is_an_error_that_names_the_key_and_nothing_runs() {
    for (key, bad) in [
        ("queue", "\"yes\""),
        ("queue", "1"),
        ("max_base_moves", "0"),
        ("max_base_moves", "-1"),
        ("max_base_moves", "\"3\""),
        ("max_base_moves", "2.5"),
    ] {
        let repo = Repo::new();
        set_manifest(&repo, &format!("[merge]\n{key} = {bad}\n"));
        let out = run_unset(&repo, &tools(&repo, &Fakes::passing())).failure();
        let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
        assert!(
            err.contains(&format!("merge.{key}")),
            "{key} = {bad}: {err}"
        );
        assert!(out.get_output().stdout.is_empty(), "{key} = {bad}");
    }
}
