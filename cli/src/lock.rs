//! A lock on a file in the repository's `ns` state directory, held until the process ends. The
//! operating system lets go of it when the process dies, so a crash leaves no lock behind. The
//! file is removed when the lock is let go, so finished units leave nothing in the directory.
//! The holder writes its process id and start time into the file, so `lock_holder` can say who
//! holds a lock without taking it.

use std::fs::{File, OpenOptions, TryLockError};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Held for as long as it lives.
pub struct Lock {
    _file: File,
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Removed while still locked, so no one can take it between the two.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Takes the lock called `name` in `dir`, or fails at once when another process holds it.
/// `busy` is what the error says in that case.
pub fn lock_take(dir: &Path, name: &str, busy: &str) -> Result<Lock> {
    lock_open(dir, name, Some(busy))
}

/// Takes the lock called `name` in `dir`, waiting for as long as another process holds it.
pub fn lock_wait(dir: &Path, name: &str) -> Result<Lock> {
    lock_open(dir, name, None)
}

/// The process that holds a lock: its id and when it took the lock, in seconds since the epoch.
#[derive(Debug, PartialEq, Eq)]
pub struct Holder {
    pub pid: u32,
    pub since: u64,
}

/// Who holds the lock called `name` in `dir`, or `None` when no process does. It takes nothing for
/// good and removes nothing, so a leftover file whose holder died reads as `None`: the operating
/// system's lock decides, never the pid in the file.
pub fn lock_holder(dir: &Path, name: &str) -> Option<Holder> {
    let path = dir.join(name);
    let file = File::open(&path).ok()?;
    match file.try_lock_shared() {
        // Nobody holds it. The shared lock lasts only until `file` drops, which `lock_take`
        // steps past with a few short tries.
        Ok(()) => None,
        Err(TryLockError::WouldBlock) => {
            // The holder writes its record just after it locks; give it a moment. A plain loop, not
            // `retry::retry`: this is a local read of an instant's gap, and that helper prints each
            // wait and doubles it, which would make a probe slow and noisy.
            for _ in 0..20 {
                if let Some(holder) = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| holder_parse(&text))
                {
                    return Some(holder);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            None
        }
        Err(TryLockError::Error(_)) => None,
    }
}

fn holder_parse(text: &str) -> Option<Holder> {
    let (pid, since) = text.trim().split_once(' ')?;
    Some(Holder {
        pid: pid.parse().ok()?,
        since: since.parse().ok()?,
    })
}

/// Records this process as the holder of the lock on `file`.
fn holder_write(file: &File) -> Result<()> {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let record = format!("{} {since}\n", std::process::id());
    file.set_len(0)?;
    file.write_all_at(record.as_bytes(), 0)?;
    Ok(())
}

/// Takes the lock; when `busy` is set, a lock that is held fails with it instead of waiting.
fn lock_open(dir: &Path, name: &str, busy: Option<&str>) -> Result<Lock> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(name);
    loop {
        // Not truncated: a refused attempt must leave the holder's record alone.
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        match busy {
            Some(busy) => {
                // A `lock_holder` probe holds the lock for an instant; a few tries step past it. A
                // plain loop, not `retry::retry`, for the reason given in `lock_holder`.
                let mut tries = 0;
                loop {
                    match file.try_lock() {
                        Ok(()) => break,
                        Err(TryLockError::WouldBlock) if tries < 5 => {
                            tries += 1;
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(TryLockError::WouldBlock) => bail!("{busy}"),
                        Err(TryLockError::Error(e)) => {
                            return Err(e)
                                .with_context(|| format!("cannot lock {}", path.display()))
                        }
                    }
                }
            }
            None => file
                .lock()
                .with_context(|| format!("cannot lock {}", path.display()))?,
        }
        // The holder before us removed the file we opened; what we hold is no lock. Try again.
        let same = std::fs::metadata(&path)
            .is_ok_and(|m| m.ino() == file.metadata().map_or(0, |f| f.ino()));
        if same {
            holder_write(&file).with_context(|| format!("cannot write {}", path.display()))?;
            return Ok(Lock { _file: file, path });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_is_held_until_it_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let first = lock_take(tmp.path(), "x.lock", "busy").unwrap();
        let err = lock_take(tmp.path(), "x.lock", "busy").err().unwrap();
        assert_eq!(err.to_string(), "busy");
        lock_take(tmp.path(), "y.lock", "busy").unwrap();
        drop(first);
        lock_take(tmp.path(), "x.lock", "busy").unwrap();
    }

    #[test]
    fn waiting_for_a_lock_takes_it_once_the_holder_lets_go() {
        let tmp = tempfile::tempdir().unwrap();
        let first = lock_take(tmp.path(), "x.lock", "busy").unwrap();
        let dir = tmp.path().to_path_buf();
        let waiter = std::thread::spawn(move || lock_wait(&dir, "x.lock").map(|_| ()));
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!waiter.is_finished());
        drop(first);
        waiter.join().unwrap().unwrap();
    }

    #[test]
    fn a_lock_let_go_leaves_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = lock_take(tmp.path(), "x.lock", "busy").unwrap();
        assert!(tmp.path().join("x.lock").exists());
        drop(lock);
        assert!(!tmp.path().join("x.lock").exists());
    }

    fn since_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn a_held_lock_reports_its_own_pid_and_when_it_was_taken() {
        let tmp = tempfile::tempdir().unwrap();
        let before = since_now();
        let _lock = lock_take(tmp.path(), "x.lock", "busy").unwrap();
        let holder = lock_holder(tmp.path(), "x.lock").unwrap();
        assert_eq!(holder.pid, std::process::id());
        assert!(holder.since >= before && holder.since <= since_now());
    }

    #[test]
    fn a_missing_or_released_lock_has_no_holder() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(lock_holder(tmp.path(), "x.lock"), None);
        drop(lock_take(tmp.path(), "x.lock", "busy").unwrap());
        assert_eq!(lock_holder(tmp.path(), "x.lock"), None);
    }

    #[test]
    fn a_leftover_file_with_a_record_but_no_holder_reads_as_none_and_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.lock");
        std::fs::write(&path, "4242 100\n").unwrap();
        assert_eq!(lock_holder(tmp.path(), "x.lock"), None);
        assert!(path.exists());
    }

    #[test]
    fn probing_never_stops_a_take_and_never_blocks_the_holder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let prober = {
            let (dir, stop) = (dir.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    lock_holder(&dir, "x.lock");
                }
            })
        };
        for _ in 0..200 {
            drop(lock_take(&dir, "x.lock", "busy").unwrap());
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        prober.join().unwrap();
    }

    #[test]
    fn a_refused_take_leaves_the_holders_record() {
        let tmp = tempfile::tempdir().unwrap();
        let _lock = lock_take(tmp.path(), "x.lock", "busy").unwrap();
        let before = std::fs::read_to_string(tmp.path().join("x.lock")).unwrap();
        assert!(lock_take(tmp.path(), "x.lock", "busy").is_err());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("x.lock")).unwrap(),
            before
        );
        assert!(before.starts_with(&std::process::id().to_string()));
    }

    #[test]
    fn a_held_lock_with_a_junk_or_empty_record_reads_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.lock");
        let file = File::create(&path).unwrap();
        file.lock().unwrap();
        assert_eq!(lock_holder(tmp.path(), "x.lock"), None);
        file.write_all_at(b"junk", 0).unwrap();
        assert_eq!(lock_holder(tmp.path(), "x.lock"), None);
    }

    #[test]
    fn a_take_steps_past_a_probe_that_holds_the_lock_for_an_instant() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.lock");
        let probe = File::create(&path).unwrap();
        probe.lock_shared().unwrap();
        let dir = tmp.path().to_path_buf();
        let (started, wait) = std::sync::mpsc::channel();
        let taker = std::thread::spawn(move || {
            started.send(()).unwrap();
            lock_take(&dir, "x.lock", "busy").map(|_| ())
        });
        wait.recv().unwrap();
        drop(probe);
        taker.join().unwrap().unwrap();
    }

    #[test]
    fn a_take_refused_by_a_real_holder_says_busy_after_its_tries() {
        let tmp = tempfile::tempdir().unwrap();
        let _lock = lock_take(tmp.path(), "x.lock", "already being run").unwrap();
        let err = lock_take(tmp.path(), "x.lock", "already being run")
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "already being run");
    }
}
