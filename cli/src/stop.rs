//! A stop that `ns watch` asks for on SIGINT or SIGTERM (docs/factory/watch.md). The
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

/// Run `f` with [`install`]'s handlers, then put back the ones that were set. For a command
/// whose children are in sessions of their own, so a terminal's Ctrl-C reaches only this
/// process and `f` has to end them itself, but whose later code does not poll for a stop.
#[cfg(unix)]
pub fn scoped<T>(f: impl FnOnce() -> T) -> std::io::Result<T> {
    // SAFETY: zeroed sigaction structs, filled in by sigaction(2) below.
    let mut saved: [libc::sigaction; 2] = unsafe { std::mem::zeroed() };
    install_saving(&mut saved)?;
    let r = f();
    for (sig, old) in [libc::SIGINT, libc::SIGTERM].into_iter().zip(&saved) {
        // SAFETY: sigaction(2) with the action it returned earlier.
        unsafe {
            libc::sigaction(sig, old, std::ptr::null_mut());
        }
    }
    Ok(r)
}

#[cfg(not(unix))]
pub fn scoped<T>(f: impl FnOnce() -> T) -> std::io::Result<T> {
    Ok(f())
}

#[cfg(unix)]
fn install_saving(saved: &mut [libc::sigaction; 2]) -> std::io::Result<()> {
    for (sig, old) in [libc::SIGINT, libc::SIGTERM].into_iter().zip(saved) {
        // SAFETY: as in `handle`; the previous action goes to `old`.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = record as extern "C" fn(libc::c_int) as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART | libc::SA_RESETHAND;
            libc::sigemptyset(&mut sa.sa_mask);
            if libc::sigaction(sig, &sa, old) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(())
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

/// Hold SIGINT and SIGTERM pending instead of delivering them to this thread, so one that lands
/// while this process is replaced by `exec` reaches the new program rather than being lost.
#[cfg(unix)]
pub fn block() {
    change_mask(libc::SIG_BLOCK);
}

/// Deliver the SIGINT and SIGTERM [`block`] held back, now to the handlers [`install`] set.
#[cfg(unix)]
pub fn unblock() {
    change_mask(libc::SIG_UNBLOCK);
}

#[cfg(unix)]
fn change_mask(how: libc::c_int) {
    // SAFETY: pthread_sigmask(3) with a set made here; async-signal-safe enough for `pre_exec`.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(how, &set, std::ptr::null_mut());
    }
}

#[cfg(not(unix))]
pub fn block() {}

#[cfg(not(unix))]
pub fn unblock() {}

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
    extern "C" fn other(_: libc::c_int) {}

    fn action(sig: libc::c_int) -> (libc::sighandler_t, libc::c_int) {
        // SAFETY: sigaction(2) with no new action only reads the current one.
        let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::sigaction(sig, std::ptr::null(), &mut sa) },
            0
        );
        (sa.sa_sigaction, sa.sa_flags)
    }

    fn set(sig: libc::c_int, handler: libc::sighandler_t) {
        // SAFETY: sigaction(2) with a zeroed struct and an empty mask.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handler;
            libc::sigemptyset(&mut sa.sa_mask);
            assert_eq!(libc::sigaction(sig, &sa, std::ptr::null_mut()), 0);
        }
    }

    #[test]
    fn scoped_puts_back_the_sigint_and_sigterm_handlers_it_found() {
        let theirs = other as extern "C" fn(libc::c_int) as libc::sighandler_t;
        set(libc::SIGINT, theirs);
        set(libc::SIGTERM, libc::SIG_IGN);
        let before = (action(libc::SIGINT), action(libc::SIGTERM));
        let inside = super::scoped(|| (action(libc::SIGINT), action(libc::SIGTERM))).unwrap();
        let after = (action(libc::SIGINT), action(libc::SIGTERM));
        set(libc::SIGINT, libc::SIG_DFL);
        set(libc::SIGTERM, libc::SIG_DFL);
        assert_ne!(inside, before, "scoped installed nothing");
        assert_eq!(after, before);
        assert_eq!(before.0 .0, theirs);
        assert_eq!(before.1 .0, libc::SIG_IGN);
    }

    const SCOPED_MARK: &str = "NS_STOP_SCOPED_PROBE";

    #[test]
    fn a_sigint_or_sigterm_inside_scoped_is_recorded_as_a_stop() {
        for (mode, sig) in [("int", libc::SIGINT), ("term", libc::SIGTERM)] {
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "stop::tests::scoped_probe",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env(SCOPED_MARK, mode)
                .output()
                .unwrap();
            let log = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{mode}: {log}");
            assert!(log.contains(&format!("requested={sig}\n")), "{mode}: {log}");
        }
    }

    /// Run by the test above in a process of its own, since a stop request is never undone.
    #[test]
    fn scoped_probe() {
        let Ok(mode) = std::env::var(SCOPED_MARK) else {
            return;
        };
        let sig = if mode == "int" {
            libc::SIGINT
        } else {
            libc::SIGTERM
        };
        assert_eq!(super::requested(), None);
        super::scoped(|| {
            // SAFETY: raise(3) delivers the signal to this thread, whose handler only stores.
            assert_eq!(unsafe { libc::raise(sig) }, 0);
        })
        .unwrap();
        println!("requested={}", super::requested().unwrap_or(0));
    }

    #[test]
    fn name_knows_the_common_signals() {
        assert_eq!(super::name(libc::SIGKILL), "SIGKILL");
        assert_eq!(super::name(libc::SIGSEGV), "SIGSEGV");
        assert_eq!(super::name(libc::SIGTERM), "SIGTERM");
        let rt = libc::SIGRTMIN() + 1;
        assert_eq!(super::name(rt), format!("signal {rt}"));
    }
}
