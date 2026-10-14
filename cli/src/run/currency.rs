//! Artifact currency: which artifacts hold at HEAD, and the phase `ns run` drives next.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::pr_number;
use crate::factory::{artifact_of, PHASES};
use crate::frontmatter;
use crate::git::{self, same_sha};

#[derive(Debug, Clone, Default)]
pub struct Art {
    pub status: String,
    pub sha: Option<String>,
    pub pr: Option<String>,
    pub body: String,
    /// What `ns artifact check` rejects in an `ns-artifact/2` file. An invalid artifact counts as
    /// absent: its phase runs again.
    pub invalid: Option<Vec<String>>,
    /// An `ns-artifact/2` file's `next`, which routes it; `None` for `/1`.
    pub next: Option<String>,
}

/// The phase whose artifact `path` is, from its file name.
fn phase_of_file(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    PHASES.iter().copied().find(|p| artifact_of(p) == name)
}

/// Read an artifact. An `ns-artifact/2` file is read through its typed fields, and its body is
/// given the first line today's state table routes on (docs/design/factory-core/v1.md, step 3):
/// `base moved: <reason>` for `cause: base_moved`, `<next>: <reason>` for a ship that names verify
/// or review, else the reason. A `/1` file is read as before.
pub(super) fn read_art(path: &Path) -> Option<Art> {
    let text = fs::read_to_string(path).ok()?;
    if let Some(phase) = phase_of_file(path).and_then(crate::artifact::Phase::parse) {
        if crate::artifact::is_v2(&text) {
            return Some(match crate::artifact::read(&text, phase) {
                Ok(t) => typed_art(&t, &text),
                Err(errors) => Art {
                    invalid: Some(errors),
                    // Kept so an invalid pr.md still names its PR and ship doesn't open another.
                    pr: frontmatter::split(&text)
                        .and_then(|(y, _)| frontmatter::raw_field(y, "pr")),
                    ..Art::default()
                },
            });
        }
    }
    let (status, sha, pr, body) = match (frontmatter::parse(&text), frontmatter::split(&text)) {
        (Ok(Some(_)), Some((yaml, body))) => (
            frontmatter::raw_field(yaml, "status").unwrap_or_default(),
            frontmatter::raw_field(yaml, "sha"),
            frontmatter::raw_field(yaml, "pr"),
            body.to_string(),
        ),
        _ => (String::new(), None, None, text.clone()),
    };
    Some(Art {
        status,
        sha,
        pr,
        body,
        invalid: None,
        next: None,
    })
}

fn typed_art(t: &crate::artifact::Typed, text: &str) -> Art {
    use crate::artifact::Cause;
    let first = match t.cause {
        Cause::BaseMoved => format!("base moved: {}", t.reason),
        _ => t.reason.clone(),
    };
    let rest = frontmatter::split(text).map_or("", |(_, b)| b);
    Art {
        status: t.status.as_str().to_string(),
        sha: t.sha.clone(),
        pr: t.pr.clone(),
        body: format!("{first}\n\n{rest}"),
        invalid: None,
        next: Some(t.next.as_str().to_string()),
    }
}

/// Move `file` into `<dir>/history/<stem>-<n>.md`, n one past the highest already there.
fn archive_file(dir: &Path, file: &str, moves: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
    if let Some(dst) = archive_as(dir, file, file.trim_end_matches(".md"))? {
        moves.push((dir.join(file), dst));
    }
    Ok(())
}

/// Like `archive_file` with a custom `stem`; returns the destination, or None if `file` is absent.
pub(super) fn archive_as(dir: &Path, file: &str, stem: &str) -> Result<Option<PathBuf>> {
    let src = dir.join(file);
    if !src.exists() {
        return Ok(None);
    }
    let hist = dir.join("history");
    fs::create_dir_all(&hist).with_context(|| format!("cannot create {}", hist.display()))?;
    let prefix = format!("{stem}-");
    let next = fs::read_dir(&hist)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix(&prefix)?
                .strip_suffix(".md")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        + 1;
    let dst = hist.join(format!("{stem}-{next}.md"));
    fs::rename(&src, &dst)
        .with_context(|| format!("cannot move {} to {}", src.display(), dst.display()))?;
    Ok(Some(dst))
}

/// Undo `archive_for` after an attempt that wrote nothing, so the unit's state is as before.
pub(super) fn restore(moves: &[(PathBuf, PathBuf)]) {
    for (src, dst) in moves.iter().rev() {
        if !src.exists() {
            let _ = fs::rename(dst, src);
        }
    }
}

/// Before running `phase`: archive its artifact, and every downstream artifact that is not
/// `pass` and current, or follows one that was archived. Whatever exists afterwards is current,
/// so presence alone drives `decide`.
pub fn archive_for(dir: &Path, phase: &str, s: &State) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut moves = Vec::new();
    let Some(i) = PHASES.iter().position(|p| *p == phase) else {
        return Ok(moves);
    };
    archive_file(dir, artifact_of(phase), &mut moves)?;
    let mut cascading = true;
    for p in &PHASES[i + 1..] {
        let file = artifact_of(p);
        cascading = cascading && read_art(&dir.join(file)).is_some_and(|a| s.passes(&a));
        if !cascading {
            archive_file(dir, file, &mut moves)?;
        }
    }
    Ok(moves)
}

/// The unit's PR number: from `pr.md`, else the newest archived `pr-<n>.md` or `pr-timeout-<n>.md` that names one.
/// Newest is the latest `updated` (ISO-8601 UTC, so string order is time order), then the higher
/// counter, then the file name; an archive with no `updated` is oldest.
pub fn known_pr(dir: &Path) -> Option<u64> {
    if let Some(n) = read_art(&dir.join("pr.md"))
        .and_then(|a| a.pr)
        .and_then(|v| pr_number(&v))
    {
        return Some(n);
    }
    let mut archived: Vec<(Option<String>, u32, PathBuf)> = fs::read_dir(dir.join("history"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let rest = name.strip_prefix("pr-")?.strip_suffix(".md")?;
            let n = rest
                .strip_prefix("timeout-")
                .or_else(|| rest.strip_prefix("invalid-"))
                .unwrap_or(rest)
                .parse()
                .ok()?;
            let updated = fs::read_to_string(e.path())
                .ok()
                .and_then(|t| frontmatter::field(&t, "updated"));
            Some((updated, n, e.path()))
        })
        .collect();
    archived.sort();
    archived
        .iter()
        .rev()
        .find_map(|(_, _, p)| read_art(p).and_then(|a| a.pr).and_then(|v| pr_number(&v)))
}

pub struct State {
    pub arts: BTreeMap<&'static str, Art>,
    pub head: String,
    pub has_issue: bool,
    /// The unit's diff at HEAD; `None` compares shas only.
    pub unit_diff: Option<UnitDiff>,
    /// Verify and review artifacts that are not current, but only because ship took in the base
    /// and that changed few of the unit's own lines (CORE-1): evidence carries forward, and
    /// review runs over just that change.
    pub carried: BTreeMap<&'static str, Carried>,
}

/// An artifact carried forward over a small change the base brought in.
#[derive(Debug, Clone, PartialEq)]
pub struct Carried {
    /// The sha the artifact was written at.
    pub from: String,
    /// How many of the unit's own changed lines differ since (`git::interdiff_lines`).
    pub lines: usize,
}

/// The most changed lines of the unit's own diff a base merge may bring in and still carry
/// evidence forward with only a delta review (CORE-1 in docs/design/factory-core/v1.md).
pub const DELTA_MAX_LINES: usize = 40;

#[derive(Debug, Clone)]
pub struct UnitDiff {
    worktree: PathBuf,
    base: String,
    head_id: Option<String>,
}

impl UnitDiff {
    pub fn new(worktree: PathBuf, base: String, head: &str) -> Self {
        let head_id = git::diff_id(&worktree, &base, head);
        Self {
            worktree,
            base,
            head_id,
        }
    }

    fn matches(&self, sha: &str) -> bool {
        self.head_id.is_some() && git::diff_id(&self.worktree, &self.base, sha) == self.head_id
    }
}

impl State {
    /// `sha` is HEAD, or carries the same diff against the base as HEAD (a rebase).
    pub fn current(&self, sha: &str) -> bool {
        same_sha(sha, &self.head) || self.unit_diff.as_ref().is_some_and(|d| d.matches(sha))
    }

    pub(super) fn passes(&self, a: &Art) -> bool {
        a.status == "pass" && self.current(a.sha.as_deref().unwrap_or(""))
    }
}

pub(super) fn read_state(
    dir: &Path,
    head: String,
    has_issue: bool,
    unit_diff: Option<UnitDiff>,
) -> State {
    let mut arts = BTreeMap::new();
    for p in PHASES {
        if let Some(a) = read_art(&dir.join(artifact_of(p))).filter(|a| a.invalid.is_none()) {
            arts.insert(*p, a);
        }
    }
    State {
        arts,
        head,
        has_issue,
        unit_diff,
        carried: BTreeMap::new(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Run {
        phase: &'static str,
        feedback: String,
        why: String,
    },
    Done,
    Stuck(String),
    /// Triage split the issue into child issues, which replace the unit.
    Split(String),
}

pub(super) fn run(phase: &'static str, feedback: &str, why: impl Into<String>) -> Decision {
    Decision::Run {
        phase,
        feedback: feedback.to_string(),
        why: why.into(),
    }
}

/// The body's first non-empty line, the contract's one-sentence reason. A heading marker is
/// dropped and its text kept.
fn first_line(body: &str) -> String {
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim_start())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

/// A NO-GO whose first line starts with `base moved`, after an optional `NO-GO:`: the branch only needs a rebase.
fn base_moved(body: &str) -> bool {
    let line = first_line(body).to_lowercase();
    let rest = line.strip_prefix("no-go:").unwrap_or(&line);
    rest.trim_start().starts_with("base moved")
}

/// `d` sends a passed build back only to rebase, because verify, review or ship said the base
/// moved.
pub(super) fn rebases_for_base(s: &State, d: &Decision) -> bool {
    matches!(d, Decision::Run { phase: "build", feedback, .. } if base_moved(feedback))
        && s.arts.get("build").is_some_and(|b| b.status == "pass")
}

/// The phase a failed ship names first: `verify` or `review`.
fn named_phase(body: &str) -> Option<&'static str> {
    let lower = body.to_lowercase();
    let v = lower.find("verify");
    let r = lower.find("review");
    match (v, r) {
        (Some(a), Some(b)) => Some(if a < b { "verify" } else { "review" }),
        (Some(_), None) => Some("verify"),
        (None, Some(_)) => Some("review"),
        (None, None) => None,
    }
}

/// What a delta review is told: review only what the base merge changed in the unit's diff.
fn delta_feedback(c: &Carried, head: &str) -> String {
    format!(
        "Delta review. review.md passed at {from}. Since then only ship's merge of the base changed \
         this unit's own diff, by {n} lines, so verify's evidence carries forward. Review only \
         that change, not the whole unit: compare the unit's diff at {from} with its diff at \
         {head}, for example with `git range-diff $(git merge-base {from} origin/HEAD)..{from} \
         $(git merge-base {head} origin/HEAD)..{head}`. Keep the earlier review's findings for \
         everything else, and add `delta_of: {from}` to review.md's frontmatter.",
        from = c.from,
        n = c.lines,
    )
}

/// The state table in docs/factory/run.md.
pub fn decide(s: &State) -> Decision {
    if let Some(brief) = s.arts.get("triage").filter(|a| a.status == "split") {
        return Decision::Split(format!("brief.md is split: {}", first_line(&brief.body)));
    }
    for p in PHASES {
        if let Some(a) = s.arts.get(p) {
            if a.status == "blocked" {
                return Decision::Stuck(format!(
                    "{} is blocked: {}",
                    artifact_of(p),
                    first_line(&a.body)
                ));
            }
        }
    }
    let stale = |a: &Art| !s.current(a.sha.as_deref().unwrap_or(""));
    let passes = |p: &str| s.arts.get(p).is_some_and(|a| s.passes(a));
    let verify_carried = s.carried.contains_key("verify")
        && s.arts.get("verify").is_some_and(|a| a.status == "pass");
    if passes("ship") && (passes("verify") || verify_carried) && passes("review") {
        return Decision::Done;
    }
    let Some(brief) = s.arts.get("triage") else {
        return if s.has_issue {
            run("triage", "", "no brief.md")
        } else {
            Decision::Stuck("no brief: no brief.md and no issue to triage".into())
        };
    };
    if let Some(n) = brief.next.as_deref().filter(|n| *n != "build") {
        return Decision::Stuck(format!(
            "triage routed the issue to {n}: {}",
            first_line(&brief.body)
        ));
    }
    if brief.status != "pass" {
        return Decision::Stuck(format!(
            "triage decided a human or define is needed (brief.md status {:?})",
            brief.status
        ));
    }
    let Some(b) = s.arts.get("build") else {
        return run("build", "", "no build.md");
    };
    match b.status.as_str() {
        "pass" => {}
        "fail" => return run("build", &b.body, "build.md failed; build retries"),
        other => return Decision::Stuck(format!("build.md has status {other:?}")),
    }
    let stale_why = |file: &str, a: &Art| {
        format!(
            "{file} sha {} is not current at HEAD {}",
            a.sha.as_deref().unwrap_or("(none)"),
            s.head
        )
    };
    for (phase, file) in [("verify", "evidence.md"), ("review", "review.md")] {
        let Some(a) = s.arts.get(phase) else {
            return run(phase, "", format!("no {file}"));
        };
        match s.carried.get(phase).filter(|_| a.status == "pass") {
            Some(_) if phase == "verify" => continue,
            Some(c) => {
                return run(
                    "review",
                    &delta_feedback(c, &s.head),
                    format!(
                    "{file} sha {} is behind a {}-line change the base brought in: delta review",
                    c.from, c.lines
                ),
                )
            }
            None => {}
        }
        if stale(a) {
            return run(phase, "", stale_why(file, a));
        }
        match (a.status.as_str(), a.next.as_deref()) {
            ("pass", _) => {}
            ("fail", Some("human")) => {
                return Decision::Stuck(format!(
                    "{file} failed and needs a human: {}",
                    first_line(&a.body)
                ))
            }
            ("fail", _) => return run("build", &a.body, format!("{file} failed")),
            (other, _) => return Decision::Stuck(format!("{file} has status {other:?}")),
        }
    }
    let Some(pr) = s.arts.get("ship") else {
        return run("ship", "", "no pr.md");
    };
    if pr.status == "fail" {
        if let Some(n) = pr.next.as_deref() {
            return match n {
                "build" => run(
                    "build",
                    &pr.body,
                    format!("pr.md failed and names build: {}", first_line(&pr.body)),
                ),
                "verify" | "review" => run(
                    if n == "verify" { "verify" } else { "review" },
                    &pr.body,
                    format!("pr.md failed and names {n}"),
                ),
                "ship" => run("ship", &pr.body, "pr.md is not merge-ready yet"),
                _ => Decision::Stuck(format!("ship failed: {}", first_line(&pr.body))),
            };
        }
    }
    match pr.status.as_str() {
        "pass" => run("ship", "", stale_why("pr.md", pr)),
        "fail" if base_moved(&pr.body) => run("build", &pr.body, "pr.md failed: base moved"),
        "fail" => match named_phase(&pr.body) {
            Some(p) => run(
                if p == "verify" { "verify" } else { "review" },
                &pr.body,
                format!("pr.md failed and names {p}"),
            ),
            None => Decision::Stuck(format!("ship failed: {}", first_line(&pr.body))),
        },
        other => Decision::Stuck(format!("pr.md has status {other:?}")),
    }
}

#[cfg(test)]
mod tests;
