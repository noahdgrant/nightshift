use std::path::Path;

use anyhow::{bail, Result};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::commands::init::{CLOSES_UNIT_CHECK, CLOSES_UNIT_PATH};
use crate::forge::Forge;
use crate::{checks, forge, harness, labels, manifest, skill, status, vcs};

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
        protection(&root, forge.as_ref()),
        workflow(&root),
        standards(&root),
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
    let wanted = labels::all(&forge::unit_types_get(forge, root));
    let missing: Vec<String> = wanted
        .iter()
        .filter(|(name, _)| !have.iter().any(|l| &l.name == name))
        .map(|(name, _)| name.clone())
        .collect();
    let stale: Vec<String> = wanted
        .iter()
        .filter(|(name, description)| {
            have.iter()
                .any(|l| &l.name == name && &l.description != description)
        })
        .map(|(name, _)| name.clone())
        .collect();
    let mut problems = Vec::new();
    if have
        .iter()
        .any(|l| l.name == status::LEGACY_READY_FOR_HUMAN)
    {
        problems.push(format!(
            "{} still exists and `ns` no longer sets it",
            status::LEGACY_READY_FOR_HUMAN
        ));
    }
    if !missing.is_empty() {
        problems.push(format!("missing {}", missing.join(", ")));
    }
    if !stale.is_empty() {
        problems.push(format!(
            "the description of {} differs from this version's",
            stale.join(", ")
        ));
    }
    if problems.is_empty() {
        pass("labels", "every label is there")
    } else {
        fail("labels", format!("{}; run `ns init`", problems.join("; ")))
    }
}

fn protection(root: &Path, forge: &dyn Forge) -> Check {
    let got = match forge.branch_protection_get(root) {
        Ok(got) => got,
        Err(e) => {
            return fail(
                "protection",
                format!("the branch protection cannot be read: {e:#}; fix the forge check first"),
            )
        }
    };
    let branch = got.branch;
    let rules = match forge.branch_rules_get(root, &branch) {
        Ok(rules) => rules,
        Err(e) => {
            return fail(
                "protection",
                format!("the rules of {branch} cannot be read: {e:#}; fix the forge check first"),
            )
        }
    };
    let required = got.required_checks.clone().unwrap_or_default();
    if required
        .iter()
        .chain(&rules.required_checks)
        .any(|c| c == CLOSES_UNIT_CHECK)
    {
        return pass("protection", branch);
    }
    if rules.applies {
        return fail(
            "protection",
            format!(
                "the rules of {branch} do not require the check {CLOSES_UNIT_CHECK}; add it to {}",
                rules.setting
            ),
        );
    }
    match got.required_checks {
        None if got.protected => fail(
            "protection",
            format!(
                "the protection of {branch} does not require status checks, so it cannot require the check {CLOSES_UNIT_CHECK}; edit the protection in the repository settings to require it, which keeps the other rules"
            ),
        ),
        None => fail(
            "protection",
            format!(
                "{branch} is not protected; run `gh api --method PUT repos/{{owner}}/{{repo}}/branches/{branch}/protection -F 'required_status_checks[strict]=false' -f 'required_status_checks[contexts][]={CLOSES_UNIT_CHECK}' -F enforce_admins=false -F required_pull_request_reviews=null -F restrictions=null`"
            ),
        ),
        Some(_) => fail(
            "protection",
            format!(
                "the protection of {branch} does not require the check {CLOSES_UNIT_CHECK}; run `gh api --method POST repos/{{owner}}/{{repo}}/branches/{branch}/protection/required_status_checks/contexts -f 'contexts[]={CLOSES_UNIT_CHECK}'`"
            ),
        ),
    }
}

fn workflow(root: &Path) -> Check {
    const PATH: &str = CLOSES_UNIT_PATH;
    let Ok(text) = std::fs::read_to_string(root.join(PATH)) else {
        return fail(
            "workflow",
            format!("{PATH} is missing; run `ns init`, then commit it"),
        );
    };
    if text.contains("labeled") || text.contains("cancel-in-progress") {
        fail(
            "workflow",
            format!("{PATH} still has `labeled` or `cancel-in-progress`, which block a merge once its check is required; remove `labeled` and `unlabeled` from the pull_request types and remove the `concurrency` block"),
        )
    } else {
        pass("workflow", PATH)
    }
}

fn standards(root: &Path) -> Check {
    const PATH: &str = skill::STANDARDS_PATH;
    match std::fs::read_to_string(root.join(PATH)) {
        Ok(text) if text == skill::STANDARDS => pass("standards", PATH),
        Ok(_) => fail(
            "standards",
            format!("{PATH} differs from the pinned version's; run `ns sync`, then commit it"),
        ),
        Err(_) => fail(
            "standards",
            format!("{PATH} is missing; run `ns sync`, then commit it"),
        ),
    }
}
