use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::checks::COMMAND_FILE;
use crate::{forge, harness, labels, manifest, skill, status, sync, vcs};

/// The check that fails a change request which closes no unit.
const CLOSES_UNIT: &str = include_str!("../../../.github/workflows/closes-unit.yml");
pub(crate) const CLOSES_UNIT_PATH: &str = ".github/workflows/closes-unit.yml";
/// The name the workflow's check reports, which the branch protection must require.
pub(crate) const CLOSES_UNIT_CHECK: &str = "Closes a unit";

/// The pages of `docs/agents/` that `ns init` writes: path from the repository root, and text.
/// Each is a fixed template for a person to fill in.
const AGENT_PAGES: [(&str, &str); 3] = [
    (
        "docs/agents/stack.md",
        "# Stack\n\nWhat the repository is built with. Read by `ns-triage` and `ns-design-doc`.\n\nReplace this with the languages, frameworks and tools the repository uses.\n",
    ),
    (
        "docs/agents/how-to-run.md",
        "# How to run\n\nHow to run and verify the repository. Read by `ns-verify`.\n\nThe checks are in `.nightshift/check`.\n\nReplace this with how to start the app and use it the way a user would.\n",
    ),
    (
        "docs/agents/labels.md",
        "# Labels\n\nWhat the repository's labels mean. Read by `ns-triage`.\n\nReplace this with the labels the repository uses and when each applies.\n",
    ),
];

/// Writes each page of `docs/agents/` that is missing; an existing page is left alone.
fn agent_pages_write(root: &std::path::Path) -> Result<()> {
    for (rel, text) in AGENT_PAGES {
        let path = root.join(rel);
        if path.symlink_metadata().is_ok() {
            continue;
        }
        std::fs::create_dir_all(path.parent().context("a page has no directory")?)?;
        std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}

#[derive(ClapArgs)]
pub struct Args {
    /// The command that runs the repository's checks, written to `.nightshift/check` without asking.
    #[arg(long, value_name = "COMMAND")]
    check: Option<String>,
}

/// Pins the repository to this `ns`: records its version in `.nightshift/nightshift.toml`, along
/// with the harnesses the repository uses, writes their skills, and makes a session start with
/// the skills up to date, and creates the labels the forge lacks. When the repository has neither
/// `AGENTS.md` nor `CLAUDE.md`, writes both. Either way `AGENTS.md`, or `CLAUDE.md` when that is
/// all there is, is given a pointer to the coding standards. The files it leaves to commit are the
/// manifest, the hook, those files and the workflow that checks a change request closes a unit.
/// It also writes `.nightshift/check`, the command that runs the repository's checks: given by
/// `--check`, or else proposed from what the repository holds and confirmed by the user. An
/// existing file is left alone. It writes the pages of `docs/agents/` that are missing. Run again, it changes only what is missing, and it never
/// overwrites an existing workflow.
pub fn run(args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    check_write(&root, args.check, &mut prompt_terminal)?;
    let mut pin = manifest::pin_read(&root)?;
    pin.version = Some(env!("CARGO_PKG_VERSION").to_string());
    manifest::pin_write(&root, &pin)?;
    let synced = sync::run(&root, &pin.harness)?;
    let instruction_files = sync::instruction_files_write(&root)?;
    agents_point(&root)?;
    agent_pages_write(&root)?;
    let mut hooks = Vec::new();
    for kind in &pin.harness {
        let wrote = harness::of(*kind).session_start_hook_install(&root, sync::HOOK)?;
        hooks.push(json!({ "harness": kind.name(), "written": wrote }));
    }
    let workflow = root.join(CLOSES_UNIT_PATH);
    let workflows_written = if workflow.exists() {
        0
    } else {
        std::fs::create_dir_all(workflow.parent().context("a workflow has no directory")?)?;
        std::fs::write(&workflow, CLOSES_UNIT)
            .with_context(|| format!("cannot write {}", workflow.display()))?;
        1
    };
    let ensured = labels::ensure(forge::get().as_ref(), &root)
        .context("the labels were not created; run `ns init` again")?;
    if ensured.both_left {
        eprintln!(
            "ns: the repository has both {} and {}; left both alone. Move the units from the old one and delete it",
            status::LEGACY_READY_FOR_HUMAN,
            status::WAITING_FOR_HUMAN
        );
    }
    println!(
        "{}",
        json!({
            "version": pin.version,
            "harness": pin.harness.iter().map(|k| k.name()).collect::<Vec<_>>(),
            "skills_written": synced.iter().map(|s| s.written).sum::<usize>(),
            "hooks": hooks,
            "workflows_written": workflows_written,
            "instruction_files": instruction_files,
            "labels_created": ensured.created,
            "labels_renamed": ensured.renamed,
        })
    );
    Ok(())
}

/// The repository's usual checks command, by the first of its files that names one.
fn check_detect(root: &std::path::Path) -> Option<&'static str> {
    let has = |name: &str| root.join(name).is_file();
    let makefile_check = std::fs::read_to_string(root.join("Makefile")).is_ok_and(|text| {
        text.lines().any(|l| {
            l.strip_prefix("check")
                .is_some_and(|r| r.trim_start().starts_with(':'))
        })
    });
    let npm_test = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v["scripts"]["test"].is_string());
    if makefile_check {
        Some("make check")
    } else if has("Cargo.toml") {
        Some("cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test")
    } else if npm_test {
        Some("npm test")
    } else if has("go.mod") {
        Some("go vet ./... && go test ./...")
    } else if has("pyproject.toml") {
        Some("pytest")
    } else {
        None
    }
}

/// Asks on the terminal for the checks command, offering `proposal`. An empty answer accepts the
/// proposal. `None` when there is no terminal to ask.
fn prompt_terminal(proposal: Option<&str>) -> Result<Option<String>> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return Ok(None);
    }
    match proposal {
        Some(p) => eprint!("Command that runs the checks [{p}]: "),
        None => eprint!("Command that runs the checks: "),
    }
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let answer = line.trim();
    Ok(Some(
        if answer.is_empty() {
            proposal.unwrap_or("")
        } else {
            answer
        }
        .to_string(),
    ))
}

/// Writes `.nightshift/check` unless it exists. The command is `given`, else what `ask` returns
/// when offered what the repository's files suggest; `ask` returns `None` with no terminal.
fn check_write(
    root: &std::path::Path,
    given: Option<String>,
    ask: &mut dyn FnMut(Option<&str>) -> Result<Option<String>>,
) -> Result<()> {
    let path = root.join(COMMAND_FILE);
    if path.symlink_metadata().is_ok() {
        return Ok(());
    }
    let command = match given {
        Some(c) => c,
        None => {
            let found = check_detect(root);
            match ask(found)? {
                Some(c) => c,
                None => match found {
                    Some(c) => c.to_string(),
                    None => anyhow::bail!(
                        "no checks command found and no terminal to ask on; pass `--check '<command>'` or create {COMMAND_FILE}"
                    ),
                },
            }
        }
    };
    let command = command.trim();
    anyhow::ensure!(
        !command.is_empty(),
        "the checks command is empty; pass `--check '<command>'` or create {COMMAND_FILE}"
    );
    std::fs::create_dir_all(path.parent().context("the check file has no directory")?)?;
    std::fs::write(&path, format!("{command}\n"))
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Makes sure the repository's `AGENTS.md`, or its `CLAUDE.md` when that is all it has, points to
/// the coding standards that `ns sync` writes, and leaves the rest of the file as it is.
/// It creates nothing: a repository with neither file is given both by `instruction_files_write`.
fn agents_point(root: &std::path::Path) -> Result<()> {
    let file = skill::STANDARDS_PATH;
    let Some(path) = ["AGENTS.md", "CLAUDE.md"]
        .map(|name| root.join(name))
        .into_iter()
        .find(|p| p.symlink_metadata().is_ok())
    else {
        return Ok(());
    };
    let have = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // A link to nothing is the owner's; writing would create what it points to.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    if have.contains(file) {
        return Ok(());
    }
    let line = format!("- **Coding standards**: `{file}`, which holds in any language.\n");
    let text = format!("{}\n\n{line}", have.trim_end());
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARGO: &str =
        "cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test";

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn written(d: &std::path::Path) -> String {
        std::fs::read_to_string(d.join(COMMAND_FILE)).unwrap()
    }

    #[test]
    fn a_detected_command_that_is_accepted_is_written() {
        let d = dir();
        std::fs::write(d.path().join("go.mod"), "").unwrap();
        let mut offered = None;
        check_write(d.path(), None, &mut |p| {
            offered = p.map(String::from);
            Ok(Some(p.unwrap().to_string()))
        })
        .unwrap();
        assert_eq!(offered.as_deref(), Some("go vet ./... && go test ./..."));
        assert_eq!(written(d.path()), "go vet ./... && go test ./...\n");
    }

    #[test]
    fn a_detected_command_that_is_edited_is_written_as_edited() {
        let d = dir();
        std::fs::write(d.path().join("pyproject.toml"), "").unwrap();
        check_write(d.path(), None, &mut |_| Ok(Some("pytest -x".into()))).unwrap();
        assert_eq!(written(d.path()), "pytest -x\n");
    }

    #[test]
    fn with_nothing_detected_the_user_is_asked_with_no_proposal_and_types_one() {
        let d = dir();
        let mut offered = Some("unset".to_string());
        check_write(d.path(), None, &mut |p| {
            offered = p.map(String::from);
            Ok(Some("./verify.sh".into()))
        })
        .unwrap();
        assert_eq!(offered, None);
        assert_eq!(written(d.path()), "./verify.sh\n");
    }

    #[test]
    fn an_existing_file_is_left_alone_and_nothing_is_asked() {
        let d = dir();
        std::fs::create_dir_all(d.path().join(".nightshift")).unwrap();
        std::fs::write(d.path().join(COMMAND_FILE), "mine\n").unwrap();
        check_write(d.path(), Some("other".into()), &mut |_| panic!("asked")).unwrap();
        assert_eq!(written(d.path()), "mine\n");
    }

    #[test]
    fn the_given_command_skips_the_question() {
        let d = dir();
        check_write(d.path(), Some("just test".into()), &mut |_| panic!("asked")).unwrap();
        assert_eq!(written(d.path()), "just test\n");
    }

    #[test]
    fn no_terminal_and_nothing_detected_fails_and_says_what_to_do() {
        let d = dir();
        let e = check_write(d.path(), None, &mut |_| Ok(None)).unwrap_err();
        let msg = format!("{e:#}");
        assert!(msg.contains("--check '<command>'") && msg.contains(".nightshift/check"));
        assert!(!d.path().join(COMMAND_FILE).exists());
    }

    #[test]
    fn no_terminal_with_a_detected_command_writes_it() {
        let d = dir();
        std::fs::write(
            d.path().join("package.json"),
            r#"{"scripts":{"test":"jest"}}"#,
        )
        .unwrap();
        check_write(d.path(), None, &mut |_| Ok(None)).unwrap();
        assert_eq!(written(d.path()), "npm test\n");
    }

    #[test]
    fn an_empty_answer_with_no_proposal_fails() {
        let d = dir();
        assert!(check_write(d.path(), None, &mut |_| Ok(Some(String::new()))).is_err());
    }

    #[test]
    fn a_makefile_check_target_is_offered_before_cargo() {
        let d = dir();
        std::fs::write(d.path().join("Makefile"), "build:\n\tx\ncheck:\n\ty\n").unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(check_detect(d.path()), Some("make check"));
    }

    #[test]
    fn cargo_is_offered_when_the_makefile_has_no_check_target() {
        let d = dir();
        std::fs::write(d.path().join("Makefile"), "build:\n\tx\n").unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(check_detect(d.path()), Some(CARGO));
    }

    #[test]
    fn npm_needs_a_test_script() {
        let d = dir();
        std::fs::write(d.path().join("package.json"), r#"{"scripts":{}}"#).unwrap();
        assert_eq!(check_detect(d.path()), None);
    }

    #[test]
    fn the_agent_pages_are_written_and_an_existing_one_is_left_alone() {
        let d = dir();
        std::fs::create_dir_all(d.path().join("docs/agents")).unwrap();
        std::fs::write(d.path().join("docs/agents/stack.md"), "mine\n").unwrap();
        agent_pages_write(d.path()).unwrap();
        let read = |n: &str| std::fs::read_to_string(d.path().join("docs/agents").join(n)).unwrap();
        assert_eq!(read("stack.md"), "mine\n");
        assert!(read("how-to-run.md").contains("`.nightshift/check`"));
        assert!(read("labels.md").starts_with("# Labels\n"));
    }

    #[test]
    fn the_workflow_reports_the_check_the_protection_requires() {
        assert!(CLOSES_UNIT.contains(&format!("name: {CLOSES_UNIT_CHECK}\n")));
    }
}
