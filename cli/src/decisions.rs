//! `ns lint`'s decision checks: every decision ID cited in the repo has a file in
//! `docs/decisions/`, no two files share an ID, and every `amends:` and
//! `amended_by:` target exists.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_yaml::Value;

use crate::frontmatter;
use crate::lint::LintError;

/// Where citations are looked for, relative to the repo root.
const CITED_IN: &[&str] = &["skills", "cli/src", "docs", "AGENTS.md"];

/// A decision ID as a whole token: `D<n>` or `ADR-<issue>` with an optional
/// letter. IDs never start with 0, so `D0` (a logic analyzer channel) and
/// `ADR-0001` (another repo's ADR) are not citations.
const ID: &str = r"\b(D[1-9][0-9]*|ADR-[1-9][0-9]*[a-z]?)\b";

fn error(file: &Path, message: String) -> LintError {
    LintError {
        skill: "decisions".into(),
        file: file.to_string_lossy().into_owned(),
        message,
    }
}

fn files(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(path) else {
        out.push(path.to_path_buf());
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            files(&e.path(), out);
        } else {
            out.push(e.path());
        }
    }
}

fn targets(fm: &Value, key: &str) -> Vec<String> {
    match fm.get(key) {
        Some(Value::Sequence(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// Checks `<repo>/docs/decisions/*.md` and the citations of them.
pub fn check(repo: &Path) -> Vec<LintError> {
    let dir = repo.join("docs/decisions");
    let mut decisions: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    decisions.sort();

    let mut errors = Vec::new();
    let mut owner: HashMap<String, PathBuf> = HashMap::new();
    let mut amendments = Vec::new();
    for path in &decisions {
        let fm = fs::read_to_string(path)
            .ok()
            .and_then(|t| frontmatter::parse(&t).ok().flatten());
        let Some((fm, id)) = fm
            .as_ref()
            .and_then(|fm| Some((fm, fm.get("id")?.as_str()?)))
        else {
            errors.push(error(path, "frontmatter has no id".into()));
            continue;
        };
        if let Some(first) = owner.get(id) {
            let name = first.file_name().unwrap_or_default().to_string_lossy();
            errors.push(error(path, format!("id {id} is also used by {name}")));
        } else {
            owner.insert(id.to_string(), path.clone());
        }
        for key in ["amends", "amended_by"] {
            for target in targets(fm, key) {
                amendments.push((path, key, target));
            }
        }
    }
    let ids: HashSet<&str> = owner.keys().map(String::as_str).collect();
    for (path, key, target) in amendments {
        if !ids.contains(target.as_str()) {
            errors.push(error(
                path,
                format!("{key}: {target} has no file in docs/decisions/"),
            ));
        }
    }

    let re = Regex::new(ID).unwrap();
    let mut cited = Vec::new();
    for place in CITED_IN {
        files(&repo.join(place), &mut cited);
    }
    for path in cited.iter().filter(|p| !p.starts_with(&dir)) {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            for m in re.find_iter(line) {
                if !ids.contains(m.as_str()) {
                    errors.push(error(
                        path,
                        format!(
                            "line {}: cites {}, which has no file in docs/decisions/",
                            n + 1,
                            m.as_str()
                        ),
                    ));
                }
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // Built with concat! so this file cites no decision that doesn't exist.
    const MISSING_D: &str = concat!("D", "99");
    const MISSING_ADR: &str = concat!("ADR-", "3871");
    const SUFFIXED: &str = concat!("ADR-", "387a");

    fn decision(repo: &Path, file: &str, frontmatter: &str) {
        let dir = repo.join("docs/decisions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), format!("---\n{frontmatter}\n---\n\nbody\n")).unwrap();
    }

    fn write(repo: &Path, rel: &str, body: &str) {
        let p = repo.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        decision(tmp.path(), "D1-one.md", "id: D1\ntitle: \"One\"");
        decision(
            tmp.path(),
            "387-watch.md",
            "id: ADR-387\ntitle: \"Watch\"\namends: [D1]",
        );
        tmp
    }

    fn messages(repo: &Path) -> Vec<String> {
        check(repo).into_iter().map(|e| e.message).collect()
    }

    #[test]
    fn valid_citations_pass() {
        let tmp = repo();
        write(tmp.path(), "AGENTS.md", "See D1 and ADR-387.\n");
        write(tmp.path(), "skills/ns-x/SKILL.md", "(D1, ADR-387)\n");
        write(tmp.path(), "cli/src/x.rs", "// D1-one\n");
        write(tmp.path(), "docs/DESIGN.md", "ADR-387: x\n");
        assert_eq!(messages(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn a_missing_citation_fails_in_each_scanned_place() {
        for rel in [
            "AGENTS.md",
            "skills/ns-x/SKILL.md",
            "cli/src/x.rs",
            "docs/x.md",
        ] {
            let tmp = repo();
            write(tmp.path(), rel, &format!("ok D1\nsee {MISSING_D}.\n"));
            let errors = check(tmp.path());
            assert_eq!(errors.len(), 1, "{rel}: {errors:?}");
            assert!(errors[0].file.ends_with(rel), "{errors:?}");
            assert_eq!(
                errors[0].message,
                format!("line 2: cites {MISSING_D}, which has no file in docs/decisions/")
            );
        }
    }

    #[test]
    fn citations_match_whole_tokens_only() {
        let tmp = repo();
        write(tmp.path(), "docs/x.md", &format!("D10 {MISSING_ADR}\n"));
        assert_eq!(
            messages(tmp.path()),
            [
                "line 1: cites D10, which has no file in docs/decisions/".to_string(),
                format!("line 1: cites {MISSING_ADR}, which has no file in docs/decisions/"),
            ]
        );
    }

    #[test]
    fn ids_with_a_leading_zero_or_inside_a_word_are_not_citations() {
        let tmp = repo();
        write(
            tmp.path(),
            "docs/x.md",
            "-C D0,D1 ADR-0001 D01 UD5 D5x D5_ ADR-5ab 0xD7\n",
        );
        assert_eq!(messages(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn a_suffixed_adr_needs_its_own_file() {
        let tmp = repo();
        write(tmp.path(), "docs/x.md", &format!("{SUFFIXED}\n"));
        assert_eq!(
            messages(tmp.path()),
            [format!(
                "line 1: cites {SUFFIXED}, which has no file in docs/decisions/"
            )]
        );
        decision(tmp.path(), "387-second.md", &format!("id: {SUFFIXED}"));
        assert_eq!(messages(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn decision_files_are_not_scanned_for_citations() {
        let tmp = repo();
        write(
            tmp.path(),
            "docs/decisions/D1-one.md",
            &format!("---\nid: D1\n---\n\nUnlike {MISSING_D}.\n"),
        );
        assert_eq!(messages(tmp.path()), Vec::<String>::new());
    }

    #[test]
    fn a_duplicate_id_fails() {
        let tmp = repo();
        decision(tmp.path(), "D1-copy.md", "id: D1\ntitle: \"Copy\"");
        let errors = check(tmp.path());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].file.ends_with("D1-one.md"), "{errors:?}");
        assert_eq!(errors[0].message, "id D1 is also used by D1-copy.md");
    }

    #[test]
    fn a_bad_amends_or_amended_by_target_fails() {
        let tmp = repo();
        decision(
            tmp.path(),
            "D2-two.md",
            &format!("id: D2\namends: [D1, {MISSING_D}]\namended_by: [{MISSING_ADR}]"),
        );
        assert_eq!(
            messages(tmp.path()),
            [
                format!("amends: {MISSING_D} has no file in docs/decisions/"),
                format!("amended_by: {MISSING_ADR} has no file in docs/decisions/"),
            ]
        );
    }

    #[test]
    fn a_single_amends_target_is_checked_too() {
        let tmp = repo();
        decision(
            tmp.path(),
            "D2-two.md",
            &format!("id: D2\namends: {MISSING_D}"),
        );
        assert_eq!(
            messages(tmp.path()),
            [format!(
                "amends: {MISSING_D} has no file in docs/decisions/"
            )]
        );
    }

    #[test]
    fn unreadable_files_are_skipped() {
        let tmp = repo();
        write(tmp.path(), "docs/x.md", &format!("{MISSING_D}\n"));
        fs::write(tmp.path().join("docs/blob.bin"), [0xff, 0xfe, b'D', b'9']).unwrap();
        assert_eq!(
            messages(tmp.path()),
            [format!(
                "line 1: cites {MISSING_D}, which has no file in docs/decisions/"
            )]
        );
    }

    #[test]
    fn a_decision_without_an_id_fails() {
        let tmp = repo();
        decision(tmp.path(), "D2-two.md", "title: \"Two\"");
        write(tmp.path(), "docs/decisions/D3-three.md", "no frontmatter\n");
        decision(tmp.path(), "D4-four.md", "id: [unclosed");
        assert_eq!(
            messages(tmp.path()),
            [
                "frontmatter has no id",
                "frontmatter has no id",
                "frontmatter has no id",
            ]
        );
    }

    #[test]
    fn non_markdown_files_in_decisions_are_ignored() {
        let tmp = repo();
        write(tmp.path(), "docs/decisions/.keep", "");
        assert_eq!(messages(tmp.path()), Vec::<String>::new());
    }
}
