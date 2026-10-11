//! The CI state of the default branch's head, which `[queue] fix_main_first` reads
//! (docs/FACTORY.md, Main's CI).

use anyhow::Result;
use serde::Serialize;

use super::forge::Forge;
use crate::factory::Factory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Running,
}

/// One check run or commit status on the head. `required` is the forge's word where it has one,
/// else true.
#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub outcome: Outcome,
    pub required: bool,
}

/// The default branch's head and the checks on it.
#[derive(Debug, Clone, Default)]
pub struct HeadChecks {
    pub sha: String,
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// `fix_main_first` is off, so nothing was read.
    Off,
    /// No check ran on the head.
    None,
    Pending,
    Green,
    Red,
}

#[derive(Debug, Clone, Serialize)]
pub struct MainCi {
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// The checks that make the state red.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failing: Vec<String>,
}

impl MainCi {
    /// Red holds the queue under `fix_main_first`; pending and no CI don't.
    pub fn holds_queue(&self) -> bool {
        self.state == State::Red
    }

    /// What `ns watch` says once at start when `fix_main_first` has nothing to act on.
    pub fn notice(&self) -> Option<&'static str> {
        match self.state {
            State::Off => Some("fix_main_first is off; main's CI is not read"),
            State::None => Some("fix_main_first does nothing: main's head has no CI to read"),
            _ => None,
        }
    }
}

/// Main's CI state, read through `forge` unless `[queue] fix_main_first` is off.
pub fn read(forge: &dyn Forge, fac: &Factory) -> Result<MainCi> {
    if !fac.queue.fix_main_first {
        return Ok(MainCi {
            state: State::Off,
            sha: None,
            failing: Vec::new(),
        });
    }
    Ok(classify(forge.main_checks()?))
}

/// Red: a check failed while others still run, or every check finished and a required one
/// failed. Green: every check finished and no required one failed.
fn classify(head: HeadChecks) -> MainCi {
    let running = head.checks.iter().any(|c| c.outcome == Outcome::Running);
    let failing: Vec<String> = head
        .checks
        .iter()
        .filter(|c| c.outcome == Outcome::Fail && (running || c.required))
        .map(|c| c.name.clone())
        .collect();
    let state = if head.checks.is_empty() {
        State::None
    } else if !failing.is_empty() {
        State::Red
    } else if running {
        State::Pending
    } else {
        State::Green
    };
    MainCi {
        state,
        sha: Some(head.sha),
        failing,
    }
}
