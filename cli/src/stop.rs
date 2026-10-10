//! A stop that `ns watch` asks for on SIGINT or SIGTERM (docs/FACTORY.md, ns watch). The
//! handler only records the signal; the code polls `requested()` between steps, while a phase
//! runs and while it sleeps, and winds down from there.

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
#[cfg(unix)]
pub fn install() -> std::io::Result<()> {
    for sig in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: sigaction(2) with a zeroed struct, an empty mask and a handler that only
        // stores an atomic. SA_RESTART keeps interrupted reads and waits going; SA_RESETHAND
        // puts back the default action once the handler runs.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = record as extern "C" fn(libc::c_int) as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART | libc::SA_RESETHAND;
            libc::sigemptyset(&mut sa.sa_mask);
            if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn install() -> std::io::Result<()> {
    Ok(())
}

/// The signal that asked for a stop, if one has.
pub fn requested() -> Option<i32> {
    match SIGNAL.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

pub fn name(sig: i32) -> &'static str {
    #[cfg(unix)]
    {
        match sig {
            libc::SIGINT => return "SIGINT",
            libc::SIGTERM => return "SIGTERM",
            _ => {}
        }
    }
    let _ = sig;
    "signal"
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
