//! `ns lint [path]`: structural checks on the skills under a directory, and the
//! decision checks when its parent has `docs/decisions/`.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use serde_yaml::Value;

use crate::decisions;
use crate::error::SfError;
use crate::frontmatter;

pub const MAX_DESCRIPTION: usize = 1024;

const USER_INVOKED_ONLY: &[&str] = &["ns-auto"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LintError {
    pub skill: String,
    pub file: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub ok: bool,
    pub skills: usize,
    pub errors: Vec<LintError>,
}

/// One parsed `metadata.upstream` entry: `owner/repo@sha:path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream<'a> {
    /// `owner/repo`.
    pub repo: &'a str,
    pub sha: &'a str,
    pub path: &'a str,
}

/// Parse `owner/repo@hex:path`, hex being 7 to 40 lowercase hex digits.
pub fn parse_upstream(s: &str) -> Option<Upstream<'_>> {
    let (repo, rest) = s.split_once('@')?;
    let (sha, path) = rest.split_once(':')?;
    let (owner, name) = repo.split_once('/')?;
    let ident = |x: &str| {
        !x.starts_with('.')
            && !x.is_empty()
            && x.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let valid = ident(owner)
        && ident(name)
        && (7..=40).contains(&sha.len())
        && sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        && !path.is_empty()
        && !path.chars().any(char::is_whitespace);
    valid.then_some(Upstream { repo, sha, path })
}

pub fn valid_upstream(s: &str) -> bool {
    parse_upstream(s).is_some()
}

/// The `metadata.upstream` entries of parsed frontmatter: a single value or a list.
pub fn upstream_entries(fm: &Value) -> Vec<&Value> {
    match fm.get("metadata").and_then(|m| m.get("upstream")) {
        None => Vec::new(),
        Some(Value::Sequence(s)) => s.iter().collect(),
        Some(v) => vec![v],
    }
}

/// Check frontmatter of one SKILL.md. Returns messages (no skill/file context).
pub fn check_frontmatter(dir_name: &str, text: &str, root: SkillRoot) -> Vec<String> {
    let mut errs = Vec::new();
    let fm = match frontmatter::parse(text) {
        Ok(Some(Value::Mapping(m))) => Value::Mapping(m),
        Ok(Some(_)) => return vec!["frontmatter is not a YAML mapping".into()],
        Ok(None) => return vec!["missing frontmatter: the file must open with a --- block".into()],
        Err(e) => return vec![format!("frontmatter is not valid YAML: {e}")],
    };

    match fm.get("name") {
        Some(Value::String(name)) => {
            if name != dir_name {
                errs.push(format!(
                    "name {name:?} does not match directory {dir_name:?}"
                ));
            }
            if root == SkillRoot::Shipped && !name.starts_with("ns-") {
                errs.push(format!("name {name:?} must start with \"ns-\""));
            }
        }
        Some(_) => errs.push("name must be a string".into()),
        None => errs.push("missing name".into()),
    }

    match fm.get("description") {
        Some(Value::String(d)) if d.trim().is_empty() => errs.push("description is empty".into()),
        Some(Value::String(d)) => {
            let n = d.chars().count();
            if n > MAX_DESCRIPTION {
                errs.push(format!(
                    "description is {n} chars; the limit is {MAX_DESCRIPTION}"
                ));
            }
        }
        Some(_) => errs.push("description must be a string".into()),
        None => errs.push("missing description".into()),
    }

    if USER_INVOKED_ONLY.contains(&dir_name)
        && fm.get("disable-model-invocation") != Some(&Value::Bool(true))
    {
        errs.push("disable-model-invocation: true is required".into());
    }

    for e in upstream_entries(&fm) {
        match e.as_str() {
            Some(s) if valid_upstream(s) => {}
            Some(s) => errs.push(format!(
                "metadata.upstream {s:?} must look like owner/repo@<hex sha>:path"
            )),
            None => errs.push("metadata.upstream entries must be strings".into()),
        }
    }
    errs
}

/// Whether a link target starts with a URL scheme (`https:`, `mailto:`).
fn has_scheme(target: &str) -> bool {
    target.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// The lines of `text` outside fenced code blocks, fence lines dropped.
fn prose_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let t = line.trim_start();
        let marker = if t.starts_with("```") {
            Some("```")
        } else if t.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        if let Some(m) = marker {
            match fence {
                None => fence = Some(m),
                Some(open) if open == m => fence = None,
                Some(_) => {}
            }
        } else if fence.is_none() {
            lines.push(line);
        }
    }
    lines
}

/// The H2 sections a `features/*.md` file holds, in order. `None` matches any
/// heading starting with `Driving it with `, since the harness name varies.
const FEATURE_H2S: [Option<&str>; 4] = [
    Some("Sub-features"),
    Some("How to get to it (user POV)"),
    None,
    Some("Gotchas"),
];

/// Check a feature file's H2 headings against `FEATURE_H2S`.
fn check_feature(text: &str) -> Option<String> {
    let found: Vec<&str> = prose_lines(text)
        .into_iter()
        .filter_map(|l| l.strip_prefix("## "))
        .map(str::trim)
        .collect();
    let ok = found.len() == FEATURE_H2S.len()
        && found.iter().zip(FEATURE_H2S).all(|(h, want)| match want {
            Some(w) => *h == w,
            None => h.starts_with("Driving it with "),
        });
    (!ok).then(|| {
        format!(
            "H2 headings must be, in order: Sub-features, How to get to it (user POV), Driving it with <tool>, Gotchas; found: {}",
            found.join(", ")
        )
    })
}

/// `line` with its inline code spans removed. A backtick run opens a span that the next
/// run of the same length closes; a run with no closer is literal text.
fn strip_code_spans(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(i) = rest.find('`') {
        out.push_str(&rest[..i]);
        let tick = &rest[i..];
        let n = tick.len() - tick.trim_start_matches('`').len();
        let after = &tick[n..];
        let mut search = 0;
        let close = loop {
            let Some(j) = after[search..].find('`') else {
                break None;
            };
            let start = search + j;
            let m = after[start..].len() - after[start..].trim_start_matches('`').len();
            if m == n {
                break Some(start + m);
            }
            search = start + m;
        };
        match close {
            Some(end) => rest = &after[end..],
            None => {
                out.push_str(&tick[..n]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Relative link targets (`x`, `./x`, `../x`) outside code blocks and code spans, anchors stripped.
/// URLs with a scheme, absolute paths and bare `#anchor` links are skipped.
pub fn relative_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    for line in prose_lines(text) {
        let line = strip_code_spans(line);
        let mut rest = line.as_str();
        while let Some(i) = rest.find("](") {
            rest = &rest[i + 2..];
            let end = rest.find(')').unwrap_or(rest.len());
            let target = rest[..end].trim();
            // Drop an optional title: [x](./a.md "title")
            let target = target.split_whitespace().next().unwrap_or("");
            let target = target.trim_start_matches('<').trim_end_matches('>');
            if !target.starts_with('/') && !has_scheme(target) {
                let path = target.split('#').next().unwrap_or("");
                if !path.is_empty() {
                    links.push(path.to_string());
                }
            }
            rest = &rest[end..];
        }
    }
    links
}

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            markdown_files(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(p);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SkillRoot {
    Shipped,
    RepoLocal,
}

fn skill_root_kind(root: &Path) -> Result<SkillRoot> {
    let local = fs::canonicalize(root)?
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == ".agents");
    Ok(if local {
        SkillRoot::RepoLocal
    } else {
        SkillRoot::Shipped
    })
}

fn is_skill_dir(p: &Path, root: SkillRoot) -> bool {
    if !p.is_dir() {
        return false;
    }
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match root {
        SkillRoot::Shipped => p.join("SKILL.md").is_file() || name.starts_with("ns-"),
        SkillRoot::RepoLocal => !name.starts_with('.'),
    }
}

fn feature_files(skill_dir: &Path) -> Vec<PathBuf> {
    let mut features: Vec<PathBuf> = fs::read_dir(skill_dir.join("features"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "md")
                && p.file_name().is_some_and(|n| n != "README.md")
        })
        .collect();
    features.sort();
    features
}

/// Checks that `rules/README.md` links `<pack>/README.md` for exactly the `rules/*/` packs.
fn check_rules_index(rules: &Path) -> Vec<LintError> {
    let index = rules.join("README.md");
    let error = |message: String| LintError {
        skill: "rules".into(),
        file: index.to_string_lossy().into_owned(),
        message,
    };
    let packs: BTreeSet<String> = fs::read_dir(rules)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    if packs.is_empty() {
        return Vec::new();
    }
    let Ok(text) = fs::read_to_string(&index) else {
        return vec![error(
            "missing rules/README.md, the index of the rule packs".into(),
        )];
    };
    let named: BTreeSet<String> = relative_links(&text)
        .iter()
        .filter_map(|l| l.trim_start_matches("./").strip_suffix("/README.md"))
        .filter(|p| !p.contains('/'))
        .map(str::to_string)
        .collect();
    let unnamed = packs
        .difference(&named)
        .map(|p| error(format!("rules/README.md does not name the pack rules/{p}/")));
    let missing = named.difference(&packs).map(|p| {
        error(format!(
            "rules/README.md names rules/{p}/, which does not exist"
        ))
    });
    unnamed.chain(missing).collect()
}

pub fn lint_dir(root: &Path) -> Result<Report> {
    if !root.is_dir() {
        return Err(SfError::usage(
            format!("skills directory {} does not exist", root.display()),
            "ns lint skills\n  ns lint path/to/skills --human",
        )
        .into());
    }
    let root_kind = skill_root_kind(root)?;
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_skill_dir(p, root_kind))
        .collect();
    dirs.sort();

    let mut errors = Vec::new();
    let mut skills = 0;
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let skill_md = dir.join("SKILL.md");
        let push = |errors: &mut Vec<LintError>, file: &Path, message: String| {
            errors.push(LintError {
                skill: name.clone(),
                file: file.to_string_lossy().into_owned(),
                message,
            })
        };
        let text = match fs::read_to_string(&skill_md) {
            Ok(t) => t,
            Err(_) => {
                push(&mut errors, &skill_md, "missing SKILL.md".into());
                continue;
            }
        };
        skills += 1;
        for m in check_frontmatter(&name, &text, root_kind) {
            push(&mut errors, &skill_md, m);
        }
        let mut files = Vec::new();
        markdown_files(dir, &mut files);
        let mut linked = HashSet::new();
        for file in &files {
            let Ok(body) = fs::read_to_string(file) else {
                continue;
            };
            let base = file.parent().unwrap_or(dir);
            for link in relative_links(&body) {
                match fs::canonicalize(base.join(&link)) {
                    // A directory link renders, and so links, its README.
                    Ok(target) if target.is_dir() => {
                        linked.insert(target.join("README.md"));
                    }
                    Ok(target) => {
                        linked.insert(target);
                    }
                    Err(_) => push(&mut errors, file, format!("broken link: {link}")),
                }
            }
        }
        for f in feature_files(dir) {
            let rel = f.strip_prefix(dir).unwrap_or(&f).to_string_lossy();
            match fs::read_to_string(&f) {
                Ok(body) => {
                    if let Some(m) = check_feature(&body) {
                        push(&mut errors, &f, format!("{rel}: {m}"));
                    }
                }
                Err(e) => push(&mut errors, &f, format!("{rel}: unreadable: {e}")),
            }
        }
        let mut refs = Vec::new();
        markdown_files(&dir.join("references"), &mut refs);
        for r in refs {
            if fs::canonicalize(&r).is_ok_and(|c| !linked.contains(&c)) {
                let rel = r.strip_prefix(dir).unwrap_or(&r).to_string_lossy();
                push(&mut errors, &r, format!("unlinked reference: {rel}"));
            }
        }
    }
    if let Some(repo) = fs::canonicalize(root)?.parent() {
        if repo.join("docs/decisions").is_dir() {
            errors.extend(decisions::check(repo));
        }
        if repo.join("rules").is_dir() {
            errors.extend(check_rules_index(&repo.join("rules")));
        }
    }
    Ok(Report {
        ok: errors.is_empty(),
        skills,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\nname: ns-tdd\ndescription: Test first.\nmetadata:\n  upstream: mattpocock/skills@b0618bc436ad:skills/engineering/tdd\n---\nbody\n";

    #[test]
    fn good_frontmatter() {
        assert!(check_frontmatter("ns-tdd", GOOD, SkillRoot::Shipped).is_empty());
    }

    #[test]
    fn name_mismatch_and_prefix() {
        let e = check_frontmatter("tdd", GOOD, SkillRoot::Shipped);
        assert_eq!(e.len(), 1, "{e:?}");
        let e = check_frontmatter(
            "tdd",
            "---\nname: tdd\ndescription: x\n---\n",
            SkillRoot::Shipped,
        );
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("ns-"));
    }

    #[test]
    fn description_rules() {
        let e = check_frontmatter(
            "ns-a",
            "---\nname: ns-a\ndescription: \"  \"\n---\n",
            SkillRoot::Shipped,
        );
        assert_eq!(e, ["description is empty"]);
        let long = "x".repeat(1025);
        let e = check_frontmatter(
            "ns-a",
            &format!("---\nname: ns-a\ndescription: {long}\n---\n"),
            SkillRoot::Shipped,
        );
        assert!(e[0].contains("1025"));
        let e = check_frontmatter("ns-a", "---\nname: ns-a\n---\n", SkillRoot::Shipped);
        assert_eq!(e, ["missing description"]);
    }

    #[test]
    fn user_invoked_skills_disable_model_invocation() {
        let without = "---\nname: ns-auto\ndescription: x\n---\n";
        let e = check_frontmatter("ns-auto", without, SkillRoot::Shipped);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("disable-model-invocation"));
        let with = "---\nname: ns-auto\ndescription: x\ndisable-model-invocation: true\n---\n";
        assert!(check_frontmatter("ns-auto", with, SkillRoot::Shipped).is_empty());
        for bad in ["false", "\"true\""] {
            let src = format!(
                "---\nname: ns-auto\ndescription: x\ndisable-model-invocation: {bad}\n---\n"
            );
            assert!(
                !check_frontmatter("ns-auto", &src, SkillRoot::Shipped).is_empty(),
                "{bad} must error"
            );
        }
        assert!(check_frontmatter(
            "ns-a",
            "---\nname: ns-a\ndescription: x\n---\n",
            SkillRoot::Shipped
        )
        .is_empty());
    }

    #[test]
    fn bad_yaml_and_missing() {
        assert!(
            check_frontmatter("ns-a", "---\nname: [unclosed\n---\n", SkillRoot::Shipped)[0]
                .contains("YAML")
        );
        assert!(
            check_frontmatter("ns-a", "# no fm\n", SkillRoot::Shipped)[0]
                .contains("missing frontmatter")
        );
    }

    #[test]
    fn upstream_formats() {
        assert!(valid_upstream(
            "cursor/plugins@ccb5507cec15:pstack/skills/x"
        ));
        assert!(!valid_upstream("cursor/plugins@main:x"));
        assert!(!valid_upstream("cursor/plugins@ccb5507cec15"));
        assert!(!valid_upstream("plugins@ccb5507cec15:x"));
        assert!(!valid_upstream("cursor/plugins@ABCDEF0:x"));
        for bad in [
            "../..@abc1234:x",
            "./x@abc1234:x",
            "a/..@abc1234:x",
            "a/.git@abc1234:x",
            ".a/b@abc1234:x",
        ] {
            assert!(!valid_upstream(bad), "{bad}");
        }
        assert_eq!(
            parse_upstream("a/b@1234567:p/q.md"),
            Some(Upstream {
                repo: "a/b",
                sha: "1234567",
                path: "p/q.md"
            })
        );
        let list = "---\nname: ns-a\ndescription: x\nmetadata:\n  upstream:\n    - a/b@1234567:p\n    - a/b@nothex:p\n---\n";
        let e = check_frontmatter("ns-a", list, SkillRoot::Shipped);
        assert_eq!(e.len(), 1);
        assert!(e[0].contains("nothex"));
    }

    #[test]
    fn finds_relative_links_only() {
        let text = "See [a](./a.md), [b](../ns-b/SKILL.md#top), [c](https://x.y/z), [d](#anchor), [e](./e.md \"t\").\n```\n[f](./ignored.md)\n```\n[g](refs/no-dot.md#x), [h](mailto:a@b.c), [i](/abs.md), [j](<refs/angle.md>), [k](refs/a:b.md), [l](:c.md), [m](git+ssh://h/r), [n](x-y.z:q)\n";
        assert_eq!(
            relative_links(text),
            [
                "./a.md",
                "../ns-b/SKILL.md",
                "./e.md",
                "refs/no-dot.md",
                "refs/angle.md",
                "refs/a:b.md",
                ":c.md"
            ]
        );
    }

    #[test]
    fn links_inside_code_spans_are_skipped() {
        let text = "Run `printf '[a](./gone.md)'` or ``x [b](./gone2.md) ` y`` then [c](./c.md), and `[d](./d.md)\n";
        assert_eq!(relative_links(text), ["./c.md", "./d.md"]);
        let text = "`a `` [x](./x.md) b` [c](./c.md) and [e](./e`.md)\n";
        assert_eq!(relative_links(text), ["./c.md", "./e`.md"]);
    }

    fn skill(body: &str, refs: &[(&str, &str)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ns-a");
        fs::create_dir_all(dir.join("references")).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: ns-a\ndescription: A.\n---\n{body}"),
        )
        .unwrap();
        for (name, text) in refs {
            fs::write(dir.join("references").join(name), text).unwrap();
        }
        tmp
    }

    fn messages(tmp: &tempfile::TempDir) -> Vec<String> {
        let report = lint_dir(tmp.path()).unwrap();
        report.errors.into_iter().map(|e| e.message).collect()
    }

    #[test]
    fn unprefixed_broken_link_fails() {
        let tmp = skill("[m](references/missing.md)\n", &[]);
        assert_eq!(messages(&tmp), ["broken link: references/missing.md"]);
    }

    #[test]
    fn unlinked_reference_fails_until_linked() {
        let tmp = skill("no links\n", &[("orphan.md", "x\n")]);
        assert_eq!(messages(&tmp), ["unlinked reference: references/orphan.md"]);
        let tmp = skill("[o](references/orphan.md)\n", &[("orphan.md", "x\n")]);
        assert!(messages(&tmp).is_empty(), "{:?}", messages(&tmp));
    }

    #[test]
    fn directory_link_covers_its_readme() {
        let tmp = skill("[ex](references/ex/)\n", &[]);
        let ex = tmp.path().join("ns-a/references/ex");
        fs::create_dir_all(&ex).unwrap();
        fs::write(ex.join("README.md"), "[f](f.md)\n").unwrap();
        fs::write(ex.join("f.md"), "x\n").unwrap();
        assert!(messages(&tmp).is_empty(), "{:?}", messages(&tmp));
        fs::write(
            tmp.path().join("ns-a/SKILL.md"),
            "---\nname: ns-a\ndescription: A.\n---\n",
        )
        .unwrap();
        assert_eq!(
            messages(&tmp),
            ["unlinked reference: references/ex/README.md"]
        );
    }

    fn write_skill_at(root: &Path, name: &str, extra: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: A.\n---\n{extra}"),
        )
        .unwrap();
    }

    fn lint_messages(root: &Path) -> (usize, Vec<String>) {
        let report = lint_dir(root).unwrap();
        let msgs = report.errors.into_iter().map(|e| e.message).collect();
        (report.skills, msgs)
    }

    #[test]
    fn decisions_are_checked_when_the_parent_has_docs_decisions() {
        let tmp = tempfile::tempdir().unwrap();
        let skills = tmp.path().join("skills");
        write_skill_at(&skills, "ns-a", "Cites D1.\n");
        assert_eq!(lint_messages(&skills), (1, vec![]));

        fs::create_dir_all(tmp.path().join("docs/decisions")).unwrap();
        let report = lint_dir(&skills).unwrap();
        assert!(!report.ok);
        assert_eq!(report.skills, 1);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert_eq!(report.errors[0].skill, "decisions");
        assert_eq!(
            report.errors[0].message,
            "line 5: cites D1, which has no file in docs/decisions/"
        );

        fs::write(
            tmp.path().join("docs/decisions/D1-one.md"),
            "---\nid: D1\n---\n",
        )
        .unwrap();
        assert_eq!(lint_messages(&skills), (1, vec![]));
    }

    #[test]
    fn rules_index_must_name_exactly_the_rule_pack_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let skills = tmp.path().join("skills");
        write_skill_at(&skills, "ns-a", "");
        let rules = tmp.path().join("rules");
        fs::create_dir_all(&rules).unwrap();
        assert_eq!(lint_messages(&skills), (1, vec![]));

        fs::create_dir_all(rules.join("c")).unwrap();
        fs::create_dir_all(rules.join("rust")).unwrap();
        let report = lint_dir(&skills).unwrap();
        assert!(!report.ok);
        assert_eq!(report.errors[0].skill, "rules");
        assert_eq!(
            lint_messages(&skills).1,
            ["missing rules/README.md, the index of the rule packs"]
        );

        let index = "# Rule packs\n\nSee [FACTORY](../docs/FACTORY.md).\n\n\
                     - [`c/`](c/README.md): C.\n- [`go/`](go/README.md): Go.\n";
        fs::write(rules.join("README.md"), index).unwrap();
        assert_eq!(
            lint_messages(&skills).1,
            [
                "rules/README.md does not name the pack rules/rust/",
                "rules/README.md names rules/go/, which does not exist",
            ]
        );

        fs::create_dir_all(rules.join(".cache")).unwrap();
        fs::write(
            rules.join("README.md"),
            "- [`c/`](c/README.md): C.\n- [`rust/`](./rust/README.md): Rust, \
             tested as [its tests](rust/tests/README.md) say.\n",
        )
        .unwrap();
        assert_eq!(lint_messages(&skills), (1, vec![]));
    }

    #[test]
    fn repo_local_skill_is_exempt_from_the_prefix_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join(".agents/skills");
        write_skill_at(&local, "verify-x", "");
        assert_eq!(lint_messages(&local), (1, vec![]));

        fs::write(
            local.join("verify-x/SKILL.md"),
            "---\nname: verify-y\ndescription: A.\n---\n",
        )
        .unwrap();
        assert_eq!(
            lint_messages(&local),
            (
                1,
                vec!["name \"verify-y\" does not match directory \"verify-x\"".to_string()]
            )
        );

        let shipped = tmp.path().join("skills");
        write_skill_at(&shipped, "verify-x", "");
        assert_eq!(
            lint_messages(&shipped),
            (
                1,
                vec!["name \"verify-x\" must start with \"ns-\"".to_string()]
            )
        );
    }

    fn feature(root: &Path, file: &str, body: &str) {
        let dir = root.join("verify-x/features");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), body).unwrap();
    }

    const FEATURE: &str = "# F\n\nWhat it does.\n\n## Sub-features\n\n- `f-a` a.\n\n## How to get to it (user POV)\n\n- `x`\n\n## Driving it with control-x\n\nPreconditions:\n\n```\n## not a heading\n```\n\n## Gotchas\n\n- none\n";

    #[test]
    fn ns_dir_without_skill_md_fails_and_other_dirs_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        fs::create_dir_all(root.join("ns-empty")).unwrap();
        fs::create_dir_all(root.join("not-a-skill")).unwrap();
        assert_eq!(
            lint_messages(&root),
            (0, vec!["missing SKILL.md".to_string()])
        );
    }

    #[test]
    fn repo_local_dir_without_skill_md_fails_and_hidden_dirs_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join(".agents/skills");
        fs::create_dir_all(local.join("verify-y")).unwrap();
        fs::create_dir_all(local.join("verify-z")).unwrap();
        fs::write(local.join("verify-z/skill.md"), "x\n").unwrap();
        fs::create_dir_all(local.join(".cache")).unwrap();
        write_skill_at(&local, "verify-x", "");
        assert_eq!(
            lint_messages(&local),
            (1, vec!["missing SKILL.md".to_string(); 2])
        );
    }

    #[test]
    fn regular_files_in_a_root_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join(".agents/skills");
        write_skill_at(&local, "verify-x", "");
        fs::write(local.join("README.md"), "x\n").unwrap();
        assert_eq!(lint_messages(&local), (1, vec![]));

        let shipped = tmp.path().join("skills");
        write_skill_at(&shipped, "ns-a", "");
        fs::write(shipped.join("README.md"), "x\n").unwrap();
        fs::write(shipped.join("ns-notes.md"), "x\n").unwrap();
        assert_eq!(lint_messages(&shipped), (1, vec![]));
    }

    #[cfg(unix)]
    #[test]
    fn exemption_follows_symlinks_to_the_real_parent() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join(".agents/real");
        write_skill_at(&real, "verify-x", "");
        let link = tmp.path().join("skills");
        symlink(&real, &link).unwrap();
        assert_eq!(lint_messages(&link), (1, vec![]));

        let plain = tmp.path().join("plain");
        write_skill_at(&plain, "verify-x", "");
        let agents = tmp.path().join("proj/.agents");
        fs::create_dir_all(&agents).unwrap();
        symlink(&plain, agents.join("skills")).unwrap();
        assert_eq!(
            lint_messages(&agents.join("skills")),
            (
                1,
                vec!["name \"verify-x\" must start with \"ns-\"".to_string()]
            )
        );
    }

    #[test]
    fn prefix_exemption_needs_agents_as_the_direct_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join(".agents/other/skills");
        write_skill_at(&nested, "verify-x", "");
        assert_eq!(
            lint_messages(&nested),
            (
                1,
                vec!["name \"verify-x\" must start with \"ns-\"".to_string()]
            )
        );
    }

    #[test]
    fn near_miss_feature_headings_fail_and_trailing_space_passes() {
        let bare = FEATURE.replace("## Driving it with control-x", "## Driving it with");
        assert!(check_feature(&bare).unwrap().contains("Driving it with,"));
        let no_with = FEATURE.replace("## Driving it with control-x", "## Driving it");
        assert!(check_feature(&no_with).is_some());
        let padded = FEATURE.replace("## Gotchas", "## Gotchas  ");
        assert_eq!(check_feature(&padded), None);
    }

    #[test]
    fn feature_files_need_the_four_h2s_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join(".agents/skills");
        write_skill_at(&local, "verify-x", "");
        feature(&local, "README.md", "# Index\n\n## Features\n");
        feature(&local, "good.md", FEATURE);
        feature(&local, "notes.txt", "## Anything\n");
        assert_eq!(lint_messages(&local), (1, vec![]));

        feature(
            &local,
            "renamed.md",
            &FEATURE.replace("## Driving it with control-x", "## Running it"),
        );
        feature(
            &local,
            "missing.md",
            &FEATURE.replace("## Gotchas", "Gotchas"),
        );
        feature(
            &local,
            "swapped.md",
            &FEATURE
                .replace("## Sub-features", "## TMP")
                .replace("## How to get to it (user POV)", "## Sub-features")
                .replace("## TMP", "## How to get to it (user POV)"),
        );
        let report = lint_dir(&local).unwrap();
        let errs: Vec<_> = report
            .errors
            .iter()
            .map(|e| (e.file.rsplit('/').next().unwrap(), e.message.as_str()))
            .collect();
        let want = "H2 headings must be, in order: Sub-features, How to get to it (user POV), Driving it with <tool>, Gotchas";
        assert_eq!(
            errs,
            [
                (
                    "missing.md",
                    format!("features/missing.md: {want}; found: Sub-features, How to get to it (user POV), Driving it with control-x").as_str()
                ),
                (
                    "renamed.md",
                    format!("features/renamed.md: {want}; found: Sub-features, How to get to it (user POV), Running it, Gotchas").as_str()
                ),
                (
                    "swapped.md",
                    format!("features/swapped.md: {want}; found: How to get to it (user POV), Sub-features, Driving it with control-x, Gotchas").as_str()
                ),
            ]
        );
    }

    #[test]
    fn unreadable_feature_file_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join(".agents/skills");
        write_skill_at(&local, "verify-x", "");
        let dir = local.join("verify-x/features");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("x.md"), [0xff, 0xfe, 0xfd]).unwrap();
        let (skills, errs) = lint_messages(&local);
        assert_eq!(skills, 1);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].starts_with("features/x.md: unreadable: "),
            "{errs:?}"
        );
    }

    #[test]
    fn feature_files_are_checked_under_a_shipped_root() {
        let tmp = tempfile::tempdir().unwrap();
        let shipped = tmp.path().join("skills");
        write_skill_at(&shipped, "ns-a", "");
        let dir = shipped.join("ns-a/features");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bad.md"), "# F\n\n## Nope\n").unwrap();
        let (_, errs) = lint_messages(&shipped);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].starts_with("features/bad.md: H2 headings must be"),
            "{errs:?}"
        );
    }

    #[test]
    fn extra_or_missing_h2s_fail() {
        let extra = format!("{FEATURE}\n## Appendix\n\n- more\n");
        assert!(check_feature(&extra)
            .unwrap()
            .ends_with("Gotchas, Appendix"));
        assert!(check_feature("# F\n\nNo sections.\n")
            .unwrap()
            .ends_with("found: "));
        assert_eq!(check_feature(FEATURE), None);
    }

    #[test]
    fn only_column_zero_h2_lines_count_as_headings() {
        let body = format!("{FEATURE}\n### Detail\n\n    ## indented\n");
        assert_eq!(check_feature(&body), None);
    }

    #[test]
    fn a_fence_closes_only_on_its_own_marker() {
        let tilde_inside = "```\n~~~\n## Fake\n[x](./gone.md)\n```\n";
        let backtick_inside = "~~~\n```\n## Fake\n[x](./gone.md)\n~~~\n";
        for text in [tilde_inside, backtick_inside] {
            assert_eq!(prose_lines(text), Vec::<&str>::new(), "{text}");
            assert!(relative_links(text).is_empty(), "{text}");
            let body = format!("{FEATURE}\n{text}");
            assert_eq!(check_feature(&body), None, "{text}");
        }
    }

    #[test]
    fn setup_verify_feature_map_example_passes() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../skills/ns-setup-verify/references/feature-map-example");
        let mut checked = 0;
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "md") && !path.ends_with("README.md") {
                let body = fs::read_to_string(&path).unwrap();
                assert_eq!(check_feature(&body), None, "{}", path.display());
                checked += 1;
            }
        }
        assert!(checked > 0);
    }

    #[test]
    fn reference_linked_from_another_reference_passes() {
        let tmp = skill(
            "[a](./references/a.md#top)\n",
            &[("a.md", "[b](b.md)\n"), ("b.md", "x\n")],
        );
        assert!(messages(&tmp).is_empty(), "{:?}", messages(&tmp));
    }
}
