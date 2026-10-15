//! `[limits]`: what bounds a night and each unit (docs/FACTORY.md).

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    /// Unset: no cap; `--until`, the queue and usage limits end the night.
    pub max_units: Option<u32>,
    /// Unset: no cap under `billing = "subscription"`, 25.0 under `api`.
    pub budget_usd: Option<f64>,
    /// Unset: no cap. Set: each phase and the gate run under this many MiB (`crate::memcap`).
    pub memory_mb: Option<u64>,
    /// Unset: the machine's total less 4 GB. The memory all the units `ns watch` runs share, in
    /// MiB (`crate::memory`).
    pub total_memory_mb: Option<u64>,
    /// Units `ns watch` runs at once; `--parallel` wins. Unset: 1.
    pub parallel: Option<u32>,
    /// GiB free under which `ns watch` starts no unit until space returns. Unset: 20.
    pub disk_wait_gb: Option<u64>,
    /// GiB free under which `ns watch` stops the night. Unset: 10.
    pub disk_stop_gb: Option<u64>,
}

impl Limits {
    pub fn disk_wait_gb(&self) -> u64 {
        self.disk_wait_gb.unwrap_or(20)
    }

    pub fn disk_stop_gb(&self) -> u64 {
        self.disk_stop_gb.unwrap_or(10)
    }

    pub(super) fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(b) = self.budget_usd {
            if b < 0.0 {
                out.push("limits.budget_usd must not be negative".into());
            }
        }
        if self.memory_mb == Some(0) {
            out.push("limits.memory_mb must be at least 1; remove it for no cap".into());
        }
        match (self.total_memory_mb, self.memory_mb) {
            (Some(0), _) => out.push(
                "limits.total_memory_mb must be at least 1; remove it for the machine's total less 4 GB"
                    .into(),
            ),
            (Some(total), Some(unit)) if total < unit => out.push(format!(
                "limits.total_memory_mb = {total} is under limits.memory_mb = {unit}, the cap of one unit"
            )),
            _ => {}
        }
        let (wait, stop) = (self.disk_wait_gb(), self.disk_stop_gb());
        if wait > 0 && stop > wait {
            out.push(format!("limits.disk_stop_gb = {stop} is above limits.disk_wait_gb = {wait}; the stop limit must not exceed the wait limit"));
        }
        out
    }

    pub fn disk(&self) -> crate::disk::DiskLimits {
        crate::disk::DiskLimits {
            wait_gb: self.disk_wait_gb(),
            stop_gb: self.disk_stop_gb(),
        }
    }
}
