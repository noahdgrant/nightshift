//! The rubrics a retrospective grades a unit by. A rubric is a markdown file with the front matter
//! `name` and `pass` (a number from 0 to 1) and a body that says what to judge and gives a scale of
//! labelled values from 0 to 1.
//!
//! The default rubrics are kept in the repository's `rubrics` directory and built into the binary.

use anyhow::{bail, Context, Result};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/rubrics.rs"));
}

#[derive(Debug, PartialEq)]
pub struct Rubric {
    pub name: String,
    /// The lowest score that passes.
    pub pass: f64,
    /// What to judge and the scale, as the file says it.
    pub body: String,
}

/// The default rubrics that came with this `ns`.
pub fn defaults() -> Result<Vec<Rubric>> {
    embedded::EMBEDDED.iter().map(|text| parse(text)).collect()
}

/// A rubric from the text of its file.
pub fn parse(text: &str) -> Result<Rubric> {
    let (front, body) = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .context("a rubric starts with front matter between `---` lines")?;
    let (mut name, mut pass) = (None, None);
    for line in front.lines() {
        match line.split_once(':') {
            Some(("name", v)) => name = Some(v.trim().to_string()),
            Some(("pass", v)) => pass = Some(v.trim().to_string()),
            _ => {}
        }
    }
    let name = name
        .filter(|n| !n.is_empty())
        .context("a rubric has no `name`")?;
    let pass = pass
        .with_context(|| format!("rubric {name} has no `pass`"))?
        .parse::<f64>()
        .ok()
        .filter(|p| (0.0..=1.0).contains(p))
        .with_context(|| format!("`pass` of rubric {name} is not a number from 0 to 1"))?;
    if body.trim().is_empty() {
        bail!("rubric {name} has no body");
    }
    Ok(Rubric {
        name,
        pass,
        body: body.trim().to_string(),
    })
}

/// One rubric's result for one unit, read from a retrospective's reply.
#[derive(Debug, PartialEq)]
pub struct Grade {
    pub rubric: String,
    pub score: f64,
    pub reason: String,
}

/// The grade of each rubric in `reply`, from its `GRADE: <rubric> <score> <reason>` lines (the
/// last line for a rubric counts). `None` when a rubric has no valid line: the score is a number
/// from 0 to 1 and the reason is not empty.
pub fn grades_parse(reply: &str, rubrics: &[Rubric]) -> Option<Vec<Grade>> {
    rubrics
        .iter()
        .map(|rubric| {
            reply.lines().rev().find_map(|line| {
                let rest = line.trim().strip_prefix("GRADE: ")?;
                let mut words = rest.splitn(3, ' ');
                if words.next()? != rubric.name {
                    return None;
                }
                let score: f64 = words.next()?.parse().ok()?;
                let reason = words.next()?.trim();
                ((0.0..=1.0).contains(&score) && !reason.is_empty()).then(|| Grade {
                    rubric: rubric.name.clone(),
                    score,
                    reason: reason.to_string(),
                })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn efficiency() -> Rubric {
        Rubric {
            name: "efficiency".into(),
            pass: 0.6,
            body: "judge".into(),
        }
    }

    #[test]
    fn a_rubric_is_front_matter_then_a_body() {
        assert_eq!(
            parse("---\nname: x\npass: 0.5\n---\n\nJudge.\n\n- 1.0: good\n").unwrap(),
            Rubric {
                name: "x".into(),
                pass: 0.5,
                body: "Judge.\n\n- 1.0: good".into()
            }
        );
    }

    #[test]
    fn a_rubric_that_is_missing_or_wrong_in_front_matter_is_an_error() {
        for text in [
            "no front matter",
            "---\npass: 0.5\n---\nbody\n",
            "---\nname: x\n---\nbody\n",
            "---\nname: x\npass: 1.5\n---\nbody\n",
            "---\nname: x\npass: high\n---\nbody\n",
            "---\nname: x\npass: 0.5\n---\n\n",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn the_default_rubrics_are_efficiency_and_it_passes_at_six_tenths() {
        let rubrics = defaults().unwrap();
        assert_eq!(rubrics.len(), 1);
        assert_eq!(rubrics[0].name, "efficiency");
        assert_eq!(rubrics[0].pass, 0.6);
        assert!(rubrics[0].body.contains("## Workarounds"));
    }

    #[test]
    fn a_grade_line_gives_the_score_and_the_reason() {
        let reply = "Looked at it.\nGRADE: efficiency 0.75 one failed call, then straight on\n";
        assert_eq!(
            grades_parse(reply, &[efficiency()]),
            Some(vec![Grade {
                rubric: "efficiency".into(),
                score: 0.75,
                reason: "one failed call, then straight on".into()
            }])
        );
    }

    #[test]
    fn the_last_line_for_a_rubric_counts() {
        let reply = "GRADE: efficiency 0.1 first\nGRADE: efficiency 0.9 second";
        assert_eq!(grades_parse(reply, &[efficiency()]).unwrap()[0].score, 0.9);
    }

    #[test]
    fn a_missing_out_of_range_or_reasonless_grade_is_no_grade() {
        for reply in [
            "no line",
            "GRADE: efficiency 1.5 too high",
            "GRADE: efficiency -0.1 too low",
            "GRADE: efficiency lots many",
            "GRADE: efficiency 0.5",
            "GRADE: other 0.5 wrong rubric",
        ] {
            assert_eq!(grades_parse(reply, &[efficiency()]), None, "{reply}");
        }
    }
}
