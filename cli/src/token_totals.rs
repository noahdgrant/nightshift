//! The tokens a unit and a night have used, totaled over their kept sessions. The four kinds are
//! never summed together, and a session whose tokens cannot be read is counted as unknown and
//! left out of every total, never counted as zero.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{Harness, Tokens};
use crate::state::State;
use crate::transcripts::Session;

/// The totals of each kind over the sessions whose tokens are known, and how many are not.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Sum {
    pub tokens: Tokens,
    /// How many sessions are in `tokens`.
    pub known: usize,
    /// How many sessions are left out of `tokens` because their tokens cannot be read.
    pub unknown: usize,
}

impl Sum {
    fn add(&mut self, tokens: Option<Tokens>) {
        match tokens {
            Some(t) => {
                self.tokens += t;
                self.known += 1;
            }
            None => self.unknown += 1,
        }
    }

    /// Adds `other`'s totals and counts to these.
    pub fn merge(&mut self, other: &Sum) {
        self.tokens += other.tokens;
        self.unknown += other.unknown;
        self.known += other.known;
    }

    /// The four totals, then how many sessions are known and unknown. With no known session there
    /// are no totals, which is not the same as totals of zero.
    pub fn json(&self) -> Value {
        let mut v = json!({ "known": self.known, "unknown": self.unknown });
        if self.known > 0 {
            v["input"] = self.tokens.input.into();
            v["output"] = self.tokens.output.into();
            v["cache_read"] = self.tokens.cache_read.into();
            v["cache_write"] = self.tokens.cache_write.into();
        }
        v
    }

    /// The totals as one line of text for a person, `unknown` mentioned when there are any.
    pub fn text(&self) -> String {
        let mut text = if self.known == 0 {
            "no tokens known".to_string()
        } else {
            format!(
                "input {}, output {}, cache_read {}, cache_write {}",
                self.tokens.input,
                self.tokens.output,
                self.tokens.cache_read,
                self.tokens.cache_write
            )
        };
        if self.unknown > 0 {
            text.push_str(&format!(", {} unreadable and left out", self.unknown));
        }
        text
    }
}

/// The totals over all sessions and by the state each ran in.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub all: Sum,
    pub by_state: BTreeMap<String, Sum>,
}

impl Totals {
    pub fn json(&self) -> Value {
        let mut v = self.all.json();
        v["by_state"] = Value::Object(
            self.by_state
                .iter()
                .map(|(state, sum)| (state.clone(), sum.json()))
                .collect(),
        );
        v
    }
}

/// Totals sessions given as the state each ran in and the tokens it used, `None` when unknown.
pub fn total(sessions: &[(State, Option<Tokens>)]) -> Totals {
    let mut totals = Totals::default();
    for (state, tokens) in sessions {
        totals.all.add(*tokens);
        totals
            .by_state
            .entry(state.to_string())
            .or_default()
            .add(*tokens);
    }
    totals
}

/// The one reader of kept transcripts for the four kinds, as `grades::kept_tokens` is for a grade.
/// Totals the kept `sessions`, reading each transcript. A transcript that cannot be read or says
/// no tokens is unknown.
pub fn read(harness: &dyn Harness, sessions: &[Session]) -> Totals {
    let used: Vec<_> = sessions
        .iter()
        .map(|s| {
            let tokens = std::fs::read(&s.path).ok().and_then(|raw| {
                harness
                    .transcript_usage(&String::from_utf8_lossy(&raw))
                    .tokens
            });
            (s.state, tokens)
        })
        .collect();
    total(&used)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Tokens {
        Tokens {
            input,
            output,
            cache_read,
            cache_write,
        }
    }

    #[test]
    fn kinds_are_totaled_apart_and_by_state() {
        let totals = total(&[
            (State::Build, Some(t(1, 2, 3, 4))),
            (State::Build, Some(t(10, 20, 30, 40))),
            (State::Verify, Some(t(100, 200, 300, 400))),
        ]);
        assert_eq!(
            totals.json(),
            json!({
                "known": 3, "unknown": 0,
                "input": 111, "output": 222, "cache_read": 333, "cache_write": 444,
                "by_state": {
                    "build": {"known": 2, "unknown": 0, "input": 11, "output": 22, "cache_read": 33, "cache_write": 44},
                    "verify": {"known": 1, "unknown": 0, "input": 100, "output": 200, "cache_read": 300, "cache_write": 400},
                }
            })
        );
    }

    #[test]
    fn an_unreadable_session_is_counted_unknown_and_left_out() {
        let totals = total(&[(State::Build, Some(t(1, 2, 3, 4))), (State::Build, None)]);
        assert_eq!(
            totals.json(),
            json!({
                "known": 1, "unknown": 1,
                "input": 1, "output": 2, "cache_read": 3, "cache_write": 4,
                "by_state": {
                    "build": {"known": 1, "unknown": 1, "input": 1, "output": 2, "cache_read": 3, "cache_write": 4},
                }
            })
        );
    }

    #[test]
    fn only_unreadable_sessions_give_no_totals_and_not_zeros() {
        let totals = total(&[(State::Review, None)]);
        assert_eq!(
            totals.json(),
            json!({
                "known": 0, "unknown": 1,
                "by_state": {"review": {"known": 0, "unknown": 1}}
            })
        );
        assert_eq!(
            total(&[]).json(),
            json!({"known": 0, "unknown": 0, "by_state": {}})
        );
    }

    #[test]
    fn merging_adds_totals_and_counts() {
        let mut a = total(&[(State::Build, Some(t(1, 2, 3, 4)))]).all;
        a.merge(
            &total(&[
                (State::Build, Some(t(10, 20, 30, 40))),
                (State::Build, None),
            ])
            .all,
        );
        assert_eq!(
            a.json(),
            json!({"known": 2, "unknown": 1, "input": 11, "output": 22, "cache_read": 33, "cache_write": 44})
        );
        let mut none = Sum::default();
        none.merge(&Sum::default());
        assert_eq!(none, Sum::default());
    }

    #[test]
    fn text_names_each_kind_and_the_unknown() {
        let sum = total(&[(State::Build, Some(t(1, 2, 3, 4))), (State::Build, None)]).all;
        assert_eq!(
            sum.text(),
            "input 1, output 2, cache_read 3, cache_write 4, 1 unreadable and left out"
        );
        assert_eq!(Sum::default().text(), "no tokens known");
    }
}
