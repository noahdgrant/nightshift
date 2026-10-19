use crate::common::{script, Repo};
use crate::plan::{forge, gh_log, ns};

const BRIEF: &str =
    "### Repro\nrun it\n\n### Root cause\nthe loop is off by one\n\n### Fix\nfix the loop";

/// A fake `claude` that records where it worked and what it was asked, then answers `reply`.
fn claude(repo: &Repo, reply: &str, exit: i32) {
    let bin = repo.path.parent().unwrap().join("bin");
    script(
        &bin,
        "claude",
        &format!(
            r#"pwd > "{bin}/session.cwd"
cat > "{bin}/session.prompt"
touch scratch-edit.txt
cat <<'EOF'
{reply}
EOF
exit {exit}"#,
            bin = bin.display()
        ),
    );
}

fn scratch(repo: &Repo) -> std::path::PathBuf {
    repo.workspace("troubleshoot-4")
}

#[test]
fn a_diagnosed_unit_gets_the_diagnosis_in_its_description_and_goes_on() {
    let repo = Repo::new();
    forge(&repo, "It crashes.", &["status:needs-repro"]);
    claude(&repo, &format!("{BRIEF}\n\nROUTE: build"), 0);
    ns(&repo, &["troubleshoot", "4"]).success();
    let log = gh_log(&repo);
    assert!(
        log.contains("issue edit 4 --body 🤖 *Posted by nightshift, an AI agent.*\n\nIt crashes.\n\n## Diagnosis\n\n### Repro\nrun it"),
        "{log}"
    );
    assert!(
        log.contains("--add-label status:ready-for-agent --remove-label status:needs-repro"),
        "{log}"
    );
}

#[test]
fn the_session_works_in_a_scratch_workspace_that_is_gone_afterwards() {
    let repo = Repo::new();
    forge(&repo, "It crashes.", &["status:needs-repro"]);
    claude(&repo, &format!("{BRIEF}\n\nROUTE: build"), 0);
    ns(&repo, &["troubleshoot", "4"]).success();
    let bin = repo.path.parent().unwrap().join("bin");
    let cwd = std::fs::read_to_string(bin.join("session.cwd")).unwrap();
    assert!(
        cwd.trim().ends_with("repo.workspaces/troubleshoot-4"),
        "{cwd}"
    );
    let prompt = std::fs::read_to_string(bin.join("session.prompt")).unwrap();
    assert!(prompt.contains("You diagnose one bug"));
    assert!(prompt.contains("## unit 4\n\nIt crashes."));
    assert!(!scratch(&repo).exists());
    assert!(!repo.branches().contains(&"ns/troubleshoot-4".to_string()));
    assert!(!repo.path.join("scratch-edit.txt").exists());
}

#[test]
fn each_route_sets_its_own_label() {
    for (route, label) in [
        ("define", "status:needs-define"),
        ("info", "status:needs-info"),
        ("human", "status:ready-for-human"),
    ] {
        let repo = Repo::new();
        forge(&repo, "It crashes.", &["status:needs-repro"]);
        claude(&repo, &format!("{BRIEF}\n\nROUTE: {route}"), 0);
        ns(&repo, &["troubleshoot", "4"]).success();
        let log = gh_log(&repo);
        assert!(
            log.contains(&format!(
                "--add-label {label} --remove-label status:needs-repro"
            )),
            "{route}: {log}"
        );
    }
}

#[test]
fn an_answer_ns_cannot_use_leaves_the_unit_as_it_was_and_the_scratch_workspace_gone() {
    for (reply, exit) in [
        ("no route here".to_string(), 0),
        (format!("{BRIEF}\n\nROUTE: repro"), 0),
        (format!("{BRIEF}\n\nROUTE: build"), 1),
    ] {
        let repo = Repo::new();
        forge(&repo, "It crashes.", &["status:needs-repro"]);
        claude(&repo, &reply, exit);
        ns(&repo, &["troubleshoot", "4"]).failure();
        assert!(!gh_log(&repo).contains("issue edit"), "{reply}");
        assert!(!scratch(&repo).exists());
    }
}

#[test]
fn a_unit_with_a_diagnosis_is_not_troubleshot_again() {
    let repo = Repo::new();
    forge(
        &repo,
        r"It crashes.\n\n## Diagnosis\n\nknown",
        &["status:needs-repro"],
    );
    claude(&repo, &format!("{BRIEF}\n\nROUTE: build"), 0);
    ns(&repo, &["troubleshoot", "4"]).failure();
    assert!(!gh_log(&repo).contains("issue edit"));
}
