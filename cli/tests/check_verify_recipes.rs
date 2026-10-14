//! `scripts/check-verify-recipes.sh`, run on fixture copies of the verify skill whose Launch,
//! Doctor, Drive and run recipes stand in for the real ones without building `ns` or starting a
//! sandbox: the fixture Launch makes a `$V/sandbox` that only runs its arguments.

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

const LAUNCH: &str = r#"export V=$(mktemp -d "$TMPDIR/ns-verify.XXXXXX")
mkdir -p "$V/bin"
printf '#!/bin/sh\nexec "$@"\n' > "$V/sandbox"
chmod +x "$V/sandbox"
export OUT=${OUT:-/nonexistent/evidence}
mkdir -p "$OUT"
touch "$OUT/launch.out"
echo "$V""#;

const DOCTOR_OK: &str = r#""$V/sandbox" sh -c 'for b in claude; do echo "real $b"; done'
"$V/sandbox" printf '{"ok":true,"problems":[],"forge":{"account":"octocat"},"claude":"%s/bin/claude"}\n' "$V""#;

const DRIVE: &str = r#""$V/sandbox" true"#;

const RUN_OK: &str = r#""$V/sandbox" sh -c 'echo "{\"outcome\":\"done\"}" > "$OUT/run-pass.out"'"#;

fn skill(launch: &str, doctor: &str) -> String {
    format!(
        "---\nname: verify-x\n---\n\n# Verify\n\n## Launch\n\nSet up:\n\n```bash\n{launch}\n```\n\nReady when it prints.\n\n## Doctor\n\n```bash\n{doctor}\n```\n\n## Drive\n\n```bash\n{DRIVE}\n```\n"
    )
}

/// A feature file whose `Driving it with ns` section holds `recipe` as its one bash block.
fn feature(recipe: &str) -> String {
    format!(
        "# Run\n\nRuns.\n\n## Sub-features\n\n- `run-pass` ends `done`.\n\n## How to get to it (user POV)\n\n- `ns run --issue <n>`\n\n## Driving it with ns\n\nPreconditions:\n\n- Baseline.\n\n- **Pass.** Run it:\n\n  ```bash\n  {recipe}\n  ```\n\n## Gotchas\n\n- None.\n"
    )
}

fn script(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../scripts/{name}"))
}

/// Runs the check on `skill_md`, with every temporary path under a fresh `TMPDIR` that the test
/// then expects empty: the sandbox, the evidence directory and the script's own files. With a
/// `decoy` repo, the script inherits `GIT_DIR` and `GIT_WORK_TREE` aimed at it.
fn run_in(
    skill_md: &str,
    features: &[(&str, String)],
    decoy: Option<&Path>,
    stdin: &str,
) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tempfile::tempdir().unwrap();
    let path = skill_dir.path().join("SKILL.md");
    fs::write(&path, skill_md).unwrap();
    fs::create_dir(skill_dir.path().join("features")).unwrap();
    for (name, text) in features {
        fs::write(skill_dir.path().join("features").join(name), text).unwrap();
    }
    let mut c = common::command("bash");
    c.arg(script("check-verify-recipes.sh"))
        .arg(&path)
        .env("TMPDIR", tmp.path())
        .env_remove("OUT")
        .env_remove("V")
        .env_remove("NS_UNIT");
    for v in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        c.env_remove(v);
    }
    if let Some(d) = decoy {
        c.env("GIT_DIR", d.join(".git")).env("GIT_WORK_TREE", d);
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
    run_in(skill_md, &[("run.md", feature(RUN_OK))], None, "")
}

fn run_with_stdin(skill_md: &str, stdin: &str) -> Output {
    run_in(skill_md, &[("run.md", feature(RUN_OK))], None, stdin)
}

fn run_with_features(skill_md: &str, features: &[(&str, String)]) -> Output {
    run_in(skill_md, features, None, "")
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
    let doctor = format!("\"$V/sandbox\" test \"$FROM_LAUNCH\" = 1\n{DOCTOR_OK}");
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
    for stale in [
        "\"$V/sandbox\" /nonexistent/ns doctor",
        "\"$V/sandbox\" echo \"$NO_SUCH_VAR\"",
    ] {
        let doctor = format!("\"$V/sandbox\" echo before-the-failure\n{stale}\n{DOCTOR_OK}");
        let out = run(&skill(LAUNCH, &doctor));
        assert_fails_with(&out, "Doctor block 1");
        assert_fails_with(&out, "before-the-failure");
    }
}

#[test]
fn a_failure_inside_a_pipeline_fails_the_block() {
    let doctor = format!("\"$V/sandbox\" /nonexistent/ns doctor | cat\n{DOCTOR_OK}");
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
        (
            "\"$V/sandbox\" echo no json here".to_string(),
            "no JSON line",
        ),
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
    let md =
        skill(LAUNCH, DOCTOR_OK).replace(&format!("```bash\n{DOCTOR_OK}\n```"), "Nothing to run.");
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
    let doctor = format!("\"$V/sandbox\" echo '{{\"ok\":false}}'\n{DOCTOR_OK}");
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
    fs::create_dir(skill_dir.path().join("features")).unwrap();
    fs::write(skill_dir.path().join("features/run.md"), feature(RUN_OK)).unwrap();
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
    let decoy = tempfile::tempdir().unwrap();
    let init = common::command("git")
        .args(["init", "-q"])
        .arg(decoy.path())
        .output();
    assert!(init.status.success());
    let config = fs::read_to_string(decoy.path().join(".git/config")).unwrap();
    let out = run_in(
        &skill(&launch, DOCTOR_OK),
        &[("run.md", feature(RUN_OK))],
        Some(decoy.path()),
        "",
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(decoy.path().join(".git/config")).unwrap(),
        config
    );
}

#[test]
fn a_recipe_outside_the_sandbox_fails_naming_the_file_and_block_before_anything_runs() {
    let launch = format!("{LAUNCH}\necho LAUNCH-RAN");
    for (md, features, needle) in [
        (
            skill(&launch, DOCTOR_OK),
            vec![("run.md", feature("ns run --issue 7"))],
            "run.md ## Driving it with ns block 1",
        ),
        (
            skill(&launch, DOCTOR_OK),
            vec![
                ("run.md", feature(RUN_OK)),
                ("ask.md", feature("cd \"$V/repo\"; \"$V/sandbox\" true")),
            ],
            "ask.md ## Driving it with ns block 1",
        ),
        (
            skill(&launch, DOCTOR_OK).replace(DRIVE, "true"),
            vec![("run.md", feature(RUN_OK))],
            "SKILL.md ## Drive block 1",
        ),
        (
            skill(&launch, DOCTOR_OK).replace("\"$V/sandbox\" printf", "printf"),
            vec![("run.md", feature(RUN_OK))],
            "SKILL.md ## Doctor block 1",
        ),
    ] {
        let out = run_with_features(&md, &features);
        assert_fails_with(&out, needle);
        assert_fails_with(&out, "does not run in the sandbox");
        assert!(!stderr(&out).contains("LAUNCH-RAN"), "{}", stderr(&out));
        assert!(!String::from_utf8_lossy(&out.stdout).contains("LAUNCH-RAN"));
    }
}

#[test]
fn a_doctor_recipe_with_src_fails_naming_the_file_and_block() {
    for recipe in [
        r#""$V/sandbox" --src cap x ns doctor"#,
        r#""$V/sandbox" cap x ns doctor --src"#,
        r#""$V/sandbox" sh -c 'ns doctor --src'"#,
    ] {
        let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(recipe))]);
        assert_fails_with(
            &out,
            "run.md ## Driving it with ns block 1 runs ns doctor with --src",
        );
    }
}

#[test]
fn a_glob_in_a_recipe_fails_naming_the_block() {
    for recipe in [
        r#""$V/sandbox" ls "$V"/repo.worktrees/*/.ns/7-x"#,
        r#""$V/sandbox" ls "$V/ghd/labels-?""#,
        r#""$V/sandbox" ls $V/ghd/labels-?"#,
    ] {
        let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(recipe))]);
        assert_fails_with(&out, "run.md ## Driving it with ns block 1 uses a glob");
    }
}

#[test]
fn a_glob_character_inside_single_quotes_is_not_a_glob() {
    let recipe = r#""$V/sandbox" sh -c 'echo ".x[]?" "a*" >/dev/null' _ '*?'"#;
    let out = run_with_features(
        &skill(LAUNCH, DOCTOR_OK),
        &[("run.md", feature(&format!("{RUN_OK}\n  {recipe}")))],
    );
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn ansi_c_and_locale_quoting_fails_naming_the_block() {
    for recipe in [
        r#""$V/sandbox" echo a$'\'' ; id"#,
        r#""$V/sandbox" echo $'a'"#,
        r#""$V/sandbox" echo $"a""#,
    ] {
        let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(recipe))]);
        assert_fails_with(
            &out,
            "run.md ## Driving it with ns block 1 uses $' or $\" quoting",
        );
    }
}

#[test]
fn a_quote_or_backslash_open_at_the_end_of_a_line_fails_naming_the_block() {
    for recipe in [
        "\"$V/sandbox\" echo '\n  \"$V/sandbox\" '; id; echo '\n  \"$V/sandbox\" '",
        "\"$V/sandbox\" echo \"\n  \"$V/sandbox\" \"; id; echo \"\n  \"$V/sandbox\" \"",
        "\"$V/sandbox\" echo a\\\n  \"$V/sandbox\" true",
    ] {
        let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(recipe))]);
        assert_fails_with(
            &out,
            "run.md ## Driving it with ns block 1 ends a line inside a quote or after a backslash",
        );
    }
}

#[test]
fn a_recipe_naming_a_ns_path_other_than_one_units_folder_fails() {
    for recipe in [
        r#""$V/sandbox" ls "$V/repo.worktrees/7-x/.ns/""#,
        r#""$V/sandbox" ls "$V/repo/.ns/$UNIT/""#,
        r#""$V/sandbox" ls "$V/repo/.ns/verify""#,
        r#""$V/sandbox" ls "$V/repo/.ns/foo""#,
        r#""$V/sandbox" ls "$V/repo/.ns/x/../..""#,
        r#""$V/sandbox" sh -c 'ls .ns/$NS_UNIT/../../x'"#,
        r#""$V/sandbox" sh -c 'ls .ns/$NS_UNIT/./x'"#,
        r#""$V/sandbox" sh -c 'ls .ns/7-x/..'"#,
    ] {
        let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(recipe))]);
        assert_fails_with(
            &out,
            "run.md ## Driving it with ns block 1 names a .ns/ path",
        );
    }
    for recipe in [
        r#""$V/sandbox" ls "$V/repo/.ns/7-x/../..""#,
        r#""$V/sandbox" ls "$V/repo/.ns/7-x"/{a,b}"#,
        r#""$V/sandbox" ls "$V/repo/.ns/7-x"/[a]"#,
    ] {
        let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(recipe))]);
        assert_fails_with(
            &out,
            "run.md ## Driving it with ns block 1 uses a bracket, brace or ..",
        );
    }
    for unit in ["7-x", "$NS_UNIT", "<unit-id>"] {
        let exact = format!(
            "{RUN_OK}\n  \"$V/sandbox\" true \"$V/repo.worktrees/7-x/.ns/{unit}/evidence.md\""
        );
        let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(&exact))]);
        assert!(out.status.success(), "{unit}: {}", stderr(&out));
    }
}

#[test]
fn the_recipes_see_the_run_md_unit_as_ns_unit() {
    let recipe = format!("{RUN_OK}\n  \"$V/sandbox\" test \"$NS_UNIT\" = 7-fix-the-thing");
    let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(&recipe))]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_chained_command_in_a_recipe_fails_naming_the_file_and_block() {
    for recipe in [
        r#""$V/sandbox" true; gh api user"#,
        r#""$V/sandbox" true && cd "$NS_SRC""#,
        r#""$V/sandbox" false || gh api user"#,
        r#""$V/sandbox" true | sh"#,
        r#""$V/sandbox" true & gh api user"#,
        r#""$V/sandbox" true > /tmp/x"#,
        r#""$V/sandbox" cat < /etc/passwd"#,
        r#""$V/sandbox" echo $(ssh h)"#,
        r#""$V/sandbox" echo "$(ssh h)""#,
        r#""$V/sandbox" echo `ssh h`"#,
        r#""$V/sandbox" echo "it's"; gh api user; echo ''"#,
    ] {
        let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(recipe))]);
        assert_fails_with(
            &out,
            "run.md ## Driving it with ns block 1 runs something outside the sandbox",
        );
    }
}

#[test]
fn operators_inside_single_quotes_are_not_chains() {
    let recipe =
        r#""$V/sandbox" sh -c 'true; true && true | cat > /dev/null; echo `true` $(true)' _ 'a;b'"#;
    let out = run_with_features(
        &skill(LAUNCH, DOCTOR_OK),
        &[("run.md", feature(&format!("{RUN_OK}\n  {recipe}")))],
    );
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_fence_that_is_not_bash_under_a_recipe_heading_fails_naming_the_block() {
    for fence in [
        "```sh",
        "```shell",
        "```",
        "~~~bash",
        "~~~",
        "```bash title",
    ] {
        let md = skill(LAUNCH, DOCTOR_OK)
            .replace("## Drive\n\n```bash", &format!("## Drive\n\n{fence}"));
        let out = run(&md);
        assert_fails_with(&out, "SKILL.md ## Drive block 1 is not a bash block");
    }
    let run_md = feature(RUN_OK).replace("  ```bash", "  ```sh");
    let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", run_md)]);
    assert_fails_with(
        &out,
        "run.md ## Driving it with ns block 1 is not a bash block",
    );
}

#[test]
fn commands_outside_the_recipe_blocks_are_not_recipes() {
    let md = skill(LAUNCH, DOCTOR_OK) + "\n## Cleanup\n\n```bash\nrm -rf \"$V\"\n```\n";
    let run_md = feature(RUN_OK).replace(
        "- None.",
        "- Run `ns doctor` from `$V/repo`.\n\n```bash\nns doctor *\n```",
    );
    let out = run_with_features(&md, &[("run.md", run_md)]);
    assert!(out.status.success(), "{}", stderr(&out));
}

fn run_pass_lines(outcomes: &[&str]) -> String {
    let lines: Vec<String> = outcomes
        .iter()
        .map(|o| format!(r#"echo "{{\"outcome\":\"{o}\"}}" >> "$OUT/run-pass.out""#))
        .collect();
    format!(r#""$V/sandbox" sh -c '{}'"#, lines.join("; "))
}

#[test]
fn the_last_run_pass_result_decides() {
    let recipe = run_pass_lines(&["stuck", "done"]);
    let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(&recipe))]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_run_that_does_not_end_done_fails() {
    for (recipe, needle) in [
        (
            RUN_OK.replace("done", "stuck"),
            "run-pass outcome is \"stuck\", not \"done\"",
        ),
        (r#""$V/sandbox" true"#.to_string(), "no run-pass result"),
        (
            run_pass_lines(&["done", "stuck"]),
            "run-pass outcome is \"stuck\", not \"done\"",
        ),
    ] {
        let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(&recipe))]);
        assert_fails_with(&out, needle);
    }
}

#[test]
fn the_run_recipes_run_after_the_drive() {
    let md = skill(LAUNCH, DOCTOR_OK).replace(DRIVE, "\"$V/sandbox\" touch \"$V/seeded\"");
    let recipe = format!("\"$V/sandbox\" test -f \"$V/seeded\"\n  {RUN_OK}");
    let out = run_with_features(&md, &[("run.md", feature(&recipe))]);
    assert!(out.status.success(), "{}", stderr(&out));
    let md = skill(LAUNCH, DOCTOR_OK);
    let out = run_with_features(&md, &[("run.md", feature(&recipe))]);
    assert_fails_with(&out, "FAILED in run.md block 1");
}

#[test]
fn a_missing_run_feature_fails_naming_it() {
    let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[]);
    assert_fails_with(&out, "no bash block under ## Driving it with ns in");
}

fn real_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../.agents/skills/verify-nightshift")
}

fn real_features() -> Vec<(String, String)> {
    let mut v: Vec<_> = fs::read_dir(real_dir().join("features"))
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                fs::read_to_string(e.path()).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

/// The real skill and feature files, with `stale` replaced by `fresh` wherever it is.
fn run_real(stale: &str, fresh: &str) -> Output {
    let md = fs::read_to_string(real_dir().join("SKILL.md")).unwrap();
    let features = real_features();
    let found = md.contains(stale) || features.iter().any(|(_, t)| t.contains(stale));
    assert!(found, "recipe changed, update the test: {stale}");
    let features: Vec<_> = features
        .iter()
        .map(|(n, t)| (n.as_str(), t.replace(stale, fresh)))
        .collect();
    run_with_features(&md.replace(stale, fresh), &features)
}

#[test]
fn the_real_recipes_pass() {
    let out = run_real("", "");
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_stale_command_in_the_real_recipes_fails_wherever_it_sits() {
    for (stale, fresh) in [
        (
            "--manifest-path cli/Cargo.toml",
            "--manifest-path cli/Nope.toml",
        ),
        ("sandbox.sh --new", "sandbox.sh --old"),
        ("ns --version", "ns --nosuchflag"),
        (
            "cap run-pass ns run --issue 7",
            "cap run-pass ns run --issue 77",
        ),
        ("ghd/issue-7.json", "ghd/issue-77.json"),
    ] {
        let out = run_real(stale, fresh);
        assert!(!out.status.success(), "{fresh} passed: {}", stderr(&out));
    }
}

#[test]
fn src_added_to_the_real_install_link_doctor_recipe_fails() {
    let out = run_real(
        "\"$V/sandbox\" cap install-link-doctor ns doctor",
        "\"$V/sandbox\" --src cap install-link-doctor ns doctor",
    );
    assert_fails_with(&out, "install.md ## Driving it with ns block");
    assert_fails_with(&out, "runs ns doctor with --src");
}

#[test]
fn blank_and_comment_lines_in_a_recipe_are_not_commands() {
    let recipe = format!("# seed first\n\n  {RUN_OK}");
    let out = run_with_features(&skill(LAUNCH, DOCTOR_OK), &[("run.md", feature(&recipe))]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_feature_file_with_a_misspelled_recipe_heading_fails_naming_it() {
    let bad = feature(RUN_OK).replace("## Driving it with ns", "## Driving it with nz");
    let out = run_with_features(
        &skill(LAUNCH, DOCTOR_OK),
        &[
            ("run.md", feature(RUN_OK)),
            ("README.md", "# Map\n".to_string()),
            ("typo.md", bad),
        ],
    );
    assert_fails_with(&out, "typo.md has no ## Driving it with ns");
    assert!(!stderr(&out).contains("README.md"), "{}", stderr(&out));
}
