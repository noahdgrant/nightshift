use std::path::Path;

use anyhow::{bail, Context as _, Result};
use serde_json::json;

use super::{session_run, Context, BUILT_SNAPSHOT};
use crate::artifact::{self, DIR};
use crate::grades;
use crate::harness::{usage_limit_of, Task};
use crate::manifest::{self, Retrospective};
use crate::rubric::{self, Rubric};
use crate::runlog::RunLog;
use crate::state::{Outcome, State};
use crate::transcripts::Transcripts;

/// The most a session's text may be in the retrospective's prompt, in bytes. A longer one is cut
/// from the middle.
const SESSION_TEXT_LIMIT: usize = 40_000;

/// The file in the repository's `ns` state directory that keeps every grade, one JSON line each.
pub const GRADES_FILE: &str = "grades.jsonl";

/// What the retrospective's session is told before it is shown the unit.
const INSTRUCTIONS: &str = "You grade how one unit went. Its change has merged, and what follows \
is what happened: the unit, the build summary, the change, the notes the states left and the \
sessions that worked on it. You only read; change nothing in the workspace.\n\nGrade the unit by \
each rubric below. Judge each rubric on its own terms, from the evidence, and pick the value on \
its scale that fits best.\n\nEnd your reply with one line for each rubric, and nothing after \
them:\n\nGRADE: <rubric> <score> <reason>\n\n`<rubric>` is the rubric's name, `<score>` a number \
from 0 to 1, and `<reason>` one line.";

/// Grades the merged unit once, by each rubric in force, with one read-only harness session, and
/// keeps each grade in `grades.jsonl`. It never fails the unit: when it is disabled, or the session
/// fails, times out or gives no valid grade line, the run log says `skipped` with the reason and
/// the unit goes on, for the change has merged.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let root = ctx.vcs.root_find(&ctx.from)?;
    let setting = manifest::retrospective_read(&root)?;
    let id = ctx.unit.to_string();
    let log = RunLog::open(ctx.vcs.as_ref(), &root)?;
    let mut skips = Vec::new();
    if !setting.enabled {
        skips.push("disabled".to_string());
    } else if let Err(e) = grade(ctx, &root, &setting, &mut skips) {
        // A usage limit is not the retrospective's failure: the run stops and the grading runs
        // again once the limit has reset, for the grades cannot be had later.
        if usage_limit_of(&e).is_some() {
            return Err(e);
        }
        skips.push(format!("{e:#}"));
    }
    for reason in skips {
        let reason = reason.replace('\n', " ");
        let event = json!({ "unit": id, "retrospective": "skipped", "reason": reason });
        log.append(&event)?;
    }
    Ok(Outcome::Pass)
}

/// Grades the unit. What is left out on the way, a rubric file that is not valid or a rubric the
/// reply did not grade, goes to `skips`, one reason each.
fn grade(
    ctx: &mut Context,
    root: &Path,
    setting: &Retrospective,
    skips: &mut Vec<String>,
) -> Result<()> {
    let dir = ctx
        .workspace
        .as_ref()
        .context("retrospective ran before setup made a workspace")?
        .path
        .clone();
    let id = ctx.unit.to_string();
    let (rubrics, invalid) = rubric::in_force(&dir)?;
    skips.extend(invalid);
    let unit = ctx.forge.unit_get(&dir, ctx.unit)?;
    let build = artifact::read(&dir, &id, "build.md")
        .unwrap_or_else(|_| "(the unit has no build summary)".to_string());
    let base = ctx.vcs.workspace_base_find(&dir)?;
    let diff = ctx.vcs.workspace_diff(&dir, &base)?;
    let kept = Transcripts::open(ctx.vcs.as_ref(), root)?;
    let mut sessions = Vec::new();
    for session in kept.sessions(&id)? {
        let raw = std::fs::read(&session.path)?;
        let text = ctx
            .harness
            .transcript_render(&String::from_utf8_lossy(&raw));
        sessions.push((
            format!("{} {}", session.state, session.n),
            cut_middle(&text, SESSION_TEXT_LIMIT),
        ));
    }
    let prompt = prompt(
        &rubrics,
        &unit.title,
        &unit.body,
        &build,
        &diff,
        &notes_read(&dir, &id),
        &sessions,
    );
    // What the person asked for beats the repository's setting.
    let model = ctx.model.clone().or_else(|| setting.model.clone());
    let reply = session_run(
        ctx,
        State::Retrospective,
        &Task {
            dir: dir.clone(),
            prompt,
            write: false,
            model: model.clone(),
            timeout: ctx.harness_timeout,
            resume: None,
        },
    )?;
    if !reply.success {
        bail!("the harness did not finish the retrospective");
    }
    let (grades, missing) = rubric::grades_parse(&reply.text, &rubrics);
    if grades.is_empty() {
        bail!(
            "the reply has no valid `GRADE: <rubric> <score> <reason>` line for {}",
            missing.join(", ")
        );
    }
    for name in missing {
        skips.push(format!(
            "the reply has no valid `GRADE: <rubric> <score> <reason>` line for {name}"
        ));
    }
    // The base branch's commit when graded; the main checkout's own when the remote has none.
    let base = match ctx.vcs.base_fetch(root).ok().flatten() {
        Some(tip) => tip,
        None => ctx.vcs.workspace_head_find(root)?,
    };
    // Read again: the retrospective's own session is kept only now.
    let mut used = Vec::new();
    for session in kept.sessions(&id)? {
        let raw = std::fs::read(&session.path)?;
        used.push(grades::SessionUse {
            state: session.state.to_string(),
            usage: ctx.harness.transcript_usage(&String::from_utf8_lossy(&raw)),
        });
    }
    let used = grades::with_retrospective(used);
    let store = RunLog::sibling_open(&*ctx.vcs, root, GRADES_FILE)?;
    for grade in grades {
        let pass = rubrics
            .iter()
            .find(|r| r.name == grade.rubric)
            .is_some_and(|r| grade.score >= r.pass);
        let mut line = grades::line(&id, &grade, pass, &base, &used);
        if let Some(model) = &model {
            line["model"] = model.as_str().into();
        }
        store.append(&line)?;
    }
    Ok(())
}

/// The notes the states left under `.ns/<unit>/`, other than the build summary and the snapshot,
/// each under its file name, in name order.
fn notes_read(dir: &Path, unit: &str) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir.join(DIR).join(unit)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "build.md" && n != BUILT_SNAPSHOT)
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| Some((name.clone(), artifact::read(dir, unit, &name).ok()?)))
        .collect()
}

/// `text` as it is when it passes `limit` bytes: its start and its end, with the cut marked.
fn cut_middle(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut head = limit / 2;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - (limit - limit / 2);
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n[... {} bytes cut from the middle ...]\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

/// What the retrospective's session is given.
fn prompt(
    rubrics: &[Rubric],
    title: &str,
    body: &str,
    build: &str,
    diff: &str,
    notes: &[(String, String)],
    sessions: &[(String, String)],
) -> String {
    let mut text = format!("{INSTRUCTIONS}\n\n# The rubrics\n\n");
    for rubric in rubrics {
        text.push_str(&format!(
            "## {}\n\nIt passes at {}.\n\n{}\n\n",
            rubric.name, rubric.pass, rubric.body
        ));
    }
    text.push_str(&format!(
        "# The unit\n\n## {title}\n\n{body}\n\n# The build summary\n\n{build}\n\n# The change\n\n{diff}\n\n# The notes\n\n"
    ));
    for (name, note) in notes {
        text.push_str(&format!("## {name}\n\n{note}\n\n"));
    }
    text.push_str("# The sessions\n\n");
    for (name, session) in sessions {
        text.push_str(&format!("## {name}\n\n{session}\n\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_text_is_not_cut() {
        assert_eq!(cut_middle("abcdef", 6), "abcdef");
    }

    #[test]
    fn a_long_text_keeps_its_start_and_end_and_marks_the_cut() {
        assert_eq!(
            cut_middle("0123456789", 4),
            "01\n[... 6 bytes cut from the middle ...]\n89"
        );
    }

    #[test]
    fn a_cut_never_splits_a_character() {
        assert_eq!(
            cut_middle("ééééééé", 5),
            "é\n[... 10 bytes cut from the middle ...]\né"
        );
    }

    #[test]
    fn the_prompt_has_the_rubrics_the_unit_the_change_the_notes_and_the_sessions() {
        let rubrics = [Rubric {
            name: "efficiency".into(),
            pass: 0.6,
            body: "Judge.".into(),
        }];
        let got = prompt(
            &rubrics,
            "T",
            "B",
            "built",
            "diff",
            &[("evidence.md".into(), "ok".into())],
            &[("build 1".into(), "said".into())],
        );
        assert!(got.starts_with("You grade how one unit went."));
        assert!(got.contains("# The rubrics\n\n## efficiency\n\nIt passes at 0.6.\n\nJudge.\n\n"));
        assert!(got.contains("# The unit\n\n## T\n\nB\n\n# The build summary\n\nbuilt\n\n"));
        assert!(got.contains("# The change\n\ndiff\n\n# The notes\n\n## evidence.md\n\nok\n\n"));
        assert!(got.ends_with("# The sessions\n\n## build 1\n\nsaid\n\n"));
    }
}
