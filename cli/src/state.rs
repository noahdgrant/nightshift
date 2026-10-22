//! The states a unit moves through. `next` and `on_fail` are the whole transition table, and an
//! [`Outcome`] says which way a state's run points.

use std::fmt;

/// How one run of a state ended.
pub enum Outcome {
    /// The state did its work.
    Pass,
    /// The state ran and found the work before it wrong. The unit goes to `State::on_fail`.
    Fail,
    /// The state ran and could not prove the work either way. The unit runs the same state once
    /// more, shown what was left unproven.
    Inconclusive,
    /// The state ran again and still could not prove the work. The unit is stuck for a human.
    Unprovable,
    /// The state itself went wrong. The unit runs the same state again.
    Redo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Setup,
    Build,
    Verify,
    Review,
    Ship,
    CleanUp,
    Done,
    Stuck,
}

impl State {
    /// The state called `name`, as it is printed.
    pub fn parse(name: &str) -> Option<State> {
        [
            State::Setup,
            State::Build,
            State::Verify,
            State::Review,
            State::Ship,
            State::CleanUp,
            State::Done,
            State::Stuck,
        ]
        .into_iter()
        .find(|s| s.to_string() == name)
    }

    pub const START: State = State::Setup;

    /// Where a unit goes when the state passes.
    pub fn next(self) -> Option<State> {
        match self {
            State::Setup => Some(State::Build),
            State::Build => Some(State::Verify),
            State::Verify => Some(State::Review),
            State::Review => Some(State::Ship),
            State::Ship => Some(State::CleanUp),
            State::CleanUp => Some(State::Done),
            State::Done | State::Stuck => None,
        }
    }

    /// Where a unit goes when the state finds the work before it wrong. `verify`, `review` and
    /// `ship` (a failed check on the change request) send the unit back to `build`; any other
    /// state runs again.
    pub fn on_fail(self) -> State {
        match self {
            State::Verify | State::Review | State::Ship => State::Build,
            other => other,
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            State::Setup => "setup",
            State::Build => "build",
            State::Verify => "verify",
            State::Review => "review",
            State::Ship => "ship",
            State::CleanUp => "clean_up",
            State::Done => "done",
            State::Stuck => "stuck",
        })
    }
}
