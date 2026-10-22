//! A lock on a file in the repository's `ns` state directory, held until the process ends. The
//! operating system lets go of it when the process dies, so a crash leaves no lock behind. The
//! file is removed when the lock is let go, so finished units leave nothing in the directory.

use std::fs::{File, TryLockError};
use std::os::unix::fs::MetadataExt;
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

/// Takes the lock; when `busy` is set, a lock that is held fails with it instead of waiting.
fn lock_open(dir: &Path, name: &str, busy: Option<&str>) -> Result<Lock> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(name);
    loop {
        let file =
            File::create(&path).with_context(|| format!("cannot open {}", path.display()))?;
        match busy {
            Some(busy) => match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => bail!("{busy}"),
                Err(TryLockError::Error(e)) => {
                    return Err(e).with_context(|| format!("cannot lock {}", path.display()))
                }
            },
            None => file
                .lock()
                .with_context(|| format!("cannot lock {}", path.display()))?,
        }
        // The holder before us removed the file we opened; what we hold is no lock. Try again.
        let same = std::fs::metadata(&path)
            .is_ok_and(|m| m.ino() == file.metadata().map_or(0, |f| f.ino()));
        if same {
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
}
