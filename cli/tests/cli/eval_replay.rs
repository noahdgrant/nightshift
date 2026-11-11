use std::path::PathBuf;

use crate::common::{git, script, Repo};

use assert_cmd::Command;

/// A repository whose checks pass once `built.txt` exists, with a case `old` for unit 4 whose
/// base is the commit that set the checks, and one later commit that must not be in a replay.
/// Unit 4 has a grade line that records 1000 tokens. Returns the repository and the base.
fn fixture() -> (Repo, String) {
    let repo = Repo::new();
    repo.set_check("test -f built.txt");
    let base = git(&repo.path, &["rev-parse", "HEAD"]).trim().to_string();
    std::fs::write(repo.path.join("later.txt"), "later\n").unwrap();
    git(&repo.path, &["add", "later.txt"]);
    git(&repo.path, &["commit", "-q", "-m", "later"]);
    grade_line(&repo, "4", &base, Some(1000));
    case(&repo, "old", Some("4"), &base);
    (repo, base)
}

/// Appends a grade line for `unit` to `grades.jsonl`, with one session of `tokens` when given.
fn grade_line(repo: &Repo, unit: &str, base: &str, tokens: Option<u64>) {
    use std::io::Write;
    let dir = repo.path.join(".git/ns");
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("grades.jsonl"))
        .unwrap();
    let sessions = match tokens {
        Some(t) => format!(
            r#"[{{"state":"build","model":"m","tokens":{{"input":{t},"output":0,"cache_read":0,"cache_write":0}}}}]"#
        ),
        None => "[]".to_string(),
    };
    writeln!(
        file,
        r#"{{"unit":"{unit}","rubric":"efficiency","score":1,"pass":true,"reason":"r","version":"0.1.0","base":"{base}","sessions":{sessions},"at":1700000000}}"#
    )
    .unwrap();
}

/// Writes the case `name` into `evals/replay`.
fn case(repo: &Repo, name: &str, unit: Option<&str>, base: &str) {
    let dir = repo.path.join("evals/replay").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("unit.md"), "feat: add built\n\nMake built.txt.\n").unwrap();
    std::fs::write(dir.join("base"), format!("{base}\n")).unwrap();
    if let Some(unit) = unit {
        std::fs::write(dir.join("unit"), format!("{unit}\n")).unwrap();
    }
}

fn bin(repo: &Repo) -> PathBuf {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    bin
}

/// A fake `claude`. Its build sessions commit `built.txt` unless the model is `lazy`, or `flaky`
/// on its second build; they use 100 input and 50 output tokens (`big`: 300 and 100). Its
/// grading sessions use 20 and 10, and grade `efficiency` 0.1 for the model `picky` and every
/// rubric 1 for the others. It records each call's model and directory in `claude.log`, and what
/// a build sees in `seen.<n>`.
fn fake(repo: &Repo) {
    let b = bin(repo);
    let names = [
        "efficiency",
        "followed-the-brief",
        "redundant-tests",
        "verbosity",
    ];
    let grades = |eff: &str| {
        names
            .iter()
            .map(|n| {
                format!(
                    "GRADE: {n} {} fine",
                    if *n == "efficiency" { eff } else { "1" }
                )
            })
            .collect::<Vec<_>>()
            .join("\\n")
    };
    script(
        &b,
        "claude",
        &format!(
            r#"prompt=$(cat)
model=$(echo "$*" | sed -n 's/.*--model \([^ ]*\).*/\1/p')
echo "$model $PWD" >> "{b}/claude.log"
echo x >> "{b}/claude.count"
usage() {{ printf '%s\n' '{{"type":"assistant","message":{{"id":"a","model":"m","usage":{{"input_tokens":'$1',"output_tokens":'$2'}}}}}}'; }}
case "$prompt" in
*"You grade how one unit went"*)
  usage 20 10
  if [ "$model" = picky ]; then
    printf '%s\n' '{{"type":"result","result":"{picky}"}}'
  else
    printf '%s\n' '{{"type":"result","result":"{good}"}}'
  fi;;
*)
  echo x >> "{b}/builds.$model"
  n=$(wc -l < "{b}/builds.$model")
  echo "$prompt" > "{b}/prompt.$model.$n"
  ls > "{b}/seen.$n"
  if [ "$model" = big ]; then usage 300 100; else usage 100 50; fi
  if [ "$model" = hang ]; then exec sleep 30; fi
  if [ "$model" = limited ]; then echo "Claude AI usage limit reached|1773180000"; exit 1; fi
  if [ "$model" = lazy ] || [ "$model" = flaky -a $n = 2 ]; then :; else
    echo built > built.txt
    git add built.txt
    git -c user.name=t -c user.email=t@x commit -q -m "feat: built"
  fi
  printf '%s\n' '{{"type":"result","result":"Built it."}}';;
esac"#,
            b = b.display(),
            good = grades("1"),
            picky = grades("0.1"),
        ),
    );
}

fn replay(repo: &Repo, extra: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("ns")
        .unwrap()
        .args(["eval", "replay"])
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin(repo).display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

fn calls(repo: &Repo) -> usize {
    std::fs::read_to_string(bin(repo).join("claude.count"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn stdout(out: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

fn stderr(out: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(out.get_output().stderr.clone()).unwrap()
}

fn json_lines(out: &assert_cmd::assert::Assert) -> Vec<serde_json::Value> {
    stdout(out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn two_models_over_three_runs_give_the_pass_rate_its_spread_and_the_median_tokens() {
    let (repo, base) = fixture();
    fake(&repo);
    let branches = git(&repo.path, &["branch", "--format=%(refname)"]);
    let head = git(&repo.path, &["rev-parse", "HEAD"]);
    let out = replay(
        &repo,
        &["--model", "flaky", "--model", "big", "--yes", "--json"],
    )
    .success();
    assert_eq!(
        json_lines(&out),
        [
            serde_json::json!({
                "model": "flaky", "replays": 3, "pass_rate": 2.0 / 3.0,
                "lowest": 0.0, "highest": 1.0, "median_tokens": 180,
            }),
            serde_json::json!({
                "model": "big", "replays": 3, "pass_rate": 1.0,
                "lowest": 1.0, "highest": 1.0, "median_tokens": 430,
            }),
        ]
    );
    // 3 runs of 2 models: 6 builds and 5 gradings (the flaky model's second build fails the checks).
    assert_eq!(calls(&repo), 11);
    // A build got the case's brief, on the base and not on the later commit.
    let prompt = std::fs::read_to_string(bin(&repo).join("prompt.flaky.1")).unwrap();
    assert!(
        prompt.contains("## feat: add built\n\nMake built.txt.\n\nThe branch started at commit "),
        "{prompt}"
    );
    assert!(prompt.contains(&base), "{prompt}");
    assert_eq!(
        std::fs::read_to_string(bin(&repo).join("seen.1")).unwrap(),
        "",
        "the workspace held later.txt, or the work of an earlier replay"
    );
    // Nothing changed in the repository it was run from.
    assert_eq!(
        git(&repo.path, &["branch", "--format=%(refname)"]),
        branches
    );
    assert_eq!(git(&repo.path, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&repo.path, &["worktree", "list"]).lines().count(), 1);
}

#[test]
fn a_replay_that_fails_the_checks_or_a_rubric_or_the_clock_is_a_fail() {
    let (repo, _) = fixture();
    fake(&repo);
    for (model, extra) in [
        ("lazy", vec![]),
        ("picky", vec![]),
        ("hang", vec!["--harness-timeout-seconds", "1"]),
    ] {
        let mut args = vec!["--model", model, "--runs", "1", "--yes", "--json"];
        args.extend(extra);
        let out = replay(&repo, &args).success();
        let line = &json_lines(&out)[0];
        assert_eq!(line["pass_rate"], 0.0, "{model}");
        assert_eq!(line["replays"], 1, "{model}");
    }
}

#[test]
fn the_workspace_and_its_branch_are_removed_after_a_failed_replay_and_a_timeout() {
    let (repo, _) = fixture();
    fake(&repo);
    let branches = git(&repo.path, &["branch", "--format=%(refname)"]);
    for (model, extra) in [
        ("lazy", vec![]),
        ("hang", vec!["--harness-timeout-seconds", "1"]),
    ] {
        let mut args = vec!["--model", model, "--runs", "1", "--yes"];
        args.extend(extra);
        replay(&repo, &args).success();
        let log = std::fs::read_to_string(bin(&repo).join("claude.log")).unwrap();
        let dir = log.lines().rfind(|l| l.starts_with(model)).unwrap();
        let dir = dir.split_once(' ').unwrap().1;
        assert!(dir.contains("workspace"), "{dir}");
        assert!(!std::path::Path::new(dir).exists(), "{model}: {dir}");
        assert_eq!(
            git(&repo.path, &["branch", "--format=%(refname)"]),
            branches
        );
        assert_eq!(git(&repo.path, &["worktree", "list"]).lines().count(), 1);
    }
}

#[test]
fn a_case_whose_commit_is_not_found_stops_before_any_session() {
    let (repo, _) = fixture();
    fake(&repo);
    case(&repo, "lost", Some("4"), &"0".repeat(40));
    let out = replay(&repo, &["--model", "m", "--yes"]).code(1);
    let err = stderr(&out);
    assert!(
        err.contains("case lost") && err.contains("not a commit"),
        "{err}"
    );
    assert_eq!(calls(&repo), 0);
}

#[test]
fn a_case_that_cannot_be_read_exits_1() {
    let (repo, _) = fixture();
    fake(&repo);
    std::fs::remove_file(repo.path.join("evals/replay/old/unit.md")).unwrap();
    let out = replay(&repo, &["--model", "m", "--yes"]).code(1);
    assert!(
        stderr(&out).contains("case old: cannot read"),
        "{}",
        stderr(&out)
    );
    assert_eq!(calls(&repo), 0);
}

#[test]
fn it_prints_the_sessions_and_the_token_estimate_and_counts_cases_with_no_usage() {
    let (repo, base) = fixture();
    fake(&repo);
    case(&repo, "newer", Some("5"), &base);
    grade_line(&repo, "5", &base, None);
    let out = replay(
        &repo,
        &["--model", "a", "--model", "b", "--runs", "2", "--yes"],
    )
    .success();
    let err = stderr(&out);
    assert!(
        err.contains("2 cases on 2 models, 2 runs: 8 replays, up to 16 sessions"),
        "{err}"
    );
    // 1000 tokens for the one case with usage, over 2 models and 2 runs.
    assert!(
        err.contains("about 4000") && err.contains("1 of 2 cases have no recorded usage"),
        "{err}"
    );
}

#[test]
fn a_usage_limit_stops_the_run_with_exit_1_and_prints_no_figures() {
    let (repo, _) = fixture();
    fake(&repo);
    let out = replay(&repo, &["--model", "limited", "--yes", "--json"]).code(1);
    assert_eq!(stdout(&out), "");
    assert_eq!(calls(&repo), 1);
}

#[test]
fn a_case_without_a_unit_file_counts_as_unknown_in_the_estimate() {
    let (repo, base) = fixture();
    fake(&repo);
    case(&repo, "hand", None, &base);
    let out = replay(&repo, &["--model", "a", "--runs", "1", "--yes"]).success();
    let err = stderr(&out);
    assert!(err.contains("1 of 2 cases have no recorded usage"), "{err}");
}

#[test]
fn without_a_terminal_and_without_yes_it_refuses_to_start() {
    let (repo, _) = fixture();
    fake(&repo);
    let out = replay(&repo, &["--model", "m"]).failure();
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
    assert_eq!(calls(&repo), 0);
}

#[test]
fn max_runs_refuses_when_the_sessions_would_be_more() {
    let (repo, _) = fixture();
    fake(&repo);
    // 1 case, 2 models, 3 runs: 12 sessions.
    let args = ["--model", "a", "--model", "b", "--yes", "--max-runs"];
    let out = replay(&repo, &[&args[..], &["11"]].concat()).failure();
    assert!(stderr(&out).contains("12 sessions"), "{}", stderr(&out));
    assert_eq!(calls(&repo), 0);
    replay(&repo, &[&args[..], &["12"]].concat()).success();
    assert_eq!(calls(&repo), 12);
}

#[test]
fn case_runs_only_that_case_and_the_text_report_names_the_figures() {
    let (repo, base) = fixture();
    fake(&repo);
    case(&repo, "other", Some("5"), &base);
    let out = replay(
        &repo,
        &["--model", "m", "--case", "old", "--runs", "2", "--yes"],
    )
    .success();
    assert_eq!(
        stdout(&out),
        "m: 2 replays, pass rate 100% (lowest 100%, highest 100% over the runs), median tokens 180\n"
    );
}

/// Runs `ns eval add replay` with `extra` arguments. The fake `gh` knows unit 4 only.
fn add(repo: &Repo, extra: &[&str]) -> assert_cmd::assert::Assert {
    let b = bin(repo);
    script(
        &b,
        "gh",
        r#"case "$1 $2" in
"api repos/{owner}/{repo}/issues/4") printf '%s\n' '{"number":4,"title":"feat: a title","body":"A description.\n\nMore.","labels":[],"author_association":"OWNER","html_url":"https://github.com/o/r/issues/4"}';;
*) echo 'not found' >&2; exit 1;;
esac"#,
    );
    Command::cargo_bin("ns")
        .unwrap()
        .args(["eval", "add", "replay"])
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", b.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

fn read(repo: &Repo, case: &str, file: &str) -> String {
    std::fs::read_to_string(repo.path.join("evals/replay").join(case).join(file)).unwrap()
}

#[test]
fn add_writes_the_unit_text_and_the_commit_of_its_grade_line() {
    let repo = Repo::new();
    let base = git(&repo.path, &["rev-parse", "HEAD"]).trim().to_string();
    grade_line(&repo, "4", "somethingelse", None);
    grade_line(&repo, "4", &base, None);
    let out = add(&repo, &["4", "--as", "first-case"]).success();
    assert_eq!(
        json_lines(&out),
        [serde_json::json!({
            "case": "first-case",
            "dir": repo.path.join("evals/replay/first-case"),
        })]
    );
    assert_eq!(
        read(&repo, "first-case", "unit.md"),
        "feat: a title\n\nA description.\n\nMore.\n"
    );
    assert_eq!(read(&repo, "first-case", "base"), format!("{base}\n"));
}

#[test]
fn add_that_cannot_write_a_case_writes_nothing() {
    let repo = Repo::new();
    let base = git(&repo.path, &["rev-parse", "HEAD"]).trim().to_string();
    grade_line(&repo, "4", &base, None);
    grade_line(&repo, "9", &base, None);
    grade_line(&repo, "8", &"0".repeat(40), None);
    add(&repo, &["4", "--as", "taken"]).success();
    let before = read(&repo, "taken", "unit.md");
    let cases = repo.path.join("evals/replay");
    let names = || {
        let mut n: Vec<_> = std::fs::read_dir(&cases)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        n.sort();
        n
    };
    for (args, said) in [
        (vec!["4", "--as", "taken"], "already exists"),
        (
            vec!["4", "--as", "Bad Name"],
            "lowercase letters, digits and hyphens",
        ),
        (vec!["6", "--as", "ungraded"], "unit 6 has no grade line"),
        (vec!["8", "--as", "lost"], "is not a commit"),
        (vec!["9", "--as", "orphan"], "cannot read unit 9"),
    ] {
        let out = add(&repo, &args).failure();
        assert!(stderr(&out).contains(said), "{said}: {}", stderr(&out));
        assert_eq!(names(), ["taken"], "{said}");
    }
    assert_eq!(read(&repo, "taken", "unit.md"), before);
}
