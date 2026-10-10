//! A stop that `ns watch` asks for on SIGINT or SIGTERM (docs/FACTORY.md, ns watch). The
//! handler only records the signal; the code polls `requested()` between steps, while a phase
//! runs and while it sleeps, and winds down from there.

use std::borrow::Cow;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::error::SfError;

static SIGNAL: AtomicI32 = AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn record(sig: libc::c_int) {
    // An atomic store is async-signal-safe. The first signal wins.
    let _ = SIGNAL.compare_exchange(0, sig, Ordering::SeqCst, Ordering::SeqCst);
}

/// Record the first SIGINT and the first SIGTERM instead of dying on them. Each handler is
/// one-shot, so a second signal of the same kind ends the process at once.
pub fn install() -> std::io::Result<()> {
    handle(true)
}

/// As [`install`], for an `ns run` that `ns watch` started: every SIGINT and SIGTERM is only
/// recorded, so a signal that reaches the run both from watch and from a service manager
/// still lets it wind down. Watch dying sends the run SIGTERM (see `watch::scheduler`).
pub fn install_for_watch() -> std::io::Result<()> {
    handle(false)
}

/// Record `sig` as the stop, as if it had arrived, unless a stop is already recorded: `ns watch`
/// stops the night when a signal stopped one of its units.
pub fn request(sig: i32) {
    let _ = SIGNAL.compare_exchange(0, sig, Ordering::SeqCst, Ordering::SeqCst);
}

#[cfg(unix)]
fn handle(one_shot: bool) -> std::io::Result<()> {
    for sig in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: sigaction(2) with a zeroed struct, an empty mask and a handler that only
        // stores an atomic. SA_RESTART keeps interrupted reads and waits going; SA_RESETHAND,
        // when one-shot, puts back the default action once the handler runs.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = record as extern "C" fn(libc::c_int) as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART | if one_shot { libc::SA_RESETHAND } else { 0 };
            libc::sigemptyset(&mut sa.sa_mask);
            if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn handle(_: bool) -> std::io::Result<()> {
    Ok(())
}

/// The signal that asked for a stop, if one has.
pub fn requested() -> Option<i32> {
    match SIGNAL.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// "SIGKILL", or "signal 40" for one without a common name.
pub fn name(sig: i32) -> Cow<'static, str> {
    #[cfg(unix)]
    {
        const NAMES: &[(libc::c_int, &str)] = &[
            (libc::SIGHUP, "SIGHUP"),
            (libc::SIGINT, "SIGINT"),
            (libc::SIGQUIT, "SIGQUIT"),
            (libc::SIGILL, "SIGILL"),
            (libc::SIGTRAP, "SIGTRAP"),
            (libc::SIGABRT, "SIGABRT"),
            (libc::SIGBUS, "SIGBUS"),
            (libc::SIGFPE, "SIGFPE"),
            (libc::SIGKILL, "SIGKILL"),
            (libc::SIGUSR1, "SIGUSR1"),
            (libc::SIGSEGV, "SIGSEGV"),
            (libc::SIGUSR2, "SIGUSR2"),
            (libc::SIGPIPE, "SIGPIPE"),
            (libc::SIGALRM, "SIGALRM"),
            (libc::SIGTERM, "SIGTERM"),
            (libc::SIGXCPU, "SIGXCPU"),
            (libc::SIGXFSZ, "SIGXFSZ"),
            (libc::SIGSYS, "SIGSYS"),
        ];
        if let Some((_, n)) = NAMES.iter().find(|(s, _)| *s == sig) {
            return Cow::Borrowed(n);
        }
    }
    Cow::Owned(format!("signal {sig}"))
}

/// The error a step returns when it stops for `sig`; its exit code is 128 + the signal.
pub fn error(sig: i32) -> anyhow::Error {
    SfError::new(128 + sig, format!("stopped by {}", name(sig))).into()
}

/// `Err` once a stop is requested.
pub fn check() -> anyhow::Result<()> {
    match requested() {
        Some(sig) => Err(error(sig)),
        None => Ok(()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn name_knows_the_common_signals() {
        assert_eq!(super::name(libc::SIGKILL), "SIGKILL");
        assert_eq!(super::name(libc::SIGSEGV), "SIGSEGV");
        assert_eq!(super::name(libc::SIGTERM), "SIGTERM");
        let rt = libc::SIGRTMIN() + 1;
        assert_eq!(super::name(rt), format!("signal {rt}"));
    }
}
