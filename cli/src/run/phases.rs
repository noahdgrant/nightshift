//! Running one phase: its harness command under a timeout and memory cap, in a private `TMPDIR`.

use std::fs::{self, File};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::billing;
use crate::error::SfError;
use crate::eval::trial::run_process;
use crate::gate;
use crate::memcap::{self, Cap};

/// A phase's timeout. `NS_PHASE_TIMEOUT_MS` overrides it: `<ms>` for every phase, or
/// comma-separated `<phase>=<ms>` for the named phases only.
pub(super) fn phase_timeout(phase: &str, minutes: u64) -> Duration {
    let var = std::env::var("NS_PHASE_TIMEOUT_MS").ok();
    timeout_override(var.as_deref(), phase).unwrap_or_else(|| Duration::from_secs(minutes * 60))
}

/// The override for `phase` in an `NS_PHASE_TIMEOUT_MS` value; a named entry beats a bare one.
fn timeout_override(var: Option<&str>, phase: &str) -> Option<Duration> {
    let mut bare = None;
    for part in var?.split(',') {
        match part.split_once('=') {
            Some((p, ms)) if p.trim() == phase => {
                if let Ok(ms) = ms.trim().parse::<u64>() {
                    return Some(Duration::from_millis(ms));
                }
            }
            Some(_) => {}
            None => {
                if bare.is_none() {
                    bare = part.trim().parse::<u64>().ok();
                }
            }
        }
    }
    bare.map(Duration::from_millis)
}

pub(super) struct PhaseRun {
    pub(super) exit: Option<i32>,
    pub(super) timed_out: bool,
    /// Why the run failed at the memory cap or by a signal `ns` didn't send.
    pub(super) killed: Option<String>,
    pub(super) wall_s: f64,
    pub(super) stdout: String,
}

#[allow(
    clippy::too_many_arguments,
    reason = "each argument is one thing the harness runs with"
)]
pub(super) fn run_harness(
    argv: &[String],
    prompt: &str,
    cwd: &Path,
    env: &[(&str, String)],
    timeout: Duration,
    transcript: &Path,
    subscription: bool,
    cap: Option<&Cap>,
) -> Result<PhaseRun> {
    if let Some(d) = transcript.parent() {
        fs::create_dir_all(d).with_context(|| format!("cannot create {}", d.display()))?;
    }
    let out = File::create(transcript)
        .with_context(|| format!("cannot create {}", transcript.display()))?;
    let stderr = transcript.with_extension("stderr");
    let err = File::create(&stderr)?;
    let not_found = || -> anyhow::Error {
        SfError::usage(
            format!("harness binary `{}` is not on PATH", argv[0]),
            "install it, or set [defaults] harness in nightshift.toml; check with:\n  ns factory validate",
        )
        .into()
    };
    // Under a cap the spawned program is systemd-run or prlimit, so a missing harness would
    // be a failed attempt rather than the usage error: look for it first.
    if cap.is_some() {
        let found = if argv[0].contains(std::path::MAIN_SEPARATOR) {
            crate::which::which(&cwd.join(&argv[0]).to_string_lossy())
        } else {
            crate::which::which(&argv[0])
        };
        if found.is_none() {
            return Err(not_found());
        }
    }
    let (mut cmd, watch) = memcap::command(cap, &argv[0]);
    crate::git::scrub(&mut cmd);
    crate::cpus::apply(&mut cmd);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdout(out)
        .stderr(err);
    for (k, v) in env {
        cmd.env(k, v);
    }
    if subscription {
        billing::scrub(&mut cmd, argv);
    }
    if billing::is_claude(argv) {
        billing::wait_for_bg_tasks(&mut cmd);
        billing::disable_auto_memory(&mut cmd);
    }
    let (status, timed_out, wall_s) =
        match run_process(cmd, Some(prompt.as_bytes().to_vec()), timeout) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found()),
            Err(e) => return Err(e).with_context(|| format!("cannot start {}", argv[0])),
        };
    let killed = watch
        .cut_short(status, timed_out, || {
            String::from_utf8_lossy(&gate::read_window(&stderr)).into_owned()
        })
        .map(memcap::Cut::reason);
    Ok(PhaseRun {
        exit: status.and_then(|s| s.code()),
        timed_out,
        killed,
        wall_s,
        stdout: fs::read_to_string(transcript).unwrap_or_default(),
    })
}

/// A new, empty `TMPDIR` for one attempt of `phase` in `parent`, the unit's `Repo::unit_tmp`, so
/// parallel units never share a scratch file. It is `<phase>-<n>` for the first `n` from `attempt`
/// not taken, so a directory a failed phase of an earlier run kept survives until `ns clean`.
pub(super) fn phase_tmpdir(parent: &Path, phase: &str, attempt: u32) -> Result<PathBuf> {
    private_dir(parent)?;
    for n in attempt.. {
        let dir = parent.join(format!("{phase}-{n}"));
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.exists() => {}
            made => {
                made.with_context(|| format!("cannot create {}", dir.display()))?;
                return Ok(dir);
            }
        }
    }
    anyhow::bail!("no free {phase}-<n> directory in {}", parent.display())
}

/// Whether a path with these attributes is a directory owned by `euid` that no group or other
/// bit opens.
fn usable_only_by_us(is_dir: bool, uid: u32, euid: u32, mode: u32) -> bool {
    is_dir && uid == euid && mode & 0o077 == 0
}

/// Create `dir` with mode 0700, or check the one there is a directory only this user can use: the
/// system temp dir is shared, and another user could have made it first.
fn private_dir(dir: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        made => return made.with_context(|| format!("cannot create {}", dir.display())),
    }
    let m = fs::symlink_metadata(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if !usable_only_by_us(m.is_dir(), m.uid(), euid, m.mode()) {
        anyhow::bail!(
            "{} is not a directory only this user can use; remove it",
            dir.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_tmpdir_takes_the_first_free_attempt_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("ns-r-u1");
        assert_eq!(
            phase_tmpdir(&parent, "build", 2).unwrap(),
            parent.join("build-2")
        );
        let made = phase_tmpdir(&parent, "build", 2).unwrap();
        assert_eq!(made, parent.join("build-3"));
        for d in [&parent, &made] {
            let mode = fs::metadata(d).unwrap().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", d.display());
        }
        assert_eq!(
            phase_tmpdir(&parent, "build", 1).unwrap(),
            parent.join("build-1")
        );
    }

    #[test]
    fn a_dangling_symlink_where_a_phase_tmpdir_goes_is_an_error_not_a_skipped_slot() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("ns-r-u1");
        let slot = parent.join("build-1");
        phase_tmpdir(&parent, "verify", 1).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), &slot).unwrap();
        let err = phase_tmpdir(&parent, "build", 1).unwrap_err();
        assert!(err.to_string().contains("cannot create"), "{err}");
        assert!(!parent.join("build-2").exists());
    }

    #[test]
    fn a_phase_tmpdir_parent_that_is_not_a_private_dir_of_ours_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let open = dir.path().join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o750)).unwrap();
        let link = dir.path().join("link");
        let private = dir.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&private, &link).unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        for p in [&open, &link, &file] {
            let err = private_dir(p).unwrap_err().to_string();
            assert!(
                err.contains("not a directory only this user can use"),
                "{err}"
            );
        }
        private_dir(&private).unwrap();
        let err = private_dir(&dir.path().join("no/such")).unwrap_err();
        assert!(err.to_string().contains("cannot create"), "{err}");
    }

    #[test]
    fn a_dir_is_ours_alone_only_when_we_own_it_and_group_and_other_have_no_bits() {
        assert!(usable_only_by_us(true, 1000, 1000, 0o40700));
        assert!(usable_only_by_us(true, 1000, 1000, 0o40500));
        assert!(!usable_only_by_us(true, 1001, 1000, 0o40700), "owner");
        assert!(!usable_only_by_us(false, 1000, 1000, 0o100600), "not a dir");
        for bit in [0o040, 0o020, 0o010, 0o004, 0o002, 0o001] {
            assert!(
                !usable_only_by_us(true, 1000, 1000, 0o40700 | bit),
                "{bit:o}"
            );
        }
    }

    #[test]
    fn timeout_override_parses_bare_named_and_malformed_values() {
        let ms = |v: Option<&str>, p: &str| timeout_override(v, p).map(|d| d.as_millis());
        assert_eq!(ms(None, "build"), None);
        assert_eq!(ms(Some(""), "build"), None);
        assert_eq!(ms(Some("1500"), "build"), Some(1500));
        assert_eq!(ms(Some("1500"), "review"), Some(1500));
        assert_eq!(ms(Some("build=1500"), "build"), Some(1500));
        assert_eq!(ms(Some("build=1500"), "review"), None);
        assert_eq!(ms(Some("build=1,review=2"), "review"), Some(2));
        assert_eq!(ms(Some(" build = 7 , review = 8 "), "review"), Some(8));
        assert_eq!(ms(Some(" 9 "), "ship"), Some(9));
        assert_eq!(ms(Some("abc"), "build"), None);
        assert_eq!(ms(Some("build=abc"), "build"), None);
        assert_eq!(ms(Some("build=,review=3"), "review"), Some(3));
        assert_eq!(ms(Some("5,build=1"), "build"), Some(1));
        assert_eq!(ms(Some("build=1,5"), "build"), Some(1));
        assert_eq!(ms(Some("build=1,5"), "review"), Some(5));
        assert_eq!(ms(Some("build=abc,5"), "build"), Some(5));
    }
}
