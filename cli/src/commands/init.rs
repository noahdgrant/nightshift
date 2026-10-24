use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::{forge, harness, labels, manifest, skill, sync, vcs};

/// The check that fails a change request which closes no unit.
const CLOSES_UNIT: &str = include_str!("../closes-unit.yml");
pub(crate) const CLOSES_UNIT_PATH: &str = ".github/workflows/closes-unit.yml";
/// The name the workflow's check reports, which the branch protection must require.
pub(crate) const CLOSES_UNIT_CHECK: &str = "Closes a unit";

#[derive(ClapArgs)]
pub struct Args {}

/// Pins the repository to this `ns`: records its version in `.nightshift/nightshift.toml`, along
/// with the harnesses the repository uses, writes their skills, and makes a session start with
/// the skills up to date, and creates the labels the forge lacks. When the repository has neither
/// `AGENTS.md` nor `CLAUDE.md`, writes both. Either way `AGENTS.md`, or `CLAUDE.md` when that is
/// all there is, is given a pointer to the coding standards. The files it leaves to commit are the
/// manifest, the hook, those files and the workflow that checks a change request closes a unit.
/// Run again, it changes only what is missing, and it never overwrites an existing workflow.
pub fn run(_args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let mut pin = manifest::pin_read(&root)?;
    pin.version = Some(env!("CARGO_PKG_VERSION").to_string());
    manifest::pin_write(&root, &pin)?;
    let synced = sync::run(&root, &pin.harness)?;
    let instruction_files = sync::instruction_files_write(&root)?;
    agents_point(&root)?;
    let mut hooks = Vec::new();
    for kind in &pin.harness {
        let wrote = harness::of(*kind).session_start_hook_install(&root, sync::HOOK)?;
        hooks.push(json!({ "harness": kind.name(), "written": wrote }));
    }
    let workflow = root.join(CLOSES_UNIT_PATH);
    let workflows_written = if workflow.exists() {
        0
    } else {
        std::fs::create_dir_all(workflow.parent().context("a workflow has no directory")?)?;
        std::fs::write(&workflow, CLOSES_UNIT)
            .with_context(|| format!("cannot write {}", workflow.display()))?;
        1
    };
    let labels = labels::ensure(forge::get().as_ref(), &root)
        .context("the labels were not created; run `ns init` again")?;
    println!(
        "{}",
        json!({
            "version": pin.version,
            "harness": pin.harness.iter().map(|k| k.name()).collect::<Vec<_>>(),
            "skills_written": synced.iter().map(|s| s.written).sum::<usize>(),
            "hooks": hooks,
            "workflows_written": workflows_written,
            "instruction_files": instruction_files,
            "labels_created": labels,
        })
    );
    Ok(())
}

/// Makes sure the repository's `AGENTS.md`, or its `CLAUDE.md` when that is all it has, points to
/// the coding standards that `ns sync` writes, and leaves the rest of the file as it is.
/// It creates nothing: a repository with neither file is given both by `instruction_files_write`.
fn agents_point(root: &std::path::Path) -> Result<()> {
    let file = skill::STANDARDS_PATH;
    let Some(path) = ["AGENTS.md", "CLAUDE.md"]
        .map(|name| root.join(name))
        .into_iter()
        .find(|p| p.symlink_metadata().is_ok())
    else {
        return Ok(());
    };
    let have = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // A link to nothing is the owner's; writing would create what it points to.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    if have.contains(file) {
        return Ok(());
    }
    let line = format!("- **Coding standards**: `{file}`, which holds in any language.\n");
    let text = format!("{}\n\n{line}", have.trim_end());
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workflow_reports_the_check_the_protection_requires() {
        assert!(CLOSES_UNIT.contains(&format!("name: {CLOSES_UNIT_CHECK}\n")));
    }
}
