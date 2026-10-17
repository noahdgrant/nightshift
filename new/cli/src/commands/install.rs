use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{os, skill};

#[derive(ClapArgs)]
pub struct Args {
    /// The directory holding the skills, one folder each with a `SKILL.md`.
    #[arg(long, default_value = "skills")]
    source: PathBuf,
}

/// Links each skill in `--source` into `ns`'s skills directory, so a change to the source takes
/// effect on the next run. A link that is already there is replaced.
pub fn run(args: Args) -> Result<()> {
    let source = args
        .source
        .canonicalize()
        .with_context(|| format!("cannot read {}", args.source.display()))?;
    let os = os::get();
    let target = skill::installed_dir()?;
    std::fs::create_dir_all(&target)?;
    let mut installed = Vec::new();
    for entry in std::fs::read_dir(&source)? {
        let from = entry?.path();
        if !from.join("SKILL.md").is_file() {
            continue;
        }
        let name = from.file_name().context("a skill has no name")?;
        let to = target.join(name);
        match std::fs::symlink_metadata(&to) {
            Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(&to)?,
            Ok(_) => bail!("{} exists and is not a link; remove it first", to.display()),
            Err(_) => {}
        }
        os.file_link(&from, &to)?;
        installed.push(name.to_string_lossy().into_owned());
    }
    installed.sort();
    println!("{}", json!({ "target": target, "installed": installed }));
    Ok(())
}
