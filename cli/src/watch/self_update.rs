//! `ns watch` on the nightshift source updates its own binary (docs/FACTORY.md, "When the
//! watched repo is nightshift"). When origin's default branch gains commits touching `cli/`
//! after the commit this `ns` was built from, the night drains: no new unit starts, running
//! ones finish. Then it builds that commit to a staging path, checks the new binary, and execs
//! it with the night's state, so one summary covers the whole night.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{triage, Tonight};
use crate::git::{self, Repo};

const FLAG: &str = "--resume-night";

/// The commit this `ns` was built from: `NS_BUILD_COMMIT` in the environment, which tests set,
/// else the one `build.rs` stamped. `None` for a build from outside a git checkout, or an empty
/// `NS_BUILD_COMMIT`.
fn built_from() -> Option<String> {
    let c = std::env::var("NS_BUILD_COMMIT").unwrap_or_else(|_| env!("NS_BUILD_COMMIT").into());
    (!c.is_empty() && c != "unknown").then_some(c)
}

/// Whether `root` is the nightshift source: its `cli/Cargo.toml` names the package `nightshift`.
fn is_nightshift(root: &Path) -> bool {
    fs::read_to_string(root.join("cli/Cargo.toml"))
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .is_some_and(|t| {
            t.get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                == Some("nightshift")
        })
}

pub(super) struct SelfUpdate {
    /// The commit this `ns` was built from.
    pub from: String,
    /// Commits whose build or check failed tonight; none is tried again. A hand-off needs none
    /// of them: the new binary's commit comes after every one.
    pub failed: BTreeSet<String>,
    /// `--factory`, for the new binary's dry run.
    factory: Option<PathBuf>,
}

impl SelfUpdate {
    /// Self-update for `repo`, or `None` when it is off: `off` (`--no-self-update`), another repo,
    /// or an `ns` with no build commit.
    pub fn new(repo: &Repo, off: bool, factory: Option<PathBuf>) -> Option<SelfUpdate> {
        if off || !is_nightshift(&repo.root) {
            return None;
        }
        let Some(from) = built_from() else {
            eprintln!("ns watch: self-update off: this ns has no build commit");
            return None;
        };
        Some(SelfUpdate {
            from,
            failed: BTreeSet::new(),
            factory,
        })
    }

    /// The commit at `base` (`origin/<default>`) to update to: one with `cli/` changes since
    /// this `ns`'s commit that hasn't failed tonight. A base or build commit git can't read
    /// gives none.
    pub fn pending(&self, root: &Path, base: &str) -> Option<String> {
        let to = git::run(root, &["rev-parse", &format!("{base}^{{commit}}")]).ok()?;
        if self.failed.contains(&to) {
            return None;
        }
        let range = format!("{}..{to}", self.from);
        let n = git::run(root, &["rev-list", "--count", &range, "--", "cli"]).ok()?;
        (n != "0").then_some(to)
    }

    /// Build `to` to `<common>/ns/self-update/ns-<to>` and check it: `--version` names `to` and
    /// `watch --dry-run` exits 0. The new binary's path.
    pub fn build(&self, repo: &Repo, to: &str) -> Result<PathBuf> {
        let stage = repo.common_dir.join("ns/self-update");
        let src = stage.join("src");
        let _ = fs::remove_dir_all(&src);
        fs::create_dir_all(&src).with_context(|| format!("cannot create {}", src.display()))?;
        extract(&repo.root, to, &src)?;
        let out = Command::new("cargo")
            .args(["build", "--release", "--locked", "--manifest-path"])
            .arg(src.join("cli/Cargo.toml"))
            .arg("--target-dir")
            .arg(stage.join("target"))
            .env("NS_BUILD_COMMIT", to)
            .stdin(Stdio::null())
            .output()
            .context("cannot run cargo")?;
        if !out.status.success() {
            bail!("cargo build: {}: {}", out.status, tail(&out.stderr));
        }
        let staged = stage.join(format!("ns-{to}"));
        fs::copy(stage.join("target/release/ns"), &staged)
            .with_context(|| format!("cannot copy the new ns to {}", staged.display()))?;
        self.check(&repo.root, &staged, to)?;
        Ok(staged)
    }

    fn check(&self, root: &Path, staged: &Path, to: &str) -> Result<()> {
        let out = Command::new(staged)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("cannot run {}", staged.display()))?;
        let said = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || !said.contains(to) {
            bail!("the new ns --version said {:?}, not {to}", said.trim());
        }
        let mut dry = Command::new(staged);
        dry.args(["watch", "--dry-run"]).current_dir(root);
        if let Some(f) = &self.factory {
            dry.arg("--factory").arg(f);
        }
        let out = dry
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .output()
            .with_context(|| format!("cannot run {}", staged.display()))?;
        if !out.status.success() {
            bail!(
                "the new ns watch --dry-run: {}: {}",
                out.status,
                tail(&out.stderr)
            );
        }
        Ok(())
    }
}

/// `git archive <to> cli | tar -x -C <into>`: the source to build, with no worktree to clean up.
fn extract(root: &Path, to: &str, into: &Path) -> Result<()> {
    let mut archive = git::command()
        .arg("-C")
        .arg(root)
        .args(["archive", "--format=tar", to, "cli"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("cannot run git archive")?;
    let tar = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(into)
        .stdin(archive.stdout.take().expect("piped"))
        .output()
        .context("cannot run tar")?;
    let git = archive.wait_with_output().context("git archive")?;
    if !git.status.success() {
        bail!("git archive {to}: {}", tail(&git.stderr));
    }
    if !tar.status.success() {
        bail!("tar: {}", tail(&tar.stderr));
    }
    Ok(())
}

/// The last lines of a command's stderr, for a warning.
fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// The night so far, which the new binary carries on from.
#[derive(Serialize, Deserialize, Default)]
pub(super) struct Carried {
    pub deadline: Option<i64>,
    pub spent_usd: f64,
    /// Units claimed tonight, toward `--max-units`.
    pub started: u32,
    pub harness_fails: u32,
    pub finished: BTreeSet<u64>,
    pub triage: triage::Tally,
    pub tonight: Tonight,
}

impl Carried {
    /// Read the state `--resume-night` names, and remove its file.
    pub fn take(path: &Path) -> Result<Carried> {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let _ = fs::remove_file(path);
        serde_json::from_str(&text).with_context(|| format!("{} is not a night", path.display()))
    }

    /// Write the state to a file for the new binary to read.
    pub fn save(&self, common: &Path) -> Result<PathBuf> {
        let dir = common.join("ns/self-update");
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = dir.join(format!("night-{}.json", std::process::id()));
        fs::write(&path, serde_json::to_string(self)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(path)
    }
}

/// This `ns watch`'s arguments for the new binary: the same ones, with `--resume-night state`
/// in place of any it was given.
fn args_for(args: impl Iterator<Item = OsString>, state: &Path) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut args = args.peekable();
    while let Some(a) = args.next() {
        if a == FLAG {
            args.next();
            continue;
        }
        if a.to_str()
            .is_some_and(|s| s.starts_with(&format!("{FLAG}=")))
        {
            continue;
        }
        out.push(a);
    }
    out.extend([FLAG.into(), state.into()]);
    out
}

/// Replace this process with `staged`, carrying the night in `state`. Returns only on failure.
pub(super) fn exec(staged: &Path, state: &Path) -> anyhow::Error {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    let args = args_for(std::env::args_os().skip(1), state);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = Command::new(staged).args(args).exec();
        anyhow!("cannot exec {}: {e}", staged.display())
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        anyhow!("cannot exec {}: needs unix", staged.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_night_survives_the_state_file_and_the_file_goes() {
        let night = serde_json::json!({
            "deadline": 1000, "spent_usd": 2.5, "started": 3, "harness_fails": 1,
            "finished": [2, 3],
            "triage": {
                "tried": [9], "seen": [9, 10], "last_error": "no login", "off": true,
                "retry": 10, "records": [{"issue": 9, "outcome": "triaged"}],
            },
            "tonight": {
                "units": [{"issue": 2, "outcome": "done"}], "requeued": [5],
                "cleaned": ["1-a"], "cleanup_tried": ["1-a"],
            },
        });
        let c: Carried = serde_json::from_value(night.clone()).unwrap();
        let common = tempfile::tempdir().unwrap();
        let path = c.save(common.path()).unwrap();
        let back = Carried::take(&path).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), night);
        assert!(!path.exists());
        assert!(Carried::take(&path).is_err());
    }

    #[test]
    fn the_new_binary_gets_the_same_arguments_and_only_the_latest_state() {
        let state = Path::new("/s/night.json");
        assert_eq!(
            args_for(os(&["watch", "--until", "06:30"]).into_iter(), state),
            os(&["watch", "--until", "06:30", FLAG, "/s/night.json"])
        );
        // A second hand-off replaces the first's state, in either form.
        for given in [
            os(&["watch", FLAG, "/old.json", "--parallel", "2"]),
            os(&["watch", "--resume-night=/old.json", "--parallel", "2"]),
        ] {
            assert_eq!(
                args_for(given.into_iter(), state),
                os(&["watch", "--parallel", "2", FLAG, "/s/night.json"])
            );
        }
    }
}
