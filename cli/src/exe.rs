//! The path of the running `ns` binary, for the commands `ns` gives to child processes (the
//! phase hook guard and the gate). See D68 for why this is `current_exe` and not
//! `/proc/self/exe`.

use std::path::PathBuf;

use anyhow::Context;

/// The running `ns` binary, which must still exist for a child process to run it by path.
///
/// `/proc/self/exe` names this process only, so a command line handed to claude or a shell
/// cannot use it; the hook and the gate need a path that other processes resolve. `ns watch`
/// does use `/proc/self/exe` (`watch::scheduler`) because it execs the binary itself.
pub fn ns_exe() -> anyhow::Result<PathBuf> {
    live_exe(std::env::current_exe().context("cannot find the ns binary")?)
}

/// `path` with the kernel's ` (deleted)` suffix removed, or an error when no binary is there.
pub fn live_exe(path: PathBuf) -> anyhow::Result<PathBuf> {
    let exe = strip_deleted(path);
    anyhow::ensure!(
        exe.exists(),
        "the ns binary {} no longer exists; the hook guard could not run it. Rebuild or reinstall ns and run again",
        exe.display()
    );
    Ok(exe)
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
        let err = live_exe(PathBuf::from("/nonexistent/ns (deleted)"))
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
        assert_eq!(live_exe(replaced).unwrap(), ns);
        assert_eq!(live_exe(ns.clone()).unwrap(), ns);
    }

    #[test]
    fn the_running_test_binary_is_live() {
        assert!(ns_exe().unwrap().exists());
    }
}
