//! `ns run`'s record of a unit's review loop, `.ns/<unit>/review/cycles.json`: one entry per
//! review pass and fix cycle that wrote its artifact, with the times `ns` saw it start and end.
//! The loop's cycle count comes from here, never from what the agent wrote (#446). A loop's first
//! pass starts a new record; the old one is archived with the `review.md` it belongs to.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The record's path under a unit's `.ns/<unit>/` directory.
pub const FILE: &str = "review/cycles.json";

/// One review pass or fix cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewRun {
    /// `review` for a pass, `review-fix` for a fix cycle.
    pub phase: String,
    /// A fix cycle's number, or the fix cycle a pass follows: 0 for the first pass.
    pub cycle: u32,
    /// `first`, `full` or `fix-only`.
    pub kind: String,
    /// UTC, as `ns` saw the run start and end.
    pub started: String,
    pub ended: String,
    /// HEAD when the run ended.
    pub sha: String,
}

/// The record in `cycles_dir` (`review/`, or an archived attempt's `history/<stem>/`). A missing
/// one is empty; one that can't be read or parsed is an error, so a loop's count never starts
/// again from nothing.
pub fn read_in(cycles_dir: &Path) -> Result<Vec<ReviewRun>> {
    let path = cycles_dir.join("cycles.json");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    serde_json::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))
}

/// The unit's current record, in its `.ns/<unit>/` directory.
pub fn read(unit_dir: &Path) -> Result<Vec<ReviewRun>> {
    read_in(&unit_dir.join("review"))
}

/// The fix cycles the record holds.
pub fn fixes(runs: &[ReviewRun]) -> u32 {
    runs.iter().filter(|r| r.phase == "review-fix").count() as u32
}

/// Add `run` to the unit's record, starting a new one when `fresh`, through a temp file and a
/// rename. A run that isn't `fresh` needs the record its loop started.
pub fn append(unit_dir: &Path, run: ReviewRun, fresh: bool) -> Result<PathBuf> {
    let path = unit_dir.join(FILE);
    if !fresh && !path.exists() {
        anyhow::bail!(
            "{} is missing: the loop's earlier runs are not recorded",
            path.display()
        );
    }
    let mut runs = if fresh { vec![] } else { read(unit_dir)? };
    runs.push(run);
    let dir = unit_dir.join("review");
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&runs)?)
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(phase: &str, cycle: u32) -> ReviewRun {
        ReviewRun {
            phase: phase.into(),
            cycle,
            kind: "full".into(),
            started: "2026-10-14T01:00:00Z".into(),
            ended: "2026-10-14T01:10:00Z".into(),
            sha: "abc1234".into(),
        }
    }

    #[test]
    fn a_fresh_pass_starts_the_record_and_later_runs_add_to_it() {
        let d = tempfile::tempdir().unwrap();
        assert!(read(d.path()).unwrap().is_empty());
        append(d.path(), run("review", 0), true).unwrap();
        append(d.path(), run("review-fix", 1), false).unwrap();
        append(d.path(), run("review", 1), false).unwrap();
        let r = read(d.path()).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(fixes(&r), 1);
        append(d.path(), run("review", 0), true).unwrap();
        assert_eq!(read(d.path()).unwrap(), vec![run("review", 0)]);
        assert!(!d.path().join("review/cycles.json.tmp").exists());
    }

    #[test]
    fn an_unparseable_record_is_an_error_and_takes_no_run() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("review")).unwrap();
        fs::write(d.path().join(FILE), "not json").unwrap();
        let e = read(d.path()).unwrap_err();
        assert!(format!("{e:#}").contains("cannot parse"), "{e:#}");
        assert!(append(d.path(), run("review-fix", 1), false).is_err());
        assert_eq!(fs::read_to_string(d.path().join(FILE)).unwrap(), "not json");
    }

    #[test]
    fn an_unreadable_record_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join(FILE)).unwrap();
        let e = read(d.path()).unwrap_err();
        assert!(format!("{e:#}").contains("cannot read"), "{e:#}");
    }

    #[test]
    fn a_run_inside_a_loop_needs_the_record_its_loop_started() {
        let d = tempfile::tempdir().unwrap();
        let e = append(d.path(), run("review-fix", 1), false).unwrap_err();
        assert!(e.to_string().contains("is missing"), "{e}");
        assert!(!d.path().join(FILE).exists());
    }

    #[test]
    fn a_record_that_cannot_be_written_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        append(d.path(), run("review", 0), true).unwrap();
        fs::create_dir_all(d.path().join("review/cycles.json.tmp")).unwrap();
        assert!(append(d.path(), run("review-fix", 1), false).is_err());
        assert_eq!(read(d.path()).unwrap(), vec![run("review", 0)]);
    }
}
