use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand};
use serde_json::json;

use crate::vcs;
use crate::workspace;

#[derive(ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Subcommand)]
enum Sub {
    /// Create a unit's workspace, or print the one that exists.
    New {
        unit: String,
        /// The ref to branch from. Default: HEAD.
        #[arg(long)]
        base: Option<String>,
    },
    /// Remove a unit's workspace and keep its branch.
    Remove { unit: String },
}

pub fn run(args: Args) -> Result<()> {
    let here = std::env::current_dir()?;
    let vcs = vcs::get();
    match args.command {
        Sub::New { unit, base } => {
            let wt = workspace::ensure(vcs.as_ref(), &here, &unit, base.as_deref())?;
            println!(
                "{}",
                json!({ "unit": unit, "path": wt.path, "branch": wt.branch })
            );
        }
        Sub::Remove { unit } => {
            let removed = workspace::remove(vcs.as_ref(), &here, &unit)?;
            println!("{}", json!({ "unit": unit, "removed": removed }));
        }
    }
    Ok(())
}
