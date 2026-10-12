//! Prefer a ready issue whose predicted `touches` overlap no running unit's (docs/FACTORY.md,
//! "Parallel units"). Overlap orders the queue and never holds an issue back.

use std::fs;

use crate::git::{self, Repo};
use crate::touches::{touches, Overlap};
use crate::watch::{Forge, Issue};
use crate::worktree;

/// The heading that marks the brief in an issue's body or comments.
const BRIEF: &str = "## Agent Brief";

/// The overlap rule for `repo`: its tracked files, less those `ignore` globs match.
pub(in crate::watch) fn rule(repo: &Repo, ignore: &[String]) -> Overlap {
    let files = git::run(&repo.root, &["ls-files", "-z"]).unwrap_or_else(|e| {
        eprintln!("ns watch: overlap checks only paths briefs name: {e:#}");
        String::new()
    });
    let files = files
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(String::from)
        .collect();
    Overlap::new(files, ignore)
}

/// A ready issue's `touches`, from its newest Agent Brief: the last team comment carrying one,
/// else the body. Empty when no brief predicts any, or the comments can't be read.
pub(in crate::watch) fn predicted(forge: &dyn Forge, issue: &Issue) -> Vec<String> {
    let comments = forge.team_comments(issue.number).unwrap_or_else(|e| {
        eprintln!("ns watch: #{} brief not read: {e:#}", issue.number);
        Vec::new()
    });
    std::iter::once(&issue.body)
        .chain(&comments)
        .rfind(|t| t.contains(BRIEF))
        .map_or_else(Vec::new, |t| touches(t))
}

/// A running unit's `touches`: from `brief.md` in its worktree, else as predicted on its issue.
pub(in crate::watch) fn running(
    repo: &Repo,
    forge: &dyn Forge,
    issue: &Issue,
    unit: &str,
) -> Vec<String> {
    let brief = worktree::planned_path(repo, unit)
        .join(".ns")
        .join(unit)
        .join("brief.md");
    match fs::read_to_string(brief) {
        Ok(text) => touches(&text),
        Err(_) => predicted(forge, issue),
    }
}

/// The unit id of each running unit `mine` overlaps.
pub(in crate::watch) fn overlapped<'a>(
    rule: &Overlap,
    mine: &[String],
    running: &'a [(String, Vec<String>)],
) -> Vec<&'a str> {
    running
        .iter()
        .filter(|(_, theirs)| rule.overlaps(mine, theirs))
        .map(|(unit, _)| unit.as_str())
        .collect()
}

/// Which of `ready`, in queue order, to start next: the first whose predicted `touches` overlap
/// no `running` unit's, else the first. `rule` is read only when some running unit predicts.
pub(super) fn pick(
    ready: &[Issue],
    running: &[(String, Vec<String>)],
    rule: impl FnOnce() -> Overlap,
    forge: &dyn Forge,
) -> usize {
    if running.iter().all(|(_, t)| t.is_empty()) {
        return 0;
    }
    let rule = rule();
    ready
        .iter()
        .position(|i| overlapped(&rule, &predicted(forge, i), running).is_empty())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::watch::tests::Fake;

    fn issue(number: u64, body: &str) -> Issue {
        Issue {
            number,
            title: format!("Fix {number}"),
            labels: Vec::new(),
            body: body.into(),
            team: true,
        }
    }

    fn brief(globs: &str) -> String {
        format!("## Agent Brief\n**Touches:** {globs}\n")
    }

    fn rule(ignore: &[&str]) -> Overlap {
        let files = ["cli/src/run.rs", "cli/src/watch.rs", "docs/DESIGN.md"];
        Overlap::new(
            files.iter().map(|f| f.to_string()).collect(),
            &ignore.iter().map(|g| g.to_string()).collect::<Vec<_>>(),
        )
    }

    fn running(globs: &[&str]) -> Vec<(String, Vec<String>)> {
        vec![(
            "1-fix-1".into(),
            globs.iter().map(|g| g.to_string()).collect(),
        )]
    }

    /// Issues 2, 3 and 4 in queue order; issue n's brief comment predicts `briefs[n - 2]`.
    fn queued(briefs: [&str; 3]) -> (Fake, Vec<Issue>) {
        let comments: BTreeMap<u64, Vec<String>> = (2..)
            .zip(briefs)
            .map(|(n, b)| (n, vec![brief(b)]))
            .collect();
        let ready = (2..=4).map(|n| issue(n, "")).collect();
        (
            Fake {
                comments,
                ..Fake::default()
            },
            ready,
        )
    }

    #[test]
    fn a_ready_issue_clear_of_running_units_goes_before_a_higher_one_that_overlaps() {
        let (forge, ready) = queued(["`cli/src/run.rs`", "`cli/**`", "`docs/**`"]);
        let at = pick(&ready, &running(&["cli/src/*.rs"]), || rule(&[]), &forge);
        assert_eq!(ready[at].number, 4);
    }

    #[test]
    fn when_every_ready_issue_overlaps_the_highest_runs() {
        let (forge, ready) = queued(["`cli/src/run.rs`", "`cli/**`", "`cli/src/watch.rs`"]);
        let at = pick(&ready, &running(&["cli/**"]), || rule(&[]), &forge);
        assert_eq!(ready[at].number, 2);
    }

    #[test]
    fn files_overlap_ignore_names_never_overlap() {
        let (forge, ready) = queued(["`cli/src/run.rs`", "`docs/**`", "`docs/DESIGN.md`"]);
        let run = running(&["docs/DESIGN.md", "cli/src/run.rs"]);
        assert_eq!(pick(&ready, &run, || rule(&[]), &forge), 0);
        assert_eq!(
            ready[pick(&ready, &run, || rule(&["docs/*"]), &forge)].number,
            3
        );
    }

    #[test]
    fn a_brief_with_no_touches_never_overlaps() {
        // The running unit predicts nothing: the queue's order stands and the rule goes unread.
        let (forge, ready) = queued(["`cli/**`", "`cli/**`", "`docs/**`"]);
        let at = pick(&ready, &running(&[]), || panic!("rule read"), &forge);
        assert_eq!(at, 0);
        // Beside it, one that predicts still counts.
        let mut run = running(&[]);
        run.extend(running(&["cli/**"]));
        assert_eq!(ready[pick(&ready, &run, || rule(&[]), &forge)].number, 4);
        // A ready issue that predicts nothing runs beside anything.
        let (forge, ready) = queued(["`cli/**`", "", "`cli/**`"]);
        let at = pick(&ready, &running(&["**"]), || rule(&[]), &forge);
        assert_eq!(ready[at].number, 3);
    }

    #[test]
    fn the_newest_brief_predicts_and_an_unreadable_one_predicts_nothing() {
        let mut forge = Fake::default();
        let body = issue(2, &brief("`a.rs`"));
        assert_eq!(predicted(&forge, &body), ["a.rs"]);
        forge.comments.insert(
            2,
            vec![brief("`b.rs`"), "Not a brief: **Touches:** `c.rs`".into()],
        );
        assert_eq!(predicted(&forge, &body), ["b.rs"]);
        assert!(predicted(&forge, &issue(3, "**Touches:** `c.rs`")).is_empty());
        // Comments that can't be read leave the body's brief.
        forge.fails.insert("team_comments");
        assert_eq!(predicted(&forge, &body), ["a.rs"]);
        assert!(predicted(&forge, &issue(4, "")).is_empty());
    }

    #[test]
    fn the_rule_reads_the_tracked_files_and_without_git_only_named_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(root.join("cli")).unwrap();
        fs::write(root.join("cli/run.rs"), "").unwrap();
        fs::write(root.join("cli/untracked.rs"), "").unwrap();
        let globs = |g: &[&str]| g.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let repo = Repo {
            root: root.clone(),
            common_dir: root.join(".git"),
        };
        let bare = super::rule(&repo, &[]);
        assert!(!bare.overlaps(&globs(&["cli/*"]), &globs(&["cli/**"])));
        assert!(bare.overlaps(&globs(&["cli/run.rs"]), &globs(&["cli/**"])));
        assert!(!bare.overlaps(&globs(&["*"]), &globs(&["**"])));
        for args in [&["init", "-q"][..], &["add", "cli/run.rs"]] {
            git::run(&root, args).unwrap();
        }
        let tracked = super::rule(&repo, &[]);
        assert!(tracked.overlaps(&globs(&["cli/*"]), &globs(&["cli/**"])));
        assert!(!tracked.overlaps(&globs(&["cli/u*"]), &globs(&["cli/**"])));
        assert!(!super::rule(&repo, &globs(&["cli/run.rs"]))
            .overlaps(&globs(&["cli/*"]), &globs(&["cli/**"])));
    }

    #[test]
    fn overlapped_names_each_running_unit_hit() {
        let run = vec![
            ("1-a".to_string(), vec!["cli/**".to_string()]),
            ("5-b".to_string(), vec!["docs/**".to_string()]),
            ("6-c".to_string(), vec!["cli/src/run.rs".to_string()]),
        ];
        let mine = vec!["cli/src/run.rs".to_string()];
        assert_eq!(overlapped(&rule(&[]), &mine, &run), ["1-a", "6-c"]);
    }
}
