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

#[cfg(test)]
mod tests {
    use super::*;

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
