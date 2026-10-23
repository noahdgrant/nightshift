use crate::common::{git, script, Repo};

use assert_cmd::Command;

/// Writes the case `name` into the repository's evals, with `expect` as its `expect` file when
/// it is given.
fn case(repo: &Repo, name: &str, expect: Option<&str>) {
    let dir = repo.path.join("evals/review").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("unit.md"), "feat: a title\n\nA description.\n").unwrap();
    std::fs::write(dir.join("build.md"), "built it\n").unwrap();
    std::fs::write(dir.join("change.diff"), "diff --git a/x b/x\n").unwrap();
    if let Some(expect) = expect {
        std::fs::write(dir.join("expect"), expect).unwrap();
    }
}

/// Runs `ns eval review` with `extra` arguments. The fake `claude` replies with the next of
/// `replies`, one for each call, and counts its calls in `claude.count`.
fn eval(repo: &Repo, replies: &[&str], extra: &[&str]) -> assert_cmd::assert::Assert {
    eval_in(repo, &repo.path, replies, extra)
}

/// Like `eval`, run from the directory `dir`.
fn eval_in(
    repo: &Repo,
    dir: &std::path::Path,
    replies: &[&str],
    extra: &[&str],
) -> assert_cmd::assert::Assert {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut arms = String::new();
    for (i, reply) in replies.iter().enumerate() {
        arms.push_str(&format!("{}) echo '{reply}';;\n", i + 1));
    }
    script(
        &bin,
        "claude",
        &format!(
            r#"cat >/dev/null
echo x >> "{bin}/claude.count"
echo "$*" >> "{bin}/claude.args"
n=$(wc -l < "{bin}/claude.count")
case $n in
{arms}*) echo '{last}';;
esac"#,
            bin = bin.display(),
            last = replies.last().unwrap(),
        ),
    );
    Command::cargo_bin("ns")
        .unwrap()
        .args(["eval", "review"])
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(dir)
        .assert()
}

/// Sets `[review] sessions` in the repository.
fn sessions_set(repo: &Repo, n: u32) {
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        format!("[review]\nsessions = {n}\n"),
    )
    .unwrap();
}

fn calls(repo: &Repo) -> usize {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/claude.count"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn stderr(out: assert_cmd::assert::Assert) -> String {
    String::from_utf8(out.get_output().stderr.clone()).unwrap()
}

fn report(out: &assert_cmd::assert::Assert) -> serde_json::Value {
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

#[test]
fn a_skill_that_always_gives_the_expected_verdict_is_ok() {
    let repo = Repo::new();
    case(&repo, "bad", Some("FAIL\n"));
    let out = eval(&repo, &["Nope.\nVERDICT: FAIL"], &[]).success();
    assert_eq!(
        report(&out),
        serde_json::json!({
            "skill": "review",
            "runs": 3,
            "ok": true,
            "cases": [{
                "case": "bad",
                "expected": "FAIL",
                "verdicts": ["FAIL", "FAIL", "FAIL"],
                "matches": 3,
            }],
        })
    );
}

#[test]
fn one_different_verdict_is_not_ok_and_exits_1() {
    let repo = Repo::new();
    sessions_set(&repo, 1);
    case(&repo, "good", Some("PASS"));
    let out = eval(
        &repo,
        &["VERDICT: PASS", "VERDICT: FAIL", "VERDICT: PASS"],
        &[],
    )
    .code(1);
    let report = report(&out);
    assert_eq!(report["ok"], false);
    assert_eq!(
        report["cases"][0]["verdicts"],
        serde_json::json!(["PASS", "FAIL", "PASS"])
    );
    assert_eq!(report["cases"][0]["matches"], 2);
}

#[test]
fn a_reply_with_no_verdict_is_a_miss() {
    let repo = Repo::new();
    sessions_set(&repo, 1);
    case(&repo, "good", Some("PASS"));
    let out = eval(
        &repo,
        &["VERDICT: PASS", "I think it is fine."],
        &["--runs", "2"],
    )
    .code(1);
    let report = report(&out);
    assert_eq!(
        report["cases"][0]["verdicts"],
        serde_json::json!(["PASS", null])
    );
    assert_eq!(report["cases"][0]["matches"], 1);
}

#[test]
fn a_harness_that_fails_is_a_miss() {
    let repo = Repo::new();
    case(&repo, "good", Some("PASS"));
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "claude",
        "cat >/dev/null\necho 'VERDICT: PASS'\nexit 1",
    );
    let out = Command::cargo_bin("ns")
        .unwrap()
        .args(["eval", "review", "--runs", "1"])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
        .code(1);
    assert_eq!(
        report(&out)["cases"][0]["verdicts"],
        serde_json::json!([null])
    );
}

#[test]
fn runs_sets_how_many_times_each_case_runs() {
    let repo = Repo::new();
    sessions_set(&repo, 1);
    case(&repo, "a", Some("PASS"));
    case(&repo, "b", Some("PASS"));
    let out = eval(&repo, &["VERDICT: PASS"], &["--runs", "2"]).success();
    assert_eq!(report(&out)["runs"], 2);
    assert_eq!(calls(&repo), 4);
}

#[test]
fn case_runs_only_that_case() {
    let repo = Repo::new();
    sessions_set(&repo, 1);
    case(&repo, "a", Some("PASS"));
    case(&repo, "b", Some("PASS"));
    let out = eval(&repo, &["VERDICT: PASS"], &["--case", "b"]).success();
    let report = report(&out);
    assert_eq!(report["cases"].as_array().unwrap().len(), 1);
    assert_eq!(report["cases"][0]["case"], "b");
    assert_eq!(calls(&repo), 3);
}

#[test]
fn an_unknown_case_is_an_error() {
    let repo = Repo::new();
    case(&repo, "a", Some("PASS"));
    let err = stderr(eval(&repo, &["VERDICT: PASS"], &["--case", "nope"]).failure());
    assert!(err.contains("nope"), "{err}");
    assert_eq!(calls(&repo), 0);
}

#[test]
fn a_case_with_no_expect_is_an_error_naming_it() {
    let repo = Repo::new();
    case(&repo, "no-expect", None);
    let err = stderr(eval(&repo, &["VERDICT: PASS"], &[]).failure());
    assert!(err.contains("no-expect"), "{err}");
    assert_eq!(calls(&repo), 0);
}

#[test]
fn an_expect_that_is_not_pass_or_fail_is_an_error_naming_the_case() {
    let repo = Repo::new();
    case(&repo, "odd", Some("MAYBE"));
    let err = stderr(eval(&repo, &["VERDICT: PASS"], &[]).failure());
    assert!(err.contains("odd") && err.contains("MAYBE"), "{err}");
    assert_eq!(calls(&repo), 0);
}

#[test]
fn the_review_prompt_carries_the_case() {
    let repo = Repo::new();
    case(&repo, "good", Some("PASS"));
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "claude",
        &format!("cat > \"{}/prompt\"\necho 'VERDICT: PASS'", bin.display()),
    );
    Command::cargo_bin("ns")
        .unwrap()
        .args(["eval", "review", "--runs", "1"])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
        .success();
    let prompt = std::fs::read_to_string(bin.join("prompt")).unwrap();
    assert!(
        prompt.contains("## feat: a title\n\nA description.\n\n# The build summary\n\nbuilt it"),
        "{prompt}"
    );
    assert!(
        prompt.contains("# The change\n\ndiff --git a/x b/x"),
        "{prompt}"
    );
}

#[test]
fn the_eval_review_prompt_carries_the_shipped_standards() {
    let repo = Repo::new();
    case(&repo, "good", Some("PASS"));
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "claude",
        &format!("cat > \"{}/prompt\"\necho 'VERDICT: PASS'", bin.display()),
    );
    Command::cargo_bin("ns")
        .unwrap()
        .args(["eval", "review", "--runs", "1"])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
        .success();
    let prompt = std::fs::read_to_string(bin.join("prompt")).unwrap();
    assert!(
        prompt.contains(
            "# The coding standards\n\n## .nightshift/coding-standards.md\n\n# Coding standards"
        ),
        "{prompt}"
    );
}

#[test]
fn a_harness_that_errors_is_a_miss_and_is_reported() {
    let repo = Repo::new();
    case(&repo, "slow", Some("PASS"));
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(&bin, "claude", "cat >/dev/null\nexec sleep 30");
    let out = Command::cargo_bin("ns")
        .unwrap()
        .args([
            "eval",
            "review",
            "--runs",
            "1",
            "--harness-timeout-seconds",
            "1",
        ])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
        .code(1);
    let err = stderr_of(&out);
    assert!(err.contains("slow"), "{err}");
    let report = report(&out);
    assert_eq!(report["cases"][0]["verdicts"], serde_json::json!([null]));
    assert_eq!(report["cases"][0]["matches"], 0);
}

#[test]
fn an_error_verdict_is_reported_and_is_a_miss() {
    let repo = Repo::new();
    sessions_set(&repo, 1);
    case(&repo, "good", Some("PASS"));
    let out = eval(&repo, &["VERDICT: ERROR"], &["--runs", "1"]).code(1);
    let report = report(&out);
    assert_eq!(report["cases"][0]["verdicts"], serde_json::json!(["ERROR"]));
    assert_eq!(report["cases"][0]["matches"], 0);
}

#[test]
fn zero_runs_is_refused_before_any_call() {
    let repo = Repo::new();
    case(&repo, "good", Some("PASS"));
    eval(&repo, &["VERDICT: PASS"], &["--runs", "0"]).failure();
    assert_eq!(calls(&repo), 0);
}

fn stderr_of(out: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(out.get_output().stderr.clone()).unwrap()
}

#[test]
fn run_from_a_workspace_it_reads_that_workspaces_cases() {
    let repo = Repo::new();
    let workspace = repo.path.parent().unwrap().join("other");
    git(
        &repo.path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            &workspace.to_string_lossy(),
        ],
    );
    let dir = workspace.join("evals/review/only-here");
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in [
        ("unit.md", "feat: a title\n"),
        ("build.md", "built it\n"),
        ("change.diff", "diff --git a/x b/x\n"),
        ("expect", "PASS\n"),
    ] {
        std::fs::write(dir.join(file), text).unwrap();
    }
    let out = eval_in(&repo, &workspace, &["VERDICT: PASS"], &["--runs", "1"]).success();
    assert_eq!(report(&out)["cases"][0]["case"], "only-here");
}

#[test]
fn a_run_is_a_whole_review_of_three_sessions_by_default() {
    let repo = Repo::new();
    case(&repo, "good", Some("PASS"));
    let out = eval(&repo, &["VERDICT: PASS"], &["--runs", "2"]).success();
    assert_eq!(
        report(&out)["cases"][0]["verdicts"],
        serde_json::json!(["PASS", "PASS"])
    );
    assert_eq!(calls(&repo), 6);
}

#[test]
fn one_failing_session_fails_the_run_and_six_sessions_make_six_calls() {
    let repo = Repo::new();
    sessions_set(&repo, 6);
    case(&repo, "bad", Some("FAIL"));
    let replies = [
        "VERDICT: PASS",
        "VERDICT: PASS",
        "VERDICT: PASS",
        "VERDICT: PASS",
        "VERDICT: PASS",
        "VERDICT: FAIL",
    ];
    let out = eval(&repo, &replies, &["--runs", "1"]).success();
    assert_eq!(
        report(&out)["cases"][0]["verdicts"],
        serde_json::json!(["FAIL"])
    );
    assert_eq!(calls(&repo), 6);
}

#[test]
fn a_session_without_a_verdict_leaves_the_run_without_one() {
    let repo = Repo::new();
    case(&repo, "good", Some("PASS"));
    let out = eval(
        &repo,
        &["VERDICT: PASS", "VERDICT: FAIL", "I think it is fine."],
        &["--runs", "1"],
    )
    .code(1);
    assert_eq!(
        report(&out)["cases"][0]["verdicts"],
        serde_json::json!([null])
    );
}

#[test]
fn a_session_that_errors_makes_the_run_an_error_even_after_a_fail() {
    let repo = Repo::new();
    case(&repo, "good", Some("PASS"));
    let out = eval(
        &repo,
        &["VERDICT: FAIL", "VERDICT: ERROR", "VERDICT: PASS"],
        &["--runs", "1"],
    )
    .code(1);
    assert_eq!(
        report(&out)["cases"][0]["verdicts"],
        serde_json::json!(["ERROR"])
    );
}

#[test]
fn a_sessions_setting_that_is_not_1_3_or_6_stops_the_eval_before_any_call() {
    let repo = Repo::new();
    sessions_set(&repo, 2);
    case(&repo, "good", Some("PASS"));
    let err = stderr(eval(&repo, &["VERDICT: PASS"], &[]).failure());
    assert!(err.contains("review.sessions"), "{err}");
    assert_eq!(calls(&repo), 0);
}

fn models(repo: &Repo) -> Vec<String> {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/claude.args"))
        .unwrap()
        .lines()
        .map(|l| match l.split_once("--model ") {
            Some((_, rest)) => rest.split(' ').next().unwrap().to_string(),
            None => "-".to_string(),
        })
        .collect()
}

fn eval_models(manifest: &str, extra: &[&str]) -> Vec<String> {
    let repo = Repo::new();
    case(&repo, "good", Some("PASS"));
    crate::run::set_manifest(&repo, manifest);
    eval(
        &repo,
        &["VERDICT: PASS"],
        &[&["--runs", "1"], extra].concat(),
    )
    .success();
    models(&repo)
}

#[test]
fn every_review_session_of_a_case_uses_the_review_model() {
    assert_eq!(eval_models("", &[]), ["-"; 3]);
    let table = "[model]\ndefault = \"sonnet\"\nreview = \"opus\"\n";
    assert_eq!(eval_models(table, &[]), ["opus"; 3]);
    assert_eq!(
        eval_models("[model]\ndefault = \"sonnet\"\n", &[]),
        ["sonnet"; 3]
    );
}

#[test]
fn the_model_option_beats_the_review_model() {
    let table = "[model]\nreview = \"opus\"\n";
    assert_eq!(eval_models(table, &["--model", "haiku"]), ["haiku"; 3]);
}

#[test]
fn a_bad_model_table_stops_the_eval_before_any_call() {
    let repo = Repo::new();
    case(&repo, "good", Some("PASS"));
    crate::run::set_manifest(&repo, "[model]\nreview = \"\"\n");
    let out = eval(&repo, &["VERDICT: PASS"], &[]).failure();
    let err = stderr(out);
    assert!(
        err.contains("model.review") && err.contains("nightshift.toml"),
        "{err}"
    );
    assert_eq!(calls(&repo), 0);
}
