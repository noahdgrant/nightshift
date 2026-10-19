use crate::common::{script, Repo};

use assert_cmd::Command;

pub const SLICES: &str =
    "## Slices\n\n1. **First**: does a. Proof: test a.\n2. **Second**: does b. Proof: test b.";

/// A fake `gh` that serves unit 4 with `body` and `labels`, and logs what it is asked.
pub fn forge(repo: &Repo, body: &str, labels: &[&str]) {
    let bin = repo.path.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let labels: Vec<_> = labels
        .iter()
        .map(|l| format!(r#"{{"name":"{l}"}}"#))
        .collect();
    let unit = format!(
        r#"{{"number":4,"title":"unit 4","body":"{body}","labels":[{}],"author_association":"OWNER"}}"#,
        labels.join(",")
    );
    script(
        &bin,
        "gh",
        &format!(
            r#"echo "$@" >> "{bin}/gh.log"
case "$1 $2" in
"api repos/{{owner}}/{{repo}}/issues/4") printf '%s\n' '{unit}';;
esac"#,
            bin = bin.display()
        ),
    );
}

pub fn gh_log(repo: &Repo) -> String {
    std::fs::read_to_string(repo.path.parent().unwrap().join("bin/gh.log")).unwrap_or_default()
}

/// Runs `ns` with `args`, with the fakes in `bin` first on the path.
pub fn ns(repo: &Repo, args: &[&str]) -> assert_cmd::assert::Assert {
    let bin = repo.path.parent().unwrap().join("bin");
    Command::cargo_bin("ns")
        .unwrap()
        .args(args)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .current_dir(&repo.path)
        .assert()
}

/// A fake `claude` that answers every headless session with `reply`.
pub fn claude_answers(repo: &Repo, reply: &str) {
    let bin = repo.path.parent().unwrap().join("bin");
    script(
        &bin,
        "claude",
        &format!("cat >/dev/null\ncat <<'EOF'\n{reply}\nEOF"),
    );
}

#[test]
fn a_planned_unit_gets_its_slices_in_its_description_and_is_made_ready() {
    let repo = Repo::new();
    forge(&repo, "Make the thing.", &["status:needs-define"]);
    claude_answers(&repo, &format!("Here is the plan.\n\n{SLICES}"));
    ns(&repo, &["plan", "4"]).success();
    let log = gh_log(&repo);
    assert!(log.contains("issue edit 4 --body 🤖 *Posted by nightshift, an AI agent.*\n\nMake the thing.\n\n## Slices\n\n1. **First**"), "{log}");
    assert!(
        log.contains(
            "issue edit 4 --add-label status:ready-for-agent --remove-label status:needs-define"
        ),
        "{log}"
    );
}

#[test]
fn an_answer_with_no_slices_leaves_the_unit_as_it_was() {
    let repo = Repo::new();
    forge(&repo, "Make the thing.", &["status:needs-define"]);
    claude_answers(&repo, "I could not decide.");
    ns(&repo, &["plan", "4"]).failure();
    assert!(!gh_log(&repo).contains("issue edit"));
}

#[test]
fn a_unit_that_already_has_slices_is_not_planned_again() {
    let repo = Repo::new();
    forge(
        &repo,
        r"Spec.\n\n## Slices\n\n1. **A**: x.",
        &["status:needs-define"],
    );
    claude_answers(&repo, SLICES);
    let out = ns(&repo, &["plan", "4"]).failure();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("already has slices"), "{err}");
    assert!(!gh_log(&repo).contains("issue edit"));
}
