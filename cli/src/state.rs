//! The states a unit moves through. `next` and `on_fail` are the whole transition table, and an
//! [`Outcome`] says which way a state's run points.

use std::fmt;

/// How one run of a state ended.
pub enum Outcome {
    /// The state did its work.
    Pass,
    /// `ship` did its work but the change has not merged, so a person merges it: the unit skips
    /// `retrospective` and goes to `clean_up`.
    Unmerged,
    /// The state ran and found the work before it wrong. The unit goes to `State::on_fail`.
    Fail,
    /// The state ran and could not prove the work either way. The unit runs the same state once
    /// more, shown what was left unproven.
    Inconclusive,
    /// The state ran again and still could not prove the work. The unit is stuck.
    Unprovable,
    /// The state could not go on because the base branch kept moving. The unit is stuck.
    BaseMoved,
    /// The state could not resume the session it needed to. The unit is stuck for a human.
    Unresumable,
    /// The state itself went wrong. The unit runs the same state again.
    Redo,
    /// The harness has no usage left and `ns` cannot wait for it. The run stops with the unit as
    /// it was, and the state is not counted as an attempt.
    Halt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Setup,
    Build,
    Verify,
    Review,
    Ship,
    Retrospective,
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
            State::Retrospective,
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
            State::Ship => Some(State::Retrospective),
            State::Retrospective => Some(State::CleanUp),
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
            State::Retrospective => "retrospective",
            State::CleanUp => "clean_up",
            State::Done => "done",
            State::Stuck => "stuck",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrospective_sits_between_ship_and_clean_up() {
        assert_eq!(State::Ship.next(), Some(State::Retrospective));
        assert_eq!(State::Retrospective.next(), Some(State::CleanUp));
        assert_eq!(State::parse("retrospective"), Some(State::Retrospective));
        assert_eq!(State::Retrospective.to_string(), "retrospective");
    }
}
