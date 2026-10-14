//! How Critical and Important findings arrive and get fixed, over every review attempt.

use serde::Serialize;

use super::metrics::Unit;
use crate::review_md::{Severity, Status};
use crate::stats::{median, round};

/// Review passes with their own bucket in `raised_per_pass`: pass 0 and fix cycles 1–5.
const PASS_BUCKETS: usize = 6;

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct BySeverity<T> {
    pub critical: T,
    pub important: T,
}

/// Findings first raised in each pass: `passes[n]` for pass `n`, `later` past pass 5.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Raised {
    pub passes: [usize; PASS_BUCKETS],
    pub later: usize,
}

/// Open findings and fixed ones with a known attempt count. Dismissed and deferred findings
/// are left out, and so is a fix with no attempt count.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct FixRate {
    pub findings: usize,
    pub fixed_first_attempt: usize,
    pub fixed_second_attempt: usize,
    pub fixed_later_attempt: usize,
    pub open: usize,
    pub fixed_attempts_unknown: usize,
    /// fixed_first_attempt / findings.
    pub first_attempt_rate: Option<f64>,
    /// fixed_second_attempt / (findings - fixed_first_attempt).
    pub second_attempt_rate: Option<f64>,
    /// open / findings.
    pub open_rate: Option<f64>,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct CyclesToFix {
    pub findings: usize,
    pub median: Option<f64>,
    pub max: Option<u32>,
    /// Fixed in the cycle after the one that raised it.
    pub next_cycle: usize,
    /// Fixed, but with no fixed cycle or one before the cycle that raised it.
    pub left_out: usize,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Share {
    pub count: usize,
    pub findings: usize,
    pub rate: Option<f64>,
}

impl Share {
    fn of(count: usize, findings: usize) -> Share {
        Share {
            count,
            findings,
            rate: ratio(count, findings),
        }
    }
}

fn ratio(n: usize, d: usize) -> Option<f64> {
    (d > 0).then(|| round(n as f64 / d as f64, 3))
}

/// How Critical and Important findings arrive and get fixed, over every review attempt.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct FindingFlow {
    /// Critical and Important findings with a known `cycle`.
    pub findings: usize,
    /// Critical and Important findings with no `cycle`, left out of every number here.
    pub without_cycle: usize,
    pub raised_per_pass: BySeverity<Raised>,
    pub fix_rate: BySeverity<FixRate>,
    pub cycles_to_fix: CyclesToFix,
    pub dismiss_rate: Share,
    pub late_finding_rate: Share,
}

/// Each attempt is its own review loop, so every attempt's findings count, not only the newest.
pub fn finding_flow(units: &[&Unit]) -> FindingFlow {
    let mut f = FindingFlow::default();
    let (mut dismissed, mut late, mut to_fix) = (0, 0, Vec::new());
    for x in units
        .iter()
        .flat_map(|u| &u.attempts)
        .flat_map(|a| &a.findings)
    {
        let (raised, rate) = match x.severity {
            Severity::Critical => (&mut f.raised_per_pass.critical, &mut f.fix_rate.critical),
            Severity::Important => (&mut f.raised_per_pass.important, &mut f.fix_rate.important),
            _ => continue,
        };
        let Some(cycle) = x.cycle else {
            f.without_cycle += 1;
            continue;
        };
        f.findings += 1;
        match raised.passes.get_mut(cycle as usize) {
            Some(n) => *n += 1,
            None => raised.later += 1,
        }
        late += usize::from(cycle > 0);
        match (x.status, x.fix_attempts) {
            (Status::Dismissed, _) => dismissed += 1,
            (Status::Open, _) => rate.open += 1,
            (Status::Fixed, Some(1)) => rate.fixed_first_attempt += 1,
            (Status::Fixed, Some(2)) => rate.fixed_second_attempt += 1,
            (Status::Fixed, Some(3..)) => rate.fixed_later_attempt += 1,
            (Status::Fixed, _) => rate.fixed_attempts_unknown += 1,
            _ => {}
        }
        if x.status == Status::Fixed {
            match x.fixed_cycle.and_then(|c| c.checked_sub(cycle)) {
                Some(n) => to_fix.push(n),
                None => f.cycles_to_fix.left_out += 1,
            }
        }
    }
    for r in [&mut f.fix_rate.critical, &mut f.fix_rate.important] {
        r.findings =
            r.fixed_first_attempt + r.fixed_second_attempt + r.fixed_later_attempt + r.open;
        r.first_attempt_rate = ratio(r.fixed_first_attempt, r.findings);
        r.second_attempt_rate = ratio(r.fixed_second_attempt, r.findings - r.fixed_first_attempt);
        r.open_rate = ratio(r.open, r.findings);
    }
    f.cycles_to_fix = CyclesToFix {
        findings: to_fix.len(),
        median: median(to_fix.iter().map(|c| f64::from(*c)).collect()),
        max: to_fix.iter().copied().max(),
        next_cycle: to_fix.iter().filter(|c| **c == 1).count(),
        left_out: f.cycles_to_fix.left_out,
    };
    f.dismiss_rate = Share::of(dismissed, f.findings);
    f.late_finding_rate = Share::of(late, f.findings);
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::metrics::{Attempt, RunStats};
    use crate::review_md::parse;

    fn attempt(cycles: Option<u32>, status: &str, review: &str) -> Attempt {
        Attempt {
            path: ".ns/u/review.md".into(),
            status: Some(status.into()),
            cycles,
            findings: parse(review),
            ..Attempt::default()
        }
    }

    fn unit(attempts: Vec<Attempt>) -> Unit {
        Unit {
            id: "u".into(),
            worktree: None,
            attempts,
            day: None,
            run: RunStats::default(),
        }
    }

    /// Both severities, a fix-only cycle (5), a fix on the second attempt, a dismissal, a pass
    /// past 5, a record from before `Cycle:`, and fixes with no cycle, a cycle before the raise
    /// or no attempt count.
    const FLOW: &str = "\
### C1. a
- Cycle: 0
- Attempts: 1
- Status: fixed (cycle 1, x)
### C2. b
- Cycle: 3
- Attempts: 2
- Status: fixed (cycle 5, x)
### I1. c
- Cycle: 0
- Attempts: 1
- Status: fixed (cycle 1, x)
### I2. d
- Cycle: 1
- Attempts: 2
- Status: open
### I3. e
- Cycle: 2
- Status: dismissed: wrong
### I4. f
- Cycle: 6
- Status: fixed (cycle 3, x)
### I5. g
- Cycle: 0
- Scope: pre-existing
- Status: deferred: #9
### I6. h
- Cycle: 1
- Status: fixed
### I7. old
- Status: fixed (cycle 1, x)
### S1. s
- Cycle: 0
- Attempts: 1
- Status: fixed (cycle 1, x)
";

    #[test]
    fn finding_flow_reads_every_attempt_of_every_unit() {
        let older = attempt(
            Some(1),
            "blocked",
            "### I1. z\n- Cycle: 0\n- Attempts: 1\n- Status: fixed (cycle 1, x)\n",
        );
        let u = unit(vec![older, attempt(Some(5), "pass", FLOW)]);
        let f = finding_flow(&[&u]);
        assert_eq!((f.findings, f.without_cycle), (9, 1));
        assert_eq!(
            f.raised_per_pass,
            BySeverity {
                critical: Raised {
                    passes: [1, 0, 0, 1, 0, 0],
                    later: 0
                },
                important: Raised {
                    passes: [3, 2, 1, 0, 0, 0],
                    later: 1
                },
            }
        );
        assert_eq!(
            f.fix_rate.critical,
            FixRate {
                findings: 2,
                fixed_first_attempt: 1,
                fixed_second_attempt: 1,
                fixed_later_attempt: 0,
                open: 0,
                fixed_attempts_unknown: 0,
                first_attempt_rate: Some(0.5),
                second_attempt_rate: Some(1.0),
                open_rate: Some(0.0),
            }
        );
        // I1 twice (both attempts) and I2 count; I4 and I6 are fixed with no attempt count.
        assert_eq!(
            f.fix_rate.important,
            FixRate {
                findings: 3,
                fixed_first_attempt: 2,
                fixed_second_attempt: 0,
                fixed_later_attempt: 0,
                open: 1,
                fixed_attempts_unknown: 2,
                first_attempt_rate: Some(0.667),
                second_attempt_rate: Some(0.0),
                open_rate: Some(0.333),
            }
        );
        // C1, I1, I1 in one cycle and C2 in two; I4 was fixed before it was raised, I6 has
        // no fixed cycle.
        assert_eq!(
            f.cycles_to_fix,
            CyclesToFix {
                findings: 4,
                median: Some(1.0),
                max: Some(2),
                next_cycle: 3,
                left_out: 2,
            }
        );
        assert_eq!(
            f.dismiss_rate,
            Share {
                count: 1,
                findings: 9,
                rate: Some(0.111)
            }
        );
        assert_eq!(
            f.late_finding_rate,
            Share {
                count: 5,
                findings: 9,
                rate: Some(0.556)
            }
        );
    }

    #[test]
    fn finding_flow_with_nothing_to_count_has_no_rates() {
        let f = finding_flow(&[]);
        assert_eq!((f.findings, f.without_cycle), (0, 0));
        assert_eq!(f.raised_per_pass, BySeverity::default());
        assert_eq!(f.fix_rate.critical, FixRate::default());
        assert_eq!(f.fix_rate.critical.open_rate, None);
        assert_eq!(f.cycles_to_fix.median, None);
        assert_eq!(f.cycles_to_fix.max, None);
        assert_eq!(f.dismiss_rate.rate, None);
        assert_eq!(f.late_finding_rate.rate, None);
        // Every one fixed on attempt 1 leaves no rest to rate.
        let all_first = attempt(
            Some(1),
            "pass",
            "### I1. a\n- Cycle: 0\n- Attempts: 1\n- Status: fixed (cycle 1, x)\n",
        );
        let f = finding_flow(&[&unit(vec![all_first])]);
        assert_eq!(f.fix_rate.important.first_attempt_rate, Some(1.0));
        assert_eq!(f.fix_rate.important.second_attempt_rate, None);
    }

    #[test]
    fn a_fix_after_two_attempts_is_neither_first_nor_second() {
        let a = attempt(Some(5), "pass", "### C1. a\n- Cycle: 2\n- Attempts: 3\n- Status: fixed (cycle 5, x)\n### C2. b\n- Cycle: 0\n- Attempts: 0\n- Status: fixed (cycle 1, x)\n");
        let f = finding_flow(&[&unit(vec![a])]);
        let c = &f.fix_rate.critical;
        assert_eq!((c.findings, c.fixed_later_attempt), (1, 1));
        assert_eq!(c.fixed_attempts_unknown, 1);
        assert_eq!(
            (c.first_attempt_rate, c.second_attempt_rate),
            (Some(0.0), Some(0.0))
        );
        assert_eq!(f.cycles_to_fix.median, Some(2.0));
        assert_eq!(f.cycles_to_fix.next_cycle, 1);
    }
}
