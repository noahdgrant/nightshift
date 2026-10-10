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
use crate::stop;
use crate::worktree::BRANCH_PREFIX;

mod snapshot;
mod triage;

pub use snapshot::NightDir;

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

/// Issue number -> the PR whose body closes it and that PR's head branch, from
/// `gh pr list --state <state>`.
fn closing_prs(root: &Path, state: &str) -> Result<BTreeMap<u64, (u64, String)>> {
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
            "number,body,headRefName,isCrossRepository",
        ],
    )?;
    let mut by_issue = BTreeMap::new();
    for p in prs.as_array().cloned().unwrap_or_default() {
        for n in closed_by(p["body"].as_str().unwrap_or("")) {
            // A fork's branch name says nothing about this repo's units.
            let head = match p["isCrossRepository"].as_bool() {
                Some(false) => p["headRefName"].as_str().unwrap_or("").to_string(),
                _ => String::new(),
            };
            by_issue.insert(n, (p["number"].as_u64().unwrap_or(0), head));
        }
    }
    Ok(by_issue)
}

/// Open issues, never PRs, and the open and merged PRs that close each.
struct Listing {
    issues: Vec<Issue>,
    has_pr: BTreeMap<u64, (u64, String)>,
    merged_pr: BTreeMap<u64, (u64, String)>,
}

/// Open issues, never PRs; `labels` narrows the list to issues carrying that label.
fn open_issues(root: &Path, labels: Option<&str>) -> Result<Vec<Issue>> {
    let filter = labels.map_or(String::new(), |l| format!("&labels={l}"));
    let path = format!("repos/{{owner}}/{{repo}}/issues?state=open{filter}&per_page=100");
    // `gh issue list --json` has no author association, so read the REST list.
    Ok(parse_issues(&gh(
        root,
        &[
            "api",
            "--paginate",
            &path,
            "--jq",
            ".[] | select(.pull_request | not) | {number, title, labels, body, authorAssociation: .author_association}",
        ],
    )?))
}

impl Listing {
    /// `labels` narrows the list to issues carrying that label.
    fn read(root: &Path, labels: Option<&str>) -> Result<Listing> {
        let issues = open_issues(root, labels)?;
        Ok(Listing {
            issues,
            has_pr: closing_prs(root, "open")?,
            merged_pr: closing_prs(root, "merged")?,
        })
    }

    /// Why `ns watch` leaves the issue alone: its author is outside the team, or a PR closes it.
    /// With `resume_own`, an open PR from the issue's own unit branch doesn't count: that unit
    /// stopped before its merge step ended, and `ns run` resumes it there.
    fn passed_over(&self, i: &Issue, resume_own: bool) -> Option<String> {
        if !i.team {
            return Some("author outside the team".into());
        }
        if let Some((pr, head)) = self.has_pr.get(&i.number) {
            if !(resume_own && *head == unit_branch(i)) {
                return Some(format!("open PR #{pr} closes it"));
            }
        }
        self.merged_pr
            .get(&i.number)
            .map(|(pr, _)| format!("merged PR #{pr} closes it"))
    }
}

/// The branch of the unit `ns run --issue <n>` runs for the issue: `ns/<n>-<slug of the title>`,
/// or `ns/<n>` for a title with no slug.
fn unit_branch(i: &Issue) -> String {
    match run::slug(&i.title) {
        s if s.is_empty() => format!("{BRANCH_PREFIX}{}", i.number),
        s => format!("{BRANCH_PREFIX}{}-{s}", i.number),
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
        if let Some(why) = listing.passed_over(i, true) {
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
            &q.split_label,
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

/// Open issues carrying `in_progress_label`: those no live `ns run` holds, left so by a night
/// that ended mid-unit, and those one does, each with its run lock's holder.
struct InProgress {
    stale: Vec<Issue>,
    held: Vec<(Issue, Value)>,
}

fn in_progress(repo: &Repo, q: &QueueConfig) -> Result<InProgress> {
    let holders = run::Lock::run_holders(&repo.common_dir)?;
    let mut found = InProgress {
        stale: Vec::new(),
        held: Vec::new(),
    };
    for i in open_issues(&repo.root, Some(&q.in_progress_label))? {
        if !i.labels.contains(&q.in_progress_label) {
            continue;
        }
        match holders.iter().find(|h| holds(h, i.number)) {
            Some(h) => found.held.push((i, h.clone())),
            None => found.stale.push(i),
        }
    }
    found.stale.sort_by_key(|i| i.number);
    Ok(found)
}

/// Whether a run lock's `holder` is running issue `n`'s unit.
fn holds(holder: &Value, n: u64) -> bool {
    if holder["issue"].as_u64() == Some(n) {
        return true;
    }
    holder["unit"]
        .as_str()
        .is_some_and(|u| u == n.to_string() || u.starts_with(&format!("{n}-")))
}

/// Put each stale in-progress issue back to `ready_label`, keeping its worktree and artifacts so
/// its unit resumes, and push its number onto `requeued`.
fn requeue_stale(repo: &Repo, q: &QueueConfig, requeued: &mut Vec<u64>) -> Result<()> {
    let found = in_progress(repo, q)?;
    for (i, h) in &found.held {
        eprintln!(
            "ns watch: #{} is held by a live ns run (pid {}, unit {}); left in progress",
            i.number,
            run::holder_pid(h),
            h["unit"].as_str().unwrap_or("?")
        );
    }
    for i in &found.stale {
        set_status(&repo.root, q, i.number, Some(&q.ready_label))?;
        eprintln!(
            "ns watch: #{} was in progress with no live ns run; back to {}",
            i.number, q.ready_label
        );
        run::log_event(
            &repo.common_dir,
            json!({"event": "requeued", "issue": i.number, "reason": "in progress with no live ns run"}),
        );
        requeued.push(i.number);
    }
    Ok(())
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
    let loaded = run::Loaded::read(args.factory.as_deref(), args.dry_run)?;
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
    night.shared.until = deadline;
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
        let tr = if triage::on(fac) {
            triage::candidates(&repo.root, fac, &BTreeSet::new())?
        } else {
            triage::Candidates::default()
        };
        let requeue: Vec<u64> = in_progress(&repo, &q)?
            .stale
            .iter()
            .map(|i| i.number)
            .collect();
        // The night's first pass treats every candidate as backlog: it takes them in order
        // while no issue is ready, and none while one is. The night requeues stale issues
        // before that pass, so they count as ready.
        let next_pass: Vec<u64> = if qu.ready.is_empty() && requeue.is_empty() {
            tr.issues.iter().map(|(i, _)| i.number).collect()
        } else {
            Vec::new()
        };
        let triage: Vec<Value> = tr
            .issues
            .iter()
            .map(|(i, why)| {
                json!({
                    "number": i.number,
                    "title": i.title,
                    "reason": why,
                    "priority_label": q.priority.get(rank(&q.priority, &i.labels)),
                })
            })
            .collect();
        // As in the night itself, a cleanup that can't plan doesn't stop the rest.
        let clean = crate::clean::plan(&repo).unwrap_or_else(|e| {
            eprintln!("ns watch: cleanup skipped: {e:#}");
            crate::clean::Plan::default()
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "requeue": requeue,
                "clean": clean.merged.iter().map(|m| &m.unit).collect::<Vec<_>>(),
                "queue": ready,
                "skipped": qu.skipped,
                "triage_pass": triage::on(fac),
                "triage": triage,
                "triage_next_pass": next_pass,
                "triage_skipped": tr.skipped,
                "max_units": max_units,
                "until": deadline.map(clock::local_iso),
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }

    let _watch = run::Lock::watch(&repo.common_dir)?;
    stop::install().context("cannot handle SIGINT and SIGTERM")?;
    let mut tonight = Tonight::default();
    let stopped = match work(
        &repo,
        &loaded,
        deadline,
        max_units,
        &mut night,
        &mut tonight,
    ) {
        Ok(stopped) => stopped,
        Err(e) => {
            let Some(sig) = stop::requested() else {
                return Err(e);
            };
            if let Some(n) = tonight.current.take() {
                if let Err(le) = set_status(&repo.root, &q, n, Some(&q.ready_label)) {
                    eprintln!("ns watch: #{n} not returned to the queue: {le:#}");
                }
                tonight.units.push(json!({
                    "issue": n,
                    "outcome": "interrupted",
                    "reason": format!("{e:#}"),
                }));
            }
            stop::name(sig).into()
        }
    };
    // A signal that came after the last check still decides the exit code, so name it too.
    let stopped = stop::requested().map_or(stopped, |sig| stop::name(sig).into());
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "units": tonight.units,
            "triaged": night.triage.records,
            "requeued": tonight.requeued,
            "cleaned": tonight.cleaned,
            "stopped": stopped,
            "until": deadline.map(clock::local_iso),
            "cost_usd": night.shared.spent(),
            "started_with": loaded.files,
        }))?
    );
    Ok(match stop::requested() {
        Some(sig) => ExitCode::from((128 + sig) as u8),
        None => ExitCode::SUCCESS,
    })
}

/// What the night's work leaves for its summary.
#[derive(Default)]
struct Tonight {
    units: Vec<Value>,
    /// Issues found in progress at start with no live `ns run`, returned to the queue.
    requeued: Vec<u64>,
    /// The issue taken from the queue whose unit has not ended yet.
    current: Option<u64>,
    /// Units whose worktree and branch the night removed, their change merged.
    cleaned: Vec<String>,
    /// Merged units cleanup tried tonight, removed or not: each is tried once a night.
    cleanup_tried: BTreeSet<String>,
}

/// Whether the outcome is surely the unit's own end: a merge, a split or a clean `done`. Under a
/// stop, any other outcome may come from the stop itself, such as a `gh` call that Ctrl-C killed,
/// so the issue goes back to the queue instead.
fn ended_cleanly(r: &RunResult) -> bool {
    matches!(r.outcome, Outcome::Merged | Outcome::Split)
        || (r.outcome == Outcome::Done && !r.needs_human)
}

/// Return stale in-progress issues to the queue, then run units until the night ends; why it
/// ended.
fn work(
    repo: &Repo,
    loaded: &run::Loaded,
    deadline: Option<i64>,
    max_units: Option<u32>,
    night: &mut Night,
    tonight: &mut Tonight,
) -> Result<String> {
    let fac = &loaded.fac;
    let q = &fac.queue;
    requeue_stale(repo, q, &mut tonight.requeued)?;
    let Tonight {
        units,
        current,
        cleaned,
        cleanup_tried,
        ..
    } = tonight;
    let mut started = 0u32;
    let stopped: String = 'outer: loop {
        if let Some(sig) = stop::requested() {
            break stop::name(sig).into();
        }
        // At start and between units: units merged since go, after their records are saved.
        cleaned.extend(crate::clean::between_units(repo, cleanup_tried));
        if max_units.is_some_and(|m| started >= m) {
            break "max_units".into();
        }
        if let Some(stop) = night.over(deadline, fac) {
            break stop.into();
        }
        if let Some(stop) = triage::pass(repo, loaded, deadline, night)? {
            break stop;
        }
        let qu = queue(&repo.root, fac, &night.finished)?;
        let Some(issue) = qu.ready.first().cloned() else {
            break "queue empty".into();
        };
        started += 1;
        *current = Some(issue.number);
        set_status(&repo.root, q, issue.number, Some(&q.in_progress_label))?;
        eprintln!("ns watch: #{} {}", issue.number, issue.title);
        loop {
            let rargs = RunArgs {
                issue: Some(issue.number),
                base: night.base(repo, issue.number),
                ..RunArgs::default()
            };
            let r = match run::execute(&rargs, &mut night.shared, loaded) {
                Ok(r) => r,
                Err(e) => {
                    let _ = set_status(&repo.root, q, issue.number, Some(&q.ready_label));
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
            if let Some(sig) = stop::requested().filter(|_| !ended_cleanly(&r)) {
                set_status(&repo.root, q, issue.number, Some(&q.ready_label))?;
                *current = None;
                rec["outcome"] = json!("interrupted");
                units.push(rec);
                break 'outer stop::name(sig).into();
            }
            *current = None;
            let failing = night.harness(&r);
            match r.outcome {
                Outcome::Merged => {
                    let status = set_status(&repo.root, q, issue.number, None);
                    // GitHub may not have closed it yet; an already-closed issue makes gh fail.
                    let n = issue.number.to_string();
                    if let Err(e) = gh(&repo.root, &["issue", "close", &n, "--reason", "completed"])
                    {
                        eprintln!("ns watch: #{n} not closed: {e}");
                    }
                    status?;
                }
                Outcome::Split => {
                    set_status(&repo.root, q, issue.number, Some(&q.split_label))?;
                }
                Outcome::Done if !r.needs_human => {
                    set_status(&repo.root, q, issue.number, Some(&q.done_label))?;
                }
                Outcome::Stuck if failing => {
                    set_status(&repo.root, q, issue.number, Some(&q.ready_label))?;
                    rec["outcome"] = json!("harness_failing");
                    if night.harness_fails >= HARNESS_FAIL_LIMIT {
                        units.push(rec);
                        break 'outer "harness failing".into();
                    }
                }
                Outcome::Done | Outcome::Stuck => {
                    let status = set_status(&repo.root, q, issue.number, Some(&q.stuck_label));
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
                    set_status(&repo.root, q, issue.number, Some(&q.ready_label))?;
                    units.push(rec);
                    // A runner lock wait cut short by `--until` ends the run as `budget` too.
                    break 'outer night.over(deadline, fac).unwrap_or("budget").into();
                }
                Outcome::Paused => {
                    let reset = night.resume_at(&r);
                    rec["reset_at"] = json!(clock::local_iso(reset));
                    units.push(rec);
                    if deadline.is_some_and(|d| reset >= d) {
                        set_status(&repo.root, q, issue.number, Some(&q.ready_label))?;
                        break 'outer PAUSED_PAST_UNTIL.into();
                    }
                    *current = Some(issue.number);
                    night.sleep_until(reset)?;
                    continue;
                }
            }
            units.push(rec);
            break;
        }
        night.finished.insert(issue.number);
    };
    Ok(stopped)
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
            .filter(|b| self.shared.spent() >= *b)
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

    fn sleep_until(&mut self, reset: i64) -> Result<()> {
        eprintln!(
            "ns watch: usage limit, sleeping until {}",
            clock::local_iso(reset)
        );
        self.shared.clock.sleep_until(reset)
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
    fn a_run_lock_holder_holds_its_issue_or_its_unit_ids_issue() {
        let h = json!({"pid": 1, "unit": "13-fix-uart", "issue": 13});
        assert!(holds(&h, 13));
        assert!(!holds(&h, 1));
        assert!(!holds(&h, 130));
        // A unit run without --issue names its issue only through the unit id.
        let by_unit = json!({"pid": 1, "unit": "13-fix-uart", "issue": null});
        assert!(holds(&by_unit, 13));
        assert!(!holds(&by_unit, 1));
        assert!(holds(&json!({"pid": 1, "unit": "13"}), 13));
        assert!(!holds(&json!({"pid": 1, "unit": "130-x"}), 13));
        // A holder that hasn't written its record yet is known by its lock's file name.
        assert!(holds(
            &json!({"pid": null, "unit": "99-x", "issue": null}),
            99
        ));
        assert!(!holds(
            &json!({"pid": null, "unit": "uart", "issue": null}),
            99
        ));
    }

    #[test]
    fn a_unit_branch_is_the_one_ns_run_names_for_the_issue() {
        let issue = |title: &str| Issue {
            number: 13,
            title: title.into(),
            labels: Vec::new(),
            body: String::new(),
            team: true,
        };
        assert_eq!(unit_branch(&issue("Fix UART")), "ns/13-fix-uart");
        assert_eq!(unit_branch(&issue("")), "ns/13");
    }

    #[test]
    fn ranks_by_first_matching_order_label() {
        let order: Vec<String> = vec!["type:fix".into(), "type:feat".into()];
        assert_eq!(rank(&order, &["type:feat".into()]), 1);
        assert_eq!(rank(&order, &["area:cli".into()]), 2);
    }
}
