use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{manifest, skill, vcs};

#[derive(ClapArgs)]
pub struct Args {
    /// Overwrite the skills the repository has patched too. `git diff` then shows the patches.
    #[arg(long)]
    force: bool,
}

/// Copies the skills that came with this `ns` into the repository's `.nightshift/skills/`, to be
/// committed and patched, and records in `.nightshift/nightshift.toml` what it copied. A skill
/// the repository has not touched since is updated; one it has patched is kept unless `--force`.
pub fn run(args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let dir = skill::dir_find(&root);
    let recorded = manifest::skills_read(&root)?;
    let mut report = Vec::new();
    let mut now = Vec::new();
    for (name, text) in skill::embedded() {
        let file = dir.join(name).join("SKILL.md");
        let had = std::fs::read_to_string(&file).ok();
        let before = recorded.iter().find(|(n, _)| n == name).map(|(_, h)| h);
        let result = match &had {
            None => "created",
            Some(had) if had == text => "same",
            Some(_) if args.force => "overwritten",
            Some(had) if before == Some(&manifest::hash(had)) => "updated",
            Some(_) => "patched, kept",
        };
        if matches!(result, "created" | "overwritten" | "updated") {
            std::fs::create_dir_all(file.parent().context("a skill has no directory")?)?;
            std::fs::write(&file, text)
                .with_context(|| format!("cannot write {}", file.display()))?;
        }
        // A kept patch stays recorded as what was copied, so it is still known to be a patch.
        match (result, before) {
            ("patched, kept", Some(before)) => now.push((name.to_string(), before.clone())),
            _ => now.push((name.to_string(), manifest::hash(text))),
        }
        report.push(json!({ "skill": name, "result": result }));
    }
    manifest::skills_write(&root, env!("CARGO_PKG_VERSION"), &now)?;
    println!("{}", json!({ "dir": dir, "skills": report }));
    Ok(())
}
