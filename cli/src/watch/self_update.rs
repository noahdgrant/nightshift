//! `ns watch` on the nightshift source updates its own binary (docs/FACTORY.md, "When the
//! watched repo is nightshift"). When origin's default branch gains commits touching `cli/`
//! after the commit this `ns` was built from, the night drains: no new unit starts, running
//! ones finish. Then it builds that commit to a staging path, checks the new binary, and execs
//! it with the night's state, so one summary covers the whole night.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{triage, Tonight};
use crate::git::{self, Repo};
use crate::stop;

const FLAG: &str = "--resume-night";

/// The ceiling on the cargo build.
const BUILD_CEILING: Duration = Duration::from_secs(30 * 60);
/// The ceiling on each check of the staged binary.
const CHECK_CEILING: Duration = Duration::from_secs(60);
/// Tests set this (milliseconds) to shorten both ceilings.
const CEILING_ENV: &str = "NS_SELF_UPDATE_TIMEOUT_MS";

fn ceiling(default: Duration) -> Duration {
    std::env::var(CEILING_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(default, Duration::from_millis)
}

/// Run `cmd` to its end, or kill it and its whole process group when a stop is requested or
/// `ceiling` passes. A kill is an error.
fn run_bounded(mut cmd: Command, ceiling: Duration) -> Result<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let why = if stop::requested().is_some() {
            "stopped"
        } else if start.elapsed() >= ceiling {
            "timed out"
        } else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        #[cfg(unix)]
        // SAFETY: kill(2) on the process group this function made for the child.
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
        if why == "timed out" {
            bail!("timed out after {}s", ceiling.as_secs_f32().round());
        }
        bail!("{why}");
    };
    Ok(Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

fn drain(pipe: Option<impl std::io::Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    })
}

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
        let mut cargo = Command::new("cargo");
        cargo
            .args(["build", "--release", "--locked", "--manifest-path"])
            .arg(src.join("cli/Cargo.toml"))
            .arg("--target-dir")
            .arg(stage.join("target"))
            .env("NS_BUILD_COMMIT", to);
        let out = run_bounded(cargo, ceiling(BUILD_CEILING)).context("cargo build")?;
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
        let mut version = Command::new(staged);
        version.arg("--version");
        let out = run_bounded(version, ceiling(CHECK_CEILING))
            .with_context(|| format!("{} --version", staged.display()))?;
        let said = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || !said.contains(to) {
            bail!("the new ns --version said {:?}, not {to}", said.trim());
        }
        let mut dry = Command::new(staged);
        dry.args(["watch", "--dry-run"]).current_dir(root);
        if let Some(f) = &self.factory {
            dry.arg("--factory").arg(f);
        }
        let out = run_bounded(dry, ceiling(CHECK_CEILING))
            .with_context(|| format!("{} watch --dry-run", staged.display()))?;
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
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Carried {
    /// The state file's layout. A new binary from another commit reads only its own.
    #[serde(default)]
    pub version: u32,
    pub deadline: Option<i64>,
    pub spent_usd: f64,
    /// Units claimed tonight, toward `--max-units`.
    pub started: u32,
    pub harness_fails: u32,
    pub finished: BTreeSet<u64>,
    pub triage: triage::Tally,
    pub tonight: Tonight,
}

impl Default for Carried {
    fn default() -> Carried {
        Carried {
            version: Carried::VERSION,
            deadline: None,
            spent_usd: 0.0,
            started: 0,
            harness_fails: 0,
            finished: BTreeSet::new(),
            triage: triage::Tally::default(),
            tonight: Tonight::default(),
        }
    }
}

impl Carried {
    pub const VERSION: u32 = 1;

    /// Read the state `--resume-night` names, and remove its file. Fields a state lacks take
    /// their defaults; a state of another version is an error.
    pub fn take(path: &Path) -> Result<Carried> {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let _ = fs::remove_file(path);
        let c: Carried = serde_json::from_str(&text)
            .with_context(|| format!("{} is not a night", path.display()))?;
        if c.version != Carried::VERSION {
            bail!(
                "{} is night state version {}, not {}",
                path.display(),
                c.version,
                Carried::VERSION
            );
        }
        Ok(c)
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
            "version": 1, "deadline": 1000, "spent_usd": 2.5, "started": 3, "harness_fails": 1,
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
    fn state_of_another_version_or_none_is_refused_and_missing_fields_default() {
        let dir = tempfile::tempdir().unwrap();
        let write = |text: &str| {
            let p = dir.path().join("night.json");
            fs::write(&p, text).unwrap();
            p
        };
        for bad in [r#"{"version": 2}"#, r#"{"deadline": 5}"#, "not json"] {
            assert!(Carried::take(&write(bad)).is_err(), "{bad}");
        }
        let c = Carried::take(&write(r#"{"version": 1, "started": 4}"#)).unwrap();
        assert_eq!((c.started, c.deadline, c.harness_fails), (4, None, 0));
    }

    #[test]
    fn a_command_that_outlives_its_ceiling_is_killed_and_one_that_ends_is_not() {
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 30"]);
        let start = Instant::now();
        let e = run_bounded(slow, Duration::from_millis(200)).unwrap_err();
        assert!(e.to_string().contains("timed out"), "{e}");
        assert!(start.elapsed() < Duration::from_secs(20));
        let mut quick = Command::new("sh");
        quick.args(["-c", "echo out; echo err >&2; exit 3"]);
        let o = run_bounded(quick, Duration::from_secs(30)).unwrap();
        assert_eq!(o.status.code(), Some(3));
        assert_eq!(String::from_utf8_lossy(&o.stdout), "out\n");
        assert_eq!(String::from_utf8_lossy(&o.stderr), "err\n");
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
