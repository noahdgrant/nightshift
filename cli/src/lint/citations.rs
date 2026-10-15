//! Each citation of a section of the coding standards or the quality bar names one of its `## `
//! headings: `CODING_STANDARDS.md, "<heading>"`. A citation of the standards that closes its
//! parenthesis right after the file name names no section, and fails too.

use std::fs;
use std::path::{Path, PathBuf};

use super::LintError;

/// The cited docs: the name a citation uses, the doc's path in the repo, and whether a mention
/// closed by `)` is a citation with no section. The quality bar's is a plain path: the rule
/// packs' messages end `(quality bar, <section>: <path>)`.
const DOCS: &[(&str, &str, bool)] = &[
    ("CODING_STANDARDS.md", "docs/CODING_STANDARDS.md", true),
    (
        "quality-bar.md",
        "skills/ns-contract/references/quality-bar.md",
        false,
    ),
];

/// The repo dirs whose files may cite them.
const ROOTS: &[&str] = &["cli", "docs", "rules", "skills"];

/// The citation checks for the repo at `repo`, for each of `DOCS` it holds.
pub(super) fn check_citations(repo: &Path) -> Vec<LintError> {
    let mut files = Vec::new();
    for root in ROOTS {
        all_files(&repo.join(root), &mut files);
    }
    let texts: Vec<(PathBuf, String)> = files
        .into_iter()
        .filter_map(|p| {
            let text = String::from_utf8_lossy(&fs::read(&p).ok()?).into_owned();
            Some((p, text))
        })
        .collect();
    let mut errors = Vec::new();
    for &(name, path, bare) in DOCS {
        let Ok(doc) = fs::read_to_string(repo.join(path)) else {
            continue;
        };
        if !texts
            .iter()
            .any(|(_, text)| !citations(name, text).is_empty())
        {
            errors.push(LintError {
                skill: "citations".into(),
                file: path.into(),
                message: format!(
                    "no file under {} cites {name}; the citation scan reads nothing",
                    ROOTS.join(", ")
                ),
            });
        }
        for (file, text) in &texts {
            for (line, cited) in unresolved(name, &doc, text) {
                if cited.is_none() && !bare {
                    continue;
                }
                let message = match cited {
                    Some(h) => format!("line {line}: {name}, {h:?} names no `## ` heading of {path}"),
                    None => format!("line {line}: a citation of {name} names no section; cite it as {name}, \"<heading>\""),
                };
                errors.push(LintError {
                    skill: "citations".into(),
                    file: file.to_string_lossy().into_owned(),
                    message,
                });
            }
        }
    }
    errors
}

/// The files under `dir`, skipping build output and installed packages.
fn all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if !p.is_dir() {
            out.push(p);
        } else if !p.ends_with("target") && !p.ends_with("node_modules") {
            all_files(&p, out);
        }
    }
}

/// Each citation of `name` in `text`, with its 1-based line: the heading a `<name>, "<heading>"`
/// names, or `None` for a `<name>)` that names none. The rule packs' `quality bar, <heading>:
/// <path to name>` also names a heading. Other mentions of the file are not citations, and
/// neither is a `<placeholder>` heading.
fn citations(name: &str, text: &str) -> Vec<(usize, Option<String>)> {
    let mut found = quality_bar_messages(name, text);
    found.extend(quoted_citations(name, text));
    found.sort_by_key(|(line, _)| *line);
    found
}

/// The headings of `quality bar, <heading>: <path>` messages, where the path ends in `name`.
fn quality_bar_messages(name: &str, text: &str) -> Vec<(usize, Option<String>)> {
    text.match_indices("quality bar, ")
        .filter_map(|(at, m)| {
            let rest = &text[at + m.len()..];
            let (heading, after) = rest.split_once(": ")?;
            let path = after.split([')', ' ', '\n', '"']).next()?;
            (path.ends_with(name) && !heading.contains('\n')).then(|| {
                (
                    text[..at].matches('\n').count() + 1,
                    Some(heading.to_owned()),
                )
            })
        })
        .collect()
}

fn quoted_citations(name: &str, text: &str) -> Vec<(usize, Option<String>)> {
    text.match_indices(name)
        .filter_map(|(at, m)| {
            let line = text[..at].matches('\n').count() + 1;
            let rest = text[at + m.len()..].trim_start_matches('`');
            if rest.starts_with(')') {
                return Some((line, None));
            }
            let quoted = rest
                .strip_prefix(", \"")
                .or_else(|| rest.strip_prefix(", \\\""))?;
            let heading = &quoted[..quoted.find(['"', '\\'])?];
            (!heading.starts_with('<')).then(|| (line, Some(heading.to_owned())))
        })
        .collect()
}

/// The citations of `name` in `text` that name no `## ` heading of `doc`.
fn unresolved(name: &str, doc: &str, text: &str) -> Vec<(usize, Option<String>)> {
    citations(name, text)
        .into_iter()
        .filter(|(_, cited)| {
            !cited
                .as_ref()
                .is_some_and(|h| doc.lines().any(|l| l.strip_prefix("## ") == Some(h)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDARDS: &str = "CODING_STANDARDS.md";
    const BAR: &str = "quality-bar.md";

    fn some(line: usize, h: &str) -> (usize, Option<String>) {
        (line, Some(h.to_owned()))
    }

    #[test]
    fn a_citation_resolves_only_to_a_heading_of_the_doc() {
        let doc = "# Coding standards\n\n## Checks\n\nChecks the rules.\n";
        let text = format!(
            "({STANDARDS}, \"Checks\") ({STANDARDS}, \"Coding standards\")\n({STANDARDS}, \"Checks the rules.\") ({STANDARDS})"
        );
        assert_eq!(
            unresolved(STANDARDS, doc, &text),
            [
                some(1, "Coding standards"),
                some(2, "Checks the rules."),
                (2, None)
            ]
        );
    }

    #[test]
    fn a_citation_reads_its_heading_and_a_bare_mention_reads_none() {
        let s = STANDARDS;
        assert_eq!(
            citations(s, &format!("({s}, \"Checks\") and ({s})")),
            [some(1, "Checks"), (1, None)]
        );
        assert_eq!(
            citations(s, &format!("message: \"x ({s}, \\\"A test\\\")\"")),
            [some(1, "A test")]
        );
        assert_eq!(
            citations(s, &format!("enforces `{s}`, \"Checks\".")),
            [some(1, "Checks")]
        );
        assert_eq!(
            citations(
                s,
                &format!(".contains(\"{s}\") and {s} --test, `{s}, \"<heading>\"`")
            ),
            []
        );
    }

    /// A repo with both docs, each with one `## Checks`/`## Size` heading.
    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (dir, file, body) in [
            ("docs", STANDARDS, "# Standards\n\n## Checks\n"),
            (
                "skills/ns-contract/references",
                BAR,
                "# Quality bar\n\n## Size\n",
            ),
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
            fs::write(root.join(dir).join(file), body).unwrap();
        }
        tmp
    }

    fn messages(repo: &Path) -> Vec<String> {
        check_citations(repo)
            .into_iter()
            .map(|e| {
                let file = e
                    .file
                    .strip_prefix(&*repo.to_string_lossy())
                    .unwrap_or(&e.file)
                    .to_owned();
                format!("{file}: {}", e.message)
            })
            .collect()
    }

    #[test]
    fn a_broken_citation_in_cli_skills_docs_or_rules_fails_ns_lint() {
        let tmp = repo();
        let root = tmp.path();
        let good = format!("({STANDARDS}, \"Checks\") ({BAR}, \"Size\") (see {BAR})\n");
        for (dir, file) in [
            ("cli/src", "a.rs"),
            ("skills/ns-x", "SKILL.md"),
            ("docs", "b.md"),
            ("rules/rust", "c.yml"),
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
            fs::write(root.join(dir).join(file), &good).unwrap();
        }
        assert_eq!(messages(root), Vec::<String>::new());

        fs::write(
            root.join("skills/ns-x/SKILL.md"),
            format!("x\n({BAR}, \"Sizes\")\n"),
        )
        .unwrap();
        fs::write(root.join("cli/src/a.rs"), format!("// ({STANDARDS})\n")).unwrap();
        fs::write(root.join("docs/b.md"), format!("({STANDARDS}, \"Gone\")\n")).unwrap();
        fs::write(
            root.join("rules/rust/c.yml"),
            format!("({BAR}, \"Tests\")\n"),
        )
        .unwrap();
        let (s, b) = (STANDARDS, BAR);
        let bar = format!("skills/ns-contract/references/{b}");
        assert_eq!(
            messages(root),
            [
                format!("/cli/src/a.rs: line 1: a citation of {s} names no section; cite it as {s}, \"<heading>\""),
                format!("/docs/b.md: line 1: {s}, \"Gone\" names no `## ` heading of docs/{s}"),
                format!("/rules/rust/c.yml: line 1: {b}, \"Tests\" names no `## ` heading of {bar}"),
                format!("/skills/ns-x/SKILL.md: line 2: {b}, \"Sizes\" names no `## ` heading of {bar}"),
            ]
        );
    }

    #[test]
    fn the_quality_bar_message_form_is_a_citation() {
        let msg = format!("x (quality bar, Size: skills/ns-contract/references/{BAR})");
        assert_eq!(citations(BAR, &msg), [some(1, "Size"), (1, None)]);
        assert_eq!(citations(BAR, "quality bar, Size: other.md"), []);
        assert_eq!(
            unresolved(BAR, "## Size\n", &msg),
            [(1, None)],
            "only the bare mention of the path is left, and the bar ignores it"
        );
        assert_eq!(
            unresolved(BAR, "## Other\n", &msg),
            [some(1, "Size"), (1, None)]
        );
    }

    #[test]
    fn a_doc_nothing_cites_fails_instead_of_passing_on_nothing() {
        let tmp = repo();
        assert_eq!(messages(tmp.path()).len(), 2);
    }

    #[test]
    fn the_real_checkout_has_citations_and_no_broken_one() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert_eq!(messages(&repo), Vec::<String>::new());
    }

    #[test]
    fn build_output_and_a_repo_without_the_doc_are_not_checked() {
        let tmp = repo();
        let root = tmp.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(
            root.join("docs/ok.md"),
            format!("({STANDARDS}, \"Checks\") ({BAR}, \"Size\")\n"),
        )
        .unwrap();
        for dir in ["cli/target", "rules/js/node_modules"] {
            fs::create_dir_all(root.join(dir)).unwrap();
            fs::write(root.join(dir).join("x"), format!("({STANDARDS}, \"No\")\n")).unwrap();
        }
        assert_eq!(messages(root), Vec::<String>::new());

        fs::write(root.join("docs/c.md"), format!("({STANDARDS}, \"No\")\n")).unwrap();
        fs::remove_file(root.join("docs").join(STANDARDS)).unwrap();
        assert_eq!(messages(root), Vec::<String>::new());
    }
}
