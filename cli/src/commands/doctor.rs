use std::path::Path;

use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::forge::Forge;
use crate::{checks, forge, harness, labels, manifest, vcs};

#[derive(ClapArgs)]
pub struct Args {}

struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn pass(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        ok: true,
        detail: detail.into(),
    }
}

fn fail(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        ok: false,
        detail: detail.into(),
    }
}

/// Checks the setup of the repository this is run in and prints what it found. It changes
/// nothing, and fails when any check does.
pub fn run(_args: Args) -> Result<()> {
    let root = vcs::get().root_find(&std::env::current_dir()?)?;
    let forge = forge::get();
    let results = [
        pin(&root),
        harness(&root),
        forge_login(&root, forge.as_ref()),
        checks_command(&root),
        labels_present(&root, forge.as_ref()),
    ];
    let ok = results.iter().all(|c| c.ok);
    let list: Vec<_> = results
        .iter()
        .map(|c| json!({ "name": c.name, "ok": c.ok, "detail": c.detail }))
        .collect();
    println!("{}", json!({ "ok": ok, "checks": list }));
    if !ok {
        bail!("the setup has problems; each failed check says how to fix it");
    }
    Ok(())
}

fn pin(root: &Path) -> Check {
    let own = env!("CARGO_PKG_VERSION");
    match manifest::version_read(root) {
        Err(e) => fail("pin", format!("{e:#}; run `ns init`")),
        Ok(version) => match version {
            None => fail("pin", "the repository is not pinned; run `ns init`"),
            Some(version) if version == own => pass("pin", version),
            Some(version) => fail(
                "pin",
                format!("the repository is pinned to {version} but this is ns {own}; run `ns upgrade`, or run the ns of {version}"),
            ),
        },
    }
}

fn harness(root: &Path) -> Check {
    let kinds = match manifest::harness_read(root) {
        Ok(kinds) => kinds,
        Err(e) => return fail("harness", format!("{e:#}; run `ns init`")),
    };
    let mut seen = Vec::new();
    for kind in kinds {
        match harness::of(kind).version_get() {
            Ok(version) => seen.push(format!("{} {version}", kind.name())),
            Err(e) => {
                return fail(
                    "harness",
                    format!("{} cannot be started: {e:#}; install it", kind.name()),
                )
            }
        }
    }
    pass("harness", seen.join(", "))
}

fn forge_login(root: &Path, forge: &dyn Forge) -> Check {
    const FIX: &str = "run `ns config forge github --account <name>`";
    match forge.login_get(root) {
        Ok(login) => pass("forge", login),
        Err(e) => fail("forge", format!("{e:#}; {FIX}")),
    }
}

fn checks_command(root: &Path) -> Check {
    match checks::command_find(root) {
        Ok(Some(command)) => pass("checks", command),
        Ok(None) => fail(
            "checks",
            "put the command that runs the repository's checks in .nightshift/check",
        ),
        Err(e) => fail(
            "checks",
            format!("{e:#}; make .nightshift/check a file with the checks command in it"),
        ),
    }
}

fn labels_present(root: &Path, forge: &dyn Forge) -> Check {
    let have = match forge.labels_list(root) {
        Ok(have) => have,
        Err(e) => {
            return fail(
                "labels",
                format!(
                "the labels cannot be listed: {e:#}; fix the forge check first, then run `ns init`"
            ),
            )
        }
    };
    let missing: Vec<String> = labels::all(&forge::unit_types_get(forge, root))
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| !have.contains(name))
        .collect();
    if missing.is_empty() {
        pass("labels", "every label is there")
    } else {
        fail(
            "labels",
            format!("missing {}; run `ns init`", missing.join(", ")),
        )
    }
}
