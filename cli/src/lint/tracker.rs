use std::fs;
use std::path::Path;

use super::{code_spans, LintError};

/// The tracker operations skills name. Each tracker template gives every one a `###`
/// heading under `## Operations`, with that tracker's command beneath it.
pub const OPERATIONS: &[&str] = &[
    "Create an issue",
    "Read an issue",
    "List issues",
    "Search for duplicates",
    "Comment on an issue",
    "Apply or remove a label",
    "Close an issue",
    "Read an author's association",
    "List labels",
    "Create a label",
    "Link a child to its parent",
    "List a parent's children",
    "Add a blocking link",
    "Check an issue is unblocked",
    "Open a PR",
    "Read the new PR's URL and draft state",
    "Mark a PR ready",
    "Read a PR's state",
    "Find a branch's PR",
    "Read a PR's merge state",
    "Read a PR's checks",
    "Wait for a PR's checks",
    "Read a PR's review threads",
    "Reply to a review thread",
    "Resolve a review thread",
    "Re-run CI",
    "Read a PR's merge commit",
    "List a commit's CI runs",
    "Read a PR and its diff",
    "List external PRs",
    "Comment on, label or close a PR",
];

/// The templates, relative to the skills root, that must define every operation.
/// They hold tracker commands on purpose, so the command check skips them.
pub const TEMPLATES: [&str; 2] = [
    "ns-setup/references/issue-tracker-github.md",
    "ns-setup/references/issue-tracker-gitlab.md",
];

/// The `###` headings inside the `## Operations` section, fenced code skipped.
fn operation_headings(text: &str) -> Vec<&str> {
    let mut in_ops = false;
    let mut found = Vec::new();
    for line in super::prose_lines(text) {
        if let Some(h2) = line.strip_prefix("## ") {
            in_ops = h2.trim() == "Operations";
        } else if let Some(h3) = line.strip_prefix("### ").filter(|_| in_ops) {
            found.push(h3.trim());
        }
    }
    found
}

/// Checks each template under `root` defines exactly the operations in `OPERATIONS`.
/// A root without `ns-setup/` has no templates to check.
pub(super) fn check_templates(root: &Path) -> Vec<LintError> {
    if !root.join("ns-setup").is_dir() {
        return Vec::new();
    }
    let mut errors = Vec::new();
    for rel in TEMPLATES {
        let path = root.join(rel);
        let mut push = |message: String| {
            errors.push(LintError {
                skill: "ns-setup".into(),
                file: path.to_string_lossy().into_owned(),
                message,
            })
        };
        let Ok(text) = fs::read_to_string(&path) else {
            push(format!("missing tracker template {rel}"));
            continue;
        };
        let found = operation_headings(&text);
        for op in OPERATIONS.iter().filter(|op| !found.contains(op)) {
            push(format!(
                "tracker operation \"{op}\" has no ### heading under ## Operations"
            ));
        }
        for h in found.iter().filter(|h| !OPERATIONS.contains(h)) {
            push(format!(
                "\"{h}\" is not a tracker operation; the list is lint::tracker::OPERATIONS"
            ));
        }
    }
    errors
}

/// Whether `code` runs `gh` or `glab` as a command other than `auth`: the word is in
/// command position (it starts the code, or follows `|`, `;`, `&`, `(`, `{` or `!`,
/// spaces between), and a subcommand follows. A word in an argument list is not one.
fn runs_tracker_cli(code: &str) -> bool {
    ["gh ", "glab "].iter().any(|cli| {
        code.match_indices(cli).any(|(at, _)| {
            let before = code[..at].trim_end_matches(' ');
            let starts = before
                .chars()
                .next_back()
                .is_none_or(|c| matches!(c, '|' | ';' | '&' | '(' | '{' | '!'));
            let sub = code[at + cli.len()..].split_whitespace().next();
            starts && sub.is_some_and(|s| s != "auth")
        })
    })
}

/// The 1-based lines of `text` whose code (a fenced line or an inline span) runs a
/// tracker command, with that code.
pub(super) fn tracker_commands(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (n, line, fenced) in super::classified_lines(text) {
        let codes = if fenced {
            vec![line.trim()]
        } else {
            code_spans(line).1
        };
        found.extend(
            codes
                .into_iter()
                .filter(|c| runs_tracker_cli(c))
                .map(|c| (n, c.to_string())),
        );
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(ops: &[&str]) -> String {
        let mut s =
            "# Tracker\n\n## Conventions\n\n### Not an operation\n\n## Operations\n\n".to_string();
        for op in ops {
            s.push_str(&format!("### {op}\n\n`cmd`\n\n"));
        }
        s.push_str("```\n### Inside a fence\n```\n\n## Team\n\n### Also not one\n");
        s
    }

    fn messages(root: &Path) -> Vec<String> {
        check_templates(root)
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    fn write_templates(root: &Path, github: &str, gitlab: &str) {
        let refs = root.join("ns-setup/references");
        fs::create_dir_all(&refs).unwrap();
        fs::write(refs.join("issue-tracker-github.md"), github).unwrap();
        fs::write(refs.join("issue-tracker-gitlab.md"), gitlab).unwrap();
    }

    #[test]
    fn templates_with_every_operation_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let full = template(OPERATIONS);
        write_templates(tmp.path(), &full, &full);
        assert!(
            messages(tmp.path()).is_empty(),
            "{:?}",
            messages(tmp.path())
        );
    }

    #[test]
    fn a_missing_or_unknown_operation_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let full = template(OPERATIONS);
        let mut ops = OPERATIONS[1..].to_vec();
        ops.push("Merge a PR");
        write_templates(tmp.path(), &full, &template(&ops));
        assert_eq!(
            messages(tmp.path()),
            [
                "tracker operation \"Create an issue\" has no ### heading under ## Operations",
                "\"Merge a PR\" is not a tracker operation; the list is lint::tracker::OPERATIONS",
            ]
        );
    }

    #[test]
    fn a_missing_template_fails_and_a_root_without_ns_setup_passes() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(messages(tmp.path()).is_empty());
        let refs = tmp.path().join("ns-setup/references");
        fs::create_dir_all(&refs).unwrap();
        fs::write(refs.join("issue-tracker-github.md"), template(OPERATIONS)).unwrap();
        assert_eq!(
            messages(tmp.path()),
            ["missing tracker template ns-setup/references/issue-tracker-gitlab.md"]
        );
    }

    #[test]
    fn the_shipped_templates_define_every_operation() {
        let skills = Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills");
        assert!(messages(&skills).is_empty(), "{:?}", messages(&skills));
    }

    #[test]
    fn runs_tracker_cli_cases() {
        for yes in [
            "gh pr view",
            "glab mr list",
            "x=$(gh pr list)",
            "a | gh api x",
            "(glab ci status",
        ] {
            assert!(runs_tracker_cli(yes), "{yes}");
        }
        for no in [
            "gh",
            "gh auth status",
            "glab auth login",
            "ns-gh pr",
            "high pr",
            "a/gh pr",
            "gh ",
            "my.gh x",
            "for b in claude gh glab ssh ns; do",
            "x=gh pr",
        ] {
            assert!(!runs_tracker_cli(no), "{no}");
        }
    }
}
