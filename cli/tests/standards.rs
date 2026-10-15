//! `docs/CODING_STANDARDS.md` and the code that cites it: each citation names one of its headings.

use std::fs;
use std::path::{Path, PathBuf};

fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The files under `dir`, skipping build output.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_dir() {
            out.push(path);
        } else if !path.ends_with("target") {
            files(&path, out);
        }
    }
}

/// Each citation of the standards in `text`: the heading a `CODING_STANDARDS.md, "<heading>"`
/// names, or `None` for a `CODING_STANDARDS.md)` that names none. Other mentions of the file
/// are not citations.
fn citations(text: &str) -> Vec<Option<String>> {
    text.match_indices("CODING_STANDARDS.md")
        .filter_map(|(at, m)| {
            let rest = text[at + m.len()..].trim_start_matches('`');
            if rest.starts_with(')') {
                return Some(None);
            }
            let quoted = rest
                .strip_prefix(", \"")
                .or_else(|| rest.strip_prefix(", \\\""))?;
            Some(Some(quoted[..quoted.find(['"', '\\']).unwrap()].to_owned()))
        })
        .collect()
}

/// The citations in `text` that name no `## ` heading of `standards`.
fn unresolved(standards: &str, text: &str) -> Vec<Option<String>> {
    citations(text)
        .into_iter()
        .filter(|cited| {
            !cited
                .as_ref()
                .is_some_and(|h| standards.lines().any(|l| l.strip_prefix("## ") == Some(h)))
        })
        .collect()
}

/// Each root holds citations, so a change to how they are written can't leave the check with
/// nothing to check.
#[test]
fn every_citation_of_the_coding_standards_names_one_of_its_headings() {
    let standards = fs::read_to_string(checkout().join("docs/CODING_STANDARDS.md")).unwrap();
    let mut bad = Vec::new();
    for root in ["cli", "rules/rust"] {
        let mut paths = Vec::new();
        files(&checkout().join(root), &mut paths);
        paths.retain(|p| !p.ends_with(file!()));
        let texts: Vec<(&PathBuf, String)> = paths
            .iter()
            .map(|p| {
                (
                    p,
                    String::from_utf8_lossy(&fs::read(p).unwrap()).into_owned(),
                )
            })
            .collect();
        assert!(
            texts.iter().any(|(_, t)| !citations(t).is_empty()),
            "no citation under {root}"
        );
        for (p, text) in &texts {
            for cited in unresolved(&standards, text) {
                bad.push(format!("{}: {cited:?}", p.display()));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_citation_resolves_only_to_a_heading_of_the_standards() {
    let standards = "# Coding standards\n\n## Checks\n\nChecks the rules.\n";
    let text = r#"(CODING_STANDARDS.md, "Checks") (CODING_STANDARDS.md, "Coding standards")
        (CODING_STANDARDS.md, "Checks the rules.") (CODING_STANDARDS.md)"#;
    assert_eq!(
        unresolved(standards, text),
        [
            Some("Coding standards".to_owned()),
            Some("Checks the rules.".to_owned()),
            None
        ]
    );
}

#[test]
fn a_citation_reads_its_heading_and_a_bare_mention_reads_none() {
    assert_eq!(
        citations(r#"(CODING_STANDARDS.md, "Checks") and (CODING_STANDARDS.md)"#),
        [Some("Checks".to_owned()), None]
    );
    assert_eq!(
        citations(r#"message: "x (CODING_STANDARDS.md, \"A test\")""#),
        [Some("A test".to_owned())]
    );
    assert_eq!(
        citations(r#"enforces `CODING_STANDARDS.md`, "Checks"."#),
        [Some("Checks".to_owned())]
    );
    assert_eq!(
        citations(r#".contains("CODING_STANDARDS.md") and CODING_STANDARDS.md --test"#),
        []
    );
}
