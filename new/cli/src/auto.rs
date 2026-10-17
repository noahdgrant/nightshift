//! What `ns auto` decides from the units a repository has open: which to triage and which to run,
//! in what order. Pure: it reads units and changes nothing.

use crate::forge::Unit;

/// A unit that is ready for the agent.
pub const READY: &str = "status:ready-for-agent";
/// A unit nobody has looked at since it was filed.
pub const NEEDS_TRIAGE: &str = "status:needs-triage";
/// Every state label starts with this.
pub const STATE_PREFIX: &str = "status:";
/// Priority labels, highest first.
const PRIORITIES: [&str; 4] = [
    "priority:critical",
    "priority:high",
    "priority:medium",
    "priority:low",
];

pub struct Plan<'a> {
    /// Ready units to run, first to run first.
    pub queue: Vec<&'a Unit>,
    /// Units to triage, first to triage first.
    pub triage: Vec<&'a Unit>,
    /// Units left out and why.
    pub skipped: Vec<(&'a Unit, String)>,
}

/// Sorts `open` units into the queue, the triage list and the skipped.
pub fn plan(open: &[Unit]) -> Plan<'_> {
    let mut plan = Plan {
        queue: Vec::new(),
        triage: Vec::new(),
        skipped: Vec::new(),
    };
    for unit in open {
        if !unit.from_team {
            plan.skipped
                .push((unit, "its author is not on the team".into()));
            continue;
        }
        let blockers: Vec<String> = blockers(&unit.body)
            .into_iter()
            .filter(|n| open.iter().any(|u| u.number == *n))
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
        } else if untriaged {
            plan.triage.push(unit);
        }
    }
    plan.queue.sort_by_key(|u| order(u));
    plan.triage.sort_by_key(|u| order(u));
    plan
}

/// Highest priority first, then lowest number. A unit with no priority goes last.
fn order(unit: &Unit) -> (usize, u64) {
    let rank = PRIORITIES
        .iter()
        .position(|p| unit.labels.iter().any(|l| l == p))
        .unwrap_or(PRIORITIES.len());
    (rank, unit.number)
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
        assert_eq!(numbers(&plan(&open).queue), [2, 9, 3, 4]);
    }

    #[test]
    fn only_the_teams_units_are_worked() {
        let open = [unit(1, &[READY], "", false), unit(2, &[], "", false)];
        let plan = plan(&open);
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
        let plan = plan(&open);
        assert_eq!(numbers(&plan.queue), [3]);
        let why: Vec<_> = plan
            .skipped
            .iter()
            .map(|(u, w)| (u.number, w.as_str()))
            .collect();
        assert_eq!(why, [(1, "blocked by #2"), (4, "blocked by #2")]);
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
        assert_eq!(numbers(&plan(&open).triage), [5, 1, 2]);
    }
}
