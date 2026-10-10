//! What `ns watch` hands each `ns run` it starts (docs/FACTORY.md, "Parallel units"): a copy of
//! the config and factory definition it read at start, the night's facts in `night.json`, the
//! hold file a usage limit writes, and the spend file every unit adds its phase costs to.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

const DIR_PREFIX: &str = "watch-";
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
    pub fn path(&self) -> &Path {
        &self.0
    }

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

    /// Hold every unit at its next phase until `reset_at`.
    pub fn hold(&self, reset_at: i64) -> Result<()> {
        let path = self.hold_file();
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, json!({"reset_at": reset_at}).to_string())
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn release(&self) -> Result<()> {
        match fs::remove_file(self.hold_file()) {
            Err(e) if e.kind() != ErrorKind::NotFound => {
                Err(e).with_context(|| format!("cannot remove {}", self.hold_file().display()))
            }
            _ => Ok(()),
        }
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

/// The night directory `ns watch` owns: `<git-common-dir>/ns/watch-<pid>/`, private to its user
/// and removed when the night ends.
pub struct Snapshot {
    night: NightDir,
    /// Symlinks in the factory directory the copy left out.
    pub skipped: Vec<PathBuf>,
}

impl Snapshot {
    /// Copy the factory directory and the config file, and write `facts`. A night directory a
    /// dead `ns watch` left behind is removed first.
    pub fn take(common: &Path, factory_root: &Path, config: &Path, facts: &Facts) -> Result<Self> {
        let ns = common.join("ns");
        sweep_dead(&ns);
        let dir = ns.join(format!("{DIR_PREFIX}{}", facts.watch_pid));
        let _ = fs::remove_dir_all(&dir);
        create_private_dir(&dir)?;
        let mut snap = Snapshot {
            night: NightDir(dir),
            skipped: Vec::new(),
        };
        let to = snap.factory();
        copy_tree(factory_root, &to, &mut snap.skipped)?;
        if config.is_file() {
            fs::copy(config, snap.config())
                .with_context(|| format!("cannot copy {}", config.display()))?;
        }
        let night = snap.night.path().join(NIGHT);
        let v = json!({"watch_pid": facts.watch_pid, "until": facts.until, "gate": facts.gate});
        fs::write(&night, v.to_string())
            .with_context(|| format!("cannot write {}", night.display()))?;
        Ok(snap)
    }

    pub fn night(&self) -> &NightDir {
        &self.night
    }

    pub fn factory(&self) -> PathBuf {
        self.night.path().join("factory")
    }

    /// Where the config copy is; no file there when there was no config to copy.
    pub fn config(&self) -> PathBuf {
        self.night.path().join("config.toml")
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.night.path());
    }
}

fn pid_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Remove the night directories of `ns watch` processes that are gone.
fn sweep_dead(ns: &Path) {
    let Ok(entries) = fs::read_dir(ns) else {
        return;
    };
    for e in entries.flatten() {
        let pid = e
            .file_name()
            .to_str()
            .and_then(|n| n.strip_prefix(DIR_PREFIX))
            .and_then(|p| p.parse::<i32>().ok())
            .filter(|p| *p > 0);
        if pid.is_some_and(|p| !pid_alive(p)) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::create_dir(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("cannot restrict {}", dir.display()))?;
    }
    Ok(())
}

/// Copy `from` to `to`, leaving out symlinks, which could reach outside the definition or loop.
/// A missing `from` copies nothing: the factory then has its defaults, as it would unsnapshotted.
fn copy_tree(from: &Path, to: &Path, skipped: &mut Vec<PathBuf>) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("cannot create {}", to.display()))?;
    let entries = match fs::read_dir(from) {
        Ok(e) => e,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", from.display())),
    };
    for e in entries {
        let e = e.with_context(|| format!("cannot read {}", from.display()))?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let kind = e
            .file_type()
            .with_context(|| format!("cannot read {}", src.display()))?;
        if kind.is_symlink() {
            skipped.push(src);
        } else if kind.is_dir() {
            copy_tree(&src, &dst, skipped)?;
        } else {
            fs::copy(&src, &dst).with_context(|| format!("cannot copy {}", src.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            watch_pid: std::process::id(),
            until: Some(1000),
            gate: Some("make check".into()),
        }
    }

    #[test]
    fn a_snapshot_copies_the_definition_and_config_and_a_unit_reads_its_facts() {
        let tmp = tempfile::tempdir().unwrap();
        let def = tmp.path().join("def");
        fs::create_dir_all(def.join("runners")).unwrap();
        fs::write(def.join("nightshift.toml"), "gates = \"auto\"\n").unwrap();
        fs::write(def.join("runners/bench.toml"), "kind = \"bench\"\n").unwrap();
        std::os::unix::fs::symlink(&def, def.join("loop")).unwrap();
        let cfg = tmp.path().join("config.toml");
        fs::write(&cfg, "[roles.default]\n").unwrap();
        let common = tmp.path().join("git");
        let snap = Snapshot::take(&common, &def, &cfg, &facts()).unwrap();
        assert_eq!(snap.skipped, [def.join("loop")]);
        // The originals change; the copies don't.
        fs::write(def.join("nightshift.toml"), "broken = [").unwrap();
        fs::write(&cfg, "broken = [").unwrap();
        let copied = fs::read_to_string(snap.factory().join("nightshift.toml")).unwrap();
        assert_eq!(copied, "gates = \"auto\"\n");
        assert!(snap.factory().join("runners/bench.toml").is_file());
        assert!(!snap.factory().join("loop").exists());
        assert_eq!(
            fs::read_to_string(snap.config()).unwrap(),
            "[roles.default]\n"
        );
        let (_, read) = NightDir::open(snap.night().path()).unwrap();
        assert_eq!(read, facts());
        let dir = snap.night().path().to_path_buf();
        drop(snap);
        assert!(!dir.exists());
    }

    #[test]
    fn no_config_file_copies_none() {
        let tmp = tempfile::tempdir().unwrap();
        let snap = Snapshot::take(
            tmp.path(),
            &tmp.path().join("no-def"),
            &tmp.path().join("no-config.toml"),
            &facts(),
        )
        .unwrap();
        assert!(!snap.config().exists());
        assert!(snap.factory().is_dir());
    }

    #[test]
    fn the_night_dir_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ns").join("watch-5");
        create_private_dir(&dir).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn a_snapshot_sweeps_the_dirs_of_dead_watches_only() {
        let tmp = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        let ns = tmp.path().join("ns");
        let alive = format!("watch-{}", std::process::id());
        for name in [
            format!("watch-{dead}"),
            alive.clone(),
            "watch-x".into(),
            "other".into(),
        ] {
            fs::create_dir_all(ns.join(name)).unwrap();
        }
        sweep_dead(&ns);
        assert!(!ns.join(format!("watch-{dead}")).exists());
        for kept in [alive.as_str(), "watch-x", "other"] {
            assert!(ns.join(kept).exists(), "{kept}");
        }
    }

    #[test]
    fn a_hold_reads_back_until_released() {
        let tmp = tempfile::tempdir().unwrap();
        let night = NightDir(tmp.path().to_path_buf());
        assert_eq!(night.held_until().unwrap(), None);
        night.hold(500).unwrap();
        night.hold(900).unwrap();
        assert_eq!(night.held_until().unwrap(), Some(900));
        night.release().unwrap();
        night.release().unwrap();
        assert_eq!(night.held_until().unwrap(), None);
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
