//! Locks: the per-repo and per-unit locks `ns run`, `ns watch` and `ns worktree new` take, and
//! the runner locks a phase holds.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::EXIT_LOCKED;
use crate::clock::Clock;
use crate::config::{self, Config};
use crate::error::SfError;
use crate::worktree;

/// A per-repo lock: an OS `flock` on a file in the git common dir, so the kernel drops a dead
/// process's hold. While held the file records the holder as JSON; it is empty otherwise.
pub struct Lock {
    file: File,
}

/// The run lock's file prefix: `ns-run-<unit>.lock`, one `ns run` at a time per unit.
const RUN_LOCK_PREFIX: &str = "ns-run-";
/// The watch lock: one `ns watch` at a time per repo.
const WATCH_LOCK: &str = "ns-watch.lock";
/// The merge lock: one merge step's update, CI wait and merge at a time per repo.
pub(super) const MERGE_LOCK: &str = "ns-merge.lock";
/// The worktree lock: one `ns worktree new` creating a worktree at a time per repo.
pub const WORKTREE_LOCK: &str = "ns-worktree.lock";
/// How many times, 10 ms apart, a lock whose holder names no one is tried before refusing.
const BRIEF_HOLD_TRIES: u32 = 10;
/// How long a wait for the merge or worktree lock sleeps between tries.
const LOCK_WAIT_POLL: Duration = Duration::from_millis(50);

pub fn run_lock(unit: &str) -> String {
    format!("{RUN_LOCK_PREFIX}{unit}.lock")
}

fn open_lock(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))
}

fn try_lock(f: &File, path: &Path) -> Result<bool> {
    try_flock(f).with_context(|| format!("cannot lock {}", path.display()))
}

impl Lock {
    fn held(file: File, holder: &Value) -> Result<Lock> {
        let lock = Lock { file };
        lock.file.set_len(0)?;
        writeln!(&lock.file, "{holder}")?;
        Ok(lock)
    }

    /// Take `<common>/<name>` and record `holder` in it, or return who holds it now.
    fn take(common: &Path, name: &str, holder: Value) -> Result<Result<Lock, Value>> {
        let path = common.join(name);
        let f = open_lock(&path)?;
        let mut tries = 0;
        while !try_lock(&f, &path)? {
            // A holder that hasn't written its record yet, or `run_holders` checking the lock,
            // holds it only for a moment.
            let held = read_holder(&path);
            tries += 1;
            if !held.is_null() || tries >= BRIEF_HOLD_TRIES {
                return Ok(Err(held));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(Ok(Lock::held(f, &holder)?))
    }

    /// Take `<common>/<name>` and record `holder` in it, waiting while another process holds
    /// it. `waiting` hears who holds it once, when the wait begins. The wait sleeps in real
    /// time, since the holder is another process, and ends on a stop. It gives up, returning
    /// who holds the lock, once the clock reaches the `deadline`.
    pub fn wait(
        common: &Path,
        name: &str,
        holder: Value,
        deadline: Option<(&Clock, i64)>,
        mut waiting: impl FnMut(&Value),
    ) -> Result<Result<Lock, Value>> {
        let path = common.join(name);
        let f = open_lock(&path)?;
        let mut first = true;
        while !try_lock(&f, &path)? {
            let held = read_holder(&path);
            if first {
                waiting(&held);
                first = false;
            }
            if deadline.is_some_and(|(clock, at)| clock.now() >= at) {
                return Ok(Err(held));
            }
            crate::stop::check()?;
            std::thread::sleep(LOCK_WAIT_POLL);
        }
        Ok(Ok(Lock::held(f, &holder)?))
    }

    /// The run lock for `unit`, refused with exit 5 while another `ns run` holds it.
    pub fn acquire(common: &Path, unit: &str, issue: Option<u64>) -> Result<Lock> {
        let me = json!({"pid": std::process::id(), "unit": unit, "issue": issue});
        let name = run_lock(unit);
        Self::take(common, &name, me)?.map_err(|held| {
            SfError::new(
                EXIT_LOCKED,
                format!(
                    "another ns run (pid {}, unit {}) holds {}",
                    held["pid"],
                    held["unit"].as_str().unwrap_or(unit),
                    common.join(&name).display()
                ),
            )
            .hint("one run per unit; wait for it to finish, or stop it")
            .into()
        })
    }

    /// The watch lock, held for the whole of one `ns watch`, refused with exit 5 while another
    /// `ns watch` holds it.
    pub fn watch(common: &Path) -> Result<Lock> {
        let me = json!({"pid": std::process::id()});
        Self::take(common, WATCH_LOCK, me)?.map_err(|held| {
            SfError::new(
                EXIT_LOCKED,
                format!("another ns watch (pid {}) is running", held["pid"]),
            )
            .hint("one ns watch at a time per repo; stop it first")
            .into()
        })
    }

    /// Who holds a run lock now: `{pid, unit, issue}` for each live `ns run`, sorted by unit.
    /// A holder that hasn't written its record yet is `{pid: null, unit, issue: null}`, its
    /// unit read from the file name.
    pub fn run_holders(common: &Path) -> Result<Vec<Value>> {
        let entries = match fs::read_dir(common) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", common.display())),
        };
        let mut holders = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("cannot read {}", common.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(unit) = name
                .strip_prefix(RUN_LOCK_PREFIX)
                .and_then(|n| n.strip_suffix(".lock"))
                .filter(|u| worktree::valid_unit_id(u))
            else {
                continue;
            };
            let path = entry.path();
            let f = File::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
            if try_lock(&f, &path)? {
                unlock_flock(&f);
                continue;
            }
            let held = read_holder(&path);
            holders.push(if held.is_object() {
                held
            } else {
                json!({"pid": null, "unit": unit, "issue": null})
            });
        }
        holders.sort_by(|a, b| a["unit"].as_str().cmp(&b["unit"].as_str()));
        Ok(holders)
    }
}

/// A lock holder's pid for a message: `?` when the holder hasn't written its record yet.
pub fn holder_pid(holder: &Value) -> String {
    holder["pid"]
        .as_u64()
        .map_or_else(|| "?".into(), |p| p.to_string())
}

fn read_holder(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_default();
    serde_json::from_str(text.trim()).unwrap_or(Value::Null)
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        unlock_flock(&self.file);
    }
}

/// The locks a phase's runner names, held from before the phase starts until it ends. Each is
/// an OS lock (`flock`) on `<dir>/<name>.lock`, so the kernel drops a dead run's hold and its
/// file is taken over. A lock held by a live run makes this one wait, up to a bound.
#[derive(Debug)]
pub struct RunnerLocks(Vec<File>);

/// The longest pause between two tries at a held lock.
const LOCK_POLL_MAX_S: i64 = 5;

/// Which bound ended a wait for a runner lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// The waiting phase's own timeout, counted from when the wait began.
    Timeout,
    /// `ns watch --until`.
    Until,
}

impl Bound {
    pub(super) fn label(self) -> &'static str {
        match self {
            Bound::Timeout => "timeout",
            Bound::Until => "until",
        }
    }
}

/// How a wait for the runner locks ended.
#[derive(Debug)]
pub enum Acquired {
    Held(RunnerLocks),
    GaveUp(GaveUp),
}

/// A wait for a runner lock that hit its bound. Every lock taken before it is released.
#[derive(Debug)]
pub struct GaveUp {
    pub lock: String,
    pub held_by: Value,
    pub bound: Bound,
}

impl GaveUp {
    pub fn reason(&self) -> String {
        format!(
            "gave up waiting for lock {} (held by pid {}, unit {}) at {}",
            self.lock,
            self.held_by["pid"],
            self.held_by["unit"].as_str().unwrap_or("?"),
            match self.bound {
                Bound::Timeout => "the phase timeout",
                Bound::Until => "--until",
            }
        )
    }
}

impl RunnerLocks {
    /// Take `names` sorted and without duplicates, so two runs can't deadlock.
    /// `waiting` hears each lock this run has to wait for, and who holds it. A held lock is
    /// retried with a backoff on `clock` until `timeout` after the wait began, or `until`,
    /// whichever is first; a lock taken at or past `until` counts as a give-up too.
    pub fn acquire(
        dir: &Path,
        names: &[String],
        unit: &str,
        clock: &Clock,
        timeout: Duration,
        until: Option<i64>,
        mut waiting: impl FnMut(&str, &Value),
    ) -> Result<Acquired> {
        let mut names = names.to_vec();
        names.sort();
        names.dedup();
        if !names.is_empty() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let holder = |path: &Path| {
            let text = fs::read_to_string(path).unwrap_or_default();
            serde_json::from_str(text.trim()).unwrap_or(Value::Null)
        };
        let mut give_up = None;
        let mut held = RunnerLocks(Vec::new());
        for name in &names {
            let path = dir.join(format!("{name}.lock"));
            let mut f = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .with_context(|| format!("cannot open {}", path.display()))?;
            let try_lock =
                |f: &File| try_flock(f).with_context(|| format!("cannot lock {}", path.display()));
            if !try_lock(&f)? {
                let (at, bound) =
                    *give_up.get_or_insert_with(|| give_up_at(clock.now(), timeout, until));
                let first = holder(&path);
                waiting(name, &first);
                if !poll_until(clock, at, || try_lock(&f))? {
                    return Ok(Acquired::GaveUp(GaveUp {
                        lock: name.clone(),
                        held_by: Some(holder(&path))
                            .filter(|h| !h.is_null())
                            .unwrap_or(first),
                        bound,
                    }));
                }
                if until.is_some_and(|u| clock.now() >= u) {
                    unlock_flock(&f);
                    return Ok(Acquired::GaveUp(GaveUp {
                        lock: name.clone(),
                        held_by: first,
                        bound: Bound::Until,
                    }));
                }
            }
            f.set_len(0)?;
            writeln!(f, "{}", json!({"pid": std::process::id(), "unit": unit}))?;
            held.0.push(f);
        }
        Ok(Acquired::Held(held))
    }
}

/// Run `try_once` until it succeeds, backing off from 1 s to `LOCK_POLL_MAX_S` between tries
/// on `clock`. `Ok(false)` once the clock reaches `at` without a success.
fn poll_until(clock: &Clock, at: i64, mut try_once: impl FnMut() -> Result<bool>) -> Result<bool> {
    let mut step = 1;
    let mut polls = 0;
    while !try_once()? {
        clock.check_polls(polls)?;
        polls += 1;
        let now = clock.now();
        if now >= at {
            return Ok(false);
        }
        clock.sleep_until((now + step).min(at))?;
        step = (step * 2).min(LOCK_POLL_MAX_S);
    }
    Ok(true)
}

/// When a wait that begins at `now` gives up: `timeout` later (a partial second counts as a
/// whole one), or at `until` if that comes first.
fn give_up_at(now: i64, timeout: Duration, until: Option<i64>) -> (i64, Bound) {
    let t = now + timeout.as_millis().div_ceil(1000) as i64;
    match until {
        Some(u) if u <= t => (u, Bound::Until),
        _ => (t, Bound::Timeout),
    }
}

impl Drop for RunnerLocks {
    fn drop(&mut self) {
        for f in &self.0 {
            let _ = f.set_len(0);
            unlock_flock(f);
        }
    }
}

#[cfg(unix)]
fn unlock_flock(f: &File) {
    use std::os::unix::io::AsRawFd;
    // SAFETY: the fd is open for as long as `f` lives.
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
}

#[cfg(not(unix))]
fn unlock_flock(_: &File) {}

/// Try to take an exclusive `flock` on `f`; `Ok(false)` means another holds it.
#[cfg(unix)]
fn try_flock(f: &File) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    loop {
        // SAFETY: the fd is open for as long as `f` lives.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EWOULDBLOCK) => return Ok(false),
            _ => return Err(e),
        }
    }
}

#[cfg(not(unix))]
fn try_flock(_: &File) -> std::io::Result<bool> {
    Ok(true)
}

/// `[runners] lock_dir` from the user config, else `<git-common-dir>/ns/locks`.
pub(super) fn lock_dir(cfg: Option<&Config>, common: &Path) -> PathBuf {
    match cfg.and_then(|c| c.runners.lock_dir.as_deref()) {
        Some(d) => config::expand_tilde(d),
        None => common.join("ns").join("locks"),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::clock::MAX_PINNED_POLLS;

    #[test]
    fn lock_dir_defaults_under_the_common_dir_and_expands_a_tilde() {
        let common = Path::new("/repo/.git");
        assert_eq!(lock_dir(None, common), common.join("ns").join("locks"));
        let mut cfg = Config::default();
        assert_eq!(
            lock_dir(Some(&cfg), common),
            common.join("ns").join("locks")
        );
        cfg.runners.lock_dir = Some("~/shared/locks".into());
        assert_eq!(
            lock_dir(Some(&cfg), common),
            config::expand_tilde("~/shared/locks")
        );
        assert!(!lock_dir(Some(&cfg), common).starts_with(common));
        cfg.runners.lock_dir = Some("/abs/locks".into());
        assert_eq!(lock_dir(Some(&cfg), common), Path::new("/abs/locks"));
    }

    #[test]
    fn acquire_takes_a_repeated_name_once() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = ["b", "a", "b"].map(String::from).into();
        let clock = Clock::pinned(1000);
        let held = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, None, |lock, _| {
            panic!("waited on {lock}")
        })
        .unwrap()
        .held();
        assert_eq!(held.0.len(), 2);
    }

    pub(in crate::run) const MIN: Duration = Duration::from_secs(60);

    impl Acquired {
        pub(in crate::run) fn held(self) -> RunnerLocks {
            match self {
                Acquired::Held(h) => h,
                Acquired::GaveUp(g) => panic!("gave up: {}", g.reason()),
            }
        }

        fn gave_up(self) -> GaveUp {
            match self {
                Acquired::GaveUp(g) => g,
                Acquired::Held(_) => panic!("took the locks"),
            }
        }
    }

    pub(in crate::run) struct Hold(File);

    impl Drop for Hold {
        fn drop(&mut self) {
            unlock_flock(&self.0);
        }
    }

    /// Another process's hold on `<dir>/<name>.lock`, naming pid 42 and unit `other`.
    pub(in crate::run) fn hold(dir: &Path, name: &str) -> Hold {
        let path = dir.join(format!("{name}.lock"));
        fs::write(&path, "{\"pid\":42,\"unit\":\"other\"}\n").unwrap();
        let f = File::open(&path).unwrap();
        assert!(try_flock(&f).unwrap());
        Hold(f)
    }

    fn is_free(dir: &Path, name: &str) -> bool {
        let f = File::open(dir.join(format!("{name}.lock"))).unwrap();
        let free = try_flock(&f).unwrap();
        unlock_flock(&f);
        free
    }

    #[test]
    fn acquire_gives_up_at_the_phase_timeout_and_releases_what_it_took() {
        let dir = tempfile::tempdir().unwrap();
        let _b = hold(dir.path(), "b");
        let clock = Clock::pinned(1000);
        let names: Vec<String> = ["b", "a"].map(String::from).into();
        let mut waits = Vec::new();
        let g = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, Some(2000), |l, by| {
            waits.push((l.to_string(), by.clone()))
        })
        .unwrap()
        .gave_up();
        assert_eq!(clock.now(), 1060);
        assert_eq!((g.lock.as_str(), g.bound), ("b", Bound::Timeout));
        assert_eq!(g.held_by, json!({"pid": 42, "unit": "other"}));
        assert_eq!(waits, [("b".to_string(), g.held_by.clone())]);
        assert!(is_free(dir.path(), "a"));
        assert_eq!(fs::read_to_string(dir.path().join("a.lock")).unwrap(), "");
        assert_eq!(
            g.reason(),
            "gave up waiting for lock b (held by pid 42, unit other) at the phase timeout"
        );
    }

    #[test]
    fn acquire_gives_up_at_until_when_it_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        let _a = hold(dir.path(), "a");
        let clock = Clock::pinned(1000);
        let names = vec!["a".to_string()];
        let g = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, Some(1030), |_, _| {})
            .unwrap()
            .gave_up();
        assert_eq!(clock.now(), 1030);
        assert_eq!(g.bound, Bound::Until);
        assert_eq!(
            g.reason(),
            "gave up waiting for lock a (held by pid 42, unit other) at --until"
        );
    }

    #[test]
    fn acquire_counts_a_partial_second_of_timeout_as_a_whole_one() {
        let dir = tempfile::tempdir().unwrap();
        let _a = hold(dir.path(), "a");
        let clock = Clock::pinned(1000);
        let names = vec!["a".to_string()];
        let timeout = Duration::from_millis(1500);
        let g = RunnerLocks::acquire(
            dir.path(),
            &names,
            "u",
            &clock,
            timeout,
            Some(1002),
            |_, _| {},
        )
        .unwrap()
        .gave_up();
        assert_eq!((clock.now(), g.bound), (1002, Bound::Until));
    }

    #[test]
    fn acquire_takes_a_lock_its_holder_releases_in_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Some(hold(dir.path(), "a"));
        let clock = Clock::pinned(1000);
        let names = vec!["a".to_string()];
        let held = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, None, |_, _| {
            a.take();
        })
        .unwrap()
        .held();
        let other = File::open(dir.path().join("a.lock")).unwrap();
        assert!(!try_flock(&other).unwrap());
        let text = fs::read_to_string(dir.path().join("a.lock")).unwrap();
        assert!(
            text.contains(&format!("\"pid\":{}", std::process::id())),
            "{text}"
        );
        drop(held);
        assert!(is_free(dir.path(), "a"));
    }

    #[test]
    fn acquire_times_out_from_when_the_first_wait_began() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Some(hold(dir.path(), "a"));
        let _b = hold(dir.path(), "b");
        let clock = Clock::pinned(1000);
        let names: Vec<String> = ["a", "b"].map(String::from).into();
        let g = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, None, |l, _| {
            if l == "a" {
                clock.sleep_until(1030).unwrap();
                a.take();
                assert!(is_free(dir.path(), "a"));
            }
        })
        .unwrap()
        .gave_up();
        assert_eq!((g.lock.as_str(), g.bound), ("b", Bound::Timeout));
        assert_eq!(clock.now(), 1060);
    }

    #[test]
    fn acquire_gives_up_when_until_passed_while_it_waited() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = Some(hold(dir.path(), "b"));
        let clock = Clock::pinned(1000);
        let names: Vec<String> = ["a", "b"].map(String::from).into();
        let g = RunnerLocks::acquire(dir.path(), &names, "u", &clock, MIN, Some(1030), |_, _| {
            clock.sleep_until(1030).unwrap();
            b.take();
        })
        .unwrap()
        .gave_up();
        assert_eq!((g.lock.as_str(), g.bound), ("b", Bound::Until));
        assert!(is_free(dir.path(), "a"));
        assert!(is_free(dir.path(), "b"));
    }

    #[test]
    fn polling_backs_off_one_two_four_then_five_seconds() {
        let clock = Clock::pinned(1000);
        let mut seen = Vec::new();
        let got = poll_until(&clock, 1 << 40, || {
            seen.push(clock.now());
            Ok(seen.len() == 6)
        })
        .unwrap();
        assert!(got);
        assert_eq!(seen, [1000, 1001, 1003, 1007, 1012, 1017]);
    }

    #[test]
    fn a_polling_loop_that_never_succeeds_fails_on_a_pinned_clock() {
        let clock = Clock::pinned(1000);
        let mut tries = 0;
        let err = poll_until(&clock, 1 << 40, || {
            tries += 1;
            Ok(false)
        })
        .unwrap_err();
        assert_eq!(tries, MAX_PINNED_POLLS + 1);
        assert!(err.to_string().contains("pinned clock (NS_NOW)"), "{err}");
    }

    #[test]
    fn polling_at_the_bound_tries_once_and_does_not_sleep() {
        let clock = Clock::pinned(1000);
        let mut tries = 0;
        let got = poll_until(&clock, 1000, || {
            tries += 1;
            Ok(false)
        })
        .unwrap();
        assert!(!got);
        assert_eq!((tries, clock.now()), (1, 1000));
    }

    #[test]
    fn polling_never_sleeps_past_the_bound() {
        let clock = Clock::pinned(1000);
        assert!(!poll_until(&clock, 1004, || Ok(false)).unwrap());
        assert_eq!(clock.now(), 1004);
    }

    #[test]
    fn a_partial_second_of_timeout_rounds_up() {
        let t = Duration::from_millis(1500);
        assert_eq!(give_up_at(1000, t, None), (1002, Bound::Timeout));
        assert_eq!(give_up_at(1000, MIN, None), (1060, Bound::Timeout));
    }

    #[test]
    fn a_tie_between_the_timeout_and_until_goes_to_until() {
        assert_eq!(give_up_at(1000, MIN, Some(1060)), (1060, Bound::Until));
        assert_eq!(give_up_at(1000, MIN, Some(1061)), (1060, Bound::Timeout));
        assert_eq!(give_up_at(1000, MIN, Some(1059)), (1059, Bound::Until));
    }

    #[test]
    fn a_lock_wait_takes_the_lock_once_its_holder_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let mut other = Some(hold(dir.path(), "ns-merge"));
        let mut waits = Vec::new();
        let me = json!({"pid": 1, "unit": "u"});
        let lock = Lock::wait(dir.path(), MERGE_LOCK, me, None, |by| {
            waits.push(by.clone());
            other = None;
        })
        .unwrap()
        .unwrap();
        assert_eq!(waits, [json!({"pid": 42, "unit": "other"})]);
        let path = dir.path().join(MERGE_LOCK);
        assert_eq!(read_holder(&path), json!({"pid": 1, "unit": "u"}));
        assert!(!is_free(dir.path(), "ns-merge"));
        drop(lock);
        assert!(is_free(dir.path(), "ns-merge"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn a_free_lock_is_taken_without_a_wait() {
        let dir = tempfile::tempdir().unwrap();
        let me = json!({"pid": 1, "unit": "u"});
        let lock = Lock::wait(dir.path(), WORKTREE_LOCK, me, None, |_| panic!("waited")).unwrap();
        assert!(lock.is_ok());
    }

    #[test]
    fn run_holders_lists_each_held_run_lock_by_unit() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Lock::run_holders(&dir.path().join("missing"))
            .unwrap()
            .is_empty());
        let _b = hold(dir.path(), "ns-run-b-2");
        // Taken, but its record not yet written.
        let _a = hold(dir.path(), "ns-run-a-1");
        fs::write(dir.path().join("ns-run-a-1.lock"), "").unwrap();
        fs::write(
            dir.path().join("ns-run-c.lock"),
            "{\"pid\":7,\"unit\":\"c\"}",
        )
        .unwrap();
        let _bad = hold(dir.path(), "ns-run-Bad");
        let _watch = hold(dir.path(), "ns-watch");
        let _merge = hold(dir.path(), "ns-merge");
        assert_eq!(
            Lock::run_holders(dir.path()).unwrap(),
            [
                json!({"pid": null, "unit": "a-1", "issue": null}),
                json!({"pid": 42, "unit": "other"}),
            ]
        );
    }

    #[test]
    fn a_run_lock_is_per_unit() {
        let dir = tempfile::tempdir().unwrap();
        let a = Lock::acquire(dir.path(), "a", Some(1)).unwrap();
        let _b = Lock::acquire(dir.path(), "b", None).unwrap();
        let err = Lock::acquire(dir.path(), "a", None).err().unwrap();
        let sf = err.downcast_ref::<SfError>().unwrap();
        assert_eq!(sf.code, EXIT_LOCKED);
        assert!(
            err.to_string().starts_with(&format!(
                "another ns run (pid {}, unit a) holds ",
                std::process::id()
            )),
            "{err}"
        );
        drop(a);
        Lock::acquire(dir.path(), "a", None).unwrap();
    }
}
