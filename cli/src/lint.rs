//! `ns lint [path]`: structural checks on `ns-*/SKILL.md` skills.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use serde_yaml::Value;

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

/// `owner/repo@hex:path`, hex being 7 to 40 lowercase hex digits.
pub fn valid_upstream(s: &str) -> bool {
    let Some((repo, rest)) = s.split_once('@') else {
        return false;
    };
    let Some((sha, path)) = rest.split_once(':') else {
        return false;
    };
    let Some((owner, name)) = repo.split_once('/') else {
        return false;
    };
    let ident = |x: &str| {
        !x.is_empty()
            && x.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    ident(owner)
        && ident(name)
        && (7..=40).contains(&sha.len())
        && sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        && !path.is_empty()
        && !path.chars().any(char::is_whitespace)
}

/// Check frontmatter of one SKILL.md. Returns messages (no skill/file context).
pub fn check_frontmatter(dir_name: &str, text: &str) -> Vec<String> {
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
            if !name.starts_with("ns-") {
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

    if let Some(up) = fm.get("metadata").and_then(|m| m.get("upstream")) {
        let entries: Vec<&Value> = match up {
            Value::Sequence(s) => s.iter().collect(),
            v => vec![v],
        };
        for e in entries {
            match e.as_str() {
                Some(s) if valid_upstream(s) => {}
                Some(s) => errs.push(format!(
                    "metadata.upstream {s:?} must look like owner/repo@<hex sha>:path"
                )),
                None => errs.push("metadata.upstream entries must be strings".into()),
            }
        }
    }
    errs
}

/// Relative link targets (`./x`, `../x`) outside fenced code blocks, anchors stripped.
pub fn relative_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
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
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let mut rest = line;
        while let Some(i) = rest.find("](") {
            rest = &rest[i + 2..];
            let end = rest.find(')').unwrap_or(rest.len());
            let target = rest[..end].trim();
            // Drop an optional title: [x](./a.md "title")
            let target = target.split_whitespace().next().unwrap_or("");
            let target = target.trim_start_matches('<').trim_end_matches('>');
            if target.starts_with("./") || target.starts_with("../") {
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

pub fn lint_dir(root: &Path) -> Result<Report> {
    if !root.is_dir() {
        return Err(SfError::usage(
            format!("skills directory {} does not exist", root.display()),
            "ns lint skills\n  ns lint path/to/skills --human",
        )
        .into());
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("ns-"))
        })
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
        for m in check_frontmatter(&name, &text) {
            push(&mut errors, &skill_md, m);
        }
        let mut files = Vec::new();
        markdown_files(dir, &mut files);
        for file in files {
            let Ok(body) = fs::read_to_string(&file) else {
                continue;
            };
            let base = file.parent().unwrap_or(dir);
            for link in relative_links(&body) {
                if !base.join(&link).exists() {
                    push(&mut errors, &file, format!("broken link: {link}"));
                }
            }
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
        assert!(check_frontmatter("ns-tdd", GOOD).is_empty());
    }

    #[test]
    fn name_mismatch_and_prefix() {
        let e = check_frontmatter("tdd", GOOD);
        assert_eq!(e.len(), 1, "{e:?}");
        let e = check_frontmatter("tdd", "---\nname: tdd\ndescription: x\n---\n");
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("ns-"));
    }

    #[test]
    fn description_rules() {
        let e = check_frontmatter("ns-a", "---\nname: ns-a\ndescription: \"  \"\n---\n");
        assert_eq!(e, ["description is empty"]);
        let long = "x".repeat(1025);
        let e = check_frontmatter(
            "ns-a",
            &format!("---\nname: ns-a\ndescription: {long}\n---\n"),
        );
        assert!(e[0].contains("1025"));
        let e = check_frontmatter("ns-a", "---\nname: ns-a\n---\n");
        assert_eq!(e, ["missing description"]);
    }

    #[test]
    fn user_invoked_skills_disable_model_invocation() {
        let without = "---\nname: ns-auto\ndescription: x\n---\n";
        let e = check_frontmatter("ns-auto", without);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("disable-model-invocation"));
        let with = "---\nname: ns-auto\ndescription: x\ndisable-model-invocation: true\n---\n";
        assert!(check_frontmatter("ns-auto", with).is_empty());
        for bad in ["false", "\"true\""] {
            let src = format!(
                "---\nname: ns-auto\ndescription: x\ndisable-model-invocation: {bad}\n---\n"
            );
            assert!(
                !check_frontmatter("ns-auto", &src).is_empty(),
                "{bad} must error"
            );
        }
        assert!(check_frontmatter("ns-a", "---\nname: ns-a\ndescription: x\n---\n").is_empty());
    }

    #[test]
    fn bad_yaml_and_missing() {
        assert!(check_frontmatter("ns-a", "---\nname: [unclosed\n---\n")[0].contains("YAML"));
        assert!(check_frontmatter("ns-a", "# no fm\n")[0].contains("missing frontmatter"));
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
        let list = "---\nname: ns-a\ndescription: x\nmetadata:\n  upstream:\n    - a/b@1234567:p\n    - a/b@nothex:p\n---\n";
        let e = check_frontmatter("ns-a", list);
        assert_eq!(e.len(), 1);
        assert!(e[0].contains("nothex"));
    }

    #[test]
    fn finds_relative_links_only() {
        let text = "See [a](./a.md), [b](../ns-b/SKILL.md#top), [c](https://x.y/z), [d](#anchor), [e](./e.md \"t\").\n```\n[f](./ignored.md)\n```\n[g](refs/no-dot.md)\n";
        assert_eq!(
            relative_links(text),
            ["./a.md", "../ns-b/SKILL.md", "./e.md"]
        );
    }
}
