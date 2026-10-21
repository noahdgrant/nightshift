//! What each harness session of a unit printed, kept as `transcripts/<unit>/<state>-<n>.jsonl` in
//! the repository's `ns` state directory so it outlives the unit's workspace. `n` counts the
//! state's sessions for the unit from 1: a retry or a rework is a new session.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

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
}

impl Transcripts {
    pub fn open(vcs: &dyn Vcs, root: &Path) -> Result<Transcripts> {
        Ok(Transcripts {
            dir: vcs.state_dir_find(root)?.join("transcripts"),
        })
    }

    /// Keeps `raw` as the next session of `state` for `unit`.
    pub fn keep(&self, unit: &str, state: State, raw: &[u8]) -> Result<()> {
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
        std::fs::write(&path, raw).with_context(|| format!("cannot write {}", path.display()))
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
        t.keep("7", State::Build, b"a").unwrap();
        t.keep("7", State::Review, b"b").unwrap();
        t.keep("7", State::Build, b"c").unwrap();
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
    fn units_are_kept_apart() {
        let (_tmp, t) = store();
        t.keep("7", State::Build, b"a").unwrap();
        t.keep("8", State::Build, b"b").unwrap();
        assert_eq!(t.sessions("8").unwrap().len(), 1);
        assert_eq!(t.sessions("8").unwrap()[0].n, 1);
    }

    #[test]
    fn sessions_come_oldest_first_across_states() {
        let (_tmp, t) = store();
        t.keep("7", State::Review, b"a").unwrap();
        t.keep("7", State::Build, b"b").unwrap();
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
        t.keep("7", State::Build, b"a").unwrap();
        for stray in ["notes.txt", "build-x.jsonl", "nowhere-1.jsonl"] {
            std::fs::write(t.dir.join("7").join(stray), "").unwrap();
        }
        assert_eq!(t.sessions("7").unwrap().len(), 1);
    }
}
