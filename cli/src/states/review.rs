use anyhow::{Context as _, Result};

use super::rounds::Rounds;
use super::verdict::{self, Verdict, FAIL_LINE, PASS_LINE};
use super::{check, Context};
use crate::artifact;
use crate::manifest;
use crate::skill;
use crate::state::{Outcome, State};

/// What one review session looks at: a name for it, and the review areas it covers. The skill has
/// a section for each area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    pub name: &'static str,
    pub areas: &'static [&'static str],
}

const AREAS: [&str; 6] = [
    "correctness",
    "security",
    "spec",
    "tests",
    "architecture",
    "docs and comments",
];

const ALL: [Focus; 1] = [Focus {
    name: "All areas",
    areas: &AREAS,
}];

const GROUPS: [Focus; 3] = [
    Focus {
        name: "Behavior",
        areas: &["correctness", "security"],
    },
    Focus {
        name: "Requirements and tests",
        areas: &["spec", "tests"],
    },
    Focus {
        name: "Maintainability",
        areas: &["architecture", "docs and comments"],
    },
];

const EACH: [Focus; 6] = [
    Focus {
        name: "Correctness",
        areas: &["correctness"],
    },
    Focus {
        name: "Security",
        areas: &["security"],
    },
    Focus {
        name: "Spec",
        areas: &["spec"],
    },
    Focus {
        name: "Tests",
        areas: &["tests"],
    },
    Focus {
        name: "Architecture",
        areas: &["architecture"],
    },
    Focus {
        name: "Docs and comments",
        areas: &["docs and comments"],
    },
];

/// The sessions a review of `sessions` (1, 3 or 6, as `[review] sessions` allows) runs, in order.
pub fn focuses(sessions: u32) -> &'static [Focus] {
    match sessions {
        1 => &ALL,
        3 => &GROUPS,
        _ => &EACH,
    }
}

/// The heading that starts a session's section in the kept review.
const SECTION: &str = "# Review session: ";

/// Has the harness review the unit's changes, in one, three or six sessions one after another. They
/// read the change and change nothing. See [`check::run`] for how a session ends; a review fails
/// when any session fails, and goes wrong when any session does.
pub fn run(ctx: &mut Context) -> Result<Outcome> {
    let dir = ctx
        .workspace
        .as_ref()
        .context("review ran before setup made a workspace")?
        .path
        .clone();
    let root = ctx.vcs.root_find(&ctx.from)?;
    let sessions = manifest::review_sessions_read(&root)?;
    let unit = ctx.forge.unit_get(&dir, ctx.unit)?;
    let build = artifact::read(&dir, &ctx.unit.to_string(), "build.md")?;
    let base = ctx.vcs.workspace_base_find(&dir)?;
    let diff = ctx.vcs.workspace_diff(&dir, &base)?;
    let skill = skill::load(skill::REVIEW)?;
    let id = ctx.unit.to_string();
    let reviews = Rounds::new(&dir, &id, "review");
    let previous = reviews.last_unmet();
    let mut kept = Vec::new();
    let mut passed = true;
    for focus in focuses(sessions) {
        let earlier = previous
            .as_deref()
            .and_then(|text| earlier_findings(text, focus));
        let outcome = check::run(
            ctx,
            State::Review,
            false,
            prompt(
                &skill,
                focus,
                &unit.title,
                &unit.body,
                &build,
                &diff,
                earlier,
            ),
        )?;
        passed &= matches!(outcome, Outcome::Pass);
        kept.push((focus, artifact::read(&dir, &id, "evidence.md")?));
    }
    // A review asks for only PASS and FAIL; any other verdict does not let the unit ship.
    let text = merge(&kept, passed);
    artifact::write(&dir, &id, "evidence.md", &text)?;
    reviews.keep(&text)?;
    Ok(if passed { Outcome::Pass } else { Outcome::Fail })
}

/// The review that merges what each session found: a section for each session, then the verdict
/// line the CLI reads.
fn merge(sessions: &[(&Focus, String)], passed: bool) -> String {
    let mut text = String::new();
    for (focus, found) in sessions {
        text.push_str(&format!("{SECTION}{}\n\n{}\n\n", focus.name, found.trim()));
    }
    text.push_str(if passed { PASS_LINE } else { FAIL_LINE });
    text.push('\n');
    text
}

/// What the session of `focus` found in the kept review `text`, when its verdict was not a pass.
fn earlier_findings<'a>(text: &'a str, focus: &Focus) -> Option<&'a str> {
    let heading = format!("{SECTION}{}\n", focus.name);
    let from = if text.starts_with(&heading) {
        0
    } else {
        text.find(&format!("\n{heading}"))? + 1
    };
    let section = &text[from + heading.len()..];
    // The last section is followed by the review's own verdict line, which is not its.
    let section = match section.find(&format!("\n{SECTION}")) {
        Some(end) => &section[..end],
        None => section
            .trim_end()
            .rsplit_once('\n')
            .map_or("", |(own, _)| own),
    };
    matches!(
        verdict::parse(section),
        Some(Verdict::Fail | Verdict::Inconclusive)
    )
    .then(|| section.trim())
}

/// The one verdict a set of sessions comes to, when each gave `verdicts`: `None` when a session
/// gave none, ERROR when one said so, FAIL when any gave anything else but PASS, PASS otherwise.
pub fn verdicts_merge(verdicts: &[Option<Verdict>]) -> Option<Verdict> {
    let mut merged = Verdict::Pass;
    for verdict in verdicts {
        match verdict {
            None => return None,
            Some(Verdict::Pass) => {}
            Some(Verdict::Error) => merged = Verdict::Error,
            Some(_) if merged == Verdict::Pass => merged = Verdict::Fail,
            Some(_) => {}
        }
    }
    Some(merged)
}

/// The prompt that has the harness review a change with one session. `ns eval review` builds the
/// same one.
pub fn prompt(
    skill: &str,
    focus: &Focus,
    title: &str,
    body: &str,
    build: &str,
    diff: &str,
    previous: Option<&str>,
) -> String {
    let previous = previous
        .map(|text| format!("# Findings from the previous review\n\n{text}\n\n"))
        .unwrap_or_default();
    format!(
        "{skill}\n\n# Your focus\n\n{}: review only these areas: {}. Leave the others out.\n\n\
         # The unit\n\n## {title}\n\n{body}\n\n# The build summary\n\n{build}\n\n\
         {previous}# The change\n\n{diff}\n",
        focus.name,
        focus.areas.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(sessions: u32) -> Vec<&'static str> {
        focuses(sessions).iter().map(|f| f.name).collect()
    }

    #[test]
    fn one_session_has_every_area_three_have_groups_and_six_have_one_each() {
        assert_eq!(focuses(1)[0].areas, AREAS);
        assert_eq!(
            names(3),
            ["Behavior", "Requirements and tests", "Maintainability"]
        );
        assert_eq!(focuses(3)[0].areas, ["correctness", "security"]);
        assert_eq!(focuses(3)[1].areas, ["spec", "tests"]);
        assert_eq!(focuses(3)[2].areas, ["architecture", "docs and comments"]);
        assert_eq!(focuses(6).len(), 6);
        let each: Vec<_> = focuses(6).iter().flat_map(|f| f.areas.to_vec()).collect();
        assert_eq!(each, AREAS);
        assert!(focuses(6).iter().all(|f| f.areas.len() == 1));
    }

    #[test]
    fn the_prompt_names_the_focus_and_only_the_focus() {
        let got = prompt("SKILL", &GROUPS[0], "T", "B", "built", "diff", None);
        assert_eq!(
            got,
            "SKILL\n\n# Your focus\n\nBehavior: review only these areas: correctness, security. \
             Leave the others out.\n\n# The unit\n\n## T\n\nB\n\n# The build summary\n\nbuilt\n\n\
             # The change\n\ndiff\n"
        );
        let got = prompt(
            "S",
            &EACH[5],
            "T",
            "B",
            "b",
            "d",
            Some("old\nVERDICT: FAIL"),
        );
        assert!(got.contains("Docs and comments: review only these areas: docs and comments."));
        assert!(got
            .contains("# Findings from the previous review\n\nold\nVERDICT: FAIL\n\n# The change"));
    }

    fn found(text: &str) -> String {
        text.to_string()
    }

    #[test]
    fn the_kept_review_has_a_section_per_session_and_ends_in_the_verdict() {
        let a = (&GROUPS[0], found("all good\nVERDICT: PASS\n"));
        let b = (&GROUPS[1], found("bad\nVERDICT: FAIL\n"));
        assert_eq!(
            merge(&[a.clone(), b.clone()], false),
            "# Review session: Behavior\n\nall good\nVERDICT: PASS\n\n\
             # Review session: Requirements and tests\n\nbad\nVERDICT: FAIL\n\nVERDICT: FAIL\n"
        );
        assert_eq!(
            merge(&[a], true),
            "# Review session: Behavior\n\nall good\nVERDICT: PASS\n\nVERDICT: PASS\n"
        );
    }

    #[test]
    fn a_session_is_shown_only_its_own_unmet_findings() {
        let text = merge(
            &[
                (&GROUPS[0], found("hole\nVERDICT: FAIL\n")),
                (&GROUPS[1], found("fine\nVERDICT: PASS\n")),
                (&GROUPS[2], found("hm\nVERDICT: INCONCLUSIVE\n")),
            ],
            false,
        );
        assert_eq!(
            earlier_findings(&text, &GROUPS[0]),
            Some("hole\nVERDICT: FAIL")
        );
        assert_eq!(earlier_findings(&text, &GROUPS[1]), None);
        assert_eq!(
            earlier_findings(&text, &GROUPS[2]),
            Some("hm\nVERDICT: INCONCLUSIVE")
        );
        assert_eq!(earlier_findings(&text, &EACH[0]), None);
        assert_eq!(
            earlier_findings("no sections\nVERDICT: FAIL\n", &GROUPS[0]),
            None
        );
    }

    #[test]
    fn the_last_section_does_not_take_the_reviews_own_verdict() {
        let text = merge(
            &[
                (&GROUPS[0], found("hole\nVERDICT: FAIL\n")),
                (&GROUPS[1], found("fine\nVERDICT: PASS\n")),
            ],
            false,
        );
        assert_eq!(earlier_findings(&text, &GROUPS[1]), None);
    }

    #[test]
    fn any_session_that_is_not_a_pass_fails_the_review_and_one_without_a_verdict_leaves_none() {
        use Verdict::*;
        assert_eq!(verdicts_merge(&[Some(Pass), Some(Pass)]), Some(Pass));
        assert_eq!(verdicts_merge(&[Some(Pass), Some(Fail)]), Some(Fail));
        assert_eq!(
            verdicts_merge(&[Some(Inconclusive), Some(Pass)]),
            Some(Fail)
        );
        assert_eq!(verdicts_merge(&[Some(Fail), Some(Error)]), Some(Error));
        assert_eq!(verdicts_merge(&[Some(Error), Some(Fail)]), Some(Error));
        assert_eq!(verdicts_merge(&[Some(Fail), None]), None);
        assert_eq!(verdicts_merge(&[None]), None);
    }
}
