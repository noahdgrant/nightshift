use anyhow::{bail, Context as _, Result};

use super::verdict::{self, Verdict, ERROR_LINE, FAIL_LINE, PASS_LINE};
use super::{Context, BUILT_SNAPSHOT};
use crate::artifact;
use crate::harness::Task;
use crate::state::Outcome;

/// Has the harness check the build against the unit's description by running it, and keeps what
/// it says as `evidence.md`. The check must leave the workspace as the build left it.
///
/// - The work passes or fails by the last line of the evidence.
/// - A check that goes wrong (the harness did not finish, the workspace changed, no verdict) says
///   nothing about the work: the evidence ends in an error verdict, the state goes wrong, and the
///   next run of `verify` is told what happened and must put it right.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let dir = ctx
        .workspace
        .as_ref()
        .context("verify ran before setup made a workspace")?
        .path
        .clone();
    let id = ctx.unit.to_string();
    let unit = ctx.forge.unit_get(&dir, ctx.unit)?;
    let build = artifact::read(&dir, &id, "build.md")?;
    let built = artifact::read(&dir, &id, BUILT_SNAPSHOT)?;
    let earlier_error = verdict::evidence_ending_in(&dir, &id, Verdict::Error);
    let reply = ctx.harness.task_run(&Task {
        dir: dir.clone(),
        prompt: prompt(&unit.title, &unit.body, &build, earlier_error.as_deref()),
        write: true,
        model: None,
    })?;
    let problem = if !reply.success {
        Some("the harness did not finish verifying")
    } else if ctx.vcs.workspace_snapshot(&dir)? != built {
        Some("the workspace is not as the build left it; a check must only look")
    } else if verdict::parse(&reply.text).is_none() {
        Some("the evidence does not end with a verdict")
    } else {
        None
    };
    let Some(problem) = problem else {
        artifact::write(&dir, &id, "evidence.md", &reply.text)?;
        return Ok(match verdict::parse(&reply.text) {
            Some(Verdict::Pass) => Outcome::Pass,
            _ => Outcome::Fail,
        });
    };
    let evidence = format!(
        "{}\n\nThe check itself went wrong: {problem}.\n{ERROR_LINE}\n",
        reply.text.trim_end()
    );
    artifact::write(&dir, &id, "evidence.md", &evidence)?;
    bail!("{problem}")
}

fn prompt(title: &str, body: &str, build: &str, earlier_error: Option<&str>) -> String {
    let mut prompt = format!(
        "Check that the work described below is done, by running it the way a user would, not \
         by reading the code. Do not change any source file. For each requirement, say what you \
         ran and what you saw. End with a last line that is exactly `{PASS_LINE}` or \
         `{FAIL_LINE}`.\n\n# {title}\n\n{body}\n\n# What the build says it did\n\n{build}\n"
    );
    if let Some(error) = earlier_error {
        prompt.push_str(&format!(
            "\n# Your last check went wrong\n\nPut the workspace back as the build left it \
             (undo any change you made to files), then check again.\n\n{error}\n"
        ));
    }
    prompt
}
