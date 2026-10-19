use crate::common::{script, Repo};
use crate::plan::{claude_answers, forge, gh_log, ns, SLICES};

/// A fake `claude`. In a session with the user it writes `spec` to the file the prompt names,
/// unless `spec` is empty; headless it answers with `plan`, or fails when that is empty.
fn claude(repo: &Repo, spec: &str, plan: &str) {
    let bin = repo.path.parent().unwrap().join("bin");
    let write = if spec.is_empty() {
        ""
    } else {
        r#"printf '%s\n' '__SPEC__' > "$path""#
    };
    let plan_step = if plan.is_empty() {
        "exit 1".to_string()
    } else {
        format!("cat >/dev/null\ncat <<'EOF'\n{plan}\nEOF")
    };
    script(
        &bin,
        "claude",
        &format!(
            r#"if [ "$1" = "-p" ]; then
{plan_step}
else
  printf %s "$1" > "{bin}/session.prompt"
  path=$(printf %s "$1" | sed -n 's/^Write the spec to: //p')
  {write}
fi"#,
            bin = bin.display(),
        )
        .replace("__SPEC__", spec),
    );
}

#[test]
fn a_defined_unit_has_the_spec_then_the_slices_and_is_ready() {
    let repo = Repo::new();
    forge(&repo, "Vague idea.", &["status:needs-define"]);
    claude(&repo, "Goal: do the thing.", SLICES);
    ns(&repo, &["define", "4"]).success();
    let log = gh_log(&repo);
    // The spec is recorded first, with the original kept, so a failed plan loses nothing.
    assert!(log.contains("issue edit 4 --body 🤖 *Posted by nightshift, an AI agent.*\n\nGoal: do the thing.\n\n<details><summary>Original description</summary>\n\nVague idea."), "{log}");
    assert!(
        log.contains("Goal: do the thing.\n\n## Slices\n\n1. **First**"),
        "{log}"
    );
    assert!(
        log.contains("--add-label status:ready-for-agent --remove-label status:needs-define"),
        "{log}"
    );
}

#[test]
fn the_session_is_given_the_grilling_the_requirements_and_the_unit() {
    let repo = Repo::new();
    forge(&repo, "Vague idea.", &["status:needs-define"]);
    claude(&repo, "Spec.", SLICES);
    ns(&repo, &["define", "4"]).success();
    let prompt =
        std::fs::read_to_string(repo.path.parent().unwrap().join("bin/session.prompt")).unwrap();
    assert!(prompt.contains("Interview the user relentlessly"));
    assert!(prompt.contains("You turn one vague unit into a spec"));
    assert!(prompt.contains("## unit 4\n\nVague idea."));
}

#[test]
fn a_session_that_writes_no_spec_leaves_the_unit_as_it_was() {
    let repo = Repo::new();
    forge(&repo, "Vague idea.", &["status:needs-define"]);
    claude(&repo, "", SLICES);
    let out = ns(&repo, &["define", "4"]).failure();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("without writing a spec"), "{err}");
    assert!(!gh_log(&repo).contains("issue edit"));
}

#[test]
fn a_plan_that_fails_keeps_the_spec_and_says_how_to_plan_it_again() {
    let repo = Repo::new();
    forge(&repo, "Vague idea.", &["status:needs-define"]);
    claude(&repo, "Goal: do the thing.", "");
    let out = ns(&repo, &["define", "4"]).failure();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("run `ns plan 4`"), "{err}");
    let log = gh_log(&repo);
    assert!(log.contains("issue edit 4 --body"), "{log}");
    assert!(!log.contains("status:ready-for-agent"), "{log}");
}

#[test]
fn a_spec_left_by_an_earlier_session_is_not_used() {
    let repo = Repo::new();
    forge(&repo, "Vague idea.", &["status:needs-define"]);
    claude_answers(&repo, SLICES);
    let stale = repo.path.join(".git/ns/define/4.md");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "Stale spec.").unwrap();
    // This fake session ends without writing anything.
    script(
        &repo.path.parent().unwrap().join("bin"),
        "claude",
        "[ \"$1\" = \"-p\" ] && { cat >/dev/null; exit 1; }\nexit 0",
    );
    ns(&repo, &["define", "4"]).failure();
    assert!(!gh_log(&repo).contains("Stale spec."));
}
