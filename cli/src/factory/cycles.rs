//! `[phases.review]`'s fix-cycle limits: how many fix cycles `ns run` drives before the review
//! loop stops with findings open (docs/factory/run.md, "Review loop").

use super::Factory;

/// The fix cycles review runs before it stops with findings open: `full` cycles, then
/// `fix_only` cycles that hand over only findings with fewer than two fix attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cycles {
    pub full: u32,
    pub fix_only: u32,
}

impl Default for Cycles {
    fn default() -> Self {
        Self {
            full: 3,
            fix_only: 2,
        }
    }
}

/// A cycle count as written, `None` when it is not a whole number from 0 up.
fn cycle_count(v: &toml::Value) -> Option<u32> {
    v.as_integer().and_then(|n| u32::try_from(n).ok())
}

impl Factory {
    /// Review's fix-cycle limits from `[phases.review]`, each falling back to its default when
    /// missing or invalid.
    pub fn cycles(&self) -> Cycles {
        let d = Cycles::default();
        let review = self.phases.get("review");
        let read = |v: Option<&toml::Value>, default| v.and_then(cycle_count).unwrap_or(default);
        Cycles {
            full: read(review.and_then(|p| p.full_cycles.as_ref()), d.full),
            fix_only: read(review.and_then(|p| p.fix_only_cycles.as_ref()), d.fix_only),
        }
    }

    /// A warning for each limit in `[phases.review]` that `cycles` falls back from.
    pub(super) fn cycle_warnings(&self) -> Vec<String> {
        let Some(review) = self.phases.get("review") else {
            return vec![];
        };
        let d = Cycles::default();
        [
            ("full_cycles", &review.full_cycles, d.full),
            ("fix_only_cycles", &review.fix_only_cycles, d.fix_only),
        ]
        .into_iter()
        .filter_map(|(key, v, default)| {
            let v = v.as_ref().filter(|v| cycle_count(v).is_none())?;
            Some(format!(
                "[phases.review] {key} = {v} is not a whole number from 0 up, so ns run uses the default, {default}"
            ))
        })
        .collect()
    }

    /// A problem for each limit set on phase `name` other than review, which nothing reads.
    pub(super) fn cycle_problems(&self, name: &str) -> Vec<String> {
        let Some(set) = self.phases.get(name).filter(|_| name != "review") else {
            return vec![];
        };
        [
            ("full_cycles", &set.full_cycles),
            ("fix_only_cycles", &set.fix_only_cycles),
        ]
        .into_iter()
        .filter(|(_, v)| v.is_some())
        .map(|(key, _)| {
            format!("phase {name}: {key} is only read for review; move it to [phases.review]")
        })
        .collect()
    }
}
