use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use serde_json::json;

use crate::config;

#[derive(ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Subcommand)]
enum Sub {
    /// Name the account whose login ns uses for the forge. A `GH_TOKEN` already set wins.
    Forge {
        forge: Forge,
        /// The account, as `gh auth status` lists it.
        #[arg(long)]
        account: String,
    },
}

#[derive(Clone, ValueEnum)]
enum Forge {
    Github,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Sub::Forge {
            forge: Forge::Github,
            account,
        } => {
            let file = config::github_account_set(&account)?;
            println!(
                "{}",
                json!({ "config": file, "forge": "github", "account": account })
            );
        }
    }
    Ok(())
}
