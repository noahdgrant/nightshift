//! Scorer definitions: `scorers/<name>.md` in the factory definition (docs/FACTORY.md, Scorers).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::frontmatter;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scorer {
    pub name: String,
    pub labels: Vec<Label>,
    pub passing_score: f64,
    pub sampling_rate: f64,
    /// An `ns ask` role: the model that applies the rubric.
    pub role: String,
    pub rubric: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Label {
    pub value: String,
    pub description: String,
    pub score: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScorer {
    #[serde(default)]
    labels: Vec<RawLabel>,
    passing_score: Option<f64>,
    sampling_rate: Option<f64>,
    role: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLabel {
    value: Option<String>,
    #[serde(default)]
    description: String,
    score: Option<f64>,
}

/// Parse `scorers/<name>.md`, or every problem in it.
pub fn parse(name: &str, text: &str) -> Result<Scorer, Vec<String>> {
    let at = format!("scorers/{name}.md");
    let Some((yaml, body)) = frontmatter::split(text) else {
        return Err(vec![format!(
            "{at}: missing frontmatter (---\\nlabels: ...\\n---)"
        )]);
    };
    let raw: RawScorer =
        serde_yaml::from_str(yaml).map_err(|e| vec![format!("{at}: bad frontmatter: {e}")])?;
    let mut out = Vec::new();
    if raw.labels.is_empty() {
        out.push(format!("{at}: no labels"));
    }
    let mut labels = Vec::new();
    for (i, l) in raw.labels.into_iter().enumerate() {
        match (l.value, l.score) {
            (Some(value), Some(score)) => labels.push(Label {
                value,
                description: l.description,
                score,
            }),
            (value, score) => {
                if value.is_none() {
                    out.push(format!("{at}: label {} has no value", i + 1));
                }
                if score.is_none() {
                    out.push(format!("{at}: label {} has no score", i + 1));
                }
            }
        }
    }
    let best = labels.iter().map(|l| l.score).reduce(f64::max);
    match (raw.passing_score, best) {
        (None, _) => out.push(format!("{at}: no passing_score")),
        (Some(p), Some(b)) if p > b => out.push(format!(
            "{at}: passing_score {p} is above every label's score (highest {b}), so nothing passes"
        )),
        _ => {}
    }
    match raw.sampling_rate {
        None => out.push(format!("{at}: no sampling_rate")),
        Some(r) if !(0.0..=1.0).contains(&r) => {
            out.push(format!("{at}: sampling_rate {r} is outside 0..1"))
        }
        _ => {}
    }
    let role = raw.role.unwrap_or_default();
    if role.trim().is_empty() {
        out.push(format!("{at}: no role (an `ns ask` role)"));
    }
    if body.trim().is_empty() {
        out.push(format!(
            "{at}: empty rubric; the body below the frontmatter is the rubric"
        ));
    }
    match (raw.passing_score, raw.sampling_rate) {
        (Some(passing_score), Some(sampling_rate)) if out.is_empty() => Ok(Scorer {
            name: name.to_string(),
            labels,
            passing_score,
            sampling_rate,
            role,
            rubric: body.to_string(),
        }),
        _ => Err(out),
    }
}

/// Every `scorers/<name>.md` under the root, and a problem for each that is invalid or names a
/// role the user config doesn't resolve.
pub fn load(root: &Path, cfg: Option<&config::Config>) -> (Vec<Scorer>, Vec<String>) {
    let mut scorers = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("scorers")) else {
        return (scorers, problems);
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                problems.push(format!("scorers/{name}.md: {e}"));
                continue;
            }
        };
        match parse(&name, &text) {
            Ok(s) if cfg.and_then(|c| c.resolve(&s.role)).is_none() => problems.push(format!(
                "scorers/{name}.md: role {:?} is not configured for `ns ask` in {}",
                s.role,
                config::path().display()
            )),
            Ok(s) => scorers.push(s),
            Err(p) => problems.extend(p),
        }
    }
    scorers.sort_by(|a, b| a.name.cmp(&b.name));
    problems.sort();
    (scorers, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "---
role: review
passing_score: 1
sampling_rate: 0.25
labels:
  - value: pass
    description: meets every criterion
    score: 1
  - value: fail
    score: 0
---
Grade the unit's evidence.md against its brief.
";

    fn problems(text: &str) -> Vec<String> {
        parse("x", text).unwrap_err()
    }

    fn has(text: &str, needle: &str) {
        let p = problems(text);
        assert!(
            p.iter().any(|s| s.contains(needle)),
            "{needle:?} not in {p:?}"
        );
    }

    #[test]
    fn a_valid_scorer_parses() {
        let s = parse("x", VALID).unwrap();
        assert_eq!(s.name, "x");
        assert_eq!(s.role, "review");
        assert_eq!(s.passing_score, 1.0);
        assert_eq!(s.sampling_rate, 0.25);
        assert_eq!(
            s.labels,
            [
                Label {
                    value: "pass".into(),
                    description: "meets every criterion".into(),
                    score: 1.0,
                },
                Label {
                    value: "fail".into(),
                    description: String::new(),
                    score: 0.0,
                },
            ]
        );
        assert_eq!(
            s.rubric,
            "Grade the unit's evidence.md against its brief.\n"
        );
    }

    #[test]
    fn missing_or_malformed_frontmatter() {
        assert_eq!(
            problems("Grade it.\n"),
            ["scorers/x.md: missing frontmatter (---\\nlabels: ...\\n---)"]
        );
        has(
            "---\nlabels: [\n---\nGrade it.\n",
            "scorers/x.md: bad frontmatter",
        );
        has("---\nrol: review\n---\nGrade it.\n", "unknown field `rol`");
        has(
            &VALID.replace("score: 0\n", "score: low\n"),
            "scorers/x.md: bad frontmatter",
        );
    }

    #[test]
    fn no_labels() {
        let text = "---\nrole: review\npassing_score: 1\nsampling_rate: 1\n---\nGrade it.\n";
        assert_eq!(problems(text), ["scorers/x.md: no labels"]);
        assert_eq!(
            problems(&text.replace("role", "labels: []\nrole")),
            ["scorers/x.md: no labels"]
        );
    }

    #[test]
    fn a_label_without_value_or_score() {
        assert_eq!(
            problems(&VALID.replace("  - value: fail\n", "  - description: x\n")),
            ["scorers/x.md: label 2 has no value"]
        );
        assert_eq!(
            problems(&VALID.replace("    score: 0\n", "")),
            ["scorers/x.md: label 2 has no score"]
        );
    }

    #[test]
    fn passing_score_no_label_can_meet() {
        assert_eq!(
            problems(&VALID.replace("passing_score: 1", "passing_score: 1.5")),
            ["scorers/x.md: passing_score 1.5 is above every label's score (highest 1), so nothing passes"]
        );
        assert_eq!(
            problems(&VALID.replace("passing_score: 1\n", "")),
            ["scorers/x.md: no passing_score"]
        );
    }

    #[test]
    fn sampling_rate_outside_zero_to_one() {
        for r in ["-0.1", "1.01", ".nan"] {
            has(
                &VALID.replace("sampling_rate: 0.25", &format!("sampling_rate: {r}")),
                "is outside 0..1",
            );
        }
        for r in ["0", "1"] {
            parse(
                "x",
                &VALID.replace("sampling_rate: 0.25", &format!("sampling_rate: {r}")),
            )
            .unwrap();
        }
        assert_eq!(
            problems(&VALID.replace("sampling_rate: 0.25\n", "")),
            ["scorers/x.md: no sampling_rate"]
        );
    }

    #[test]
    fn no_role() {
        for text in [
            VALID.replace("role: review\n", ""),
            VALID.replace("role: review", "role: ' '"),
        ] {
            assert_eq!(
                problems(&text),
                ["scorers/x.md: no role (an `ns ask` role)"]
            );
        }
    }

    #[test]
    fn an_empty_rubric() {
        let text = VALID.replace("Grade the unit's evidence.md against its brief.\n", " \n\n");
        assert_eq!(
            problems(&text),
            ["scorers/x.md: empty rubric; the body below the frontmatter is the rubric"]
        );
    }

    fn root(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("scorers")).unwrap();
        for (name, text) in files {
            fs::write(d.path().join("scorers").join(name), text).unwrap();
        }
        d
    }

    #[test]
    fn load_checks_the_role_against_the_user_config() {
        let d = root(&[
            ("a.md", VALID),
            ("b.md", &VALID.replace("role: review", "role: judge")),
            ("notes.txt", "not a scorer"),
        ]);
        let cfg = config::parse("[roles.review]\nharness = \"claude\"\n").unwrap();
        let (scorers, problems) = load(d.path(), Some(&cfg));
        assert_eq!(scorers.len(), 1);
        assert_eq!(scorers[0].name, "a");
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].starts_with("scorers/b.md: role \"judge\" is not configured for `ns ask`"),
            "{problems:?}"
        );
        let (scorers, problems) = load(d.path(), None);
        assert!(scorers.is_empty());
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn load_reports_every_invalid_file_and_tolerates_no_scorers_dir() {
        let d = root(&[("a.md", "no frontmatter\n"), ("b.md", "")]);
        fs::create_dir(d.path().join("scorers/c.md")).unwrap();
        let cfg = config::parse("[roles.default]\nharness = \"claude\"\n").unwrap();
        let (scorers, problems) = load(d.path(), Some(&cfg));
        assert!(scorers.is_empty());
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].starts_with("scorers/a.md"));
        assert!(problems[1].starts_with("scorers/b.md"));
        assert!(problems[2].starts_with("scorers/c.md"));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(load(empty.path(), Some(&cfg)), (vec![], vec![]));
    }
}
