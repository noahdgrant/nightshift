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
    /// Where the forge keeps the check's log ([`Forge::check_log`]), when it keeps one.
    pub log: Option<String>,
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
    /// The checks that make the state red, shown by name.
    #[serde(serialize_with = "names", skip_serializing_if = "Vec::is_empty")]
    pub failing: Vec<Check>,
}

fn names<S: serde::Serializer>(checks: &[Check], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(checks.iter().map(|c| &c.name))
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

/// [`read`], or `None` and a warning when main's CI can't be read: that holds nothing.
pub fn read_or_warn(forge: &dyn Forge, fac: &Factory) -> Option<MainCi> {
    match read(forge, fac) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("ns watch: warning: cannot read main's CI: {e:#}");
            None
        }
    }
}

/// Red: a check failed while others still run, or every check finished and a required one
/// failed. Green: every check finished and no required one failed.
fn classify(head: HeadChecks) -> MainCi {
    let running = head.checks.iter().any(|c| c.outcome == Outcome::Running);
    let failing: Vec<Check> = head
        .checks
        .iter()
        .filter(|c| c.outcome == Outcome::Fail && (running || c.required))
        .cloned()
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
