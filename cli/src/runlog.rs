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
    /// The unit stopped, or got stuck, at this state, and starts it again.
    At(State),
    /// The unit reached `done`.
    Finished,
}

/// One move of a unit from a state to the next.
#[derive(Debug, PartialEq, Eq)]
pub struct Move {
    pub from: State,
    pub to: State,
    /// When the log recorded it, in seconds since the epoch.
    pub at: Option<u64>,
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
            }) => Resume::At(from),
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
        let state = |key: &str| {
            last[key]
                .as_str()
                .and_then(State::parse)
                .with_context(|| format!("the run log holds a bad {key} state: {last}"))
        };
        Ok(Some(Move {
            from: state("from")?,
            to: state("to")?,
            at: last["at"].as_u64(),
        }))
    }
}
