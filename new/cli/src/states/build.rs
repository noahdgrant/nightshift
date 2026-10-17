use anyhow::{bail, Context as _, Result};

use super::verdict::{self, Verdict};
use super::{Context, BUILT_SNAPSHOT};
use crate::artifact;
use crate::forge::Unit;
use crate::harness::Task;
use crate::state::Outcome;

/// What the last build run did wrong, kept for the next one.
const PROBLEM: &str = "build.problem";

/// Has the harness implement the unit in the unit's workspace and commit its work, and keeps what
/// the harness says as `build.md` and a snapshot of the result, for `verify` to hold the
/// workspace to. The harness owns the commit and its message. `build` only makes sure nothing is
/// left uncommitted; a build that left changes goes wrong, and its next run is told which.
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
    let problem = artifact::read(&dir, &id, PROBLEM).ok();
    let reply = ctx.harness.task_run(&Task {
        dir: dir.clone(),
        prompt: prompt(&unit, evidence.as_deref(), problem.as_deref()),
        write: true,
        model: None,
    })?;
    artifact::write(&dir, &id, "build.md", &reply.text)?;
    if !reply.success {
        bail!("the harness did not finish the build");
    }
    let left = ctx.vcs.workspace_changes_list(&dir)?;
    if !left.is_empty() {
        let problem = format!(
            "The last build left uncommitted changes: {}.",
            left.join(", ")
        );
        artifact::write(&dir, &id, PROBLEM, &problem)?;
        bail!("{problem}");
    }
    artifact::remove(&dir, &id, PROBLEM)?;
    let snapshot = ctx.vcs.workspace_snapshot(&dir)?;
    artifact::write(&dir, &id, BUILT_SNAPSHOT, &snapshot)?;
    Ok(Outcome::Pass)
}

fn prompt(unit: &Unit, evidence: Option<&str>, problem: Option<&str>) -> String {
    let mut prompt = format!(
        "Implement this unit of work test-first, in small steps, and run the tests before you stop. \
         Then commit your work with a message that says what changed and why. End with a short \
         summary of what you changed.\n\n# {}\n\n{}\n",
        unit.title, unit.body
    );
    if let Some(evidence) = evidence {
        prompt.push_str(&format!(
            "\n# A check of your earlier work found it wrong\n\nFix what it found.\n\n{evidence}\n"
        ));
    }
    if let Some(problem) = problem {
        prompt.push_str(&format!("\n# {problem}\n\nCommit them.\n"));
    }
    prompt
}
