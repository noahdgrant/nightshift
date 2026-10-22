//! What each harness session of a unit printed, kept as `transcripts/<unit>/<state>-<n>.jsonl` in
//! the repository's `ns` state directory so it outlives the unit's workspace. `n` counts the
//! state's sessions for the unit from 1: a retry or a rework is a new session. The model a session
//! used is kept beside it as `<state>-<n>.model`; a session with none used the harness's default.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use crate::manifest::Keep;
use crate::state::State;
use crate::vcs::Vcs;

pub struct Transcripts {
    dir: PathBuf,
}

/// One kept session.
#[derive(Debug, PartialEq, Eq)]
pub struct Session {
    pub state: State,
    pub n: usize,
    pub path: PathBuf,
    /// The model the session used. `None` is the harness's own default.
    pub model: Option<String>,
}

impl Transcripts {
    pub fn open(vcs: &dyn Vcs, root: &Path) -> Result<Transcripts> {
        Ok(Transcripts {
            dir: vcs.state_dir_find(root)?.join("transcripts"),
        })
    }

    /// Keeps `raw` as the next session of `state` for `unit`, with the `model` it ran with.
    pub fn keep(&self, unit: &str, state: State, raw: &[u8], model: Option<&str>) -> Result<()> {
        let dir = self.dir.join(unit);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let n = self
            .sessions(unit)?
            .iter()
            .filter(|s| s.state == state)
            .map(|s| s.n)
            .max()
            .unwrap_or(0)
            + 1;
        let path = dir.join(format!("{state}-{n}.jsonl"));
        std::fs::write(&path, raw).with_context(|| format!("cannot write {}", path.display()))?;
        if let Some(model) = model {
            let path = path.with_extension("model");
            std::fs::write(&path, model)
                .with_context(|| format!("cannot write {}", path.display()))?;
        }
        Ok(())
    }

    /// Deletes the sessions last modified more than `keep` before `now`, and the unit directories
    /// left empty. What cannot be deleted is skipped: pruning is housekeeping, not a reason to stop.
    pub fn prune(&self, keep: Keep, now: SystemTime) {
        let Keep::Days(days) = keep else { return };
        let Some(cutoff) = now.checked_sub(Duration::from_secs(u64::from(days) * 86_400)) else {
            return;
        };
        let Ok(units) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for unit in units.flatten() {
            let Ok(files) = std::fs::read_dir(unit.path()) else {
                continue;
            };
            for file in files.flatten() {
                let old = file
                    .metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|at| at < cutoff);
                if old {
                    let _ = std::fs::remove_file(file.path());
                }
            }
            // Fails, and so keeps the directory, while anything is left in it.
            let _ = std::fs::remove_dir(unit.path());
        }
    }

    /// The sessions kept for `unit`, oldest first. Files that are not sessions are ignored.
    pub fn sessions(&self, unit: &str) -> Result<Vec<Session>> {
        let entries = match std::fs::read_dir(self.dir.join(unit)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).context("cannot read the transcripts"),
        };
        let mut found: Vec<(SystemTime, Session)> = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((state, n)) = name
                .strip_suffix(".jsonl")
                .and_then(|stem| stem.rsplit_once('-'))
                .and_then(|(state, n)| Some((State::parse(state)?, n.parse().ok()?)))
            else {
                continue;
            };
            found.push((
                entry.metadata()?.modified()?,
                Session {
                    state,
                    n,
                    model: std::fs::read_to_string(entry.path().with_extension("model")).ok(),
                    path: entry.path(),
                },
            ));
        }
        // The time of writing orders sessions of different states; a tie falls back to the
        // state's own count, which only grows.
        found.sort_by_key(|(at, s)| (*at, s.n));
        Ok(found.into_iter().map(|(_, s)| s).collect())
    }
}

/// The sessions to show: only those of `state` when given, and then only the last when `last`.
pub fn select(sessions: Vec<Session>, state: Option<State>, last: bool) -> Vec<Session> {
    let mut chosen: Vec<Session> = sessions
        .into_iter()
        .filter(|s| state.is_none_or(|state| s.state == state))
        .collect();
    if last {
        chosen = chosen.pop().into_iter().collect();
    }
    chosen
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn store() -> (tempfile::TempDir, Transcripts) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("transcripts");
        (tmp, Transcripts { dir })
    }

    #[test]
    fn a_state_counts_its_sessions_from_one() {
        let (_tmp, t) = store();
        t.keep("7", State::Build, b"a", None).unwrap();
        t.keep("7", State::Review, b"b", None).unwrap();
        t.keep("7", State::Build, b"c", None).unwrap();
        let names: Vec<_> = t
            .sessions("7")
            .unwrap()
            .iter()
            .map(|s| (s.state, s.n))
            .collect();
        assert!(names.contains(&(State::Build, 1)));
        assert!(names.contains(&(State::Build, 2)));
        assert!(names.contains(&(State::Review, 1)));
        assert_eq!(std::fs::read(t.dir.join("7/build-2.jsonl")).unwrap(), b"c");
    }

    #[test]
    fn a_session_keeps_the_model_it_ran_with_and_none_means_the_harness_default() {
        let (_tmp, t) = store();
        t.keep("7", State::Build, b"a", Some("opus")).unwrap();
        t.keep("7", State::Verify, b"b", None).unwrap();
        let sessions = t.sessions("7").unwrap();
        let model = |state| {
            sessions
                .iter()
                .find(|s| s.state == state)
                .unwrap()
                .model
                .clone()
        };
        assert_eq!(model(State::Build).as_deref(), Some("opus"));
        assert_eq!(model(State::Verify), None);
    }

    #[test]
    fn units_are_kept_apart() {
        let (_tmp, t) = store();
        t.keep("7", State::Build, b"a", None).unwrap();
        t.keep("8", State::Build, b"b", None).unwrap();
        assert_eq!(t.sessions("8").unwrap().len(), 1);
        assert_eq!(t.sessions("8").unwrap()[0].n, 1);
    }

    #[test]
    fn sessions_come_oldest_first_across_states() {
        let (_tmp, t) = store();
        t.keep("7", State::Review, b"a", None).unwrap();
        t.keep("7", State::Build, b"b", None).unwrap();
        // Written in the other order than the names sort in, so the time decides.
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let file = std::fs::File::options()
            .write(true)
            .open(t.dir.join("7/review-1.jsonl"))
            .unwrap();
        file.set_modified(old).unwrap();
        let order: Vec<_> = t.sessions("7").unwrap().iter().map(|s| s.state).collect();
        assert_eq!(order, [State::Review, State::Build]);
    }

    fn session(state: State, n: usize) -> Session {
        Session {
            state,
            n,
            path: PathBuf::new(),
            model: None,
        }
    }

    fn picked(state: Option<State>, last: bool) -> Vec<(State, usize)> {
        let all = vec![
            session(State::Build, 1),
            session(State::Verify, 1),
            session(State::Build, 2),
        ];
        select(all, state, last)
            .iter()
            .map(|s| (s.state, s.n))
            .collect()
    }

    #[test]
    fn select_filters_by_state_then_takes_the_last() {
        assert_eq!(picked(None, false).len(), 3);
        assert_eq!(
            picked(Some(State::Build), false),
            [(State::Build, 1), (State::Build, 2)]
        );
        assert_eq!(picked(None, true), [(State::Build, 2)]);
        assert_eq!(picked(Some(State::Verify), true), [(State::Verify, 1)]);
        assert!(picked(Some(State::Review), true).is_empty());
    }

    #[test]
    fn a_unit_with_none_has_no_sessions_and_strangers_are_ignored() {
        let (_tmp, t) = store();
        assert!(t.sessions("7").unwrap().is_empty());
        t.keep("7", State::Build, b"a", None).unwrap();
        for stray in ["notes.txt", "build-x.jsonl", "nowhere-1.jsonl"] {
            std::fs::write(t.dir.join("7").join(stray), "").unwrap();
        }
        assert_eq!(t.sessions("7").unwrap().len(), 1);
    }

    fn aged(t: &Transcripts, unit: &str, state: State, days: u64) -> PathBuf {
        t.keep(unit, state, b"x", None).unwrap();
        let path = t.sessions(unit).unwrap().pop().unwrap().path;
        let at = now() - Duration::from_secs(days * 86_400);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(at)
            .unwrap();
        path
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(100 * 86_400)
    }

    #[test]
    fn prune_deletes_what_is_older_than_the_limit_and_keeps_the_rest() {
        let (_tmp, t) = store();
        let old = aged(&t, "7", State::Build, 8);
        let new = aged(&t, "7", State::Review, 6);
        t.prune(Keep::Days(7), now());
        assert!(!old.exists());
        assert!(new.exists());
        assert_eq!(t.sessions("7").unwrap().len(), 1);
    }

    #[test]
    fn prune_removes_a_unit_directory_left_empty_and_keeps_one_with_files() {
        let (_tmp, t) = store();
        let gone = aged(&t, "7", State::Build, 30);
        aged(&t, "8", State::Build, 30);
        aged(&t, "8", State::Review, 1);
        t.prune(Keep::Days(7), now());
        assert!(!gone.parent().unwrap().exists());
        assert!(t.dir.join("8").exists());
    }

    #[test]
    fn prune_with_forever_deletes_nothing() {
        let (_tmp, t) = store();
        let old = aged(&t, "7", State::Build, 5_000_000 / 86_400);
        t.prune(Keep::Forever, now());
        assert!(old.exists());
    }

    #[test]
    fn prune_does_nothing_without_a_transcripts_directory() {
        let (_tmp, t) = store();
        t.prune(Keep::Days(1), now());
        assert!(!t.dir.exists());
    }

    #[test]
    fn prune_with_a_limit_before_the_epoch_deletes_nothing() {
        let (_tmp, t) = store();
        let path = aged(&t, "7", State::Build, 99);
        t.prune(Keep::Days(u32::MAX), now());
        assert!(path.exists());
    }

    #[test]
    fn prune_leaves_a_stray_file_beside_the_unit_directories() {
        let (_tmp, t) = store();
        let old = aged(&t, "7", State::Build, 30);
        let stray = t.dir.join("stray.txt");
        std::fs::write(&stray, "").unwrap();
        t.prune(Keep::Days(7), now());
        assert!(stray.exists());
        assert!(!old.exists());
    }

    #[test]
    fn prune_skips_what_it_cannot_delete_and_goes_on() {
        let (_tmp, t) = store();
        // A directory is old and named like a session, and `remove_file` cannot delete it.
        let stuck = t.dir.join("7/build-1.jsonl");
        std::fs::create_dir_all(&stuck).unwrap();
        std::fs::File::open(&stuck)
            .unwrap()
            .set_modified(now() - Duration::from_secs(30 * 86_400))
            .unwrap();
        let other = aged(&t, "8", State::Build, 30);
        t.prune(Keep::Days(7), now());
        assert!(stuck.exists());
        assert!(!other.exists());
    }
}
