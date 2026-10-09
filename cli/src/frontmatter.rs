//! Split a markdown file into YAML frontmatter and body.

/// Returns `(yaml, body)` if the text opens with a `---` fenced frontmatter block.
pub fn split(text: &str) -> Option<(&str, &str)> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let yaml = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return Some((yaml, body));
        }
        offset += line.len();
    }
    None
}

/// Parse the frontmatter block as YAML. `Ok(None)` means there is no frontmatter.
pub fn parse(text: &str) -> Result<Option<serde_yaml::Value>, serde_yaml::Error> {
    match split(text) {
        None => Ok(None),
        Some((yaml, _)) => serde_yaml::from_str(yaml).map(Some),
    }
}

/// A top-level frontmatter value exactly as written, unquoted. Artifact values go
/// through this, never through YAML typing: YAML reads an unquoted sha such as
/// `0e05787` or `1234567` as a number and loses it.
pub fn raw_field(yaml: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let line = yaml.lines().find(|l| l.starts_with(&prefix))?;
    let mut v = line[prefix.len()..].trim();
    if let Some(i) = v.find(" #") {
        v = v[..i].trim_end();
    }
    let v = v.trim_matches(|c| c == '"' || c == '\'');
    (!v.is_empty()).then(|| v.to_string())
}

/// `raw_field` over a whole markdown file. `None` without frontmatter.
pub fn field(text: &str, key: &str) -> Option<String> {
    raw_field(split(text)?.0, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_keep_sha_like_values_verbatim() {
        let text = "---\nstatus: pass\nsha: 0e05787\nother: 1234567  # note\npr: '12'\n---\nbody\n";
        assert_eq!(field(text, "sha").as_deref(), Some("0e05787"));
        assert_eq!(field(text, "other").as_deref(), Some("1234567"));
        assert_eq!(field(text, "pr").as_deref(), Some("12"));
        assert_eq!(field(text, "status").as_deref(), Some("pass"));
        assert_eq!(field(text, "missing"), None);
        assert_eq!(field("no frontmatter\n", "sha"), None);
    }

    #[test]
    fn splits_frontmatter_and_body() {
        let (yaml, body) = split("---\nname: x\n---\n# Title\n").unwrap();
        assert_eq!(yaml, "name: x\n");
        assert_eq!(body, "# Title\n");
    }

    #[test]
    fn no_frontmatter() {
        assert!(split("# Title\n").is_none());
        assert!(split("---\nname: x\nno close\n").is_none());
    }

    #[test]
    fn parses_yaml() {
        let v = parse("---\nname: x\n---\n").unwrap().unwrap();
        assert_eq!(v["name"].as_str(), Some("x"));
    }
}
