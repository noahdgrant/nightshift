//! The path of the running `ns` binary, for the commands `ns` gives to child processes (the
//! phase hook guard and the gate). See ADR-354 (`docs/decisions/`) for why this is `current_exe` and not
//! `/proc/self/exe`.

use std::ffi::OsStr;
use std::path::PathBuf;

use anyhow::Context;

/// The running `ns` binary, which must still exist for a child process to run it by path.
///
/// `/proc/self/exe` names this process only, so a command line handed to claude or a shell
/// cannot use it; the hook and the gate need a path that other processes resolve. `ns watch`
/// does use `/proc/self/exe` (`watch::scheduler`) because it execs the binary itself.
pub fn ns_exe() -> anyhow::Result<PathBuf> {
    live_exe(
        std::env::current_exe().context("cannot find the ns binary")?,
        &std::env::var_os("PATH").unwrap_or_default(),
    )
}

/// `path` with the kernel's ` (deleted)` suffix removed. When no file is there, as in an
/// `ns watch` unit whose pinned binary was replaced and removed, the `ns` found on `search`
/// (a `PATH` value); an error only when neither exists.
pub fn live_exe(path: PathBuf, search: &OsStr) -> anyhow::Result<PathBuf> {
    let exe = strip_deleted(path);
    if exe.exists() {
        return Ok(exe);
    }
    std::env::split_paths(search)
        .map(|dir| dir.join("ns"))
        .find(|candidate| candidate.is_file())
        .with_context(|| {
            format!(
                "the ns binary {} no longer exists and no ns is on PATH; the hook guard could not run it. Rebuild or reinstall ns and run again",
                exe.display()
            )
        })
}

/// Linux appends ` (deleted)` to `current_exe` when the binary was replaced while running.
fn strip_deleted(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(live) => PathBuf::from(live),
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_deleted_binary_path_loses_the_kernel_suffix() {
        assert_eq!(
            strip_deleted(PathBuf::from("/a/ns (deleted)")),
            PathBuf::from("/a/ns")
        );
        assert_eq!(
            strip_deleted(PathBuf::from("/a/ns")),
            PathBuf::from("/a/ns")
        );
    }

    #[test]
    fn a_missing_binary_is_an_error_that_names_it() {
        let err = live_exe(PathBuf::from("/nonexistent/ns (deleted)"), OsStr::new(""))
            .unwrap_err()
            .to_string();
        assert!(err.contains("/nonexistent/ns no longer exists"), "{err}");
    }

    #[test]
    fn a_replaced_binary_resolves_to_the_file_now_at_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let ns = dir.path().join("ns");
        std::fs::write(&ns, "").unwrap();
        let replaced = PathBuf::from(format!("{} (deleted)", ns.display()));
        assert_eq!(live_exe(replaced, OsStr::new("")).unwrap(), ns);
        assert_eq!(live_exe(ns.clone(), OsStr::new("")).unwrap(), ns);
    }

    #[test]
    fn a_removed_binary_falls_back_to_the_ns_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let on_path = dir.path().join("ns");
        std::fs::write(&on_path, "").unwrap();
        let search = std::env::join_paths([Path::new("/nonexistent"), dir.path()]).unwrap();
        let gone = PathBuf::from("/nonexistent/ns (deleted)");
        assert_eq!(live_exe(gone, &search).unwrap(), on_path);
    }

    #[test]
    fn the_running_test_binary_is_live() {
        assert!(ns_exe().unwrap().exists());
    }
}
