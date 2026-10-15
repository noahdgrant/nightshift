//! `ns lint [path]`: structural checks on the skills under a directory, and the
//! decision checks when its parent has `docs/decisions/`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use serde_yaml::Value;

use crate::decisions;
use crate::error::SfError;
use crate::frontmatter;

mod rules_index;
use rules_index::check_rules_index;
mod tracker;

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
    classified_lines(text)
        .into_iter()
        .filter(|(_, _, fenced)| !fenced)
        .map(|(_, line, _)| line)
        .collect()
}

/// Each line of `text` with its 1-based number and whether it sits inside a fenced
/// code block. Fence lines themselves are dropped.
fn classified_lines(text: &str) -> Vec<(usize, &str, bool)> {
    let mut lines = Vec::new();
    let mut fence: Option<&str> = None;
    for (i, line) in text.lines().enumerate() {
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
        } else {
            lines.push((i + 1, line, fence.is_some()));
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

/// `line` with its inline code spans removed.
fn strip_code_spans(line: &str) -> String {
    code_spans(line).0
}

/// `line` split into its text outside inline code spans and the spans' contents. A
/// backtick run opens a span that the next run of the same length closes; a run with no
/// closer is literal text.
fn code_spans(line: &str) -> (String, Vec<&str>) {
    let mut out = String::new();
    let mut spans = Vec::new();
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
            Some(end) => {
                spans.push(&after[..end - n]);
                rest = &after[end..];
            }
            None => {
                out.push_str(&tick[..n]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, spans)
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

/// The fixed `/tmp/<name>` paths in `text`, code included, with their 1-based line numbers.
/// Parallel units would share such a file; `ns run` gives each phase its own `TMPDIR`.
/// A placeholder such as `/tmp/<name>`, or `/tmp/` inside a longer path, is not one.
fn fixed_tmp_paths(text: &str) -> Vec<(usize, String)> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let name = |c: char| word(c) || matches!(c, '.' | '-');
    let mut found = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for (at, _) in line.match_indices("/tmp/") {
            let starts_path = line[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !name(c) && !matches!(c, '}' | '~'));
            let rest = &line[at + "/tmp/".len()..];
            if starts_path && rest.starts_with(word) {
                let end = rest.find(|c| !name(c) && c != '/').unwrap_or(rest.len());
                let path = rest[..end].trim_end_matches('.');
                found.push((i + 1, format!("/tmp/{path}")));
            }
        }
    }
    found
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
            if !file.starts_with(dir.join("evals")) {
                for (n, path) in fixed_tmp_paths(&body) {
                    let m =
                        format!("line {n}: fixed path {path}; use mktemp or a name under $TMPDIR");
                    push(&mut errors, file, m);
                }
            }
            let rel = file.strip_prefix(root).unwrap_or(file);
            let prose = file == &skill_md || file.starts_with(dir.join("references"));
            if prose && !tracker::TEMPLATES.iter().any(|t| rel == Path::new(t)) {
                for (n, code) in tracker::tracker_commands(&body) {
                    let m = format!("line {n}: tracker command `{code}`; name the operation from docs/agents/issue-tracker.md instead");
                    push(&mut errors, file, m);
                }
            }
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
    errors.extend(tracker::check_templates(root));
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
mod tests;
