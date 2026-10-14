//! Hold back a ready issue whose predicted `touches` overlap a running unit's (docs/factory/watch.md,
//! "Parallel units"): one clear of every running unit starts first, and a held issue starts
//! anyway once it has waited [`HOLD_LIMIT_S`].

use std::cell::LazyCell;
use std::collections::BTreeMap;
use std::fs;

use serde_json::{json, Value};

use crate::git::{self, Repo};
use crate::touches::{touches, Overlap};
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

/// How long a ready issue that overlaps a running unit waits before it starts anyway.
pub(in crate::watch) const HOLD_LIMIT_S: i64 = 2 * 60 * 60;

/// The ready issues held back tonight for overlapping a running unit, each with when it was first
/// held. An entry stays all night, so an issue is announced once and its wait counts from then.
#[derive(Default)]
pub(in crate::watch) struct Holds {
    since: BTreeMap<u64, i64>,
}

impl Holds {
    /// When `n` was first held, holding it from `now` if it wasn't yet: true for a new hold.
    fn hold(&mut self, n: u64, now: i64) -> (i64, bool) {
        match self.since.get(&n) {
            Some(t) => (*t, false),
            None => {
                self.since.insert(n, now);
                (now, true)
            }
        }
    }
}

/// Picks, for one fill, which ready issue starts next. Each issue's prediction is read once and
/// the overlap rule built once, when a running unit first predicts anything.
pub(super) struct Picker<'a> {
    forge: &'a dyn Forge,
    rule: LazyCell<Overlap, Box<dyn FnOnce() -> Overlap + 'a>>,
    predicted: BTreeMap<u64, Vec<String>>,
    running: Option<Vec<(String, Vec<String>)>>,
}

/// One ready issue that overlaps a running unit: where it is in `ready`, and the units it hits.
struct Blocked {
    at: usize,
    overlaps: Vec<String>,
}

impl<'a> Picker<'a> {
    pub(super) fn new(forge: &'a dyn Forge, rule: impl FnOnce() -> Overlap + 'a) -> Self {
        Picker {
            forge,
            rule: LazyCell::new(Box::new(rule)),
            predicted: BTreeMap::new(),
            running: None,
        }
    }

    /// Remove from `ready` and return the issue to start next: the first, in queue order, whose
    /// predicted `touches` overlap no running unit's, else the first held for [`HOLD_LIMIT_S`]
    /// by `now`, which starts anyway. `None` holds every one. Each new hold and each forced
    /// start goes on `events` for the run log. The running units are read through `running`
    /// once a fill; the issues taken after join them from their cached predictions.
    pub(super) fn take(
        &mut self,
        ready: &mut Vec<Issue>,
        running: impl FnOnce() -> Vec<(String, Vec<String>)>,
        now: i64,
        holds: &mut Holds,
        events: &mut Vec<Value>,
    ) -> Option<Issue> {
        if ready.is_empty() {
            return None;
        }
        let forge = self.forge;
        let running = self.running.get_or_insert_with(running);
        let at = if running.is_empty() {
            0
        } else {
            let (clear, blocked) = scan(forge, &self.rule, &mut self.predicted, ready, running);
            hold_all(ready, &blocked, now, holds, events);
            match clear {
                Some(at) => at,
                None => forced(ready, &blocked, now, holds, events)?,
            }
        };
        let issue = ready.remove(at);
        // The last one taken needs no prediction: no pick comes after it this fill.
        if ready.is_empty() {
            return Some(issue);
        }
        let mine = self
            .predicted
            .entry(issue.number)
            .or_insert_with(|| predicted(forge, &issue));
        if !mine.is_empty() {
            running.push((unit_id(&issue), mine.clone()));
        }
        Some(issue)
    }
}

/// The index in `ready` of the first issue clear of every `running` unit, if any, and each issue
/// before it that overlaps one. The rule is only built when this runs, with a running unit
/// predicting touches.
fn scan(
    forge: &dyn Forge,
    rule: &LazyCell<Overlap, Box<dyn FnOnce() -> Overlap + '_>>,
    cache: &mut BTreeMap<u64, Vec<String>>,
    ready: &[Issue],
    running: &[(String, Vec<String>)],
) -> (Option<usize>, Vec<Blocked>) {
    let rule = &**rule;
    let mut blocked = Vec::new();
    for (at, i) in ready.iter().enumerate() {
        let mine = cache.entry(i.number).or_insert_with(|| predicted(forge, i));
        let hit = overlapped(rule, mine, running);
        if hit.is_empty() {
            return (Some(at), blocked);
        }
        blocked.push(Blocked {
            at,
            overlaps: hit.into_iter().map(String::from).collect(),
        });
    }
    (None, blocked)
}

/// Hold each `blocked` issue, logging `overlap_held` for one held for the first time tonight.
fn hold_all(
    ready: &[Issue],
    blocked: &[Blocked],
    now: i64,
    holds: &mut Holds,
    events: &mut Vec<Value>,
) {
    for b in blocked {
        let n = ready[b.at].number;
        if holds.hold(n, now).1 {
            eprintln!(
                "ns watch: #{n} waits: its touches overlap running {}",
                b.overlaps.join(", ")
            );
            events.push(json!({"event": "overlap_held", "issue": n, "overlaps": b.overlaps}));
        }
    }
}

/// The first `blocked` issue held for [`HOLD_LIMIT_S`] by `now`, logged as `overlap_forced`.
fn forced(
    ready: &[Issue],
    blocked: &[Blocked],
    now: i64,
    holds: &mut Holds,
    events: &mut Vec<Value>,
) -> Option<usize> {
    let b = blocked.iter().find(|b| {
        let (since, _) = holds.hold(ready[b.at].number, now);
        now - since >= HOLD_LIMIT_S
    })?;
    let n = ready[b.at].number;
    let waited = now - holds.hold(n, now).0;
    eprintln!(
        "ns watch: #{n} starts beside {} after waiting {waited}s",
        b.overlaps.join(", ")
    );
    events.push(json!({
        "event": "overlap_forced",
        "issue": n,
        "waited_s": waited,
        "overlaps": b.overlaps,
    }));
    Some(b.at)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
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

    /// The issue a fill on `ready` starts first at time 0, with no issue held before.
    fn pick(
        ready: &[Issue],
        running: &[(String, Vec<String>)],
        make: impl FnOnce() -> Overlap,
        forge: &FakeForge,
    ) -> Option<u64> {
        let mut ready = ready.to_vec();
        let mut picker = Picker::new(forge, make);
        let (mut holds, mut events) = (Holds::default(), Vec::new());
        picker
            .take(&mut ready, || running.to_vec(), 0, &mut holds, &mut events)
            .map(|i| i.number)
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

    fn take_all(
        forge: &FakeForge,
        ready: &mut Vec<Issue>,
        running: &[(String, Vec<String>)],
    ) -> Vec<u64> {
        let mut picker = Picker::new(forge, || rule(&[]));
        let (mut holds, mut events) = (Holds::default(), Vec::new());
        let mut order = Vec::new();
        while let Some(i) = picker.take(ready, || running.to_vec(), 0, &mut holds, &mut events) {
            order.push(i.number);
        }
        order
    }

    #[test]
    fn no_running_unit_reads_no_rule_however_many_are_ready() {
        let (forge, mut ready) = queued(["`cli/**`", "`cli/**`", "`docs/**`"]);
        let mut picker = Picker::new(&forge, || panic!("rule read"));
        let (mut holds, mut events) = (Holds::default(), Vec::new());
        let first = picker.take(&mut ready, Vec::new, 0, &mut holds, &mut events);
        assert_eq!(first.map(|i| i.number), Some(2));
        assert!(events.is_empty());
    }

    #[test]
    fn one_ready_issue_beside_no_running_unit_reads_no_comments() {
        let forge = FakeForge::default();
        let mut ready = vec![issue(2, "")];
        let mut picker = Picker::new(&forge, || panic!("rule read"));
        let (mut holds, mut events) = (Holds::default(), Vec::new());
        let at = picker.take(&mut ready, Vec::new, 0, &mut holds, &mut events);
        assert_eq!(at.map(|i| i.number), Some(2));
        assert!(forge.comment_reads.borrow().is_empty());
        // An empty queue reads nothing at all.
        let none = picker.take(
            &mut ready,
            || panic!("running read"),
            0,
            &mut holds,
            &mut events,
        );
        assert!(none.is_none());
    }

    #[test]
    fn one_ready_issue_that_overlaps_a_running_unit_is_held() {
        let (forge, _) = queued(["`cli/**`", "", ""]);
        let mut ready = vec![issue(2, "")];
        let mut picker = Picker::new(&forge, || rule(&[]));
        let (mut holds, mut events) = (Holds::default(), Vec::new());
        let at = picker.take(
            &mut ready,
            || running(&["cli/**"]),
            0,
            &mut holds,
            &mut events,
        );
        assert!(at.is_none());
        assert_eq!(ready.len(), 1);
    }

    #[test]
    fn a_fill_reads_each_ready_issue_once_and_the_running_units_once() {
        let (forge, mut ready) = queued(["`cli/**`", "`cli/**`", "`docs/**`"]);
        let reads = std::cell::Cell::new(0);
        let mut picker = Picker::new(&forge, || rule(&[]));
        let (mut holds, mut events) = (Holds::default(), Vec::new());
        let mut order = Vec::new();
        // Past the hold limit, each held issue starts in queue order once no clear one is left.
        let late = HOLD_LIMIT_S;
        holds.hold(2, 0);
        holds.hold(3, 0);
        loop {
            let running = || {
                reads.set(reads.get() + 1);
                running(&["cli/**"])
            };
            match picker.take(&mut ready, running, late, &mut holds, &mut events) {
                Some(i) => order.push(i.number),
                None => break,
            }
        }
        assert_eq!(order, [4, 2, 3]);
        assert_eq!(reads.get(), 1);
        let mut seen = forge.comment_reads.borrow().clone();
        seen.sort();
        assert_eq!(seen, [2, 3, 4]);
    }

    #[test]
    fn an_issue_a_fill_started_counts_as_running_for_the_next_pick() {
        let (forge, mut ready) = queued(["`docs/**`", "`docs/DESIGN.md`", "`cli/src/run.rs`"]);
        let order = take_all(&forge, &mut ready, &running(&["cli/src/watch.rs"]));
        assert_eq!(order, [2, 4]);
        assert_eq!(ready[0].number, 3);
        assert_eq!(forge.comment_reads.borrow().len(), 3);
    }

    /// One fill at `now` on `ready` beside a unit `1-fix-1` touching `cli/**`, with tonight's
    /// `holds`: the issue it starts, and the events it logs.
    fn fill_at(
        forge: &FakeForge,
        ready: &[Issue],
        now: i64,
        holds: &mut Holds,
    ) -> (Option<u64>, Vec<Value>) {
        let mut ready = ready.to_vec();
        let mut events = Vec::new();
        let mut picker = Picker::new(forge, || rule(&[]));
        let run = || running(&["cli/**"]);
        let at = picker.take(&mut ready, run, now, holds, &mut events);
        (at.map(|i| i.number), events)
    }

    #[test]
    fn a_held_issue_is_logged_once_and_starts_anyway_after_two_hours() {
        let (forge, ready) = queued(["`cli/src/run.rs`", "`cli/**`", "`cli/src/watch.rs`"]);
        let mut holds = Holds::default();
        let (at, events) = fill_at(&forge, &ready, 1000, &mut holds);
        assert_eq!(at, None);
        let held: Vec<_> = events.iter().map(|e| (&e["event"], &e["issue"])).collect();
        assert_eq!(held.len(), 3, "{events:?}");
        assert!(held.iter().all(|(e, _)| *e == "overlap_held"));
        assert_eq!(events[0]["overlaps"], json!(["1-fix-1"]));
        // A later fill short of the limit holds them again, silently.
        let (at, events) = fill_at(&forge, &ready, 1000 + HOLD_LIMIT_S - 1, &mut holds);
        assert_eq!((at, events.len()), (None, 0));
        // At the limit the first starts, its wait counted from when it was first held.
        let (at, events) = fill_at(&forge, &ready, 1000 + HOLD_LIMIT_S, &mut holds);
        assert_eq!(at, Some(2));
        assert_eq!(
            events,
            [json!({
                "event": "overlap_forced",
                "issue": 2,
                "waited_s": HOLD_LIMIT_S,
                "overlaps": ["1-fix-1"],
            })]
        );
    }

    #[test]
    fn a_clear_issue_starts_before_one_held_past_the_limit() {
        let (forge, ready) = queued(["`cli/**`", "`docs/**`", "`cli/**`"]);
        let mut holds = Holds::default();
        holds.hold(2, 0);
        let (at, events) = fill_at(&forge, &ready, HOLD_LIMIT_S, &mut holds);
        assert_eq!(at, Some(3));
        // #2 was held already; nothing new is said of it.
        assert!(events.is_empty(), "{events:?}");
        // A newly held issue's wait starts now, so it is not forced.
        let (at, events) = fill_at(&forge, &[issue(4, "")], HOLD_LIMIT_S, &mut holds);
        assert_eq!(at, None);
        assert_eq!(events[0]["event"], "overlap_held");
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
