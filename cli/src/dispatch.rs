//! A repository is pinned to a version of `ns`. Whichever `ns` is run hands over to the pinned
//! one, so one install on the machine serves repositories on different versions.

use std::ffi::OsString;
use std::process::Command as Process;

use anyhow::{bail, Result};

use crate::{manifest, os, vcs, versions};

/// The commands that set the pin, install a version or check the setup. They run under whichever
/// `ns` was started. `--version` is not one of them: inside a pinned repository it names the
/// version that will run there, and anywhere else the version of this `ns`.
const UNPINNED: [&str; 5] = ["doctor", "init", "upgrade", "self", "config"];

/// Hands over to the `ns` the working repository is pinned to, when that is not this one. Returns
/// when this `ns` is the one to run the command. It looks at the arguments before they are parsed,
/// because the pinned version may have commands and flags this one has not.
pub fn run(args: &[OsString]) -> Result<()> {
    let first = args.get(1).and_then(|arg| arg.to_str()).unwrap_or_default();
    let skipped = UNPINNED.contains(&first)
        || std::env::var_os("NS_DISPATCHED").is_some()
        || std::env::var_os("NS_DEV").is_some();
    if skipped {
        return Ok(());
    }
    let Ok(top) = vcs::get().workspace_find(&std::env::current_dir()?) else {
        return Ok(());
    };
    let Some(pinned) = manifest::pin_read(&top)?.version else {
        return Ok(());
    };
    let own = env!("CARGO_PKG_VERSION");
    if pinned == own {
        return Ok(());
    }
    let binary = versions::binary(&versions::home()?, &pinned)?;
    if !binary.is_file() {
        bail!(
            "this repository is pinned to ns {pinned}, which is not installed (this is ns {own}). \
             Run `ns self install {pinned}`, or `ns upgrade` to pin this version."
        );
    }
    let mut next = Process::new(binary);
    next.args(args.iter().skip(1)).env("NS_DISPATCHED", "1");
    Err(os::get().process_replace(next))
}
