use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{skill, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// Overwrite the skills the repository already has, patched or not. `git diff` then shows
    /// what the repository had changed from this version.
    #[arg(long)]
    force: bool,
}

/// Copies the skills that came with this `ns` into the repository's `.nightshift/skills/`, to be
/// committed and patched. A skill the repository already has is kept unless `--force`.
pub fn run(args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let dir = skill::dir_find(&root);
    let mut report = Vec::new();
    for (name, text) in skill::embedded() {
        let file = dir.join(name).join("SKILL.md");
        let had = std::fs::read_to_string(&file).ok();
        let result = match &had {
            None => "created",
            Some(had) if had == text => "same",
            Some(_) if args.force => "overwritten",
            Some(_) => "patched, kept",
        };
        if matches!(result, "created" | "overwritten") {
            std::fs::create_dir_all(file.parent().context("a skill has no directory")?)?;
            std::fs::write(&file, text)
                .with_context(|| format!("cannot write {}", file.display()))?;
        }
        report.push(json!({ "skill": name, "result": result }));
    }
    println!("{}", json!({ "dir": dir, "skills": report }));
    Ok(())
}
