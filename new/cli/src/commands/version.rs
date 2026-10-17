use anyhow::Result;
use clap::Args as ClapArgs;
use serde_json::json;

#[derive(ClapArgs)]
pub struct Args {}

pub fn run(_args: Args) -> Result<()> {
    println!("{}", json!({ "version": env!("CARGO_PKG_VERSION") }));
    Ok(())
}
