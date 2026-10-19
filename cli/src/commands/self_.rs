use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand};
use serde_json::json;

use crate::{forge, os, versions};

#[derive(ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Subcommand)]
enum Sub {
    /// Download a version of ns from its release, so a repository pinned to it can run.
    Install { version: String },
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Sub::Install { version } => {
            let path = versions::install(
                &versions::home()?,
                &version,
                forge::get().as_ref(),
                os::get().as_ref(),
            )?;
            println!("{}", json!({ "version": version, "path": path }));
        }
    }
    Ok(())
}
