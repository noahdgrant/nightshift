use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{harness, manifest, sync, vcs};

#[derive(ClapArgs)]
pub struct Args {}

/// Pins the repository to this `ns`: records its version in `.nightshift/nightshift.toml`, along
/// with the harnesses the repository uses, writes their skills, and makes a session start with
/// the skills up to date. The files it leaves to commit are the manifest and the hook.
pub fn run(_args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let mut pin = manifest::pin_read(&root)?;
    pin.version = Some(env!("CARGO_PKG_VERSION").to_string());
    manifest::pin_write(&root, &pin)?;
    let synced = sync::run(&root, &pin.harness)?;
    let mut hooks = Vec::new();
    for kind in &pin.harness {
        let wrote = harness::of(*kind).session_start_hook_install(&root, sync::HOOK)?;
        hooks.push(json!({ "harness": kind.name(), "written": wrote }));
    }
    println!(
        "{}",
        json!({
            "version": pin.version,
            "harness": pin.harness.iter().map(|k| k.name()).collect::<Vec<_>>(),
            "skills_written": synced.iter().map(|s| s.written).sum::<usize>(),
            "hooks": hooks,
        })
    );
    Ok(())
}
