//! `ns watch`: pull ready issues from GitHub and run them one at a time (docs/FACTORY.md).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::clock;
use crate::error::SfError;
use crate::factory::{Factory, Queue as QueueConfig};
use crate::git::{self, Repo};
use crate::install;
use crate::review_md::{self, Status};
use crate::run::{self, gh, gh_json, Outcome, RunArgs, RunResult, Shared};
use crate::skills_sync::{self, Sync};
use crate::worktree::BRANCH_PREFIX;

mod triage;

pub struct WatchArgs {
    pub once: bool,
    pub until: Option<String>,
    pub max_units: Option<u32>,
    pub dry_run: bool,
    pub factory: Option<PathBuf>,
}

pub const DISCLAIMER: &str =
    "_This comment was written by an AI agent (nightshift `ns watch`), not a person._";

/// Seconds to wait after a usage limit with no reset time in the message.
const PAUSE_RETRY_S: i64 = 30 * 60;

#[derive(Debug, Clone)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    pub body: String,
    pub team: bool,
}

/// Author associations whose issues `ns watch` will run (docs/FACTORY.md, Trust).
const TEAM: [&str; 3] = ["OWNER", "MEMBER", "COLLABORATOR"];

/// One issue per line, as printed by the `--jq` filter in `queue()`.
fn parse_issues(text: &str) -> Vec<Issue> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|i| Issue {
            number: i["number"].as_u64().unwrap_or(0),
            title: i["title"].as_str().unwrap_or("").to_string(),
            labels: i["labels"]
                .as_array()
                .map(|l| {
                    l.iter()
                        .filter_map(|x| x["name"].as_str().or(x.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            body: i["body"].as_str().unwrap_or("").to_string(),
            team: TEAM.contains(&i["authorAssociation"].as_str().unwrap_or("")),
        })
        .filter(|i| i.number > 0)
        .collect()
}

/// Issue numbers named on a first line `Blocked by: #a, #b`.
pub fn blockers(body: &str) -> Vec<u64> {
    let first = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let lower = first.to_lowercase();
    let Some(rest) = lower.strip_prefix("blocked by:") else {
        return Vec::new();
    };
    let re = regex::Regex::new(r"#(\d+)").unwrap();
    re.captures_iter(rest)
        .filter_map(|c| c[1].parse().ok())
        .collect()
}

/// Issues an open PR body closes (`Closes #n`, also fixes/resolves).
pub fn closed_by(body: &str) -> Vec<u64> {
    let re =
        regex::Regex::new(r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(\d+)").unwrap();
    re.captures_iter(body)
        .filter_map(|c| c[1].parse().ok())
        .collect()
}

fn rank(order: &[String], labels: &[String]) -> usize {
    order
        .iter()
        .position(|o| labels.contains(o))
        .unwrap_or(order.len())
}

struct Queue {
    ready: Vec<Issue>,
    skipped: Vec<Value>,
}

/// Issue number -> the PR whose body closes it, from `gh pr list --state <state>`.
fn closing_prs(root: &Path, state: &str) -> Result<BTreeMap<u64, u64>> {
    let prs = gh_json(
        root,
        &[
            "pr",
            "list",
            "--state",
            state,
            "--limit",
            "200",
            "--json",
            "number,body",
        ],
    )?;
    let mut by_issue = BTreeMap::new();
    for p in prs.as_array().cloned().unwrap_or_default() {
        for n in closed_by(p["body"].as_str().unwrap_or("")) {
            by_issue.insert(n, p["number"].as_u64().unwrap_or(0));
        }
    }
    Ok(by_issue)
}

/// Open issues, never PRs, and the open and merged PRs that close each.
struct Listing {
    issues: Vec<Issue>,
    has_pr: BTreeMap<u64, u64>,
    merged_pr: BTreeMap<u64, u64>,
}

impl Listing {
    /// `labels` narrows the list to issues carrying that label.
    fn read(root: &Path, labels: Option<&str>) -> Result<Listing> {
        let filter = labels.map_or(String::new(), |l| format!("&labels={l}"));
        let path = format!("repos/{{owner}}/{{repo}}/issues?state=open{filter}&per_page=100");
        // `gh issue list --json` has no author association, so read the REST list.
        let issues = parse_issues(&gh(
            root,
            &[
                "api",
                "--paginate",
                &path,
                "--jq",
                ".[] | select(.pull_request | not) | {number, title, labels, body, authorAssociation: .author_association}",
            ],
        )?);
        Ok(Listing {
            issues,
            has_pr: closing_prs(root, "open")?,
            merged_pr: closing_prs(root, "merged")?,
        })
    }

    /// Why `ns watch` leaves the issue alone: its author is outside the team, or a PR closes it.
    fn passed_over(&self, i: &Issue) -> Option<String> {
        if !i.team {
            return Some("author outside the team".into());
        }
        if let Some(pr) = self.has_pr.get(&i.number) {
            return Some(format!("open PR #{pr} closes it"));
        }
        self.merged_pr
            .get(&i.number)
            .map(|pr| format!("merged PR #{pr} closes it"))
    }
}

fn skip(i: &Issue, reason: impl Into<Value>) -> Value {
    json!({"number": i.number, "title": i.title, "reason": reason.into()})
}

/// The queue's sort key: higher priority first, then category, then the oldest issue.
fn order(q: &QueueConfig, i: &Issue) -> (usize, usize, u64) {
    (
        rank(&q.priority, &i.labels),
        rank(&q.order, &i.labels),
        i.number,
    )
}

/// Ready issues, minus those in `finished`: GitHub can list an issue as open
/// for a few seconds after its closing PR merges.
fn queue(root: &Path, fac: &Factory, finished: &BTreeSet<u64>) -> Result<Queue> {
    let q = &fac.queue;
    let listing = Listing::read(root, Some(&q.ready_label))?;
    let mut open_cache: BTreeMap<u64, bool> = BTreeMap::new();
    let mut ready = Vec::new();
    let mut skipped = Vec::new();
    for i in &listing.issues {
        if finished.contains(&i.number) {
            continue;
        }
        if let Some(why) = listing.passed_over(i) {
            skipped.push(skip(i, why));
            continue;
        }
        let mut open_blockers = Vec::new();
        for b in blockers(&i.body) {
            let open = match open_cache.get(&b) {
                Some(o) => *o,
                None => {
                    let st = gh_json(root, &["issue", "view", &b.to_string(), "--json", "state"])
                        .ok()
                        .and_then(|v| v["state"].as_str().map(String::from));
                    // An unreadable blocker counts as open: never start work that may be blocked.
                    let o = st.as_deref() != Some("CLOSED");
                    open_cache.insert(b, o);
                    o
                }
            };
            if open {
                open_blockers.push(format!("#{b}"));
            }
        }
        if !open_blockers.is_empty() {
            skipped.push(skip(
                i,
                format!("blocked by open {}", open_blockers.join(", ")),
            ));
            continue;
        }
        ready.push(i.clone());
    }
    ready.sort_by_key(|i| order(q, i));
    Ok(Queue { ready, skipped })
}

const LABEL_EDIT_ATTEMPTS: u32 = 2;

struct StatusDrift {
    stray: Vec<String>,
    missing: bool,
}

impl StatusDrift {
    fn is_clean(&self) -> bool {
        self.stray.is_empty() && !self.missing
    }
}

/// A state label: any `status:` label, or one of the queue's own.
fn is_status(q: &QueueConfig, label: &str) -> bool {
    label.starts_with("status:")
        || [
            &q.ready_label,
            &q.in_progress_label,
            &q.done_label,
            &q.stuck_label,
            &q.triage_label,
        ]
        .iter()
        .any(|o| *o == label)
}

fn labels(root: &Path, n: &str) -> Result<Vec<String>> {
    let v = gh_json(root, &["issue", "view", n, "--json", "labels"])?;
    Ok(v["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str().map(String::from))
        .collect())
}

fn status_drift(root: &Path, q: &QueueConfig, n: &str, end: Option<&str>) -> Result<StatusDrift> {
    let labels = labels(root, n)?;
    let stray = labels
        .iter()
        .filter(|l| Some(l.as_str()) != end && is_status(q, l))
        .cloned()
        .collect();
    Ok(StatusDrift {
        stray,
        missing: end.is_some_and(|e| !labels.iter().any(|l| l == e)),
    })
}

fn set_status(root: &Path, q: &QueueConfig, n: u64, end: Option<&str>) -> Result<()> {
    let ns = n.to_string();
    for _ in 0..LABEL_EDIT_ATTEMPTS {
        let drift = status_drift(root, q, &ns, end)?;
        if drift.is_clean() {
            return Ok(());
        }
        let mut args = vec!["issue", "edit", ns.as_str()];
        for l in &drift.stray {
            args.extend(["--remove-label", l.as_str()]);
        }
        if let Some(e) = end.filter(|_| drift.missing) {
            args.extend(["--add-label", e]);
        }
        gh(root, &args)?;
    }
    let drift = status_drift(root, q, &ns, end)?;
    if drift.is_clean() {
        return Ok(());
    }
    let mut problems = Vec::new();
    if !drift.stray.is_empty() {
        problems.push(format!(
            "has stray status labels {}",
            drift.stray.join(", ")
        ));
    }
    if let Some(e) = end.filter(|_| drift.missing) {
        problems.push(format!("is missing {e}"));
    }
    Err(SfError::general(format!(
        "#{n} {} after {LABEL_EDIT_ATTEMPTS} label edits (want {})",
        problems.join(" and "),
        end.unwrap_or("no status label")
    ))
    .into())
}

fn comment(root: &Path, n: u64, body: &str) -> Result<()> {
    gh(root, &["issue", "comment", &n.to_string(), "--body", body]).map(|_| ())
}

/// `git fetch origin` and the `origin/<default>` ref to base new worktrees on.
fn fresh_base(root: &Path) -> Option<String> {
    let (branch, _) = run::remote_default(root)?;
    git::run(root, &["fetch", "--quiet", "origin"]).ok()?;
    Some(format!("origin/{branch}"))
}

/// Fast-forward the main checkout when installed skills resolve into it, so the unit runs
/// the skills merged so far tonight. A checkout that can't move is reported, not changed;
/// `warned` holds the last warning so a checkout that stays stuck warns once.
fn sync_skills(repo: &Repo, base: &str, issue: u64, warned: &mut Option<String>) {
    let targets = install::default_targets();
    let installed = skills_sync::installed_into(&targets, &repo.root);
    let short =
        |sha: &str| git::run(&repo.root, &["rev-parse", "--short", sha]).unwrap_or_default();
    let mut ev = json!({"issue": issue, "checkout": repo.root, "skills": installed.names});
    match skills_sync::sync(&repo.root, base, &installed) {
        Sync::NotInstalled | Sync::Current => return,
        Sync::Updated { from, to } => {
            eprintln!(
                "ns watch: skills checkout {} fast-forwarded {} -> {}",
                repo.root.display(),
                short(&from),
                short(&to)
            );
            ev["event"] = json!("skills_synced");
            ev["from"] = json!(from);
            ev["to"] = json!(to);
            match skills_sync::relink(&installed, &targets) {
                Ok(changed) => {
                    for c in &changed {
                        eprintln!("ns watch: skills {c}");
                    }
                    ev["relinked"] = json!(changed);
                }
                Err(e) => {
                    eprintln!("ns watch: warning: could not relink skills: {e:#}");
                    ev["relink_error"] = json!(format!("{e:#}"));
                }
            }
            *warned = None;
        }
        Sync::Stale { reason } => {
            if warned.as_deref() == Some(reason.as_str()) {
                return;
            }
            eprintln!(
                "ns watch: warning: installed skills may be stale ({}): {reason}",
                installed.names.join(", ")
            );
            ev["event"] = json!("skills_stale");
            ev["reason"] = json!(reason);
            *warned = Some(reason);
        }
    }
    run::log_event(&repo.common_dir, ev);
}

pub fn run(args: WatchArgs) -> Result<ExitCode> {
    let start = std::env::current_dir().context("cannot read current directory")?;
    let repo = Repo::discover(&start)?;
    // Read once: a file broken between units must not end the night.
    let loaded = run::Loaded::read(args.factory.as_deref())?;
    let fac = &loaded.fac;
    for (what, f) in [
        ("config", &loaded.files["config"]),
        ("factory", &loaded.files["factory"]),
    ] {
        eprintln!(
            "ns watch: {what} {} sha256 {}",
            f["path"].as_str().unwrap_or(""),
            f["sha256"].as_str().unwrap_or("none (file missing)")
        );
    }
    let q = fac.queue.clone();
    let mut night = Night::new();
    let deadline = match &args.until {
        Some(s) => {
            let Some((h, m)) = clock::parse_hm(s) else {
                return Err(SfError::usage(
                    format!("--until {s:?} is not HH:MM"),
                    "ns watch --until 06:30",
                )
                .into());
            };
            Some(clock::next_local(night.shared.clock.now(), h, m))
        }
        None => None,
    };
    let max_units = if args.once {
        Some(1)
    } else {
        args.max_units.or(fac.limits.max_units)
    };

    if args.dry_run {
        let qu = queue(&repo.root, fac, &BTreeSet::new())?;
        let ready: Vec<Value> = qu
            .ready
            .iter()
            .map(|i| {
                json!({
                    "number": i.number,
                    "title": i.title,
                    "priority_label": q.priority.get(rank(&q.priority, &i.labels)),
                    "order_label": q.order.get(rank(&q.order, &i.labels)),
                })
            })
            .collect();
        // With the pass off, nothing would be triaged, so list nothing.
        let tr = match triage::cap(fac) {
            0 => triage::Candidates::default(),
            _ => triage::candidates(&repo.root, fac, &BTreeSet::new())?,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "queue": ready,
                "skipped": qu.skipped,
                "triage": tr.issues.iter().map(|(i, why)| json!({"number": i.number, "title": i.title, "reason": why})).collect::<Vec<_>>(),
                "triage_skipped": tr.skipped,
                "triage_per_night": triage::cap(fac),
                "max_units": max_units,
                "until": deadline.map(clock::local_iso),
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }

    let mut units: Vec<Value> = Vec::new();
    let mut started = 0u32;
    let stopped: String = 'outer: loop {
        if max_units.is_some_and(|m| started >= m) {
            break "max_units".into();
        }
        if let Some(stop) = night.over(deadline, fac) {
            break stop.into();
        }
        if let Some(stop) = triage::pass(&repo, &loaded, deadline, &mut night)? {
            break stop;
        }
        let qu = queue(&repo.root, fac, &night.finished)?;
        let Some(issue) = qu.ready.first().cloned() else {
            break "queue empty".into();
        };
        started += 1;
        set_status(&repo.root, &q, issue.number, Some(&q.in_progress_label))?;
        eprintln!("ns watch: #{} {}", issue.number, issue.title);
        loop {
            let rargs = RunArgs {
                issue: Some(issue.number),
                base: night.base(&repo, issue.number),
                ..RunArgs::default()
            };
            let r = match run::execute(&rargs, &mut night.shared, &loaded) {
                Ok(r) => r,
                Err(e) => {
                    let _ = set_status(&repo.root, &q, issue.number, Some(&q.ready_label));
                    return Err(e);
                }
            };
            let mut rec = json!({
                "issue": issue.number,
                "unit": r.unit,
                "outcome": r.outcome.label(),
                "reason": r.reason,
                "pr": r.json["pr"],
                "cost_usd": r.cost_usd,
            });
            let failing = night.harness(&r);
            match r.outcome {
                Outcome::Merged => {
                    let status = set_status(&repo.root, &q, issue.number, None);
                    // GitHub may not have closed it yet; an already-closed issue makes gh fail.
                    let n = issue.number.to_string();
                    if let Err(e) = gh(&repo.root, &["issue", "close", &n, "--reason", "completed"])
                    {
                        eprintln!("ns watch: #{n} not closed: {e}");
                    }
                    status?;
                }
                Outcome::Done if !r.needs_human => {
                    set_status(&repo.root, &q, issue.number, Some(&q.done_label))?;
                }
                Outcome::Stuck if failing => {
                    set_status(&repo.root, &q, issue.number, Some(&q.ready_label))?;
                    rec["outcome"] = json!("harness_failing");
                    if night.harness_fails >= HARNESS_FAIL_LIMIT {
                        units.push(rec);
                        break 'outer "harness failing".into();
                    }
                }
                Outcome::Done | Outcome::Stuck => {
                    let status = set_status(&repo.root, &q, issue.number, Some(&q.stuck_label));
                    let what = if r.outcome == Outcome::Stuck {
                        "got stuck"
                    } else {
                        "needs a human"
                    };
                    let artifact = r
                        .artifact
                        .as_deref()
                        .and_then(|a| Path::new(a).file_name())
                        .map_or("none".to_string(), |f| {
                            format!(
                                "`.ns/{0}/{1}` on branch `{BRANCH_PREFIX}{0}`",
                                r.unit,
                                f.to_string_lossy()
                            )
                        });
                    let findings = r
                        .artifact
                        .as_deref()
                        .filter(|a| Path::new(a).file_name().is_some_and(|f| f == "review.md"))
                        .and_then(|a| std::fs::read_to_string(a).ok())
                        .map(|text| open_findings(&text))
                        .filter(|f| !f.is_empty())
                        .map_or(String::new(), |f| {
                            format!(
                                "\n\nOpen findings in `review.md`:\n```text\n{}\n```",
                                f.join("\n")
                            )
                        });
                    let body = format!(
                        "nightshift {what} on unit `{}`: {}{findings}\n\nLast artifact: {artifact}\n\n{DISCLAIMER}",
                        r.unit, r.reason
                    );
                    let posted = comment(&repo.root, issue.number, &body);
                    status?;
                    posted?;
                }
                Outcome::Budget => {
                    set_status(&repo.root, &q, issue.number, Some(&q.ready_label))?;
                    units.push(rec);
                    break 'outer "budget".into();
                }
                Outcome::Paused => {
                    let reset = night.resume_at(&r);
                    rec["reset_at"] = json!(clock::local_iso(reset));
                    units.push(rec);
                    if deadline.is_some_and(|d| reset >= d) {
                        set_status(&repo.root, &q, issue.number, Some(&q.ready_label))?;
                        break 'outer PAUSED_PAST_UNTIL.into();
                    }
                    night.sleep_until(reset);
                    continue;
                }
            }
            units.push(rec);
            break;
        }
        night.finished.insert(issue.number);
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "units": units,
            "triaged": night.triage.records,
            "stopped": stopped,
            "until": deadline.map(clock::local_iso),
            "cost_usd": night.shared.spent_usd,
            "started_with": loaded.files,
        }))?
    );
    Ok(ExitCode::SUCCESS)
}

const PAUSED_PAST_UNTIL: &str = "usage limit resets after --until";

/// What one `ns watch` carries from run to run.
struct Night {
    shared: Shared,
    /// Consecutive runs, units or triage-only, whose every attempt failed instantly: the
    /// harness, not the work.
    harness_fails: u32,
    /// Issues whose unit ended tonight.
    finished: BTreeSet<u64>,
    skills_warned: Option<String>,
    triage: triage::Tally,
}

impl Night {
    fn new() -> Night {
        let mut shared = Shared::new();
        shared.watch_pid = Some(std::process::id());
        Night {
            shared,
            harness_fails: 0,
            finished: BTreeSet::new(),
            skills_warned: None,
            triage: triage::Tally::default(),
        }
    }

    /// Why the night ends before the next run: `--until` has passed or the budget is spent.
    fn over(&self, deadline: Option<i64>, fac: &Factory) -> Option<&'static str> {
        if deadline.is_some_and(|d| self.shared.clock.now() >= d) {
            return Some("until");
        }
        fac.budget_usd()
            .filter(|b| self.shared.spent_usd >= *b)
            .map(|_| "budget")
    }

    /// `git fetch origin`, then keep installed skills current; the base for the run's worktree.
    fn base(&mut self, repo: &Repo, issue: u64) -> Option<String> {
        let base = fresh_base(&repo.root);
        if let Some(b) = &base {
            sync_skills(repo, b, issue, &mut self.skills_warned);
        }
        base
    }

    /// Count a finished run toward the harness breaker; true when the harness is failing.
    fn harness(&mut self, r: &RunResult) -> bool {
        let failing = r.outcome == Outcome::Stuck && harness_failing(&r.json);
        self.harness_fails = if failing { self.harness_fails + 1 } else { 0 };
        failing
    }

    /// When a paused run can resume: its reset time, or a retry later when it gave none.
    fn resume_at(&self, r: &RunResult) -> i64 {
        let now = self.shared.clock.now();
        r.reset_at
            .filter(|t| *t > now)
            .unwrap_or(now + PAUSE_RETRY_S)
    }

    fn sleep_until(&mut self, reset: i64) {
        eprintln!(
            "ns watch: usage limit, sleeping until {}",
            clock::local_iso(reset)
        );
        self.shared.clock.sleep_until(reset);
    }
}

/// The Critical and Important findings in changed code `review.md` leaves open, one
/// `<id>. <title> (<location>)` line each, so the stuck comment tells a human what is left to
/// finish. A pre-existing finding is an escape filed elsewhere (D29), not the unit's to finish.
fn open_findings(review: &str) -> Vec<String> {
    review_md::parse(review)
        .into_iter()
        .filter(|f| f.against_unit() && f.status == Status::Open)
        .map(|f| match f.location {
            Some(l) => format!("{}. {} ({l})", f.id, f.title),
            None => format!("{}. {}", f.id, f.title),
        })
        .collect()
}

/// Two units in a row that fail like this stop the night instead of draining the queue.
const HARNESS_FAIL_LIMIT: u32 = 2;

/// Every attempt exited non-zero within seconds, having produced no output tokens:
/// the harness itself is failing (an unrecognised usage limit, an auth or network
/// error), not the work.
fn harness_failing(run: &Value) -> bool {
    let Some(phases) = run["phases"].as_array() else {
        return false;
    };
    !phases.is_empty()
        && phases.iter().all(|p| {
            p["exit"].as_i64().is_some_and(|c| c != 0)
                && p["output_tokens"].as_u64().unwrap_or(0) == 0
                && p["wall_s"].as_f64().unwrap_or(f64::MAX) < 30.0
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blockers_and_closers() {
        assert_eq!(blockers("Blocked by: #3, #12\nmore"), [3, 12]);
        assert_eq!(blockers("\nblocked by: #4"), [4]);
        assert!(blockers("text\nBlocked by: #3").is_empty());
        assert_eq!(closed_by("Closes #5. Also fixes #6"), [5, 6]);
        assert!(closed_by("see #5").is_empty());
    }

    #[test]
    fn open_findings_keeps_open_criticals_and_importants() {
        let review = "\
Open after 3 fix cycles: C1, I2, I4.

### C1. Drops every `SKU`
- Location: `a.py:1`
- Status: open
### I1. Done already
- Location: `a.py:2`
- Status: dismissed: noted: open question for later
### I2. No location given
- Status: open. The fix did not land.
### I4. Formatted loosely
- **Location:** `a.py:4`
- **Status:** Open
### I5. Location after status
- Status: open
- Location: `a.py:5`
### I6. Closed by a section
## Suggestion
- Status: open
### S1. A suggestion
- Location: `a.py:3`
- Status: open
### Cycle 3 notes
- Status: open
";
        assert_eq!(
            open_findings(review),
            [
                "C1. Drops every SKU (a.py:1)",
                "I2. No location given",
                "I4. Formatted loosely (a.py:4)",
                "I5. Location after status (a.py:5)"
            ]
        );
        assert!(open_findings("no findings here").is_empty());
    }

    #[test]
    fn open_findings_reads_packed_one_line_and_range_forms() {
        let review = "\
## Important
### I1. Packed fields
- Location: `a.rs:585-605`. Raised by: architecture. Fix: split it. Status: open
### I2-I3 (cycle 1). Grouped
- Status: fixed (cycle 1, abc)
- I4. Two-line bullet inside a heading finding is not its own finding
## Critical
- C1. One-line finding (security). Open.
- C2. One-line finding, fixed in cycle 2 (correctness). Fixed.
### C3. An escape
- Scope: pre-existing
- Status: open
```
### I9. In a code fence
- Status: open
```
";
        assert_eq!(
            open_findings(review),
            [
                "I1. Packed fields (a.rs:585-605)",
                "C1. One-line finding (security). Open."
            ]
        );
    }

    #[test]
    fn ranks_by_first_matching_order_label() {
        let order: Vec<String> = vec!["type:fix".into(), "type:feat".into()];
        assert_eq!(rank(&order, &["type:feat".into()]), 1);
        assert_eq!(rank(&order, &["area:cli".into()]), 2);
    }
}
