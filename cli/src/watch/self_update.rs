//! `ns watch` on the nightshift source updates its own binary (docs/FACTORY.md, "When the
//! watched repo is nightshift"). When origin's default branch gains commits touching `cli/`
//! after the commit this `ns` was built from, the night drains: no new unit starts, running
//! ones finish. Then it builds that commit to a staging path, checks the new binary, and execs
//! it with the night's state, so one summary covers the whole night.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{triage, Tonight};
use crate::eval::trial::run_process;
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

/// Run `cmd` to its end with its output in `log`, or kill it and its process group when a stop
/// is requested or `ceiling` passes (both errors). Files, not pipes, so a straggler in the group
/// can't hold anything open. The exit status and the end of the log.
fn run_bounded(mut cmd: Command, ceiling: Duration, log: &Path) -> Result<(ExitStatus, String)> {
    let out = File::create(log).with_context(|| format!("cannot create {}", log.display()))?;
    cmd.stdout(out.try_clone()?).stderr(out);
    match run_process(cmd, None, ceiling) {
        Err(e) if e.kind() == ErrorKind::Interrupted => bail!("stopped"),
        Err(e) => Err(e.into()),
        Ok((Some(status), _, _)) => {
            Ok((status, fs::read(log).map(|b| tail(&b)).unwrap_or_default()))
        }
        Ok((None, _, _)) => bail!("timed out after {}s", ceiling.as_secs_f32().round()),
    }
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
    /// `watch --dry-run` exits 0 on a copy of the night's `state`. The new binary's path.
    pub fn build(&self, repo: &Repo, to: &str, state: &Path) -> Result<PathBuf> {
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
        let (status, said) = run_bounded(cargo, ceiling(BUILD_CEILING), &stage.join("build.log"))
            .context("cargo build")?;
        if !status.success() {
            bail!("cargo build: {status}: {said}");
        }
        let staged = stage.join(format!("ns-{to}"));
        fs::copy(stage.join("target/release/ns"), &staged)
            .with_context(|| format!("cannot copy the new ns to {}", staged.display()))?;
        self.check(&repo.root, &staged, to, state)?;
        Ok(staged)
    }

    fn check(&self, root: &Path, staged: &Path, to: &str, state: &Path) -> Result<()> {
        let log = staged.with_extension("log");
        let mut version = Command::new(staged);
        version.arg("--version");
        let (status, said) = run_bounded(version, ceiling(CHECK_CEILING), &log)
            .with_context(|| format!("{} --version", staged.display()))?;
        if !status.success() || !said.contains(to) {
            bail!("the new ns --version said {:?}, not {to}", said.trim());
        }
        // The dry run reads a copy of the state, which it consumes, so a state the new binary
        // can't read fails the check here, not after the exec.
        let probe = staged.with_extension("night.json");
        fs::copy(state, &probe).with_context(|| format!("cannot copy {}", state.display()))?;
        let mut dry = Command::new(staged);
        dry.args(["watch", "--dry-run"]).current_dir(root);
        if let Some(f) = &self.factory {
            dry.arg("--factory").arg(f);
        }
        dry.arg(FLAG).arg(&probe);
        let result = run_bounded(dry, ceiling(CHECK_CEILING), &log)
            .with_context(|| format!("{} watch --dry-run", staged.display()));
        let _ = fs::remove_file(&probe);
        let (status, said) = result?;
        if !status.success() {
            bail!("the new ns watch --dry-run: {status}: {said}");
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
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Carried {
    /// The state file's layout. A new binary from another commit reads only its own; a state
    /// with no version reads as 0 and is refused. Any change to the fields below, or to
    /// `Tonight` and `triage::Tally` inside, needs a new [`Carried::VERSION`].
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

/// Replace this process with `staged`, carrying the night in `state`. Returns only on failure,
/// which is [`std::io::ErrorKind::Interrupted`] when a stop was requested first. SIGINT and
/// SIGTERM are held pending from the last check to the exec, so one that lands in between
/// reaches the new `ns`, which unblocks them once its own handlers are in.
pub(super) fn exec(staged: &Path, state: &Path) -> anyhow::Error {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    let args = args_for(std::env::args_os().skip(1), state);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new(staged);
        cmd.args(args);
        // SAFETY: the closure only does pthread_sigmask(3) and an atomic load.
        unsafe {
            cmd.pre_exec(|| {
                stop::block();
                match stop::requested() {
                    Some(_) => Err(std::io::ErrorKind::Interrupted.into()),
                    None => Ok(()),
                }
            });
        }
        let e = cmd.exec();
        stop::unblock();
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
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 30"]);
        let start = std::time::Instant::now();
        let e = run_bounded(slow, Duration::from_millis(200), &log).unwrap_err();
        assert!(e.to_string().contains("timed out"), "{e}");
        assert!(start.elapsed() < Duration::from_secs(20));
        let mut quick = Command::new("sh");
        quick.args(["-c", "echo out; echo err >&2; (sleep 5 &); exit 3"]);
        let start = std::time::Instant::now();
        let (status, said) = run_bounded(quick, Duration::from_secs(30), &log).unwrap();
        assert_eq!(status.code(), Some(3));
        assert!(said.contains("out") && said.contains("err"), "{said}");
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "waited on a straggler"
        );
    }

    #[test]
    fn exec_holds_a_stop_pending_through_to_the_new_program() {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("grep");
        cmd.args(["-E", "^(SigBlk|SigPnd|ShdPnd)", "/proc/self/status"]);
        // SAFETY: only pthread_sigmask(3) and kill(2) in the closure.
        unsafe {
            cmd.pre_exec(|| {
                stop::block();
                libc::kill(libc::getpid(), libc::SIGTERM);
                Ok(())
            });
        }
        let out = cmd.output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let mask = |key: &str| {
            let line = text.lines().find(|l| l.starts_with(key)).unwrap();
            u64::from_str_radix(line.split_whitespace().nth(1).unwrap(), 16).unwrap()
        };
        let (int, term) = (1 << (libc::SIGINT - 1), 1 << (libc::SIGTERM - 1));
        assert_eq!(mask("SigBlk") & (int | term), int | term, "{text}");
        assert_eq!((mask("SigPnd") | mask("ShdPnd")) & term, term, "{text}");
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
