//! The findings in the evidence `verify` and `review` keep, read back for `ns status`. A
//! verification's finding is a requirement section (`### R<n>`) that is FAIL or INCONCLUSIVE; a
//! review's is a block that ends in the `Fix level:` line the review skill asks of every finding,
//! with its `Durable fix:` line when it has one. A review keeps a section per session, headed
//! `# Review session: <name>`.

/// What a review session found: its name and the text of each finding.
#[derive(Debug, PartialEq, Eq)]
pub struct Session {
    pub name: String,
    pub findings: Vec<String>,
}

/// The text of each requirement section of a verification that is FAIL or INCONCLUSIVE.
pub fn verification(text: &str) -> Vec<String> {
    let mut sections: Vec<Vec<&str>> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("### ") {
            sections.push(Vec::new());
            inside = true;
        } else if line.starts_with("# ") || line.starts_with("## ") {
            // A heading above a requirement's ends it, so `## Workarounds` is not part of one.
            inside = false;
        }
        if let Some(section) = sections.last_mut().filter(|_| inside) {
            section.push(line);
        }
    }
    sections
        .iter()
        .filter(|s| s.iter().skip(1).any(|l| unmet(l)))
        .map(|s| s.join("\n").trim().to_string())
        .collect()
}

/// Whether a line gives a requirement's verdict as FAIL or INCONCLUSIVE, however it is bulleted
/// or bolded.
fn unmet(line: &str) -> bool {
    let line = line.trim_start_matches(|c: char| c.is_whitespace() || "-*`".contains(c));
    line.starts_with("FAIL") || line.starts_with("INCONCLUSIVE")
}

/// What each session of a review found, in the order the review keeps them.
pub fn review(text: &str) -> Vec<Session> {
    let mut sessions: Vec<(String, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix(super::review::SECTION) {
            sessions.push((name.trim().to_string(), Vec::new()));
        } else if let Some((_, lines)) = sessions.last_mut() {
            lines.push(line);
        }
    }
    sessions
        .into_iter()
        .map(|(name, lines)| Session {
            name,
            findings: findings_split(&lines),
        })
        .collect()
}

/// Each block of `lines` that ends in a `Fix level:` line, with the `Durable fix:` line after it.
fn findings_split(lines: &[&str]) -> Vec<String> {
    let starts = |line: &str, label: &str| {
        line.trim_start_matches(|c: char| c.is_whitespace() || "-*`".contains(c))
            .starts_with(label)
    };
    let mut found = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let mut closed = false;
    for &line in lines {
        if closed && starts(line, "Durable fix:") {
            block.push(line);
            continue;
        }
        if closed {
            found.push(block.join("\n").trim().to_string());
            block.clear();
        }
        block.push(line);
        closed = starts(line, "Fix level:");
    }
    if closed {
        found.push(block.join("\n").trim().to_string());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verification_counts_the_sections_that_are_not_proven() {
        let text = "### R1\nPASS: ran it\n\n### R2\nFAIL: wrong output\n\n### R3\n- **INCONCLUSIVE**: needs staging\n\n## Workarounds\nNone\nVERDICT: FAIL\n";
        assert_eq!(
            verification(text),
            vec![
                "### R2\nFAIL: wrong output".to_string(),
                "### R3\n- **INCONCLUSIVE**: needs staging".to_string(),
            ]
        );
    }

    #[test]
    fn a_passing_verification_has_no_findings() {
        assert!(verification("### R1\nPASS\n\nVERDICT: PASS\n").is_empty());
        assert!(verification("").is_empty());
    }

    const REVIEW: &str = "# Review session: Behavior

- `a.rs:3`
- wrong
- `Fix level:` code
- `Durable fix:` the type X

- `b.rs:9`
- also wrong
- Fix level: none

# Review session: Spec

PASS: nothing to find
VERDICT: PASS

# Review session: Maintainability

`c.rs:1` bad
Fix level: prose

VERDICT: FAIL
";

    #[test]
    fn a_review_counts_the_findings_of_each_session() {
        let sessions = review(REVIEW);
        let counts: Vec<_> = sessions
            .iter()
            .map(|s| (s.name.as_str(), s.findings.len()))
            .collect();
        assert_eq!(
            counts,
            [("Behavior", 2), ("Spec", 0), ("Maintainability", 1)]
        );
    }

    #[test]
    fn a_finding_keeps_its_text_up_to_its_durable_fix() {
        let sessions = review(REVIEW);
        assert_eq!(
            sessions[0].findings[0],
            "- `a.rs:3`\n- wrong\n- `Fix level:` code\n- `Durable fix:` the type X"
        );
        assert_eq!(
            sessions[0].findings[1],
            "- `b.rs:9`\n- also wrong\n- Fix level: none"
        );
    }

    #[test]
    fn text_with_no_session_heading_has_no_sessions() {
        assert!(review("Fix level: code\nVERDICT: FAIL\n").is_empty());
    }
}
