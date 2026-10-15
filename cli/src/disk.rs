//! Free disk space on the filesystems a unit fills, read for `ns watch`'s disk check and for
//! `ns doctor` (docs/factory/watch.md, "Disk space").

use std::path::{Path, PathBuf};

use crate::git::Repo;

/// A GiB, the unit of `[limits] disk_wait_gb` and `disk_stop_gb`: free space as `df -h` shows it.
pub const GIB: u64 = 1 << 30;

/// One filesystem's free bytes as they were read, or why they couldn't be.
pub type Free = (PathBuf, Result<u64, String>);

/// `[limits] disk_wait_gb` and `disk_stop_gb`.
#[derive(Clone, Copy, Debug)]
pub struct DiskLimits {
    pub wait_gb: u64,
    pub stop_gb: u64,
}

/// Where free space stands against [`DiskLimits`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Under the wait limit: no unit starts.
    Wait,
    /// Under the stop limit: the night stops.
    Stop,
}

impl Level {
    /// The word `ns doctor` reports for this level.
    pub fn name(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Wait => "wait",
            Level::Stop => "stop",
        }
    }
}

impl DiskLimits {
    /// What the night says of a filesystem whose free space can't be read.
    pub fn unread_said(path: &Path, error: &str) -> String {
        format!(
            "free space on {} can't be read, so it holds nothing up: {error}",
            path.display()
        )
    }

    /// What the night says when `path` falls under the wait limit.
    pub fn low_said(&self, path: &Path, free_gb: u64) -> String {
        format!(
            "{} has {free_gb} GiB free, under [limits] disk_wait_gb = {}; no unit starts until there is more",
            path.display(),
            self.wait_gb
        )
    }

    /// What the night says when `path` falls under the stop limit.
    pub fn critical_said(&self, path: &Path, free_gb: u64) -> String {
        format!(
            "{} has {free_gb} GiB free, under [limits] disk_stop_gb = {}; the night stops",
            path.display(),
            self.stop_gb
        )
    }

    /// The warning for one filesystem's `free` space: none when it is above the wait limit.
    pub fn warning(&self, path: &Path, free: &Result<u64, String>) -> Option<String> {
        match free {
            Err(error) => Some(Self::unread_said(path, error)),
            Ok(bytes) => match self.level(*bytes) {
                Level::Ok => None,
                Level::Wait => Some(self.low_said(path, bytes / GIB)),
                Level::Stop => Some(self.critical_said(path, bytes / GIB)),
            },
        }
    }

    pub fn level(&self, bytes: u64) -> Level {
        if bytes < self.stop_gb.saturating_mul(GIB) {
            Level::Stop
        } else if bytes < self.wait_gb.saturating_mul(GIB) {
            Level::Wait
        } else {
            Level::Ok
        }
    }
}

/// Tests set this to a file holding the GiB every filesystem reports free, read at each check;
/// a file that can't be read, or doesn't hold a whole number, makes every read fail.
pub const FREE_ENV: &str = "NS_FREE_GIB_FILE";

/// Where Windows' C: drive is mounted under WSL.
const WSL_HOST_DRIVE: &str = "/mnt/c";

/// The filesystems a unit fills: the one holding `repo`'s worktrees, and under WSL the host's
/// C: drive, whose free space the WSL disk image grows into.
pub fn filesystems(repo: &Repo) -> Vec<PathBuf> {
    with_host_drive(
        existing(&crate::worktree::planned_path(repo, "unit")),
        &proc_version(),
    )
}

/// What `/proc/version` says, empty where it can't be read.
pub fn proc_version() -> String {
    std::fs::read_to_string("/proc/version").unwrap_or_default()
}

/// `path`, and after it the host's C: drive when `version`, the text of `/proc/version`, names a
/// WSL kernel.
pub fn with_host_drive(path: PathBuf, version: &str) -> Vec<PathBuf> {
    let mut paths = vec![path];
    if is_wsl(version) {
        paths.push(PathBuf::from(WSL_HOST_DRIVE));
    }
    paths
}

/// Each of `paths` with its free bytes, or why they can't be read.
pub fn read(paths: Vec<PathBuf>) -> Vec<Free> {
    paths
        .into_iter()
        .map(|p| {
            let free = free_bytes(&p);
            (p, free)
        })
        .collect()
}

/// `/proc/version` names a WSL kernel.
fn is_wsl(version: &str) -> bool {
    version.to_ascii_lowercase().contains("microsoft")
}

/// `path`, or its nearest ancestor that exists: the worktrees directory comes with the first unit.
fn existing(path: &Path) -> PathBuf {
    path.ancestors()
        .find(|p| p.exists())
        .unwrap_or(path)
        .to_path_buf()
}

/// The bytes an unprivileged user may still write on the filesystem holding `path`.
fn free_bytes(path: &Path) -> Result<u64, String> {
    match std::env::var_os(FREE_ENV) {
        Some(f) => forced(Path::new(&f)),
        None => statvfs(path),
    }
}

fn forced(file: &Path) -> Result<u64, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let v = text.trim();
    v.parse::<u64>()
        .map(|g| g.saturating_mul(GIB))
        .map_err(|_| format!("{}: {v:?} is not a whole number of GiB", file.display()))
}

#[cfg(unix)]
fn statvfs(path: &Path) -> Result<u64, String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: statvfs writes into the zeroed struct we own, and `c` outlives the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

#[cfg(not(unix))]
fn statvfs(_: &Path) -> Result<u64, String> {
    Err("free space is read only on unix".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_microsoft_kernel_is_wsl() {
        let wsl = "Linux version 6.6.114.1-microsoft-standard-WSL2 (root@machine) (gcc) #1 SMP";
        assert!(is_wsl(wsl));
        assert!(is_wsl("Linux version 4.4.0-19041-Microsoft"));
        assert!(!is_wsl(
            "Linux version 6.8.0-45-generic (buildd@lcy02-amd64-075)"
        ));
        assert!(!is_wsl(""));
    }

    #[test]
    fn the_host_drive_follows_the_worktrees_only_on_a_wsl_kernel() {
        let wsl = "Linux version 6.6.114.1-microsoft-standard-WSL2 (root@machine)";
        let generic = "Linux version 6.8.0-45-generic (buildd@lcy02-amd64-075)";
        let w = PathBuf::from("/w");
        assert_eq!(
            with_host_drive(w.clone(), wsl),
            [w.clone(), PathBuf::from("/mnt/c")]
        );
        assert_eq!(with_host_drive(w.clone(), generic), vec![w.clone()]);
        assert_eq!(with_host_drive(w.clone(), ""), [w]);
    }

    #[test]
    fn a_filesystem_is_warned_of_by_how_far_it_has_fallen() {
        let limits = DiskLimits {
            wait_gb: 20,
            stop_gb: 10,
        };
        let w = Path::new("/w");
        assert_eq!(limits.warning(w, &Ok(20 * GIB)), None);
        assert_eq!(
            limits.warning(w, &Ok(15 * GIB)).unwrap(),
            "/w has 15 GiB free, under [limits] disk_wait_gb = 20; no unit starts until there is more"
        );
        assert_eq!(
            limits.warning(w, &Ok(5 * GIB)).unwrap(),
            "/w has 5 GiB free, under [limits] disk_stop_gb = 10; the night stops"
        );
        assert_eq!(
            limits.warning(w, &Err("gone".into())).unwrap(),
            "free space on /w can't be read, so it holds nothing up: gone"
        );
        let names = [Level::Ok, Level::Wait, Level::Stop].map(Level::name);
        assert_eq!(names, ["ok", "wait", "stop"]);
    }

    #[test]
    fn each_limit_acts_alone_when_the_other_is_off() {
        let at = |wait_gb, stop_gb, gib: u64| DiskLimits { wait_gb, stop_gb }.level(gib * GIB);
        assert_eq!(at(0, 10, 5), Level::Stop);
        assert_eq!(at(0, 10, 15), Level::Ok);
        assert_eq!(at(20, 0, 5), Level::Wait);
        assert_eq!(at(20, 0, 25), Level::Ok);
    }

    #[test]
    fn a_missing_worktrees_dir_reads_its_nearest_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("repo.worktrees/unit");
        assert_eq!(existing(&wt), tmp.path());
        std::fs::create_dir_all(&wt).unwrap();
        assert_eq!(existing(&wt), wt);
    }

    #[test]
    fn statvfs_reads_free_space_and_fails_on_a_missing_path() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(statvfs(tmp.path()).unwrap() > 0);
        assert!(statvfs(&tmp.path().join("gone")).is_err());
    }

    #[test]
    fn the_test_override_gives_whole_gib_or_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("free");
        assert!(forced(&f).is_err());
        std::fs::write(&f, "15\n").unwrap();
        assert_eq!(forced(&f), Ok(15 * GIB));
        std::fs::write(&f, "lots").unwrap();
        let want = format!("{}: \"lots\" is not a whole number of GiB", f.display());
        assert_eq!(forced(&f), Err(want));
    }
}
