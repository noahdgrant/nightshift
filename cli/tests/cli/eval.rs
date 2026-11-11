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

/// Commits a change that adds `f.txt` with a body in its message; returns the commit's id.
fn defect_commit(repo: &Repo) -> String {
    std::fs::write(repo.path.join("f.txt"), "one\n").unwrap();
    git(&repo.path, &["add", "f.txt"]);
    git(
        &repo.path,
        &[
            "commit",
            "-q",
            "-m",
            "feat: add f",
            "-m",
            "Added f.\n\nTests: none.",
        ],
    );
    git(&repo.path, &["rev-parse", "HEAD"]).trim().to_string()
}

/// Runs `ns eval add review` with `extra` arguments. The fake `gh` knows unit 4 only.
fn add(repo: &Repo, extra: &[&str]) -> assert_cmd::assert::Assert {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "gh",
        r#"case "$1 $2" in
"api repos/{owner}/{repo}/issues/4") printf '%s\n' '{"number":4,"title":"feat: a title","body":"A description.\n\nMore.","labels":[],"author_association":"OWNER","html_url":"https://github.com/o/r/issues/4"}';;
*) echo 'not found' >&2; exit 1;;
esac"#,
    );
    Command::cargo_bin("ns")
        .unwrap()
        .args(["eval", "add", "review"])
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

fn case_files(repo: &Repo, name: &str) -> Vec<String> {
    let dir = repo.path.join("evals/review").join(name);
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .map(|d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn case_read(repo: &Repo, name: &str, file: &str) -> String {
    std::fs::read_to_string(repo.path.join("evals/review").join(name).join(file)).unwrap()
}

#[test]
fn add_writes_the_four_files_and_expects_fail_by_default() {
    let repo = Repo::new();
    let commit = defect_commit(&repo);
    let out = add(&repo, &[&commit, "--unit", "4", "--as", "lost-f"]).success();
    let line = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert_eq!(line.lines().count(), 1);
    let dir = repo.path.join("evals/review/lost-f");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
        serde_json::json!({ "case": "lost-f", "dir": dir })
    );
    assert_eq!(
        case_files(&repo, "lost-f"),
        ["build.md", "change.diff", "expect", "unit.md"]
    );
    assert_eq!(case_read(&repo, "lost-f", "expect"), "FAIL\n");
    assert_eq!(
        case_read(&repo, "lost-f", "unit.md"),
        "feat: a title\n\nA description.\n\nMore.\n"
    );
    assert_eq!(
        case_read(&repo, "lost-f", "build.md"),
        "Added f.\n\nTests: none.\n"
    );
    let diff = case_read(&repo, "lost-f", "change.diff");
    assert!(diff.starts_with("diff --git a/f.txt b/f.txt\n"), "{diff}");
    assert!(diff.ends_with("+one\n"), "{diff}");
}

#[test]
fn add_with_expect_pass_writes_pass() {
    let repo = Repo::new();
    let commit = defect_commit(&repo);
    add(
        &repo,
        &[&commit, "--unit", "4", "--as", "fine", "--expect", "PASS"],
    )
    .success();
    assert_eq!(case_read(&repo, "fine", "expect"), "PASS\n");
}

#[test]
fn add_that_fails_writes_nothing() {
    let repo = Repo::new();
    let commit = defect_commit(&repo);
    add(&repo, &[&commit, "--unit", "4", "--as", "taken"]).success();
    let before = case_read(&repo, "taken", "unit.md");

    let again = add(&repo, &[&commit, "--unit", "4", "--as", "taken"]).failure();
    assert!(stderr(again).contains("already exists"));
    assert_eq!(case_read(&repo, "taken", "unit.md"), before);

    let bad_name = add(&repo, &[&commit, "--unit", "4", "--as", "Bad Name"]).failure();
    assert!(stderr(bad_name).contains("lowercase letters, digits and hyphens"));
    assert!(case_files(&repo, "Bad Name").is_empty());

    let no_commit = add(&repo, &["nosuchcommit", "--unit", "4", "--as", "ghost"]).failure();
    assert!(stderr(no_commit).contains("there is no commit nosuchcommit"));
    assert!(case_files(&repo, "ghost").is_empty());

    let no_unit = add(&repo, &[&commit, "--unit", "9", "--as", "orphan"]).failure();
    assert!(stderr(no_unit).contains("cannot read unit 9"));
    assert!(case_files(&repo, "orphan").is_empty());
}

#[test]
fn add_refuses_an_expect_that_is_not_pass_or_fail() {
    let repo = Repo::new();
    let commit = defect_commit(&repo);
    for bad in ["INCONCLUSIVE", "pass"] {
        let out = add(
            &repo,
            &[&commit, "--unit", "4", "--as", "odd", "--expect", bad],
        )
        .failure();
        assert!(stderr(out).contains("expect must be PASS or FAIL"));
        assert!(case_files(&repo, "odd").is_empty());
    }
}

/// Writes the verify case `name` into the repository's evals, with a fixture `repo/` holding
/// one file, `x.txt`.
fn vcase(repo: &Repo, name: &str, expect: &str) {
    let dir = repo.path.join("evals/verify").join(name);
    std::fs::create_dir_all(dir.join("repo")).unwrap();
    std::fs::write(dir.join("repo/x.txt"), "fixture\n").unwrap();
    std::fs::write(dir.join("unit.md"), "feat: a title\n\nA description.\n").unwrap();
    std::fs::write(dir.join("build.md"), "built it\n").unwrap();
    std::fs::write(dir.join("expect"), expect).unwrap();
}

/// Runs `ns eval verify` with `extra` arguments and a fake `claude` that runs `body` (it reads
/// stdin to the file `prompt` first, and its working directory is the case's directory).
fn verify_with(repo: &Repo, body: &str, extra: &[&str]) -> assert_cmd::assert::Assert {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "claude",
        &format!("cat > \"{}/prompt\"\n{body}", bin.display()),
    );
    Command::cargo_bin("ns")
        .unwrap()
        .args(["eval", "verify"])
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

#[test]
fn verify_a_right_verdict_is_ok_and_the_run_sees_a_one_commit_repository() {
    let repo = Repo::new();
    vcase(&repo, "stuck", "INCONCLUSIVE\n");
    let bin = repo.path.parent().unwrap().join("bin");
    let out = verify_with(
        &repo,
        &format!(
            "pwd > \"{b}/cwd\"\ncat x.txt > \"{b}/seen\"\ngit rev-list --count HEAD > \"{b}/commits\"\ngit status --porcelain > \"{b}/dirty\"\necho 'Needs a token.'\necho 'VERDICT: INCONCLUSIVE'",
            b = bin.display()
        ),
        &["--runs", "2"],
    )
    .success();
    assert_eq!(
        report(&out),
        serde_json::json!({
            "skill": "verify",
            "runs": 2,
            "ok": true,
            "cases": [{
                "case": "stuck",
                "expected": "INCONCLUSIVE",
                "verdicts": ["INCONCLUSIVE", "INCONCLUSIVE"],
                "matches": 2,
            }],
        })
    );
    let read = |f: &str| std::fs::read_to_string(bin.join(f)).unwrap();
    assert_eq!(read("seen"), "fixture\n");
    assert_eq!(read("commits"), "1\n");
    assert_eq!(read("dirty"), "");
    let prompt = read("prompt");
    assert!(
        prompt.contains("## feat: a title\n\nA description.\n\n# The build summary\n\nbuilt it"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("# Findings from the previous verification"),
        "{prompt}"
    );
    // The fixture, not the repository `ns` was run from, was the working directory.
    let cwd = read("cwd");
    assert!(
        !cwd.trim().starts_with(repo.path.to_str().unwrap()),
        "{cwd}"
    );
    assert!(
        !repo.path.join("x.txt").exists(),
        "a case changed the repository it was run from"
    );
}

#[test]
fn verify_a_wrong_verdict_is_not_ok_and_exits_1() {
    let repo = Repo::new();
    vcase(&repo, "good", "PASS\n");
    let out = verify_with(&repo, "echo 'VERDICT: FAIL'", &["--runs", "1"]).code(1);
    let report = report(&out);
    assert_eq!(report["ok"], false);
    assert_eq!(report["cases"][0]["verdicts"], serde_json::json!(["FAIL"]));
    assert_eq!(report["cases"][0]["matches"], 0);
}

#[test]
fn verify_a_timeout_is_a_miss_and_the_directory_is_deleted() {
    let repo = Repo::new();
    vcase(&repo, "slow", "PASS\n");
    let bin = repo.path.parent().unwrap().join("bin");
    let out = verify_with(
        &repo,
        &format!("pwd > \"{}/cwd\"\nexec sleep 30", bin.display()),
        &["--runs", "1", "--harness-timeout-seconds", "1"],
    )
    .code(1);
    assert_eq!(
        report(&out)["cases"][0]["verdicts"],
        serde_json::json!([null])
    );
    let cwd = std::fs::read_to_string(bin.join("cwd")).unwrap();
    assert!(!std::path::Path::new(cwd.trim()).exists(), "{cwd}");
}

#[test]
fn verify_the_directory_is_deleted_after_a_failed_run() {
    let repo = Repo::new();
    vcase(&repo, "bad", "PASS\n");
    let bin = repo.path.parent().unwrap().join("bin");
    let out = verify_with(
        &repo,
        &format!(
            "pwd > \"{}/cwd\"\necho 'VERDICT: PASS'\nexit 1",
            bin.display()
        ),
        &["--runs", "1"],
    )
    .code(1);
    assert_eq!(
        report(&out)["cases"][0]["verdicts"],
        serde_json::json!([null])
    );
    let cwd = std::fs::read_to_string(bin.join("cwd")).unwrap();
    assert!(!std::path::Path::new(cwd.trim()).exists(), "{cwd}");
}

#[test]
fn verify_a_case_that_cannot_be_read_is_an_error_naming_it() {
    for (missing, what) in [
        ("expect", "expect"),
        ("repo", "repo"),
        ("build.md", "build.md"),
    ] {
        let repo = Repo::new();
        vcase(&repo, "broken", "PASS\n");
        let path = repo.path.join("evals/verify/broken").join(missing);
        if path.is_dir() {
            std::fs::remove_dir_all(path).unwrap();
        } else {
            std::fs::remove_file(path).unwrap();
        }
        let out = verify_with(&repo, "echo 'VERDICT: PASS'", &[]).failure();
        let err = stderr_of(&out);
        assert!(err.contains("broken") && err.contains(what), "{err}");
        assert!(!repo.path.parent().unwrap().join("bin/cwd").exists());
    }
}

#[test]
fn verify_an_expect_that_is_not_a_verdict_is_an_error_naming_the_case() {
    let repo = Repo::new();
    vcase(&repo, "odd", "MAYBE\n");
    let out = verify_with(&repo, "echo 'VERDICT: PASS'", &[]).failure();
    let err = stderr_of(&out);
    assert!(
        err.contains("odd") && err.contains("expect must be PASS, FAIL or INCONCLUSIVE"),
        "{err}"
    );
    assert!(!repo.path.parent().unwrap().join("bin/cwd").exists());
}

#[test]
fn verify_a_reply_with_no_verdict_line_is_a_miss() {
    let repo = Repo::new();
    vcase(&repo, "mute", "PASS\n");
    let out = verify_with(&repo, "echo 'all fine'", &["--runs", "1"]).code(1);
    let report = report(&out);
    assert_eq!(report["ok"], false);
    assert_eq!(report["cases"][0]["verdicts"], serde_json::json!([null]));
}

#[test]
fn verify_case_runs_only_that_case_and_an_unknown_one_is_an_error() {
    let repo = Repo::new();
    vcase(&repo, "a", "PASS\n");
    vcase(&repo, "b", "PASS\n");
    let out = verify_with(
        &repo,
        "echo 'VERDICT: PASS'",
        &["--runs", "1", "--case", "b"],
    )
    .success();
    assert_eq!(report(&out)["cases"].as_array().unwrap().len(), 1);
    let out = verify_with(&repo, "echo 'VERDICT: PASS'", &["--case", "nope"]).failure();
    assert!(stderr_of(&out).contains("nope"));
}

#[test]
fn verify_model_beats_the_verify_key() {
    let repo = Repo::new();
    vcase(&repo, "a", "PASS\n");
    std::fs::create_dir_all(repo.path.join(".nightshift")).unwrap();
    std::fs::write(
        repo.path.join(".nightshift/nightshift.toml"),
        "[model]\nverify = \"from-toml\"\n",
    )
    .unwrap();
    let bin = repo.path.parent().unwrap().join("bin");
    let body = format!(
        "echo \"$*\" > \"{}/args\"\necho 'VERDICT: PASS'",
        bin.display()
    );
    verify_with(&repo, &body, &["--runs", "1"]).success();
    assert!(std::fs::read_to_string(bin.join("args"))
        .unwrap()
        .contains("from-toml"));
    verify_with(&repo, &body, &["--runs", "1", "--model", "from-flag"]).success();
    let args = std::fs::read_to_string(bin.join("args")).unwrap();
    assert!(
        args.contains("from-flag") && !args.contains("from-toml"),
        "{args}"
    );
}

/// Runs `ns eval suggest` with the run log `runs` kept. The fake `gh` knows units 4 and 5.
fn suggest(repo: &Repo, runs: &str, extra: &[&str]) -> String {
    suggest_both(repo, runs, extra).0
}

/// As `suggest`, with what it wrote to stderr.
fn suggest_both(repo: &Repo, runs: &str, extra: &[&str]) -> (String, String) {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "gh",
        r#"case "$1 $2" in
"api repos/{owner}/{repo}/issues/4") printf '%s\n' '{"number":4,"title":"feat: four","body":"b","labels":[],"author_association":"OWNER","html_url":"u"}';;
"api repos/{owner}/{repo}/issues/5") printf '%s\n' '{"number":5,"title":"feat: five","body":"b","labels":[],"author_association":"OWNER","html_url":"u"}';;
*) echo 'not found' >&2; exit 1;;
esac"#,
    );
    let dir = repo.path.join(".git/ns");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("runs.jsonl"), runs).unwrap();
    let out = Command::cargo_bin("ns")
        .unwrap()
        .args(["eval", "suggest"])
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
        .success();
    (
        String::from_utf8(out.get_output().stdout.clone()).unwrap(),
        String::from_utf8(out.get_output().stderr.clone()).unwrap(),
    )
}

fn runs_now() -> (u64, String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let log = format!(
        r#"{{"unit":"4","from":"verify","to":"stuck","at":{now}}}
{{"unit":"4","took_over":1,"merged_at":{now},"at":{now}}}
{{"unit":"5","from":"ship","to":"done","merged_at":{now},"at":{now}}}
"#
    );
    (now, log)
}

#[test]
fn suggest_lists_a_unit_with_its_title_and_reason_and_changes_nothing() {
    let repo = Repo::new();
    let (_, log) = runs_now();
    let out = suggest(&repo, &log, &[]);
    assert_eq!(out, "#4 feat: four: was stuck in verify\n");
    assert!(!repo.path.join("evals").exists());
    assert_eq!(calls(&repo), 0);
}

#[test]
fn suggest_marks_a_unit_that_is_already_a_case() {
    let repo = Repo::new();
    let dir = repo.path.join("evals/review/four");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("unit.md"), "feat: four\n\nbody\n").unwrap();
    let (_, log) = runs_now();
    let out = suggest(&repo, &log, &[]);
    assert_eq!(
        out,
        "#4 feat: four [already a case: four]: was stuck in verify\n"
    );
}

#[test]
fn suggest_json_is_one_line_for_each_unit() {
    let repo = Repo::new();
    let (_, log) = runs_now();
    let out = suggest(&repo, &log, &["--json"]);
    assert_eq!(
        out,
        "{\"case\":null,\"reasons\":[\"was stuck in verify\"],\"title\":\"feat: four\",\"unit\":\"4\"}\n"
    );
}

#[test]
fn suggest_with_no_unit_to_list_says_so_and_succeeds() {
    let repo = Repo::new();
    let out = suggest(&repo, "", &[]);
    assert_eq!(
        out,
        "There is no merged unit worth keeping as a review case.\n"
    );
}

#[test]
fn suggest_says_on_stderr_when_it_cannot_read_a_units_title() {
    let repo = Repo::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let log = format!(
        r#"{{"unit":"9","from":"verify","to":"stuck","at":{now}}}
{{"unit":"9","took_over":1,"merged_at":{now},"at":{now}}}
"#
    );
    let (out, err) = suggest_both(&repo, &log, &[]);
    assert_eq!(out, "#9 : was stuck in verify\n");
    assert!(err.contains("could not read the title of unit 9"), "{err}");
}

#[test]
fn the_model_flag_help_of_review_and_verify_names_both_model_keys() {
    for skill in ["review", "verify"] {
        let out = Command::cargo_bin("ns")
            .unwrap()
            .args(["eval", skill, "--help"])
            .assert()
            .success();
        let help = String::from_utf8_lossy(&out.get_output().stdout).replace('\n', " ");
        assert!(
            help.contains(
                "its `review` key for `ns eval review`, its `verify` key for `ns eval verify`"
            ),
            "{skill}: {help}"
        );
    }
}
