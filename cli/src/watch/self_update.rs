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
use serde_json::json;

use super::{triage, Tonight};
use crate::eval::trial::run_process;
use crate::git::{self, Repo};
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

/// How a hand-off that returned ended. A successful one never returns: the process is replaced.
pub(super) enum HandOff {
    /// The build, the check or the exec failed; the night goes on with this `ns`.
    Failed(anyhow::Error),
    /// A stop was requested first; the night winds down and nothing is logged as a failure.
    Stopped,
}

/// The night's state file, removed when the hand-off ends without an exec. After an exec the
/// guard never drops, and the new binary removes the file when it reads it.
struct StateFile(PathBuf);

impl Drop for StateFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

pub(super) struct SelfUpdate {
    /// The commit this `ns` was built from.
    from: String,
    /// Commits whose build or check failed tonight; none is tried again. A hand-off needs none
    /// of them: the new binary's commit comes after every one.
    failed: BTreeSet<String>,
    /// This `ns watch`'s own arguments, which the new binary runs with and its check too.
    args: Vec<OsString>,
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
        })
    }

    /// The commit at `base` (`origin/<default>`) to update to: one with `cli/` changes since
    /// this `ns`'s commit that hasn't failed tonight, announced on stderr. A base or build commit
    /// git can't read gives none.
    pub fn pending(&self, root: &Path, base: &str) -> Option<String> {
        let to = git::run(root, &["rev-parse", &format!("{base}^{{commit}}")]).ok()?;
        if self.failed.contains(&to) {
            return None;
        }
        let range = format!("{}..{to}", self.from);
        let n = git::run(root, &["rev-list", "--count", &range, "--", "cli"]).ok()?;
        if n == "0" {
            return None;
        }
        eprintln!(
            "ns watch: {base} has cli/ changes since this ns was built ({} -> {}); draining: no new unit until the running ones end",
            short(&self.from),
            short(&to)
        );
        Some(to)
    }

    /// Build `to`, check it and exec it with `night`. Returns only when that failed or a stop
    /// came first.
    pub fn hand_off(&self, repo: &Repo, to: &str, night: &Carried) -> HandOff {
        eprintln!("ns watch: self-update: building {}", short(to));
        let state = match night.save(&repo.common_dir) {
            Ok(path) => StateFile(path),
            Err(e) => return HandOff::Failed(e),
        };
        let abandoned = || {
            eprintln!("ns watch: self-update to {} abandoned: stopping", short(to));
            HandOff::Stopped
        };
        let built = self.build(repo, to, &state.0);
        if stop::requested().is_some() {
            return abandoned();
        }
        let staged = match built {
            Ok(staged) => staged,
            Err(e) => return HandOff::Failed(e),
        };
        eprintln!(
            "ns watch: self-update {} -> {}; handing off to {}",
            short(&self.from),
            short(to),
            staged.display()
        );
        run::log_event(
            &repo.common_dir,
            json!({"event": "self_update", "from": self.from, "to": to}),
        );
        let e = exec(&staged, args_for(self.args.iter().cloned(), &state.0));
        if stop::requested().is_some() {
            return abandoned();
        }
        HandOff::Failed(e)
    }

    /// Keep this ns after a failed hand-off to `to`, and never try `to` again tonight.
    pub fn give_up(&mut self, repo: &Repo, to: String, err: anyhow::Error) {
        eprintln!(
            "ns watch: warning: self-update to {} failed; keeping this ns: {err:#}",
            short(&to)
        );
        run::log_event(
            &repo.common_dir,
            json!({"event": "self_update_failed", "from": self.from, "to": to, "error": format!("{err:#}")}),
        );
        self.failed.insert(to);
    }

    /// Build `to` to `<common>/ns/self-update/ns-<to>` and check it: `--version` names `to` and
    /// `watch --dry-run` exits 0 on a copy of the night's `state`. The new binary's path.
    fn build(&self, repo: &Repo, to: &str, state: &Path) -> Result<PathBuf> {
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
        self.check(&staged, to, state)?;
        Ok(staged)
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
