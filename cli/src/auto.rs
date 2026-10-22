//! What `ns auto` decides from the units a repository has open: which to triage and which to run,
//! in what order. Pure: it reads units and changes nothing.

use std::collections::HashMap;

use crate::forge::Unit;

/// A unit that is ready for the agent.
pub const READY: &str = "status:ready-for-agent";
/// A unit nobody has looked at since it was filed.
pub const NEEDS_TRIAGE: &str = "status:needs-triage";
/// Every state label starts with this.
pub const STATE_PREFIX: &str = "status:";
/// Priority labels, highest first.
pub const PRIORITIES: [&str; 4] = [
    "priority:critical",
    "priority:high",
    "priority:medium",
    "priority:low",
];

/// The units in a row whose harness session failed, and how many of them stop the night.
pub struct Streak {
    /// `None` never stops.
    limit: Option<usize>,
    units: Vec<u64>,
}

impl Streak {
    /// A streak that stops at `limit` units. `0` turns it off.
    pub fn new(limit: usize) -> Streak {
        Streak {
            limit: (limit > 0).then_some(limit),
            units: Vec::new(),
        }
    }

    /// Notes how unit `unit` ended: a failed session adds to the streak, anything else ends it.
    pub fn record(&mut self, unit: u64, session_failed: bool) {
        if session_failed {
            self.units.push(unit);
        } else {
            self.units.clear();
        }
    }

    /// Whether enough units in a row failed that no more should be started.
    pub fn reached(&self) -> bool {
        self.limit.is_some_and(|n| self.units.len() >= n)
    }

    /// The units in the streak, in the order they failed.
    pub fn units(&self) -> &[u64] {
        &self.units
    }
}

pub struct Plan<'a> {
    /// Ready units to run, first to run first.
    pub queue: Vec<&'a Unit>,
    /// Units to triage, first to triage first.
    pub triage: Vec<&'a Unit>,
    /// Units that need a reproduction, first to troubleshoot first.
    pub repro: Vec<&'a Unit>,
    /// Units left out and why.
    pub skipped: Vec<(&'a Unit, String)>,
}

/// Sorts `open` units into the queue, the triage list and the skipped. `native` holds, by unit, the
/// open units the forge itself says block it.
pub fn plan<'a>(open: &'a [Unit], native: &HashMap<u64, Vec<u64>>) -> Plan<'a> {
    let mut plan = Plan {
        queue: Vec::new(),
        triage: Vec::new(),
        repro: Vec::new(),
        skipped: Vec::new(),
    };
    for unit in open {
        if !unit.from_team {
            plan.skipped
                .push((unit, "its author is not on the team".into()));
            continue;
        }
        let blockers: Vec<String> = blockers_of(unit, open, native)
            .into_iter()
            .map(|n| format!("#{n}"))
            .collect();
        let has = |label: &str| unit.labels.iter().any(|l| l == label);
        let untriaged =
            has(NEEDS_TRIAGE) || !unit.labels.iter().any(|l| l.starts_with(STATE_PREFIX));
        if has(READY) {
            if blockers.is_empty() {
                plan.queue.push(unit);
            } else {
                plan.skipped
                    .push((unit, format!("blocked by {}", blockers.join(", "))));
            }
        } else if has(crate::status::NEEDS_REPRO) {
            plan.repro.push(unit);
        } else if untriaged {
            plan.triage.push(unit);
        }
    }
    plan.queue.sort_by_key(|u| order(u));
    plan.triage.sort_by_key(|u| order(u));
    plan.repro.sort_by_key(|u| order(u));
    plan
}

/// Highest priority first, then lowest number. A unit with no priority goes last.
pub fn order(unit: &Unit) -> (usize, u64) {
    let rank = PRIORITIES
        .iter()
        .position(|p| unit.labels.iter().any(|l| l == p))
        .unwrap_or(PRIORITIES.len());
    (rank, unit.number)
}

/// The open units that block `unit`: those its `Blocked by:` line names and those in `native`, each
/// once.
pub fn blockers_of(unit: &Unit, open: &[Unit], native: &HashMap<u64, Vec<u64>>) -> Vec<u64> {
    let mut found: Vec<u64> = Vec::new();
    let from_text = blockers(&unit.body)
        .into_iter()
        .filter(|n| open.iter().any(|u| u.number == *n));
    let from_forge = native.get(&unit.number).into_iter().flatten().copied();
    for n in from_text.chain(from_forge) {
        if !found.contains(&n) {
            found.push(n);
        }
    }
    found
}

/// The units named by a `Blocked by: #a, #b` line, which is the first line of the body, or the
/// first after a line that starts with 🤖.
fn blockers(body: &str) -> Vec<u64> {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = match lines.next() {
        Some(l) if l.starts_with('🤖') => lines.next(),
        other => other,
    };
    let Some(list) = first.and_then(|l| l.strip_prefix("Blocked by:")) else {
        return Vec::new();
    };
    list.split(',')
        .filter_map(|n| n.trim().strip_prefix('#')?.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_streak_is_reached_at_its_limit_and_a_success_ends_it() {
        let mut streak = Streak::new(2);
        streak.record(1, true);
        assert!(!streak.reached());
        streak.record(2, false);
        streak.record(3, true);
        assert!(!streak.reached());
        streak.record(4, true);
        assert!(streak.reached());
        assert_eq!(streak.units(), [3, 4]);
    }

    #[test]
    fn a_streak_with_a_limit_of_zero_is_never_reached() {
        let mut streak = Streak::new(0);
        for unit in 1..=5 {
            streak.record(unit, true);
        }
        assert!(!streak.reached());
    }

    fn unit(number: u64, labels: &[&str], body: &str, from_team: bool) -> Unit {
        Unit {
            number,
            title: format!("unit {number}"),
            body: body.into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            from_team,
        }
    }

    fn numbers(units: &[&Unit]) -> Vec<u64> {
        units.iter().map(|u| u.number).collect()
    }

    #[test]
    fn the_queue_runs_the_highest_priority_first_then_the_oldest() {
        let open = [
            unit(4, &[READY], "", true),
            unit(3, &[READY, "priority:low"], "", true),
            unit(9, &[READY, "priority:high"], "", true),
            unit(2, &[READY, "priority:high"], "", true),
        ];
        assert_eq!(numbers(&plan(&open, &HashMap::new()).queue), [2, 9, 3, 4]);
    }

    #[test]
    fn only_the_teams_units_are_worked() {
        let open = [unit(1, &[READY], "", false), unit(2, &[], "", false)];
        let plan = plan(&open, &HashMap::new());
        assert!(plan.queue.is_empty() && plan.triage.is_empty());
        assert_eq!(plan.skipped.len(), 2);
    }

    #[test]
    fn a_unit_with_an_open_blocker_waits_and_one_with_a_closed_blocker_runs() {
        let open = [
            unit(1, &[READY], "Blocked by: #2, #7", true),
            unit(2, &["status:needs-define"], "", true),
            unit(
                3,
                &[READY],
                "🤖 *Posted by an AI agent.*\n\nBlocked by: #7",
                true,
            ),
            unit(4, &[READY], "Blocked by: #2", true),
        ];
        let plan = plan(&open, &HashMap::new());
        assert_eq!(numbers(&plan.queue), [3]);
        let why: Vec<_> = plan
            .skipped
            .iter()
            .map(|(u, w)| (u.number, w.as_str()))
            .collect();
        assert_eq!(why, [(1, "blocked by #2"), (4, "blocked by #2")]);
    }

    #[test]
    fn a_unit_the_forge_says_is_blocked_waits_and_both_sources_name_a_blocker_once() {
        let open = [
            unit(1, &[READY], "", true),
            unit(2, &[READY], "Blocked by: #5", true),
            unit(3, &[READY], "Blocked by: #4", true),
            unit(4, &["status:needs-define"], "", true),
            unit(5, &["status:needs-define"], "", true),
            unit(6, &[READY], "", true),
        ];
        // The forge lists only open blockers: unit 6 has none, and 1's are #5 alone.
        let native = HashMap::from([(1, vec![5]), (2, vec![5]), (3, vec![9])]);
        let plan = plan(&open, &native);
        assert_eq!(numbers(&plan.queue), [6]);
        let why: Vec<_> = plan
            .skipped
            .iter()
            .map(|(u, w)| (u.number, w.as_str()))
            .collect();
        assert_eq!(
            why,
            [
                (1, "blocked by #5"),
                (2, "blocked by #5"),
                (3, "blocked by #4, #9")
            ]
        );
    }

    #[test]
    fn units_that_need_a_reproduction_are_listed_to_troubleshoot() {
        let open = [
            unit(2, &[crate::status::NEEDS_REPRO], "", true),
            unit(1, &[crate::status::NEEDS_REPRO, "priority:high"], "", true),
            unit(3, &[crate::status::NEEDS_REPRO], "", false),
        ];
        let plan = plan(&open, &HashMap::new());
        assert_eq!(numbers(&plan.repro), [1, 2]);
        assert!(plan.queue.is_empty() && plan.triage.is_empty());
    }

    #[test]
    fn units_nobody_has_triaged_are_triaged_and_those_in_another_state_are_left_alone() {
        let open = [
            unit(1, &[], "", true),
            unit(2, &[NEEDS_TRIAGE], "", true),
            unit(3, &["status:needs-info"], "", true),
            unit(4, &["status:in-progress"], "", true),
            unit(5, &["priority:high"], "", true),
        ];
        assert_eq!(numbers(&plan(&open, &HashMap::new()).triage), [5, 1, 2]);
    }
}
