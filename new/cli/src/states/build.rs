use anyhow::{bail, Context as _, Result};

use super::verdict::{self, Verdict};
use super::{Context, BUILT_SNAPSHOT};
use crate::artifact;
use crate::forge::Unit;
use crate::harness::Task;
use crate::state::Outcome;

/// Has the harness implement the unit in the unit's workspace and keeps what it says as
/// `build.md`, and a snapshot of what it left, for `verify` to hold the workspace to.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let dir = ctx
        .workspace
        .as_ref()
        .context("build ran before setup made a workspace")?
        .path
        .clone();
    let id = ctx.unit.to_string();
    let unit = ctx.forge.unit_get(&dir, ctx.unit)?;
    let evidence = verdict::evidence_ending_in(&dir, &id, Verdict::Fail);
    let reply = ctx.harness.task_run(&Task {
        dir: dir.clone(),
        prompt: prompt(&unit, evidence.as_deref()),
        write: true,
        model: None,
    })?;
    artifact::write(&dir, &id, "build.md", &reply.text)?;
    if !reply.success {
        bail!("the harness did not finish the build");
    }
    let snapshot = ctx.vcs.workspace_snapshot(&dir)?;
    artifact::write(&dir, &id, BUILT_SNAPSHOT, &snapshot)?;
    Ok(Outcome::Pass)
}

fn prompt(unit: &Unit, evidence: Option<&str>) -> String {
    let mut prompt = format!(
        "Implement this unit of work test-first, in small steps, and run the tests before you stop. \
         End with a short summary of what you changed.\n\n# {}\n\n{}\n",
        unit.title, unit.body
    );
    if let Some(evidence) = evidence {
        prompt.push_str(&format!(
            "\n# A check of your earlier work found it wrong\n\nFix what it found.\n\n{evidence}\n"
        ));
    }
    prompt
}
