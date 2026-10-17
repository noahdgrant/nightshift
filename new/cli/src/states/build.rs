use std::path::Path;

use anyhow::{bail, Context as _, Result};

use super::verdict::{self, Verdict};
use super::{Context, BUILT_SNAPSHOT};
use crate::artifact;
use crate::checks;
use crate::forge::Unit;
use crate::harness::Task;
use crate::skill;
use crate::state::Outcome;

/// What the last build run did wrong, kept for the next one.
const PROBLEM: &str = "build.problem";

/// Has the harness implement the unit in the unit's workspace and commit its work as one commit,
/// and keeps what the harness says as `build.md` and a snapshot of the result, for `verify` to
/// hold the workspace to. The harness owns the commit and its message. `build` only makes sure
/// nothing is left uncommitted and the branch holds one commit; a build that did not goes wrong,
/// and its next run is told what.
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
    let base = ctx.vcs.workspace_base_find(&dir)?;
    let skill = skill::load(&ctx.vcs.root_find(&ctx.from)?, skill::BUILD)?;
    let checks_note = checks::command_find(&dir)?
        .is_some()
        .then(checks_note)
        .transpose()?;
    let reply = ctx.harness.task_run(&Task {
        dir: dir.clone(),
        prompt: prompt(
            &skill,
            &unit,
            &base,
            checks_note.as_deref(),
            evidence.as_deref(),
            problem.as_deref(),
        ),
        write: true,
        model: None,
        timeout: ctx.harness_timeout,
    })?;
    artifact::write(&dir, &id, "build.md", &reply.text)?;
    if !reply.success {
        bail!("the harness did not finish the build");
    }
    if let Some(problem) = what_is_left(ctx, &dir, &base)? {
        artifact::write(&dir, &id, PROBLEM, &problem)?;
        bail!("{problem}");
    }
    let root = ctx.vcs.root_find(&ctx.from)?;
    if let Some(report) = checks::run(
        ctx.vcs.as_ref(),
        ctx.os.as_ref(),
        &root,
        &dir,
        false,
        ctx.checks_timeout,
    )? {
        if !report.passed {
            let problem = format!(
                "The checks failed:\n{}\nFix them.",
                checks::tail(&report.output, 40)
            );
            artifact::write(&dir, &id, PROBLEM, &problem)?;
            bail!("the checks failed");
        }
    }
    artifact::remove(&dir, &id, PROBLEM)?;
    let snapshot = ctx.vcs.workspace_snapshot(&dir)?;
    artifact::write(&dir, &id, BUILT_SNAPSHOT, &snapshot)?;
    Ok(Outcome::Pass)
}

/// What a finished build got wrong about its commits, if anything: it must leave nothing
/// uncommitted and exactly one commit on the branch.
fn what_is_left(ctx: &Context, dir: &Path, base: &str) -> Result<Option<String>> {
    let left = ctx.vcs.workspace_changes_list(dir)?;
    if !left.is_empty() {
        return Ok(Some(format!(
            "The last build left uncommitted changes: {}. Commit them.",
            left.join(", ")
        )));
    }
    Ok(match ctx.vcs.workspace_commits_count(dir, base)? {
        1 => None,
        0 => Some("The last build left no commit on the branch. Commit your work.".into()),
        n => Some(format!(
            "The last build left {n} commits on the branch. Squash them into one."
        )),
    })
}

/// Tells the harness how to run the repository's checks, as the pre-commit hook would.
fn checks_note() -> Result<String> {
    let ns = std::env::current_exe().context("cannot find the ns program")?;
    Ok(format!(
        "To run the repository's checks, run `{} check` in your workspace.",
        ns.display()
    ))
}

fn prompt(
    skill: &str,
    unit: &Unit,
    base: &str,
    checks_note: Option<&str>,
    evidence: Option<&str>,
    problem: Option<&str>,
) -> String {
    let mut prompt = format!(
        "{}\n\n# The unit\n\n## {}\n\n{}\n\nThe branch started at commit {base}.\n",
        skill, unit.title, unit.body
    );
    if let Some(note) = checks_note {
        prompt.push_str(&format!("\n{note}\n"));
    }
    if let Some(evidence) = evidence {
        prompt.push_str(&format!(
            "\n# A check of your earlier work found it wrong\n\n{evidence}\n"
        ));
    }
    if let Some(problem) = problem {
        prompt.push_str(&format!("\n# {problem}\n"));
    }
    prompt
}
