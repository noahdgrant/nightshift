use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{manifest, sync, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// The version to pin the repository to. Default: the version of this `ns`.
    version: Option<String>,
}

/// Changes the version the repository is pinned to. The skills are written only for this `ns`'s
/// own version; for another, the `ns` of that version writes them when it next runs `ns sync`.
pub fn run(args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let own = env!("CARGO_PKG_VERSION");
    let version = args.version.unwrap_or_else(|| own.to_string());
    let mut pin = manifest::pin_read(&root)?;
    pin.version = Some(version.clone());
    manifest::pin_write(&root, &pin)?;
    let synced = version == own;
    if synced {
        sync::run(&root, &pin.harness)?;
    }
    println!("{}", json!({ "version": version, "synced": synced }));
    Ok(())
}
