//! Artifact currency: which artifacts hold at HEAD, and the phase `ns run` drives next.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::core::route::{
    facts_wanted, in_review_loop, to_archive, CarryFacts, Diffs, ReviewLoop, Settings,
};
pub use super::core::route::{Art, State};
use super::core::Ledger;
use super::pr_number;
use crate::factory::{artifact_of, RUN_PHASES};
use crate::frontmatter;
use crate::git::{self, same_sha};

/// The phase whose artifact `path` is, from its file name.
fn phase_of_file(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    RUN_PHASES.iter().copied().find(|p| artifact_of(p) == name)
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
        findings: vec![],
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
        findings: t.findings.clone(),
    }
}

/// Move `file` into `<dir>/history/<stem>-<n>.md`, n one past the highest already there. An
/// archived `review.md` takes `ns`'s record of its loop along, to `history/<stem>-<n>/`, where
/// `ns quality` reads an archived attempt's cycle files.
fn archive_file(dir: &Path, file: &str, moves: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
    let Some(dst) = archive_as(dir, file, file.trim_end_matches(".md"))? else {
        return Ok(());
    };
    moves.push((dir.join(file), dst.clone()));
    let record = dir.join(crate::review_runs::FILE);
    if file == artifact_of("review") && record.exists() {
        let to = dst.with_extension("").join("cycles.json");
        fs::create_dir_all(to.parent().unwrap_or(dir))?;
        fs::rename(&record, &to)
            .with_context(|| format!("cannot move {} to {}", record.display(), to.display()))?;
        moves.push((record, to));
    }
    Ok(())
}

/// Rewrite the artifact at `path` as blocked for a human: `status: blocked`, `next: human`,
/// `cause: findings_open`, `reason` and `updated` set in its frontmatter, the rest kept.
pub(super) fn mark_blocked(path: &Path, reason: &str, updated: &str) -> Result<()> {
    set_fields(
        path,
        &[
            ("status", "blocked".to_string()),
            ("cause", "findings_open".to_string()),
            ("next", "human".to_string()),
            ("reason", serde_json::to_string(reason)?),
            ("updated", updated.to_string()),
        ],
    )
}

/// Set each of `set`'s frontmatter fields in the artifact at `path`, moved to the end of its
/// frontmatter, and keep the rest.
pub(super) fn set_fields(path: &Path, set: &[(&str, String)]) -> Result<()> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let Some((yaml, body)) = frontmatter::split(&text) else {
        anyhow::bail!("{} has no frontmatter to set fields in", path.display());
    };
    let key = |l: &str| l.split_once(':').map(|(k, _)| k.trim_end().to_string());
    let mut lines: Vec<String> = yaml
        .lines()
        .filter(|l| !set.iter().any(|(k, _)| key(l).as_deref() == Some(k)))
        .map(String::from)
        .collect();
    lines.extend(set.iter().map(|(k, v)| format!("{k}: {v}")));
    let out = format!("---\n{}\n---\n{body}", lines.join("\n"));
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, out).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))
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

/// What [`archive_for`] changed before a phase, for [`restore`] to undo.
#[derive(Debug, Default)]
pub struct Moves {
    /// Each file moved, and where to.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// The review loop's files a review pass or fix cycle changes in place, as they were before
    /// it: `ns`'s record, and `review.md` inside a loop. `None` when the file was absent.
    pub kept: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl Moves {
    /// Whether `path` is a kept file the attempt left as it was: a pass that crashed or wrote
    /// nothing leaves the loop's `review.md` in place, and that is no artifact written.
    pub fn untouched(&self, path: &Path) -> bool {
        self.kept
            .iter()
            .any(|(p, was)| p == path && fs::read(p).ok() == *was)
    }
}

/// Undo `archive_for` after an attempt that wrote nothing, or one set aside, so the unit's state
/// is as before: the kept files first, then the moved ones.
pub(super) fn restore(moves: &Moves) {
    for (path, was) in &moves.kept {
        let _ = match was {
            Some(bytes) => fs::write(path, bytes),
            None => fs::remove_file(path),
        };
    }
    for (src, dst) in moves.moved.iter().rev() {
        if !src.exists() {
            let _ = fs::rename(dst, src);
            // The directory an archived review record went to holds nothing else.
            if dst.file_name().is_some_and(|n| n == "cycles.json") {
                let _ = dst.parent().map(fs::remove_dir);
            }
        }
    }
}

/// Before running `phase`: archive the artifacts [`to_archive`] names, as they are in `dir` now.
/// Whatever exists afterwards is current, so presence alone drives `decide`.
pub fn archive_for(dir: &Path, phase: &str, s: &State) -> Result<Moves> {
    let mut moves = Moves::default();
    let now = State {
        arts: read_arts(dir),
        ..s.clone()
    };
    for p in to_archive(phase, &now) {
        archive_file(dir, artifact_of(p), &mut moves.moved)?;
    }
    if matches!(phase, "review" | "review-fix") {
        let mut keep = vec![dir.join(crate::review_runs::FILE)];
        if phase == "review" && in_review_loop(&now) {
            keep.push(dir.join(artifact_of("review")));
        }
        moves.kept = keep
            .into_iter()
            .map(|p| (p.clone(), fs::read(&p).ok()))
            .collect();
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

/// The unit's state at `head`, read from its artifacts in `dir`. `diff` is the worktree and base
/// the diff ids are read against; `None` compares shas only. The carry facts are left for
/// [`gather_facts`], which needs the ledger.
pub(super) fn read_state(
    dir: &Path,
    head: String,
    has_issue: bool,
    diff: Option<(&Path, &str)>,
    settings: Settings,
) -> State {
    let mut s = State {
        arts: read_arts(dir),
        has_issue,
        diffs: diff.map(|(wt, base)| Diffs {
            head: git::diff_id(wt, base, &head),
            of: BTreeMap::new(),
        }),
        review: review_loop(dir),
        head,
        settings,
        ..State::default()
    };
    if let Some((wt, base)) = diff {
        let shas: Vec<String> = s.arts.values().filter_map(|a| a.sha.clone()).collect();
        for sha in shas {
            learn_sha(&mut s, wt, base, &sha);
        }
    }
    s
}

/// The unit's review loop: the fix cycles `ns`'s record holds, and each finding's fix attempts as
/// `review.md` gives them.
fn review_loop(dir: &Path) -> ReviewLoop {
    let text = fs::read_to_string(dir.join(artifact_of("review"))).unwrap_or_default();
    let body = frontmatter::split(&text).map_or(text.as_str(), |(_, b)| b);
    let path = dir.join(crate::review_runs::FILE);
    let (fixes, record_problem) = match crate::review_runs::read(dir) {
        Ok(_) if !path.exists() => (0, Some(format!("{} is missing", crate::review_runs::FILE))),
        Ok(runs) => (crate::review_runs::fixes(&runs), None),
        Err(e) => (0, Some(format!("{e:#}"))),
    };
    ReviewLoop {
        fixes,
        record_problem,
        attempts: crate::review_md::parse(body)
            .into_iter()
            .map(|f| (f.id, f.fix_attempts.unwrap_or(0)))
            .collect(),
    }
}

/// The valid artifacts in `dir`, by phase.
fn read_arts(dir: &Path) -> BTreeMap<&'static str, Art> {
    let mut arts = BTreeMap::new();
    for p in RUN_PHASES {
        if let Some(a) = read_art(&dir.join(artifact_of(p))).filter(|a| a.invalid.is_none()) {
            arts.insert(*p, a);
        }
    }
    arts
}

/// Read the diff id of `sha` into `s`, so `s.current(sha)` can tell a rebase from a change.
pub(super) fn learn_sha(s: &mut State, worktree: &Path, base: &str, sha: &str) {
    let head = s.head.clone();
    if let Some(d) = s.diffs.as_mut() {
        if !sha.is_empty() && !same_sha(sha, &head) && !d.of.contains_key(sha) {
            let id = d
                .head
                .as_ref()
                .and_then(|_| git::diff_id(worktree, base, sha));
            d.of.insert(sha.to_string(), id);
        }
    }
}

/// Fill `s.facts` for every sha [`facts_wanted`] names: whether the change since it took in the
/// base, and by how many of the unit's own lines.
pub(super) fn gather_facts(s: &mut State, l: &Ledger, worktree: &Path, base: &str) {
    for sha in facts_wanted(l, s) {
        let took_in_base = git::took_in_base(worktree, base, &sha, &s.head);
        let interdiff_lines = took_in_base
            .then(|| git::interdiff_lines(worktree, base, &sha, &s.head))
            .flatten();
        s.facts.insert(
            sha,
            CarryFacts {
                took_in_base,
                interdiff_lines,
            },
        );
    }
}

#[cfg(test)]
mod tests;
