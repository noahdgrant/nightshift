//! The night directory an `ns run` reads under `ns watch` (`ns run --night <dir>`): the night's
//! facts in `night.json`, the hold file a usage limit writes, and the spend file every unit adds
//! its phase costs to.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

const NIGHT: &str = "night.json";
const HOLD: &str = "hold.json";
const SPEND: &str = "spend.jsonl";

/// The night's facts a unit needs and can't read from its own files.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub watch_pid: u32,
    /// `--until`, which bounds a unit's lock waits.
    pub until: Option<i64>,
    /// The build gate command `ns watch` read at start.
    pub gate: Option<String>,
}

/// The directory a unit reads the night from, as `ns run --night <dir>`.
#[derive(Debug, Clone)]
pub struct NightDir(PathBuf);

impl NightDir {
    /// Open `dir` and read its facts.
    pub fn open(dir: &Path) -> Result<(NightDir, Facts)> {
        let path = dir.join(NIGHT);
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let v: Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is not JSON", path.display()))?;
        let watch_pid = v["watch_pid"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
            .ok_or_else(|| anyhow!("{}: no watch_pid", path.display()))?;
        let facts = Facts {
            watch_pid,
            until: v["until"].as_i64(),
            gate: v["gate"].as_str().map(String::from),
        };
        Ok((NightDir(dir.to_path_buf()), facts))
    }

    fn hold_file(&self) -> PathBuf {
        self.0.join(HOLD)
    }

    fn spend_file(&self) -> PathBuf {
        self.0.join(SPEND)
    }

    /// When the usage limit that holds every unit resets, or `None` when nothing holds them.
    pub fn held_until(&self) -> Result<Option<i64>> {
        let path = self.hold_file();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        let v: Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is not JSON", path.display()))?;
        v["reset_at"]
            .as_i64()
            .map(Some)
            .ok_or_else(|| anyhow!("{}: a hold needs reset_at", path.display()))
    }

    /// Add one phase's cost in a single append, so units running together never split a line.
    pub fn add_spend(&self, unit: &str, usd: f64) -> Result<()> {
        let path = self.spend_file();
        let line = format!("{}\n", json!({"unit": unit, "usd": usd}));
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(line.as_bytes()))
            .with_context(|| format!("cannot write {}", path.display()))
    }

    /// Dollars every unit and triage run tonight has spent.
    pub fn spent(&self) -> f64 {
        fs::read_to_string(self.spend_file())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| v["usd"].as_f64())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_reads_the_night_s_facts() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join(NIGHT),
            r#"{"watch_pid":42,"until":1000,"gate":"make check"}"#,
        )
        .unwrap();
        let (_, facts) = NightDir::open(tmp.path()).unwrap();
        let want = Facts {
            watch_pid: 42,
            until: Some(1000),
            gate: Some("make check".into()),
        };
        assert_eq!(facts, want);
        fs::write(tmp.path().join(NIGHT), "{}").unwrap();
        assert!(NightDir::open(tmp.path()).is_err());
    }

    #[test]
    fn a_hold_names_its_reset() {
        let tmp = tempfile::tempdir().unwrap();
        let night = NightDir(tmp.path().to_path_buf());
        assert_eq!(night.held_until().unwrap(), None);
        fs::write(night.hold_file(), r#"{"reset_at":900}"#).unwrap();
        assert_eq!(night.held_until().unwrap(), Some(900));
        fs::write(night.hold_file(), "{}").unwrap();
        assert!(night.held_until().is_err());
    }

    #[test]
    fn spend_sums_every_line_and_skips_what_is_no_cost() {
        let tmp = tempfile::tempdir().unwrap();
        let night = NightDir(tmp.path().to_path_buf());
        assert_eq!(night.spent(), 0.0);
        night.add_spend("2-a", 0.5).unwrap();
        night.add_spend("3-b", 1.25).unwrap();
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(night.spend_file())
            .unwrap();
        f.write_all(b"not json\n{\"unit\":\"x\"}\n").unwrap();
        assert_eq!(night.spent(), 1.75);
    }
}
