//! Artifact currency: which artifacts hold at HEAD, and the phase `ns run` drives next.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::pr_number;
use crate::factory::{artifact_of, PHASES};
use crate::frontmatter;
use crate::git::{self, same_sha};

#[derive(Debug, Clone)]
pub struct Art {
    pub status: String,
    pub sha: Option<String>,
    pub pr: Option<String>,
    pub body: String,
}

pub(super) fn read_art(path: &Path) -> Option<Art> {
    let text = fs::read_to_string(path).ok()?;
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
    })
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
pub fn known_pr(dir: &Path) -> Option<u64> {
    if let Some(n) = read_art(&dir.join("pr.md"))
        .and_then(|a| a.pr)
        .and_then(|v| pr_number(&v))
    {
        return Some(n);
    }
    let mut archived: Vec<(u32, PathBuf)> = fs::read_dir(dir.join("history"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let rest = name.strip_prefix("pr-")?.strip_suffix(".md")?;
            let n = rest.strip_prefix("timeout-").unwrap_or(rest).parse().ok()?;
            Some((n, e.path()))
        })
        .collect();
    archived.sort();
    archived
        .iter()
        .rev()
        .find_map(|(_, p)| read_art(p).and_then(|a| a.pr).and_then(|v| pr_number(&v)))
}

pub struct State {
    pub arts: BTreeMap<&'static str, Art>,
    pub head: String,
    pub has_issue: bool,
    /// The unit's diff at HEAD; `None` compares shas only.
    pub unit_diff: Option<UnitDiff>,
}

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
        if let Some(a) = read_art(&dir.join(artifact_of(p))) {
            arts.insert(*p, a);
        }
    }
    State {
        arts,
        head,
        has_issue,
        unit_diff,
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

/// The state table in docs/FACTORY.md.
pub fn decide(s: &State) -> Decision {
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
    if passes("ship") && passes("verify") && passes("review") {
        return Decision::Done;
    }
    let Some(brief) = s.arts.get("triage") else {
        return if s.has_issue {
            run("triage", "", "no brief.md")
        } else {
            Decision::Stuck("no brief: no brief.md and no issue to triage".into())
        };
    };
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
        if stale(a) {
            return run(phase, "", stale_why(file, a));
        }
        match a.status.as_str() {
            "pass" => {}
            "fail" => return run("build", &a.body, format!("{file} failed")),
            other => return Decision::Stuck(format!("{file} has status {other:?}")),
        }
    }
    let Some(pr) = s.arts.get("ship") else {
        return run("ship", "", "no pr.md");
    };
    match pr.status.as_str() {
        "pass" => run("ship", "", stale_why("pr.md", pr)),
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
mod tests {
    use super::*;
    use crate::testutil::{commit_file, g, rebased_unit, Rebased};

    fn art(status: &str, sha: &str) -> Art {
        Art {
            status: status.into(),
            sha: Some(sha.into()),
            pr: None,
            body: format!("{status} body"),
        }
    }

    fn state(arts: &[(&'static str, Art)]) -> State {
        State {
            arts: arts.iter().cloned().collect(),
            head: "abc1234".into(),
            has_issue: true,
            unit_diff: None,
        }
    }

    fn phase_of(d: &Decision) -> &str {
        match d {
            Decision::Run { phase, .. } => phase,
            Decision::Done => "done",
            Decision::Stuck(_) => "stuck",
        }
    }

    fn next(arts: &[(&'static str, Art)]) -> String {
        phase_of(&decide(&state(arts))).to_string()
    }

    #[test]
    fn stuck_reason_is_the_first_body_line() {
        let blocked = |body: &str| {
            let mut a = art("blocked", "abc1234");
            a.body = body.into();
            match decide(&state(&[("verify", a)])) {
                Decision::Stuck(r) => r,
                _ => panic!("not stuck"),
            }
        };
        assert_eq!(
            blocked("\n  Needs a board on the bench.  \nlater text\n"),
            "evidence.md is blocked: Needs a board on the bench."
        );
        // A heading marker is dropped and its text kept: the first line is the blocker even
        // when it is written as a heading, and later body text is never used.
        assert_eq!(
            blocked("## Needs hardware\n\nlater text\n"),
            "evidence.md is blocked: Needs hardware"
        );
        let long = blocked(&"x".repeat(300));
        assert_eq!(long, format!("evidence.md is blocked: {}", "x".repeat(200)));
    }

    #[test]
    fn state_table() {
        assert_eq!(next(&[]), "triage");
        let mut s = state(&[]);
        s.has_issue = false;
        assert_eq!(phase_of(&decide(&s)), "stuck");
        let brief = ("triage", art("pass", ""));
        let build = ("build", art("pass", "abc1234"));
        let ev = ("verify", art("pass", "abc12"));
        let rv = ("review", art("pass", "abc1234"));
        assert_eq!(next(std::slice::from_ref(&brief)), "build");
        assert_eq!(next(&[brief.clone(), ("build", art("fail", ""))]), "build");
        assert_eq!(next(&[brief.clone(), build.clone()]), "verify");
        // A shorter sha prefix still matches HEAD.
        assert_eq!(next(&[brief.clone(), build.clone(), ev.clone()]), "review");
        let stale = ("verify", art("pass", "fff0000"));
        assert_eq!(next(&[brief.clone(), build.clone(), stale]), "verify");
        let d = decide(&state(&[
            brief.clone(),
            build.clone(),
            ("verify", art("fail", "abc1234")),
        ]));
        assert!(
            matches!(&d, Decision::Run { phase: "build", feedback, .. } if feedback == "fail body")
        );
        let d = decide(&state(&[
            brief.clone(),
            build.clone(),
            ev.clone(),
            ("review", art("fail", "abc1234")),
        ]));
        assert!(matches!(&d, Decision::Run { phase: "build", .. }));
        let base = [brief.clone(), build.clone(), ev.clone(), rv.clone()];
        assert_eq!(next(&base), "ship");
        let with = |extra: (&'static str, Art)| {
            let mut v = base.to_vec();
            v.push(extra);
            next(&v)
        };
        let mut sf = art("fail", "abc1234");
        sf.body = "evidence is stale; ns-review must rerun".into();
        assert_eq!(with(("ship", sf)), "review");
        assert_eq!(with(("ship", art("fail", "abc1234"))), "stuck");
        assert_eq!(with(("ship", art("pass", "abc1234"))), "done");
        let old = [
            brief.clone(),
            build.clone(),
            ("verify", art("pass", "0000000")),
            ("review", art("pass", "0000000")),
            ("ship", art("pass", "abc1234")),
        ];
        assert_eq!(next(&old), "verify");
        // A pass at an older sha ships again once verify and review hold at HEAD.
        assert_eq!(with(("ship", art("pass", "0000000"))), "ship");
        assert_eq!(
            next(&[
                brief.clone(),
                build.clone(),
                ev.clone(),
                ("review", art("blocked", "abc1234"))
            ]),
            "stuck"
        );
        assert_eq!(next(&[("triage", art("fail", ""))]), "stuck");
    }

    #[test]
    fn archiving_keeps_only_current_downstream_passes() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let w = |f: &str, status: &str, sha: &str| {
            fs::write(
                d.join(f),
                format!("---\nstatus: {status}\nsha: {sha}\npr: 12\n---\nbody\n"),
            )
            .unwrap()
        };
        w("build.md", "pass", "abc1234");
        w("evidence.md", "pass", "abc1234");
        w("review.md", "fail", "abc1234");
        w("pr.md", "pass", "0000000");
        let moves = archive_for(d, "build", &state(&[])).unwrap();
        assert!(!d.join("build.md").exists());
        assert!(d.join("evidence.md").exists());
        assert!(!d.join("review.md").exists());
        assert!(d.join("history/build-1.md").exists());
        assert!(d.join("history/review-1.md").exists());
        assert!(d.join("history/pr-1.md").exists());
        assert_eq!(known_pr(d), Some(12));
        restore(&moves);
        assert!(d.join("build.md").exists() && d.join("pr.md").exists());
        // A restored slot is free again; a second archive takes the next number.
        archive_for(d, "build", &state(&[])).unwrap();
        w("build.md", "pass", "abc1234");
        archive_for(d, "build", &state(&[])).unwrap();
        assert!(d.join("history/build-1.md").exists());
        assert!(d.join("history/build-2.md").exists());
    }

    fn pr_art(dir: &Path, name: &str, pr: u64) {
        fs::create_dir_all(dir.join("history")).unwrap();
        fs::write(
            dir.join(name),
            format!(
                "---\nunit: u\nphase: ship\nstatus: pass\nsha: abc1234\npr: https://github.com/o/r/pull/{pr}\nupdated: 2026-10-08T00:00:00Z\n---\nbody\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn known_pr_reads_timed_out_archives_newest_first() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert_eq!(known_pr(d), None);
        pr_art(d, "history/pr-timeout-1.md", 21);
        assert_eq!(known_pr(d), Some(21));
        pr_art(d, "history/pr-2.md", 22);
        assert_eq!(known_pr(d), Some(22));
        pr_art(d, "history/pr-timeout-3.md", 23);
        assert_eq!(known_pr(d), Some(23));
        pr_art(d, "pr.md", 24);
        assert_eq!(known_pr(d), Some(24));
    }

    fn real_state(r: &Rebased) -> State {
        State {
            arts: BTreeMap::new(),
            head: r.rebased.clone(),
            has_issue: true,
            unit_diff: Some(UnitDiff::new(r.dir.clone(), "main".into(), &r.rebased)),
        }
    }

    #[test]
    fn current_matches_head_a_rebased_diff_and_nothing_else() {
        let r = rebased_unit();
        let s = real_state(&r);
        assert!(s.current(&r.rebased));
        assert!(s.current(&r.rebased[..7]));
        assert!(s.current(&r.reviewed));
        assert!(!s.current("0000000"));
        assert!(!s.current(""));
        let first = g(&r.dir, &["rev-parse", "main"]);
        assert!(!s.current(&first));
        let mut no_base = real_state(&r);
        no_base.unit_diff = None;
        assert!(!no_base.current(&r.reviewed));
    }

    #[test]
    fn decide_is_done_for_artifacts_at_the_pre_rebase_sha() {
        let r = rebased_unit();
        let mut s = real_state(&r);
        for p in ["triage", "build", "verify", "review", "ship"] {
            s.arts.insert(p, art("pass", &r.reviewed));
        }
        assert_eq!(decide(&s), Decision::Done);
        s.unit_diff = None;
        assert_ne!(decide(&s), Decision::Done);
    }

    #[test]
    fn a_rebase_archives_nothing_downstream() {
        let r = rebased_unit();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for f in ["evidence.md", "review.md", "pr.md"] {
            fs::write(
                d.join(f),
                format!("---\nstatus: pass\nsha: {}\npr: 12\n---\n", r.reviewed),
            )
            .unwrap();
        }
        let moves = archive_for(d, "verify", &real_state(&r)).unwrap();
        assert_eq!(moves.len(), 1);
        assert!(!d.join("evidence.md").exists());
        assert!(d.join("review.md").exists() && d.join("pr.md").exists());
        let moves = archive_for(d, "review", &real_state(&r)).unwrap();
        assert_eq!(moves.len(), 1);
        assert!(d.join("pr.md").exists());
        fs::write(
            d.join("review.md"),
            format!("---\nstatus: pass\nsha: {}\n---\n", r.reviewed),
        )
        .unwrap();
        commit_file(&r.dir, "work.txt", "two\n");
        let head = g(&r.dir, &["rev-parse", "HEAD"]);
        let mut changed = real_state(&r);
        changed.unit_diff = Some(UnitDiff::new(r.dir.clone(), "main".into(), &head));
        changed.head = head;
        archive_for(d, "verify", &changed).unwrap();
        assert!(!d.join("review.md").exists() && !d.join("pr.md").exists());
    }

    #[test]
    fn archiving_drops_a_current_pr_behind_a_stale_review() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for (f, sha) in [
            ("evidence.md", "0000000"),
            ("review.md", "0000000"),
            ("pr.md", "abc1234"),
        ] {
            fs::write(d.join(f), format!("---\nstatus: pass\nsha: {sha}\n---\n")).unwrap();
        }
        archive_for(d, "verify", &state(&[])).unwrap();
        assert!(d.join("history/review-1.md").exists());
        assert!(d.join("history/pr-1.md").exists());
    }

    #[test]
    fn sha_and_pr_are_read_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("evidence.md");
        for (sha, want) in [
            ("0e05787", "0e05787"),
            ("1234567", "1234567"),
            ("\"00ab12c\"", "00ab12c"),
            ("3f9c2e1   # HEAD", "3f9c2e1"),
        ] {
            fs::write(
                &p,
                format!("---\nstatus: pass\nsha: {sha}\npr: 0012\n---\nbody\n"),
            )
            .unwrap();
            let a = read_art(&p).unwrap();
            assert_eq!(a.sha.as_deref(), Some(want));
            assert_eq!(a.pr.as_deref(), Some("0012"));
            assert_eq!(a.status, "pass");
        }
    }
}
