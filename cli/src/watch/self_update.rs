//! `ns watch` on the nightshift source updates its own binary (docs/factory/starting-a-night.md,
//! "When the watched repo is nightshift"). When origin's default branch gains commits touching
//! `cli/` after the commit new units run, it builds that commit in the background while units
//! keep running, copies the binary to a path named for the commit and checks it. New units start
//! on it, and running ones finish on theirs. Once no unit runs, the night execs it with the
//! night's state, so one summary covers the whole night.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{triage, Tonight};
use crate::eval::trial::run_process;
use crate::gate::stderr_tail;
use crate::git::{self, Repo};
use crate::proc::Proc;
use crate::run;
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
        Ok((Some(status), _, _)) => Ok((
            status,
            fs::read(log).map(|b| warning_tail(&b)).unwrap_or_default(),
        )),
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

/// How a hand-off that returned ended. A successful one never returns: the process is replaced.
pub(super) enum HandOff {
    /// The exec failed; the night goes on with this `ns`, its units on the old binary too.
    Failed(String, anyhow::Error),
    /// A stop was requested first; the night winds down and nothing is logged as a failure.
    Stopped,
}

/// How a build that ended in the background went, for the commit it built.
pub(super) enum Built {
    /// Cargo built it; the binary still needs its copy and check ([`SelfUpdate::adopt`]).
    Ok(String),
    Failed(String, anyhow::Error),
    /// A stop came first and the build was killed.
    Stopped(String),
}

/// The night's state file, removed when it goes out of scope without an exec. After an exec the
/// guard never drops, and the new binary removes the file when it reads it.
pub(super) struct StateFile(PathBuf);

impl Drop for StateFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// A checked binary for `to`, which new units start on.
struct Adopted {
    to: String,
    path: PathBuf,
}

/// `cargo build` of `to`, running in its own process group, its output in `log`.
struct Build {
    to: String,
    child: Child,
    deadline: Instant,
    ceiling: Duration,
    log: PathBuf,
}

impl Build {
    fn kill(&mut self) {
        // SAFETY: kill(2) on the process group of a child this process started and has not
        // reaped: the child leads the group, so the group id is still its own.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Build {
    /// A build the night leaves behind, on a panic or an exec, never outlives it.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
}

pub(super) struct SelfUpdate {
    /// The commit this `ns` was built from.
    from: String,
    /// Commits whose build or check failed tonight; none is tried again.
    failed: BTreeSet<String>,
    /// This `ns watch`'s own arguments, which the new binary runs with and its check too.
    args: Vec<OsString>,
    /// The newest binary that passed its check: new units start on it, and the night hands off
    /// to it once no unit runs.
    adopted: Option<Adopted>,
    /// The build running now, at most one.
    building: Option<Build>,
}

impl SelfUpdate {
    /// Self-update for `repo`, or `None` when it is off: `off` (`--no-self-update`), another repo,
    /// or an `ns` with no build commit.
    pub fn new(repo: &Repo, off: bool, args: Vec<OsString>) -> Option<SelfUpdate> {
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
            args,
            adopted: None,
            building: None,
        })
    }

    /// The commit new units run: the adopted build's, else this `ns`'s.
    fn current(&self) -> &str {
        self.adopted.as_ref().map_or(&self.from, |a| &a.to)
    }

    /// The binary new units start on, when a build was adopted; else this `ns`.
    pub fn child_exe(&self) -> Option<&Path> {
        self.adopted.as_ref().map(|a| a.path.as_path())
    }

    /// Whether a checked binary waits for the night to hand off to it.
    pub fn adopted(&self) -> bool {
        self.adopted.is_some()
    }

    /// The adopted binary's commit, empty when none is.
    pub fn adopted_commit(&self) -> String {
        self.adopted
            .as_ref()
            .map(|a| a.to.clone())
            .unwrap_or_default()
    }

    /// Whether a build is running.
    pub fn building(&self) -> bool {
        self.building.is_some()
    }

    /// The commit at `base` (`origin/<default>`) to build: one with `cli/` changes since the
    /// commit new units run that hasn't failed tonight, announced on stderr. None while a build
    /// runs; a base or build commit git can't read gives none too.
    pub fn pending(&self, root: &Path, base: &str) -> Option<String> {
        if self.building.is_some() {
            return None;
        }
        let to = git::run(root, &["rev-parse", &format!("{base}^{{commit}}")]).ok()?;
        if self.failed.contains(&to) {
            return None;
        }
        let range = format!("{}..{to}", self.current());
        let n = git::run(root, &["rev-list", "--count", &range, "--", "cli"]).ok()?;
        if n == "0" {
            return None;
        }
        eprintln!(
            "ns watch: {base} has cli/ changes since this ns was built ({} -> {}); building it while units run",
            short(self.current()),
            short(&to)
        );
        Some(to)
    }

    /// Start building `to` in the background. A build that can't start is given up at once.
    pub fn start(&mut self, repo: &Repo, to: String) {
        eprintln!("ns watch: self-update: building {}", short(&to));
        match spawn_build(repo, &to) {
            Ok((child, log)) => {
                let ceiling = ceiling(BUILD_CEILING);
                self.building = Some(Build {
                    to,
                    child,
                    deadline: Instant::now() + ceiling,
                    ceiling,
                    log,
                });
            }
            Err(e) => self.give_up(repo, to, e),
        }
    }

    /// The running build's end, once it ended, a stop came, or its ceiling passed (killed).
    pub fn poll(&mut self) -> Option<Built> {
        let b = self.building.as_mut()?;
        if crate::stop::requested().is_some() {
            let mut b = self.building.take()?;
            b.kill();
            return Some(Built::Stopped(b.to.clone()));
        }
        let done = match b.child.try_wait() {
            Ok(Some(status)) if status.success() => Ok(()),
            Ok(Some(status)) => {
                let said = fs::read(&b.log)
                    .map(|t| warning_tail(&t))
                    .unwrap_or_default();
                Err(anyhow!("cargo build: {status}: {said}"))
            }
            Ok(None) if Instant::now() >= b.deadline => {
                b.kill();
                let s = b.ceiling.as_secs_f32().round();
                Err(anyhow!("cargo build: timed out after {s}s"))
            }
            Ok(None) => return None,
            Err(e) => Err(anyhow!("cargo build: {e}")),
        };
        let b = self.building.take()?;
        Some(match done {
            Ok(()) => Built::Ok(b.to.clone()),
            Err(e) => Built::Failed(b.to.clone(), e),
        })
    }

    /// Kill a running build the night no longer needs, saying why.
    pub fn abandon(&mut self, why: &str) {
        if let Some(mut b) = self.building.take() {
            b.kill();
            eprintln!("ns watch: self-update to {} abandoned: {why}", short(&b.to));
        }
    }

    /// Copy the binary cargo built for `to` to `<common>/ns/self-update/ns-<to>` and check it
    /// against the night's `state`; once it passes, new units start on it. A failure is given
    /// up: units keep this `ns`.
    pub fn adopt(&mut self, repo: &Repo, to: String, state: &Path) {
        let stage = repo.common_dir.join("ns/self-update");
        let staged = stage.join(format!("ns-{to}"));
        let ready = fs::copy(stage.join("target/release/ns"), &staged)
            .with_context(|| format!("cannot copy the new ns to {}", staged.display()))
            .and_then(|_| self.check(&staged, &to, state));
        if let Err(e) = ready {
            return self.give_up(repo, to, e);
        }
        eprintln!(
            "ns watch: self-update: {} built; new units start on {}",
            short(&to),
            staged.display()
        );
        run::log_event(
            &repo.common_dir,
            json!({"event": "self_update_built", "from": self.from, "to": to, "path": staged}),
        );
        self.adopted = Some(Adopted { to, path: staged });
    }

    /// Exec the adopted binary with `night`. Returns only when that failed or a stop came first.
    pub fn hand_off(&mut self, repo: &Repo, night: &Carried) -> HandOff {
        let Some(Adopted { to, path }) = self.adopted.as_ref() else {
            return HandOff::Stopped;
        };
        let (to, path) = (to.clone(), path.clone());
        // A newer build is not carried across: the new binary looks for one itself.
        self.abandon("handing off");
        let state = match night.save(&repo.common_dir) {
            Ok(p) => StateFile(p),
            Err(e) => return HandOff::Failed(to, e),
        };
        if stop::requested().is_some() {
            return HandOff::Stopped;
        }
        eprintln!(
            "ns watch: self-update {} -> {}; handing off to {}",
            short(&self.from),
            short(&to),
            path.display()
        );
        run::log_event(
            &repo.common_dir,
            json!({"event": "self_update", "from": self.from, "to": to}),
        );
        let e = exec(&path, args_for(self.args.iter().cloned(), &state.0));
        if stop::requested().is_some() {
            eprintln!(
                "ns watch: self-update to {} abandoned: stopping",
                short(&to)
            );
            return HandOff::Stopped;
        }
        HandOff::Failed(to, e)
    }

    /// Keep this ns, and its binary for new units, after `to` failed, and never try `to` again
    /// tonight.
    pub fn give_up(&mut self, repo: &Repo, to: String, err: anyhow::Error) {
        eprintln!(
            "ns watch: warning: self-update to {} failed; keeping this ns: {err:#}",
            short(&to)
        );
        run::log_event(
            &repo.common_dir,
            json!({"event": "self_update_failed", "from": self.from, "to": to, "error": format!("{err:#}")}),
        );
        if self.adopted.as_ref().is_some_and(|a| a.to == to) {
            self.adopted = None;
        }
        self.failed.insert(to);
    }

    /// `--version` names `to`, and `watch --dry-run` takes the arguments the exec will give it.
    fn check(&self, staged: &Path, to: &str, state: &Path) -> Result<()> {
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
        dry.args(args_for(self.args.iter().cloned(), &probe));
        if !self.args.iter().any(|a| a == "--dry-run") {
            dry.arg("--dry-run");
        }
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

/// Write `night` to a state file for a check of the new binary; the file goes with the guard.
pub(super) fn probe_state(common: &Path, night: &Carried) -> Result<StateFile> {
    night.save(common).map(StateFile)
}

impl StateFile {
    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

/// Extract `cli/` at `to` and start `cargo build` on it in its own process group, its output in
/// `<common>/ns/self-update/build.log`.
fn spawn_build(repo: &Repo, to: &str) -> Result<(Child, PathBuf)> {
    let stage = repo.common_dir.join("ns/self-update");
    let src = stage.join("src");
    let _ = fs::remove_dir_all(&src);
    fs::create_dir_all(&src).with_context(|| format!("cannot create {}", src.display()))?;
    extract(&repo.root, to, &src)?;
    let log = stage.join("build.log");
    let out = File::create(&log).with_context(|| format!("cannot create {}", log.display()))?;
    let mut cargo = Command::new("cargo");
    cargo
        .args(["build", "--release", "--locked", "--manifest-path"])
        .arg(src.join("cli/Cargo.toml"))
        .arg("--target-dir")
        .arg(stage.join("target"))
        .env("NS_BUILD_COMMIT", to)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cargo.process_group(0);
    }
    let child = cargo.spawn().context("cannot run cargo build")?;
    Ok((child, log))
}

/// `git archive <to> cli | tar -x -C <into>`: the source to build, with no worktree to clean up.
fn extract(root: &Path, to: &str, into: &Path) -> Result<()> {
    let git = git::command()
        .arg("-C")
        .arg(root)
        .args(["archive", "--format=tar", to, "cli"])
        .output()
        .context("cannot run git archive")?;
    if !git.status.success() {
        bail!("git archive {to}: {}", warning_tail(&git.stderr));
    }
    let tar = Proc::tool("tar")
        .arg("-x")
        .arg("-C")
        .arg(into)
        .stdin(git.stdout)
        .output()
        .context("cannot run tar")?;
    if !tar.status.success() {
        bail!("tar: {}", warning_tail(&tar.stderr));
    }
    Ok(())
}

const WARNING_LINES: usize = 5;

/// The last `WARNING_LINES` non-blank lines of a command's stderr, for a warning.
fn warning_tail(stderr: &[u8]) -> String {
    stderr_tail(stderr, WARNING_LINES)
}

/// The night so far, which the new binary carries on from.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Carried {
    /// The state file's layout version; a state with none reads as 0.
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

/// Replace this process with `staged` run with `args`. Returns only on failure,
/// which is [`std::io::ErrorKind::Interrupted`] when a stop was requested first. SIGINT and
/// SIGTERM are held pending from the last check to the exec, so one that lands in between
/// reaches the new `ns`, which unblocks them once its own handlers are in.
fn exec(staged: &Path, args: Vec<OsString>) -> anyhow::Error {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    {
        let e = Proc::tool(staged).args(args).exec();
        // std set SIGPIPE to its default for the new program; this one goes on, and a harness
        // that exits early must not kill it.
        // SAFETY: signal(2) with SIG_IGN installs no handler.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }
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

    fn full_night() -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    fn key_paths(v: &serde_json::Value, at: &str, out: &mut BTreeSet<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    let path = format!("{at}.{k}");
                    out.insert(path.clone());
                    key_paths(v, &path, out);
                }
            }
            serde_json::Value::Array(a) => {
                a.iter().for_each(|v| key_paths(v, &format!("{at}[]"), out))
            }
            _ => {}
        }
    }

    fn update() -> SelfUpdate {
        SelfUpdate {
            from: "a".into(),
            failed: BTreeSet::new(),
            args: Vec::new(),
            adopted: None,
            building: None,
        }
    }

    #[test]
    fn a_failed_adopted_binary_is_dropped_for_new_units_and_never_tried_again() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo {
            root: dir.path().into(),
            common_dir: dir.path().join(".git"),
        };
        let mut u = update();
        assert_eq!((u.current(), u.child_exe()), ("a", None));
        u.adopted = Some(Adopted {
            to: "b".into(),
            path: "/x/ns-b".into(),
        });
        assert_eq!(u.current(), "b");
        assert_eq!(u.child_exe(), Some(Path::new("/x/ns-b")));
        // Another commit failing leaves the adopted binary in place.
        u.give_up(&repo, "c".into(), anyhow!("no"));
        assert_eq!(u.child_exe(), Some(Path::new("/x/ns-b")));
        u.give_up(&repo, "b".into(), anyhow!("cannot exec"));
        assert_eq!((u.current(), u.child_exe()), ("a", None));
        assert!(u.failed.contains("b") && u.failed.contains("c"));
        // With no build running, abandoning one says nothing and changes nothing.
        u.abandon("stopping");
        assert!(!u.building());
    }

    #[test]
    fn a_warning_keeps_the_last_five_non_blank_lines_of_stderr() {
        let stderr = "one\n\ntwo\nthree\n  \nfour\nfive\n\nsix\n\n";
        assert_eq!(
            warning_tail(stderr.as_bytes()),
            "two\nthree\nfour\nfive\nsix"
        );
        assert_eq!(warning_tail(b"\n \n"), "");
    }

    #[test]
    fn the_state_file_layout_is_pinned_to_its_version() {
        let c: Carried = serde_json::from_value(full_night()).unwrap();
        let mut keys = BTreeSet::new();
        key_paths(&serde_json::to_value(&c).unwrap(), "", &mut keys);
        let keys: Vec<_> = keys.iter().map(String::as_str).collect();
        let pinned = [
            ".deadline",
            ".finished",
            ".harness_fails",
            ".spent_usd",
            ".started",
            ".tonight",
            ".tonight.cleaned",
            ".tonight.cleanup_tried",
            ".tonight.requeued",
            ".tonight.units",
            ".tonight.units[].issue",
            ".tonight.units[].outcome",
            ".triage",
            ".triage.last_error",
            ".triage.off",
            ".triage.records",
            ".triage.records[].issue",
            ".triage.records[].outcome",
            ".triage.retry",
            ".triage.seen",
            ".triage.tried",
            ".version",
        ];
        assert_eq!(
            keys, pinned,
            "the night state layout changed: bump Carried::VERSION (a new binary refuses another \
             version's state), then update this list"
        );
        assert_eq!(Carried::VERSION, 1);
    }

    #[test]
    fn the_night_survives_the_state_file_and_the_file_goes() {
        let night = full_night();
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
    fn a_command_that_outlives_its_ceiling_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 300"]);
        let start = std::time::Instant::now();
        let e = run_bounded(slow, Duration::from_millis(200), &dir.path().join("log")).unwrap_err();
        assert!(e.to_string().contains("timed out"), "{e}");
        assert!(start.elapsed() < Duration::from_secs(120));
    }

    #[test]
    fn a_command_that_ends_is_not_held_by_a_straggler_in_its_group() {
        let dir = tempfile::tempdir().unwrap();
        let mut quick = Command::new("sh");
        quick.args(["-c", "echo out; echo err >&2; (sleep 300 &); exit 3"]);
        let start = std::time::Instant::now();
        let (status, said) =
            run_bounded(quick, Duration::from_secs(600), &dir.path().join("log")).unwrap();
        assert_eq!(status.code(), Some(3));
        assert!(said.contains("out") && said.contains("err"), "{said}");
        assert!(
            start.elapsed() < Duration::from_secs(120),
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

    fn sigpipe_ignored() -> bool {
        let status = fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|l| l.starts_with("SigIgn:")).unwrap();
        let mask = u64::from_str_radix(line.split_whitespace().nth(1).unwrap(), 16).unwrap();
        mask & (1 << (libc::SIGPIPE - 1)) != 0
    }

    #[test]
    fn a_failed_exec_leaves_sigpipe_ignored() {
        assert!(sigpipe_ignored(), "the Rust runtime ignores SIGPIPE");
        let dir = tempfile::tempdir().unwrap();
        let e = exec(&dir.path().join("no-such-ns"), Vec::new());
        assert!(e.to_string().contains("cannot exec"), "{e}");
        assert!(sigpipe_ignored(), "a broken pipe would kill the night");
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
