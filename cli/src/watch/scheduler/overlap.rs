//! Hold back a ready issue whose predicted `touches` overlap a running unit's (docs/factory/watch.md,
//! "Parallel units"): the reads behind `core::fill`'s overlap pick, which decides.

use std::fs;

use crate::git::{self, Repo};
use crate::touches::{touches, Overlap};
pub(in crate::watch) use crate::watch::core::fill::overlapped;
use crate::watch::core::fill::{plan_fill, FillAct, FillInput, Need, Overlaps};
use crate::watch::{unit_id, Forge, Issue};
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
fn running(repo: &Repo, forge: &dyn Forge, issue: &Issue, unit: &str) -> Vec<String> {
    let brief = worktree::planned_path(repo, unit)
        .join(".ns")
        .join(unit)
        .join("brief.md");
    match fs::read_to_string(brief) {
        Ok(text) => touches(&text),
        Err(_) => predicted(forge, issue),
    }
}

/// The running units that predict any `touches`, each as its unit id and globs. A unit id is the
/// one its run recorded, else the one `ns watch` names the issue's unit by.
pub(in crate::watch) fn running_units<'a>(
    repo: &Repo,
    forge: &dyn Forge,
    issues: impl IntoIterator<Item = (&'a Issue, Option<&'a str>)>,
) -> Vec<(String, Vec<String>)> {
    issues
        .into_iter()
        .map(|(issue, recorded)| {
            let unit = recorded.map_or_else(|| unit_id(issue), String::from);
            let touches = running(repo, forge, issue, &unit);
            (unit, touches)
        })
        .filter(|(_, t)| !t.is_empty())
        .collect()
}

/// The issues `input` claims at `now` and the holds it says (`core::fill::plan_fill`), reading
/// what it asks for: the running units' `touches` through `running`, the overlap rule through
/// `rule` and each brief through `forge`, each at most once.
pub(super) fn plan(
    forge: &dyn Forge,
    input: &FillInput,
    now: i64,
    running: impl FnOnce() -> Vec<(String, Vec<String>)>,
    rule: impl FnOnce() -> Overlap,
) -> Vec<FillAct> {
    let mut overlaps = Overlaps::default();
    let (mut running, mut rule) = (Some(running), Some(rule));
    loop {
        match plan_fill(input, &overlaps, now) {
            Ok(acts) => return acts,
            Err(Need::Running) => {
                overlaps.running = Some(running.take().expect("asked once")());
            }
            Err(Need::Rule) => overlaps.rule = Some(rule.take().expect("asked once")()),
            Err(Need::Brief(n)) => {
                // Red main may admit a fix issue filed too late to be listed.
                let admitted = input.gate.admit(input.ready.to_vec());
                let issue = admitted.iter().find(|i| i.number == n);
                let brief = issue.map_or_else(Vec::new, |i| predicted(forge, i));
                overlaps.briefs.insert(n, brief);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::watch::core::fill::{Claims, Holds, Memory, HOLD_LIMIT_S};
    use crate::watch::core::red::Gate;
    use crate::watch::forge::fake::FakeForge;

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
        if globs.is_empty() {
            return Vec::new();
        }
        vec![(
            "1-fix-1".into(),
            globs.iter().map(|g| g.to_string()).collect(),
        )]
    }

    /// The claims and holds a fill at `now` with `free` slots makes of `ready`, nothing taken
    /// and main not red.
    fn fill(
        forge: &FakeForge,
        ready: &[Issue],
        free: usize,
        now: i64,
        holds: &Holds,
        running: impl FnOnce() -> Vec<(String, Vec<String>)>,
        make: impl FnOnce() -> Overlap,
    ) -> Vec<FillAct> {
        let taken = BTreeSet::new();
        let input = FillInput {
            ready,
            gate: &Gate::Open,
            taken: &taken,
            free,
            claims: Claims::default(),
            holds,
            memory: Memory::default(),
        };
        plan(forge, &input, now, running, make)
    }

    fn claimed(acts: &[FillAct]) -> Vec<u64> {
        acts.iter()
            .filter_map(|a| match a {
                FillAct::Claim(i) => Some(i.number),
                _ => None,
            })
            .collect()
    }

    /// The issue a fill on `ready` starts first at time 0, with no issue held before.
    fn pick(
        ready: &[Issue],
        running: &[(String, Vec<String>)],
        make: impl FnOnce() -> Overlap,
        forge: &FakeForge,
    ) -> Option<u64> {
        let acts = fill(
            forge,
            ready,
            1,
            0,
            &Holds::default(),
            || running.to_vec(),
            make,
        );
        claimed(&acts).first().copied()
    }

    /// Issues 2, 3 and 4 in queue order; issue n's brief comment predicts `briefs[n - 2]`.
    fn queued(briefs: [&str; 3]) -> (FakeForge, Vec<Issue>) {
        let comments: BTreeMap<u64, Vec<String>> = (2..)
            .zip(briefs)
            .map(|(n, b)| (n, vec![brief(b)]))
            .collect();
        let ready = (2..=4).map(|n| issue(n, "")).collect();
        (
            FakeForge {
                comments,
                ..FakeForge::default()
            },
            ready,
        )
    }

    #[test]
    fn a_ready_issue_clear_of_running_units_goes_before_a_higher_one_that_overlaps() {
        let (forge, ready) = queued(["`cli/src/run.rs`", "`cli/**`", "`docs/**`"]);
        let at = pick(&ready, &running(&["cli/src/*.rs"]), || rule(&[]), &forge);
        assert_eq!(at, Some(4));
    }

    #[test]
    fn when_every_ready_issue_overlaps_none_starts() {
        let (forge, ready) = queued(["`cli/src/run.rs`", "`cli/**`", "`cli/src/watch.rs`"]);
        let at = pick(&ready, &running(&["cli/**"]), || rule(&[]), &forge);
        assert_eq!(at, None);
    }

    #[test]
    fn files_overlap_ignore_names_never_overlap() {
        let (forge, ready) = queued(["`cli/src/run.rs`", "`docs/**`", "`docs/DESIGN.md`"]);
        let run = running(&["docs/DESIGN.md", "cli/src/run.rs"]);
        assert_eq!(pick(&ready, &run, || rule(&[]), &forge), None);
        assert_eq!(pick(&ready, &run, || rule(&["docs/*"]), &forge), Some(3));
    }

    #[test]
    fn a_brief_with_no_touches_never_overlaps() {
        // The running unit predicts nothing: the queue's order stands and the rule goes unread.
        let (forge, ready) = queued(["`cli/**`", "`cli/**`", "`docs/**`"]);
        let at = pick(&ready, &running(&[]), || panic!("rule read"), &forge);
        assert_eq!(at, Some(2));
        // Beside it, one that predicts still counts.
        let mut run = running(&[]);
        run.extend(running(&["cli/**"]));
        assert_eq!(pick(&ready, &run, || rule(&[]), &forge), Some(4));
        // A ready issue that predicts nothing runs beside anything.
        let (forge, ready) = queued(["`cli/**`", "", "`cli/**`"]);
        let at = pick(&ready, &running(&["**"]), || rule(&[]), &forge);
        assert_eq!(at, Some(3));
    }

    #[test]
    fn the_newest_brief_predicts_and_an_unreadable_one_predicts_nothing() {
        let mut forge = FakeForge::default();
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
    fn no_running_unit_reads_no_rule_however_many_are_ready() {
        let (forge, ready) = queued(["`cli/**`", "`cli/**`", "`docs/**`"]);
        let acts = fill(&forge, &ready, 1, 0, &Holds::default(), Vec::new, || {
            panic!("rule read")
        });
        assert_eq!(claimed(&acts), [2]);
        assert_eq!(acts.len(), 1, "{acts:?}");
    }

    #[test]
    fn one_ready_issue_beside_no_running_unit_reads_no_comments() {
        let forge = FakeForge::default();
        let ready = [issue(2, "")];
        let acts = fill(&forge, &ready, 1, 0, &Holds::default(), Vec::new, || {
            panic!("rule read")
        });
        assert_eq!(claimed(&acts), [2]);
        assert!(forge.comment_reads.borrow().is_empty());
        // An empty queue reads nothing at all.
        let none = fill(
            &forge,
            &[],
            1,
            0,
            &Holds::default(),
            || panic!("running read"),
            || panic!("rule read"),
        );
        assert!(none.is_empty());
    }

    #[test]
    fn one_ready_issue_that_overlaps_a_running_unit_is_held() {
        let (forge, _) = queued(["`cli/**`", "", ""]);
        let ready = [issue(2, "")];
        let acts = fill(
            &forge,
            &ready,
            1,
            0,
            &Holds::default(),
            || running(&["cli/**"]),
            || rule(&[]),
        );
        assert!(claimed(&acts).is_empty());
        assert!(
            matches!(acts[..], [FillAct::Held { issue: 2, .. }]),
            "{acts:?}"
        );
    }

    #[test]
    fn a_fill_reads_each_ready_issue_once_and_the_running_units_once() {
        let (forge, ready) = queued(["`cli/**`", "`cli/**`", "`docs/**`"]);
        let reads = std::cell::Cell::new(0);
        // Past the hold limit, each held issue starts in queue order once no clear one is left.
        let mut holds = Holds::default();
        holds.hold(2, 0);
        holds.hold(3, 0);
        let running = || {
            reads.set(reads.get() + 1);
            running(&["cli/**"])
        };
        let acts = fill(&forge, &ready, 3, HOLD_LIMIT_S, &holds, running, || {
            rule(&[])
        });
        assert_eq!(claimed(&acts), [4, 2, 3]);
        assert_eq!(reads.get(), 1);
        let mut seen = forge.comment_reads.borrow().clone();
        seen.sort();
        assert_eq!(seen, [2, 3, 4]);
    }

    #[test]
    fn an_issue_a_fill_started_counts_as_running_for_the_next_pick() {
        let (forge, ready) = queued(["`docs/**`", "`docs/DESIGN.md`", "`cli/src/run.rs`"]);
        let run = || running(&["cli/src/watch.rs"]);
        let acts = fill(&forge, &ready, 3, 0, &Holds::default(), run, || rule(&[]));
        assert_eq!(claimed(&acts), [2, 4]);
        assert!(
            matches!(acts[1], FillAct::Held { issue: 3, .. }),
            "{acts:?}"
        );
        assert_eq!(forge.comment_reads.borrow().len(), 3);
    }

    /// One fill at `now` with one free slot on `ready` beside a unit `1-fix-1` touching `cli/**`,
    /// with tonight's `holds`, which take the fill's new holds as the scheduler does: the issue
    /// it starts, and what else it says.
    fn fill_at(
        forge: &FakeForge,
        ready: &[Issue],
        now: i64,
        holds: &mut Holds,
    ) -> (Option<u64>, Vec<FillAct>) {
        let run = || running(&["cli/**"]);
        let acts = fill(forge, ready, 1, now, holds, run, || rule(&[]));
        let mut said = Vec::new();
        let mut at = None;
        for a in acts {
            match a {
                FillAct::Claim(i) => at = Some(i.number),
                FillAct::Held { issue, .. } => {
                    holds.hold(issue, now);
                    said.push(a);
                }
                _ => said.push(a),
            }
        }
        (at, said)
    }

    #[test]
    fn a_held_issue_is_logged_once_and_starts_anyway_after_two_hours() {
        let (forge, ready) = queued(["`cli/src/run.rs`", "`cli/**`", "`cli/src/watch.rs`"]);
        let mut holds = Holds::default();
        let (at, acts) = fill_at(&forge, &ready, 1000, &mut holds);
        assert_eq!(at, None);
        let held = |n| FillAct::Held {
            issue: n,
            overlaps: vec!["1-fix-1".into()],
        };
        assert_eq!(acts, [held(2), held(3), held(4)]);
        // A later fill short of the limit holds them again, silently.
        let (at, acts) = fill_at(&forge, &ready, 1000 + HOLD_LIMIT_S - 1, &mut holds);
        assert_eq!((at, acts.len()), (None, 0));
        // At the limit the first starts, its wait counted from when it was first held.
        let (at, acts) = fill_at(&forge, &ready, 1000 + HOLD_LIMIT_S, &mut holds);
        assert_eq!(at, Some(2));
        assert_eq!(
            acts,
            [FillAct::Forced {
                issue: 2,
                waited: HOLD_LIMIT_S,
                overlaps: vec!["1-fix-1".into()],
            }]
        );
    }

    #[test]
    fn a_clear_issue_starts_before_one_held_past_the_limit() {
        let (forge, ready) = queued(["`cli/**`", "`docs/**`", "`cli/**`"]);
        let mut holds = Holds::default();
        holds.hold(2, 0);
        let (at, acts) = fill_at(&forge, &ready, HOLD_LIMIT_S, &mut holds);
        assert_eq!(at, Some(3));
        // #2 was held already; nothing new is said of it.
        assert!(acts.is_empty(), "{acts:?}");
        // A newly held issue's wait starts now, so it is not forced.
        let (at, acts) = fill_at(&forge, &[issue(4, "")], HOLD_LIMIT_S, &mut holds);
        assert_eq!(at, None);
        assert!(
            matches!(acts[..], [FillAct::Held { issue: 4, .. }]),
            "{acts:?}"
        );
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
