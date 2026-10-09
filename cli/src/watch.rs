//! `ns watch`: pull ready issues from GitHub and run them one at a time (docs/FACTORY.md).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::clock;
use crate::error::SfError;
use crate::factory::{self, Factory};
use crate::git::{self, Repo};
use crate::run::{self, gh, gh_json, Outcome, RunArgs, Shared};

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

fn queue(root: &Path, fac: &Factory) -> Result<Queue> {
    let q = &fac.queue;
    // `gh issue list --json` has no author association, so read the REST list.
    let path = format!(
        "repos/{{owner}}/{{repo}}/issues?state=open&labels={}&per_page=100",
        q.ready_label
    );
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
    let prs = gh_json(
        root,
        &[
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,body",
        ],
    )?;
    let mut has_pr: BTreeMap<u64, u64> = BTreeMap::new();
    for p in prs.as_array().cloned().unwrap_or_default() {
        for n in closed_by(p["body"].as_str().unwrap_or("")) {
            has_pr.insert(n, p["number"].as_u64().unwrap_or(0));
        }
    }
    let mut open_cache: BTreeMap<u64, bool> = BTreeMap::new();
    let mut ready = Vec::new();
    let mut skipped = Vec::new();
    for i in issues {
        if !i.team {
            skipped.push(
                json!({"number": i.number, "title": i.title, "reason": "author outside the team"}),
            );
            continue;
        }
        if let Some(pr) = has_pr.get(&i.number) {
            skipped.push(json!({"number": i.number, "title": i.title, "reason": format!("open PR #{pr} closes it")}));
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
            skipped.push(json!({"number": i.number, "title": i.title, "reason": format!("blocked by open {}", open_blockers.join(", "))}));
            continue;
        }
        ready.push(i);
    }
    ready.sort_by_key(|i| (rank(&q.order, &i.labels), i.number));
    Ok(Queue { ready, skipped })
}

fn swap(root: &Path, n: u64, remove: &str, add: Option<&str>) -> Result<()> {
    let ns = n.to_string();
    let mut args = vec!["issue", "edit", ns.as_str(), "--remove-label", remove];
    if let Some(a) = add {
        args.extend(["--add-label", a]);
    }
    gh(root, &args).map(|_| ())
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

pub fn run(args: WatchArgs) -> Result<ExitCode> {
    let start = std::env::current_dir().context("cannot read current directory")?;
    let repo = Repo::discover(&start)?;
    let froot = factory::root(args.factory.as_deref(), &repo.root);
    let fac = factory::load(&froot)?;
    run::load_config()?;
    let q = fac.queue.clone();
    let mut shared = Shared::new();
    let deadline = match &args.until {
        Some(s) => {
            let Some((h, m)) = clock::parse_hm(s) else {
                return Err(SfError::usage(
                    format!("--until {s:?} is not HH:MM"),
                    "ns watch --until 06:30",
                )
                .into());
            };
            Some(clock::next_local(shared.clock.now(), h, m))
        }
        None => None,
    };
    let max_units = if args.once {
        Some(1)
    } else {
        args.max_units.or(fac.limits.max_units)
    };

    if args.dry_run {
        let qu = queue(&repo.root, &fac)?;
        let ready: Vec<Value> = qu
            .ready
            .iter()
            .map(|i| {
                json!({
                    "number": i.number,
                    "title": i.title,
                    "order_label": q.order.get(rank(&q.order, &i.labels)),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "queue": ready,
                "skipped": qu.skipped,
                "max_units": max_units,
                "until": deadline.map(clock::iso),
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
        if deadline.is_some_and(|d| shared.clock.now() >= d) {
            break "until".into();
        }
        if let Some(b) = fac.budget_usd() {
            if shared.spent_usd >= b {
                break "budget".into();
            }
        }
        let qu = queue(&repo.root, &fac)?;
        let Some(issue) = qu.ready.first().cloned() else {
            break "queue empty".into();
        };
        started += 1;
        swap(
            &repo.root,
            issue.number,
            &q.ready_label,
            Some(&q.in_progress_label),
        )?;
        eprintln!("ns watch: #{} {}", issue.number, issue.title);
        loop {
            let base = fresh_base(&repo.root);
            let rargs = RunArgs {
                issue: Some(issue.number),
                factory: args.factory.clone(),
                base,
                ..RunArgs::default()
            };
            let r = match run::execute(&rargs, &mut shared) {
                Ok(r) => r,
                Err(e) => {
                    let _ = swap(
                        &repo.root,
                        issue.number,
                        &q.in_progress_label,
                        Some(&q.ready_label),
                    );
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
            match r.outcome {
                Outcome::Merged => {
                    gh(
                        &repo.root,
                        &[
                            "issue",
                            "edit",
                            &issue.number.to_string(),
                            "--remove-label",
                            &q.in_progress_label,
                        ],
                    )?;
                }
                Outcome::Done if !r.needs_human => {
                    swap(
                        &repo.root,
                        issue.number,
                        &q.in_progress_label,
                        Some(&q.done_label),
                    )?;
                }
                Outcome::Done | Outcome::Stuck => {
                    swap(
                        &repo.root,
                        issue.number,
                        &q.in_progress_label,
                        Some(&q.stuck_label),
                    )?;
                    let what = if r.outcome == Outcome::Stuck {
                        "got stuck"
                    } else {
                        "needs a human"
                    };
                    let body = format!(
                        "nightshift {what} on unit `{}`: {}\n\nLast artifact: `{}`\n\n{DISCLAIMER}",
                        r.unit,
                        r.reason,
                        r.artifact.as_deref().unwrap_or("none")
                    );
                    comment(&repo.root, issue.number, &body)?;
                }
                Outcome::Budget => {
                    swap(
                        &repo.root,
                        issue.number,
                        &q.in_progress_label,
                        Some(&q.ready_label),
                    )?;
                    units.push(rec);
                    break 'outer "budget".into();
                }
                Outcome::Paused => {
                    let now = shared.clock.now();
                    let reset = r
                        .reset_at
                        .filter(|t| *t > now)
                        .unwrap_or(now + PAUSE_RETRY_S);
                    rec["reset_at"] = json!(clock::iso(reset));
                    units.push(rec);
                    if deadline.is_some_and(|d| reset >= d) {
                        swap(
                            &repo.root,
                            issue.number,
                            &q.in_progress_label,
                            Some(&q.ready_label),
                        )?;
                        break 'outer "usage limit resets after --until".into();
                    }
                    eprintln!(
                        "ns watch: usage limit, sleeping until {}",
                        clock::iso(reset)
                    );
                    shared.clock.sleep_until(reset);
                    continue;
                }
            }
            units.push(rec);
            break;
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "units": units,
            "stopped": stopped,
            "cost_usd": shared.spent_usd,
        }))?
    );
    Ok(ExitCode::SUCCESS)
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
    fn ranks_by_first_matching_order_label() {
        let order: Vec<String> = vec!["type:fix".into(), "type:feat".into()];
        assert_eq!(rank(&order, &["type:feat".into()]), 1);
        assert_eq!(rank(&order, &["area:cli".into()]), 2);
    }
}
