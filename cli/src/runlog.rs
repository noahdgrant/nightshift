//! What every unit has done, one JSON line per move, kept with the repository's other `ns` state
//! so it outlives the unit's workspace. A unit that stopped is resumed from its last move.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::state::State;
use crate::vcs::Vcs;

pub struct RunLog {
    path: PathBuf,
}

/// Where a unit picks up.
#[derive(Debug, PartialEq, Eq)]
pub enum Resume {
    /// The unit has never run.
    Fresh,
    /// The unit stopped at this state, and starts it again.
    At(State),
    /// The unit got stuck in this state. Where it picks up is for the caller to decide.
    Stuck(State),
    /// The unit reached `done`.
    Finished,
}

/// One move of a unit from a state to the next.
#[derive(Debug, PartialEq, Eq)]
pub struct Move {
    /// The state it left. `None` when the log's entry has none that is a state: only a unit that
    /// is resumed from `stuck` needs it, so nothing else fails for want of it.
    pub from: Option<State>,
    pub to: State,
    /// When the log recorded it, in seconds since the epoch.
    pub at: Option<u64>,
    /// Why the unit did not go on to the next state. `None` when it did, or the log gives none
    /// that is known.
    pub reason: Option<Reason>,
}

/// Why a unit went back to a state or got stuck.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Reason {
    /// The state ran and its checks found the work wanting.
    Failed,
    /// The state kept returning an error: the harness, the forge or the clock let it down.
    WentWrong,
}

impl Reason {
    pub fn parse(text: &str) -> Option<Reason> {
        match text {
            "failed" => Some(Reason::Failed),
            "went_wrong" => Some(Reason::WentWrong),
            _ => None,
        }
    }
}

impl RunLog {
    pub fn open(vcs: &dyn Vcs, root: &Path) -> Result<RunLog> {
        Ok(RunLog {
            path: vcs.state_dir_find(root)?.join("runs.jsonl"),
        })
    }

    /// Adds `event`, with the time it was added.
    pub fn append(&self, event: &Value) -> Result<()> {
        let mut event = event.clone();
        let at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        event["at"] = at.into();
        std::fs::create_dir_all(self.path.parent().context("the run log has no directory")?)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("cannot open {}", self.path.display()))?;
        writeln!(file, "{event}")?;
        Ok(())
    }

    /// Where `unit` picks up, from its last move.
    pub fn resume(&self, unit: &str) -> Result<Resume> {
        Ok(match self.last_move(unit)? {
            None => Resume::Fresh,
            Some(Move {
                to: State::Done, ..
            }) => Resume::Finished,
            Some(Move {
                from,
                to: State::Stuck,
                ..
            }) => Resume::Stuck(from.with_context(|| {
                format!("the run log holds no state unit {unit} left before it got stuck")
            })?),
            Some(Move { to, .. }) => Resume::At(to),
        })
    }

    /// The last move of `unit`, or `None` when it has never run.
    pub fn last_move(&self, unit: &str) -> Result<Option<Move>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e).with_context(|| format!("cannot read {}", self.path.display()))
            }
        };
        let last = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .rfind(|e| e["unit"] == unit && e.get("to").is_some());
        let Some(last) = last else {
            return Ok(None);
        };
        let to = last["to"]
            .as_str()
            .and_then(State::parse)
            .with_context(|| format!("the run log holds a bad to state: {last}"))?;
        Ok(Some(Move {
            from: last["from"].as_str().and_then(State::parse),
            to,
            at: last["at"].as_u64(),
            reason: last["reason"].as_str().and_then(Reason::parse),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_with(text: &str) -> (tempfile::TempDir, RunLog) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("runs.jsonl");
        std::fs::write(&path, text).unwrap();
        (tmp, RunLog { path })
    }

    #[test]
    fn a_finished_unit_is_finished_whatever_its_last_from_says() {
        let (_tmp, log) = log_with(r#"{"unit":"5","from":"nowhere","to":"done","at":1}"#);
        assert_eq!(log.resume("5").unwrap(), Resume::Finished);
        assert_eq!(log.last_move("5").unwrap().unwrap().from, None);
    }

    #[test]
    fn a_unit_stuck_after_a_state_picks_up_at_it() {
        let (_tmp, log) = log_with(r#"{"unit":"5","from":"review","to":"stuck","at":1}"#);
        assert_eq!(log.resume("5").unwrap(), Resume::Stuck(State::Review));
    }

    #[test]
    fn a_stuck_unit_with_no_state_to_go_back_to_cannot_be_resumed() {
        let (_tmp, log) = log_with(r#"{"unit":"5","from":"nowhere","to":"stuck","at":1}"#);
        let err = log.resume("5").unwrap_err();
        assert!(
            err.to_string().contains("unit 5 left before it got stuck"),
            "{err}"
        );
    }

    #[test]
    fn a_move_says_why_the_unit_did_not_go_on() {
        let (_tmp, log) = log_with(
            r#"{"unit":"5","from":"build","to":"stuck","reason":"went_wrong"}
{"unit":"6","from":"verify","to":"stuck","reason":"failed"}
{"unit":"7","from":"verify","to":"stuck","reason":"odd"}
{"unit":"8","from":"build","to":"verify"}"#,
        );
        let reason = |unit| log.last_move(unit).unwrap().unwrap().reason;
        assert_eq!(reason("5"), Some(Reason::WentWrong));
        assert_eq!(reason("6"), Some(Reason::Failed));
        assert_eq!(reason("7"), None);
        assert_eq!(reason("8"), None);
    }

    #[test]
    fn a_last_move_to_nowhere_is_an_error() {
        let (_tmp, log) = log_with(r#"{"unit":"5","from":"build","to":"nowhere"}"#);
        assert!(log.last_move("5").is_err());
    }
}
