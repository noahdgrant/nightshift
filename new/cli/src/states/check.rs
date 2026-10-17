use anyhow::{bail, Context as _, Result};

use super::verdict::{self, Verdict, ERROR_LINE};
use super::{Context, BUILT_SNAPSHOT};
use crate::artifact;
use crate::harness::Task;
use crate::state::Outcome;

/// What a state that checks the build does with its prompt: the harness runs it, and whatever it
/// says is kept as `evidence.md`. A check must leave the workspace as the build left it.
///
/// - The build passes or fails by the last line of the evidence.
/// - A check that goes wrong (the harness did not finish, the workspace changed, no verdict) says
///   nothing about the build: the evidence ends in an error verdict, the state goes wrong, and the
///   state's next run is told what happened and must put it right.
///
/// `write` is whether the harness may change files, which running the build needs.
pub fn run(ctx: &mut Context, write: bool, prompt: String) -> Result<Outcome> {
    let dir = ctx
        .workspace
        .as_ref()
        .context("a check ran before setup made a workspace")?
        .path
        .clone();
    let id = ctx.unit.to_string();
    let built = artifact::read(&dir, &id, BUILT_SNAPSHOT)?;
    let mut prompt = prompt;
    if let Some(error) = verdict::evidence_ending_in(&dir, &id, Verdict::Error) {
        prompt.push_str(&format!(
            "\n# Your last check went wrong\n\nPut the workspace back as the build left it \
             (undo any change you made to files), then check again.\n\n{error}\n"
        ));
    }
    let reply = ctx.harness.task_run(&Task {
        dir: dir.clone(),
        prompt,
        write,
        model: None,
    })?;
    let problem = if !reply.success {
        Some("the harness did not finish the check")
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
