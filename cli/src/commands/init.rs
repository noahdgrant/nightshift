use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{manifest, vcs};

#[derive(ClapArgs)]
pub struct Args {}

/// Pins the repository to this `ns`: records its version in `.nightshift/nightshift.toml`, along
/// with the harnesses the repository uses.
pub fn run(_args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let mut pin = manifest::pin_read(&root)?;
    pin.version = Some(env!("CARGO_PKG_VERSION").to_string());
    manifest::pin_write(&root, &pin)?;
    println!(
        "{}",
        json!({ "version": pin.version, "harness": pin.harness.iter().map(|k| k.name()).collect::<Vec<_>>() })
    );
    Ok(())
}
