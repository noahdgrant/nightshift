//! What a night of `ns auto` did, as a file a person reads in the morning: the units that merged,
//! those that wait for a human and those that are stuck, each with its reason.

use chrono::NaiveDate;

use crate::runlog::Move;
use crate::state::State;
use crate::status;

/// What the summary file is called, for a person told where to look.
pub const NAME_PATTERN: &str = "night-YYYY-MM-DD.md";

/// The file of the night that started on `date`: `NAME_PATTERN` with the date in place.
pub fn file_name(date: NaiveDate) -> String {
    NAME_PATTERN.replace("YYYY-MM-DD", &date.format("%Y-%m-%d").to_string())
}

/// How the run of one unit ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Run {
    Done,
    Failed(String),
}

/// A unit that `ns auto` ran, and how its run ended.
#[derive(Debug, PartialEq, Eq)]
pub struct Ended {
    pub unit: u64,
    pub run: Run,
}

/// Where a unit ended up. Only the outcomes that need a person carry a reason.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Merged,
    Waiting(String),
    Stuck(String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub unit: u64,
    pub outcome: Outcome,
}

/// Where a unit that `ns auto` ran ended up. `labels` are its labels on the forge now, or why they
/// could not be read: a unit whose labels are unknown is never reported as merged. `last_move` is
/// its last move in the run log and `run` how its run ended.
pub fn entry_find(
    unit: u64,
    labels: Result<&[String], &str>,
    last_move: Option<&Move>,
    run: &Run,
) -> Entry {
    let error = match run {
        Run::Failed(e) => Some(e.as_str()),
        Run::Done => None,
    };
    let labels = match labels {
        Ok(labels) => labels,
        Err(e) => {
            let reason = format!("its labels could not be read from the forge: {e}");
            return Entry {
                unit,
                outcome: Outcome::Stuck(reason),
            };
        }
    };
    let has = |label: &str| labels.iter().any(|l| l == label);
    let outcome = if has(status::STUCK) {
        let reason = last_move
            .filter(|m| m.to == State::Stuck)
            .and_then(|m| m.reason)
            .map(|r| r.describe().to_string())
            .or_else(|| error.map(str::to_string))
            .unwrap_or_else(|| "the run log gives no reason".to_string());
        Outcome::Stuck(reason)
    } else if has(status::WAITING_FOR_HUMAN) || has(status::LEGACY_READY_FOR_HUMAN) {
        Outcome::Waiting(
            "a person merges it or decides: a merge, hardware, credentials or a decision".into(),
        )
    } else if let Some(error) = error {
        Outcome::Stuck(error.to_string())
    } else if labels
        .iter()
        .any(|l| l.starts_with(crate::auto::STATE_PREFIX))
    {
        Outcome::Stuck("the run ended and the unit is still marked as in progress or ready".into())
    } else {
        Outcome::Merged
    };
    Entry { unit, outcome }
}

/// The file's text. `stopped` is why the night ended before the units ran out, if it did.
pub fn render(date: NaiveDate, entries: &[Entry], stopped: Option<&str>) -> String {
    let mut text = format!("# ns auto, night of {date}\n");
    if let Some(why) = stopped {
        text.push_str(&format!("\nStopped early: {why}\n"));
    }
    if entries.is_empty() {
        text.push_str("\nNo unit was ready to run.\n");
        return text;
    }
    let mut merged = Vec::new();
    let mut waiting = Vec::new();
    let mut stuck = Vec::new();
    for e in entries {
        match &e.outcome {
            Outcome::Merged => merged.push(format!("- #{}\n", e.unit)),
            Outcome::Waiting(r) => waiting.push(format!("- #{}: {r}\n", e.unit)),
            Outcome::Stuck(r) => stuck.push(format!("- #{}: {r}\n", e.unit)),
        }
    }
    let sections = [
        ("Merged", merged),
        ("Waiting for a human", waiting),
        ("Stuck", stuck),
    ];
    for (title, lines) in sections {
        text.push_str(&format!("\n## {title}\n\n"));
        if lines.is_empty() {
            text.push_str("None\n");
        }
        for line in lines {
            text.push_str(&line);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runlog::Reason;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 10).unwrap()
    }

    #[test]
    fn the_file_is_named_by_date() {
        assert_eq!(file_name(date()), "night-2026-03-10.md");
    }

    #[test]
    fn a_night_with_no_unit_says_so() {
        assert_eq!(
            render(date(), &[], None),
            "# ns auto, night of 2026-03-10\n\nNo unit was ready to run.\n"
        );
    }

    #[test]
    fn a_night_with_units_lists_each_with_its_reason() {
        let entries = [
            Entry {
                unit: 4,
                outcome: Outcome::Merged,
            },
            Entry {
                unit: 7,
                outcome: Outcome::Waiting("a person merges it".into()),
            },
            Entry {
                unit: 9,
                outcome: Outcome::Stuck("boom".into()),
            },
            Entry {
                unit: 11,
                outcome: Outcome::Merged,
            },
        ];
        assert_eq!(
            render(date(), &entries, Some("2 units in a row went wrong")),
            "# ns auto, night of 2026-03-10\n\
             \n\
             Stopped early: 2 units in a row went wrong\n\
             \n\
             ## Merged\n\
             \n\
             - #4\n\
             - #11\n\
             \n\
             ## Waiting for a human\n\
             \n\
             - #7: a person merges it\n\
             \n\
             ## Stuck\n\
             \n\
             - #9: boom\n"
        );
    }

    #[test]
    fn a_section_with_no_unit_says_none() {
        let entries = [Entry {
            unit: 4,
            outcome: Outcome::Merged,
        }];
        let text = render(date(), &entries, None);
        assert!(text.contains("## Waiting for a human\n\nNone\n"), "{text}");
        assert!(text.contains("## Stuck\n\nNone\n"), "{text}");
    }

    fn find(labels: &[&str], last_move: Option<&Move>, run: &Run) -> Outcome {
        entry_find(1, Ok(&labels_of(labels)), last_move, run).outcome
    }

    fn labels_of(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_unit_is_sorted_by_its_label_and_a_stuck_one_gives_the_runs_reason() {
        let stuck_move = Move {
            from: Some(State::Verify),
            to: State::Stuck,
            at: Some(1),
            reason: Some(Reason::Inconclusive),
        };
        let failed = |e: &str| Run::Failed(e.to_string());
        assert_eq!(
            find(&[status::STUCK], Some(&stuck_move), &failed("x")),
            Outcome::Stuck("verification could not prove every requirement, twice".into())
        );
        assert_eq!(
            find(&[status::STUCK], None, &failed("the harness died")),
            Outcome::Stuck("the harness died".into())
        );
        assert_eq!(
            find(&[status::STUCK], None, &Run::Done),
            Outcome::Stuck("the run log gives no reason".into())
        );
        for label in [status::WAITING_FOR_HUMAN, status::LEGACY_READY_FOR_HUMAN] {
            assert!(matches!(
                find(&[label], None, &Run::Done),
                Outcome::Waiting(_)
            ));
        }
        assert_eq!(find(&[], None, &Run::Done), Outcome::Merged);
        assert_eq!(
            find(&[], None, &failed("cannot reach the forge")),
            Outcome::Stuck("cannot reach the forge".into())
        );
        assert!(matches!(
            find(&[status::IN_PROGRESS], None, &Run::Done),
            Outcome::Stuck(_)
        ));
    }

    #[test]
    fn a_unit_whose_labels_could_not_be_read_is_stuck_and_never_merged() {
        let e = entry_find(5, Err("rate limited"), None, &Run::Done);
        assert_eq!(
            e.outcome,
            Outcome::Stuck("its labels could not be read from the forge: rate limited".into())
        );
    }
}
