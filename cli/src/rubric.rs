//! The rubrics a retrospective grades a unit by. A rubric is a markdown file with the front matter
//! `name` and `pass` (a number from 0 to 1) and a body that says what to judge and gives a scale of
//! labelled values from 0 to 1.
//!
//! The default rubrics are kept in the repository's `rubrics` directory and built into the binary.
//! A repository adds its own as `docs/agents/rubrics/<name>.md`; one named like a default replaces it.

use std::path::Path;

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

/// Where a repository keeps its own rubrics, relative to its root.
pub const REPO_DIR: &str = "docs/agents/rubrics";

/// The rubrics in force for the repository at `dir`: the defaults, with each valid file in
/// `docs/agents/rubrics` replacing the default of its name or adding one. The second value has one
/// line for each file that is not valid, naming the file and what is wrong; that file is left out.
pub fn in_force(dir: &Path) -> Result<(Vec<Rubric>, Vec<String>)> {
    let mut rubrics = defaults()?;
    let (own, problems) = repo_read(dir);
    for rubric in own {
        match rubrics.iter_mut().find(|r| r.name == rubric.name) {
            Some(slot) => *slot = rubric,
            None => rubrics.push(rubric),
        }
    }
    Ok((rubrics, problems))
}

/// The valid rubric files in `dir`'s `docs/agents/rubrics`, and a line for each invalid one. A
/// file is valid when it parses and its `name` is its file name without `.md`.
pub fn repo_read(dir: &Path) -> (Vec<Rubric>, Vec<String>) {
    let folder = dir.join(REPO_DIR);
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    let (mut rubrics, mut problems) = (Vec::new(), Vec::new());
    for file in files {
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let shown = format!("{REPO_DIR}/{stem}.md");
        let parsed = std::fs::read_to_string(&file)
            .map_err(anyhow::Error::from)
            .and_then(|text| parse(&text))
            .and_then(|r| {
                if r.name == stem {
                    Ok(r)
                } else {
                    bail!("its `name` is {}, not {stem}", r.name)
                }
            });
        match parsed {
            Ok(r) => rubrics.push(r),
            Err(e) => problems.push(format!("{shown}: {e:#}")),
        }
    }
    (rubrics, problems)
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

/// The grades in `reply` for the rubrics that have one, from its `GRADE: <rubric> <score> <reason>`
/// lines (the last line for a rubric counts), and the names of the rubrics that have none. A line
/// is valid when its score is a number from 0 to 1 and its reason is not empty.
pub fn grades_parse(reply: &str, rubrics: &[Rubric]) -> (Vec<Grade>, Vec<String>) {
    let (mut grades, mut missing) = (Vec::new(), Vec::new());
    for rubric in rubrics {
        let found = reply.lines().rev().find_map(|line| {
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
        });
        match found {
            Some(g) => grades.push(g),
            None => missing.push(rubric.name.clone()),
        }
    }
    (grades, missing)
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
    fn the_default_rubrics_are_the_four_built_in_and_efficiency_passes_at_six_tenths() {
        let rubrics = defaults().unwrap();
        let names: Vec<_> = rubrics.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "efficiency",
                "followed-the-brief",
                "redundant-tests",
                "verbosity"
            ]
        );
        assert_eq!(rubrics[0].pass, 0.6);
        assert!(rubrics[0].body.contains("## Workarounds"));
    }

    fn repo_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(REPO_DIR)).unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(REPO_DIR).join(name), text).unwrap();
        }
        dir
    }

    #[test]
    fn a_repository_rubric_adds_one_and_one_named_like_a_default_replaces_it() {
        let dir = repo_with(&[
            ("mine.md", "---\nname: mine\npass: 0.3\n---\nMine.\n"),
            (
                "verbosity.md",
                "---\nname: verbosity\npass: 0.9\n---\nOwn.\n",
            ),
        ]);
        let (rubrics, problems) = in_force(dir.path()).unwrap();
        assert!(problems.is_empty());
        let names: Vec<_> = rubrics.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "efficiency",
                "followed-the-brief",
                "redundant-tests",
                "verbosity",
                "mine"
            ]
        );
        assert_eq!(rubrics[3].body, "Own.");
        assert_eq!(rubrics[3].pass, 0.9);
    }

    #[test]
    fn an_invalid_repository_rubric_is_left_out_and_its_file_is_named() {
        let dir = repo_with(&[
            ("a.md", "---\nname: b\npass: 0.3\n---\nBody.\n"),
            ("c.md", "---\nname: c\npass: 2\n---\nBody.\n"),
            ("d.md", "---\nname: d\npass: 0.5\n---\n"),
            ("ok.md", "---\nname: ok\npass: 0.5\n---\nBody.\n"),
            ("notes.txt", "ignored"),
        ]);
        let (rubrics, problems) = in_force(dir.path()).unwrap();
        assert_eq!(rubrics.len(), 5);
        assert_eq!(
            problems,
            [
                "docs/agents/rubrics/a.md: its `name` is b, not a",
                "docs/agents/rubrics/c.md: `pass` of rubric c is not a number from 0 to 1",
                "docs/agents/rubrics/d.md: rubric d has no body",
            ]
        );
    }

    #[test]
    fn a_repository_without_the_directory_has_only_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (rubrics, problems) = in_force(dir.path()).unwrap();
        assert_eq!(rubrics.len(), 4);
        assert!(problems.is_empty());
    }

    #[test]
    fn a_grade_line_gives_the_score_and_the_reason() {
        let reply = "Looked at it.\nGRADE: efficiency 0.75 one failed call, then straight on\n";
        assert_eq!(
            grades_parse(reply, &[efficiency()]),
            (
                vec![Grade {
                    rubric: "efficiency".into(),
                    score: 0.75,
                    reason: "one failed call, then straight on".into()
                }],
                vec![]
            )
        );
    }

    #[test]
    fn the_last_line_for_a_rubric_counts() {
        let reply = "GRADE: efficiency 0.1 first\nGRADE: efficiency 0.9 second";
        assert_eq!(grades_parse(reply, &[efficiency()]).0[0].score, 0.9);
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
            assert_eq!(
                grades_parse(reply, &[efficiency()]),
                (vec![], vec!["efficiency".to_string()]),
                "{reply}"
            );
        }
    }
}
