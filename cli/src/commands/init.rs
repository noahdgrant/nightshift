use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{forge, harness, labels, manifest, skill, sync, vcs};

#[derive(ClapArgs)]
pub struct Args {}

/// Pins the repository to this `ns`: records its version in `.nightshift/nightshift.toml`, along
/// with the harnesses the repository uses, writes their skills, and makes a session start with
/// the skills up to date, and creates the labels the forge lacks. The files it leaves to
/// commit are the manifest and the hook. Run again, it changes only what is missing.
pub fn run(_args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let mut pin = manifest::pin_read(&root)?;
    pin.version = Some(env!("CARGO_PKG_VERSION").to_string());
    manifest::pin_write(&root, &pin)?;
    let synced = sync::run(&root, &pin.harness)?;
    agents_point(&root, harness::of(pin.harness[0]).skills_dir())?;
    let mut hooks = Vec::new();
    for kind in &pin.harness {
        let wrote = harness::of(*kind).session_start_hook_install(&root, sync::HOOK)?;
        hooks.push(json!({ "harness": kind.name(), "written": wrote }));
    }
    let labels = labels::ensure(forge::get().as_ref(), &root)
        .context("the labels were not created; run `ns init` again")?;
    println!(
        "{}",
        json!({
            "version": pin.version,
            "harness": pin.harness.iter().map(|k| k.name()).collect::<Vec<_>>(),
            "skills_written": synced.iter().map(|s| s.written).sum::<usize>(),
            "hooks": hooks,
            "labels_created": labels,
        })
    );
    Ok(())
}

/// Makes sure the repository's `AGENTS.md` points to the coding standards that `ns sync` writes
/// in `dir`, creating the file when there is none and leaving the rest of it as it is.
fn agents_point(root: &std::path::Path, dir: &str) -> Result<()> {
    let path = root.join("AGENTS.md");
    let file = format!("{dir}/{}", skill::STANDARDS_FILE);
    let have = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    if have.contains(&file) {
        return Ok(());
    }
    let line = format!("- **Coding standards**: `{file}`, which holds in any language.\n");
    let text = if have.is_empty() {
        format!("# Agents\n\n{line}")
    } else {
        format!("{}\n{line}", have.trim_end())
    };
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}
