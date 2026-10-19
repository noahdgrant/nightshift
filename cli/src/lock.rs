//! A lock on a file in the repository's `ns` state directory, held until the process ends. The
//! operating system lets go of it when the process dies, so a crash leaves no lock behind.

use std::fs::{File, TryLockError};
use std::path::Path;

use anyhow::{bail, Context, Result};

/// Held for as long as it lives.
pub struct Lock {
    _file: File,
}

/// Takes the lock called `name` in `dir`, or fails at once when another process holds it.
/// `busy` is what the error says in that case.
pub fn lock_take(dir: &Path, name: &str, busy: &str) -> Result<Lock> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(name);
    let file = File::create(&path).with_context(|| format!("cannot open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Lock { _file: file }),
        Err(TryLockError::WouldBlock) => bail!("{busy}"),
        Err(TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("cannot lock {}", path.display()))
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
}
