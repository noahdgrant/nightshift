//! `scripts/check-verify-recipes.sh`, run on fixture copies of the verify skill whose Launch and
//! Doctor recipes stand in for the real ones without building `ns` or starting a sandbox.

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

const LAUNCH: &str = r#"export V=$(mktemp -d "$TMPDIR/ns-verify.XXXXXX")
mkdir -p "$V/bin"
export OUT=${OUT:-/nonexistent/evidence}
mkdir -p "$OUT"
touch "$OUT/launch.out"
echo "$V""#;

const DOCTOR_OK: &str = r#"for b in claude; do echo "real $b"; done
printf '{"ok":true,"problems":[],"forge":{"account":"octocat"},"claude":"%s/bin/claude"}\n' "$V""#;

fn skill(launch: &str, doctor: &str) -> String {
    format!(
        "---\nname: verify-x\n---\n\n# Verify\n\n## Launch\n\nSet up:\n\n```bash\n{launch}\n```\n\nReady when it prints.\n\n## Doctor\n\n```bash\n{doctor}\n```\n\n## Drive\n\n```bash\nfalse\n```\n"
    )
}

fn script(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../scripts/{name}"))
}

/// Runs the check on `skill_md`, with every temporary path under a fresh `TMPDIR` that the test
/// then expects empty: the sandbox, the evidence directory and the script's own files.
fn run_in(skill_md: &str, wrapper: Option<&str>, stdin: &str) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tempfile::tempdir().unwrap();
    let path = skill_dir.path().join("SKILL.md");
    fs::write(&path, skill_md).unwrap();
    let mut c = common::command("bash");
    if let Some(w) = wrapper {
        c.arg(script(w)).arg("bash");
    }
    c.arg(script("check-verify-recipes.sh"))
        .arg(&path)
        .env("TMPDIR", tmp.path())
        .env_remove("OUT")
        .env_remove("V");
    for v in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        c.env_remove(v);
    }
    let out = c.write_stdin(stdin).output();
    let left: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "left behind in TMPDIR: {left:?}\n{out:?}");
    out
}

fn run(skill_md: &str) -> Output {
    run_in(skill_md, None, "")
}

fn run_with_stdin(skill_md: &str, stdin: &str) -> Output {
    run_in(skill_md, None, stdin)
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_fails_with(out: &Output, needle: &str) {
    let err = stderr(out);
    assert!(!out.status.success(), "passed: {err}");
    assert!(err.contains(needle), "no {needle:?} in: {err}");
}

#[test]
fn a_working_recipe_passes_and_leaves_nothing_behind() {
    let out = run(&skill(LAUNCH, DOCTOR_OK));
    assert!(out.status.success(), "{}", stderr(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"ok\":true"), "{stdout}");
}

#[test]
fn the_recipes_run_in_one_shell_in_order() {
    let launch = format!("{LAUNCH}\nexport FROM_LAUNCH=1");
    let doctor = format!("[ \"$FROM_LAUNCH\" = 1 ]\n{DOCTOR_OK}");
    let out = run(&skill(&launch, &doctor));
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_stale_launch_command_fails_naming_the_block() {
    for launch in [
        format!("{LAUNCH}\n/nonexistent/ns-build --release"),
        format!("/nonexistent/ns-build --release\n{LAUNCH}"),
    ] {
        assert_fails_with(&run(&skill(&launch, DOCTOR_OK)), "Launch block 1");
    }
}

#[test]
fn a_terminated_run_still_removes_the_sandbox() {
    let launch = format!("{LAUNCH}\nkill -TERM $$");
    assert_fails_with(&run(&skill(&launch, DOCTOR_OK)), "Launch block 1");
}

#[test]
fn a_stale_doctor_command_fails_naming_the_block_and_shows_its_output() {
    for stale in ["/nonexistent/ns doctor", "echo \"$NO_SUCH_VAR\""] {
        let doctor = format!("echo before-the-failure\n{stale}\n{DOCTOR_OK}");
        let out = run(&skill(LAUNCH, &doctor));
        assert_fails_with(&out, "Doctor block 1");
        assert_fails_with(&out, "before-the-failure");
    }
}

#[test]
fn a_failure_inside_a_pipeline_fails_the_block() {
    let doctor = format!("/nonexistent/ns doctor | cat\n{DOCTOR_OK}");
    assert_fails_with(&run(&skill(LAUNCH, &doctor)), "Doctor block 1");
}

#[test]
fn the_second_block_under_a_heading_is_named() {
    let md = skill(LAUNCH, DOCTOR_OK).replace(
        "Ready when it prints.",
        "Then:\n\n```bash\ntrue\n```\n\n```bash\n/nonexistent/x\n```",
    );
    assert_fails_with(&run(&md), "Launch block 3");
}

#[test]
fn each_bad_doctor_result_fails_naming_it() {
    for (doctor, needle) in [
        (
            DOCTOR_OK.replace("\"ok\":true", "\"ok\":false"),
            "ok is false",
        ),
        (
            DOCTOR_OK.replace("\"problems\":[]", "\"problems\":[\"no git\"]"),
            "problems is [\"no git\"]",
        ),
        (
            DOCTOR_OK.replace("echo \"real $b\"", "echo \"NOT FAKE: $b\""),
            "NOT FAKE: claude",
        ),
        (
            DOCTOR_OK.replace("octocat", "someone"),
            "forge.account is \"someone\"",
        ),
        (
            DOCTOR_OK.replace("%s/bin/claude", "/usr/bin/claude"),
            "claude is \"/usr/bin/claude\"",
        ),
        (
            DOCTOR_OK.replace(
                "\"account\":\"octocat\"",
                "\"account\":\"octocat\",\"ssh\":{\"dest\":\"git@github.com\"}",
            ),
            "probed a real ssh host",
        ),
        ("echo no json here".to_string(), "no JSON line"),
    ] {
        let out = run(&skill(LAUNCH, &doctor));
        assert_fails_with(&out, needle);
        let err = stderr(&out);
        assert!(!err.contains("real claude"), "printed twice: {err}");
    }
}

#[test]
fn a_missing_heading_fails_naming_it() {
    let md = skill(LAUNCH, DOCTOR_OK).replace("## Doctor", "## Checkup");
    assert_fails_with(&run(&md), "no bash block under ## Doctor");
}

#[test]
fn a_heading_without_a_bash_block_fails_naming_it() {
    let md = skill(LAUNCH, DOCTOR_OK).replace("```bash\nfor", "```sh\nfor");
    assert_fails_with(&run(&md), "no bash block under ## Doctor");
}

#[test]
fn a_missing_skill_file_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("SKILL.md");
    let out = common::command("bash")
        .arg(script("check-verify-recipes.sh"))
        .arg(&missing)
        .output();
    assert_fails_with(&out, &format!("no skill file: {}", missing.display()));
}

#[test]
fn the_recipes_read_no_stdin() {
    let launch = format!("{LAUNCH}\nif read -r line; then echo \"read $line\"; exit 1; fi");
    let out = run_with_stdin(&skill(&launch, DOCTOR_OK), "a line\n");
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn the_last_json_line_is_the_one_checked() {
    let doctor = format!("echo '{{\"ok\":false}}'\n{DOCTOR_OK}");
    let out = run(&skill(LAUNCH, &doctor));
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn an_inherited_sandbox_variable_is_not_removed_when_launch_fails_early() {
    let canary = tempfile::tempdir().unwrap();
    let keep = canary.path().join("keep");
    fs::create_dir(&keep).unwrap();
    let skill_dir = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let path = skill_dir.path().join("SKILL.md");
    let launch = format!("/nonexistent/ns-build --release\n{LAUNCH}");
    fs::write(&path, skill(&launch, DOCTOR_OK)).unwrap();
    let out = common::command("bash")
        .arg(script("check-verify-recipes.sh"))
        .arg(&path)
        .env("TMPDIR", tmp.path())
        .env_remove("OUT")
        .env("V", &keep)
        .output();
    assert_fails_with(&out, "Launch block 1");
    assert!(
        keep.is_dir(),
        "the caller's V was removed: {}",
        stderr(&out)
    );
}

#[test]
fn the_recipes_never_see_an_inherited_git_dir() {
    let launch = format!(
        "{LAUNCH}\n[ -z \"${{GIT_DIR-}}\" ] && [ -z \"${{GIT_WORK_TREE-}}\" ]\ngit init -q \"$V/repo\"\ngit -C \"$V/repo\" config ns.touched 1"
    );
    let out = run_in(
        &skill(&launch, DOCTOR_OK),
        Some("check-git-dir-decoy.sh"),
        "",
    );
    assert!(out.status.success(), "{}", stderr(&out));
}

fn real_skill() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.agents/skills/verify-nightshift/SKILL.md"),
    )
    .unwrap()
}

#[test]
fn a_stale_command_in_the_real_recipes_fails_wherever_it_sits() {
    let real = real_skill();
    for (stale, fresh) in [
        ("push -q origin main", "push -q origin nosuchbranch"),
        ("commit -qm init", "commit --nosuchflag"),
        ("\"$NS\" --version", "\"$NS\" --nosuchflag"),
    ] {
        assert!(
            real.contains(stale),
            "recipe changed, update the test: {stale}"
        );
        let out = run(&real.replace(stale, fresh));
        assert!(!out.status.success(), "{fresh} passed: {}", stderr(&out));
    }
}
