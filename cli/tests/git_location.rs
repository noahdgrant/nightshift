//! The entry points that keep an inherited git location (`GIT_DIR` and the rest) from reaching
//! a git process (#120, #638): `ns` clears them at the top of `main`, the shared test helpers
//! refuse to run with one set, and the repo's scripts and hooks unset them first.

mod common;

use std::fs;
use std::path::Path;

use common::LOCATION_VARS;

const MARK: &str = "NS_TEST_GIT_LOCATION_PROBE";

#[test]
fn command_probe() {
    if std::env::var_os(MARK).is_none() {
        return;
    }
    common::command("git").arg("--version").output();
}

#[test]
fn command_panics_naming_each_git_location_variable_the_test_process_has() {
    let out = common::command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "command_probe",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(MARK, "1")
        .env("GIT_DIR", "/nonexistent")
        .env("GIT_PREFIX", "")
        .output();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains(r#"git location variables set: ["GIT_DIR", "GIT_PREFIX"]"#),
        "{text}"
    );
}

#[test]
fn ns_started_with_git_location_variables_passes_none_to_a_child_script() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let init = common::command("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .output();
    assert!(init.status.success());
    fs::create_dir_all(root.join("docs/agents")).unwrap();
    fs::write(
        root.join("docs/agents/stack.md"),
        "| Task | Command |\n|---|---|\n| rules | `./lint` | 1 s |\n",
    )
    .unwrap();
    let report = root.join("env.txt");
    let check: String = LOCATION_VARS
        .iter()
        .map(|v| {
            format!(
                "[ -z \"${{{v}+set}}\" ] || echo {v} >> '{}'\n",
                report.display()
            )
        })
        .collect();
    common::write_exe(
        &root.join("lint"),
        format!("#!/bin/sh\n: > '{}'\n{check}", report.display()),
    );

    let decoy = tmp.path().join("decoy");
    let out = common::ns()
        .args(["rules", "check"])
        .current_dir(&root)
        .envs(LOCATION_VARS.iter().map(|v| (*v, &decoy)))
        .output();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&report).unwrap(), "");
}

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The names in `git::LOCATION_VARS`, read from its source.
fn crate_location_vars() -> Vec<String> {
    let src = fs::read_to_string(repo_root().join("cli/src/git.rs")).unwrap();
    let start = src.find("pub const LOCATION_VARS: &[&str] = &[").unwrap();
    let list = &src[start..start + src[start..].find("];").unwrap()];
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(String::from)
        .collect()
}

/// `Ok` when the script's first command, after comments, blank lines and `set`/`shopt` option
/// lines, is an `unset` of exactly the git location variables.
fn unsets_first(script: &str) -> Result<(), String> {
    let mut lines = script
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .skip_while(|l| l.starts_with("set -") || l.starts_with("shopt -"));
    let mut first = String::new();
    for line in lines.by_ref() {
        match line.strip_suffix('\\') {
            Some(part) => first.push_str(part),
            None => {
                first.push_str(line);
                break;
            }
        }
    }
    let mut words = first.split_whitespace();
    if words.next() != Some("unset") {
        return Err(format!("its first command is not an unset: {first:?}"));
    }
    let mut got: Vec<_> = words.collect();
    got.sort_unstable();
    let mut want = LOCATION_VARS.to_vec();
    want.sort_unstable();
    if got != want {
        return Err(format!("it unsets {got:?}, not {want:?}"));
    }
    Ok(())
}

#[test]
fn the_tests_list_of_git_location_variables_matches_the_crates() {
    assert_eq!(crate_location_vars(), LOCATION_VARS);
}

#[test]
fn every_script_entry_point_unsets_the_git_location_variables_first() {
    let root = repo_root();
    let mut scripts = vec![root.join("scripts/ci-local.sh")];
    let hooks: Vec<_> = fs::read_dir(root.join(".githooks"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(hooks.len() >= 2, "{hooks:?}");
    let packs: Vec<_> = fs::read_dir(root.join("rules"))
        .unwrap()
        .map(|e| e.unwrap().path().join("tests/run.sh"))
        .filter(|p| p.is_file())
        .collect();
    assert!(packs.len() >= 2, "{packs:?}");
    scripts.extend(hooks);
    scripts.extend(packs);
    let bad: Vec<_> = scripts
        .iter()
        .filter_map(|p| {
            unsets_first(&fs::read_to_string(p).unwrap())
                .err()
                .map(|e| format!("{}: {e}", p.display()))
        })
        .collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn the_unset_check_rejects_a_late_or_partial_unset() {
    let all = LOCATION_VARS.join(" ");
    for ok in [
        format!("#!/bin/sh\n# c\n\nunset {all}\ngit status\n"),
        format!(
            "#!/bin/sh\nset -eu\nshopt -s nullglob\nunset GIT_DIR \\\n  {}\n",
            LOCATION_VARS[1..].join(" ")
        ),
    ] {
        assert_eq!(unsets_first(&ok), Ok(()), "{ok}");
    }
    for (bad, why) in [
        (format!("#!/bin/sh\ncd x\nunset {all}\n"), "not an unset"),
        (
            "#!/bin/sh\nunset GIT_DIR GIT_WORK_TREE\n".to_owned(),
            "it unsets",
        ),
        (format!("#!/bin/sh\nunset {all} PATH\n"), "it unsets"),
        ("#!/bin/sh\n".to_owned(), "not an unset"),
    ] {
        let err = unsets_first(&bad).unwrap_err();
        assert!(err.contains(why), "{bad}: {err}");
    }
}

/// `Ok` when the first statement in the body of `fn main() -> ExitCode {` is the call to
/// `clear_git_location_vars();`.
fn main_clears_first(src: &str) -> Result<(), String> {
    const HEAD: &str = "fn main() -> ExitCode {";
    let start = src.find(HEAD).ok_or("no `fn main() -> ExitCode {`")? + HEAD.len();
    let first = src[start..]
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("//"))
        .unwrap_or("");
    if first == "clear_git_location_vars();" {
        Ok(())
    } else {
        Err(format!("main's first statement is {first:?}"))
    }
}

#[test]
fn main_clears_the_git_location_variables_before_anything_else() {
    let src = fs::read_to_string(repo_root().join("cli/src/main.rs")).unwrap();
    assert_eq!(main_clears_first(&src), Ok(()));
}

#[test]
fn the_main_check_rejects_a_late_or_missing_call() {
    let ok =
        "fn main() -> ExitCode {\n    clear_git_location_vars();\n    let cli = Cli::parse();\n}\n";
    assert_eq!(main_clears_first(ok), Ok(()));
    for bad in [
        "fn main() -> ExitCode {\n    let cli = Cli::parse();\n    clear_git_location_vars();\n}\n",
        "fn main() -> ExitCode {\n    let cli = Cli::parse();\n}\n",
        "fn main() -> ExitCode {\n}\n",
        "fn other() {}\n",
    ] {
        assert!(main_clears_first(bad).is_err(), "{bad}");
    }
}
