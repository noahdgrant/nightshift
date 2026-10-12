//! Read the `touches` prediction from an Agent Brief: the path globs the unit
//! will probably change. `brief.md` carries it as a frontmatter list; the brief
//! comment on the issue carries it as one `**Touches:**` line of backticked globs.

use crate::frontmatter;

const LINE_KEY: &str = "Touches:";

/// The brief's `touches` globs, from `brief.md` frontmatter or the brief comment
/// text. Empty when the brief makes no prediction or the value is malformed.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the ns watch overlap scheduler (#418) is its first caller"
    )
)]
pub fn touches(text: &str) -> Vec<String> {
    if let Some((yaml, body)) = frontmatter::split(text) {
        match serde_yaml::from_str::<serde_yaml::Value>(yaml) {
            Ok(v) if v.get("touches").is_some() => return from_yaml(&v["touches"]),
            Ok(_) => return from_line(body),
            Err(_) => return Vec::new(),
        }
    }
    from_line(text)
}

fn from_yaml(v: &serde_yaml::Value) -> Vec<String> {
    let Some(items) = v.as_sequence() else {
        return Vec::new();
    };
    let globs: Option<Vec<String>> = items
        .iter()
        .map(|i| {
            i.as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
        })
        .collect();
    globs.unwrap_or_default()
}

fn from_line(text: &str) -> Vec<String> {
    let Some(rest) = text.lines().find_map(|l| {
        l.trim_start_matches(['*', '_', ' ', '\t'])
            .strip_prefix(LINE_KEY)
    }) else {
        return Vec::new();
    };
    let globs: Option<Vec<String>> = rest
        .trim_start_matches(['*', '_'])
        .split(',')
        .map(|item| {
            item.trim()
                .strip_prefix('`')?
                .strip_suffix('`')
                .filter(|g| !g.trim().is_empty() && !g.contains('`'))
                .map(str::to_string)
        })
        .collect();
    globs.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn globs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn frontmatter_block_list() {
        let text = "---\nunit: 1-x\nstatus: pass\ntouches:\n  - cli/src/run.rs\n  - \"skills/ns-review/**\"\n---\nIssue: x\n";
        assert_eq!(
            touches(text),
            globs(&["cli/src/run.rs", "skills/ns-review/**"])
        );
    }

    #[test]
    fn frontmatter_flow_list() {
        let text = "---\ntouches: [cli/src/run.rs, 'docs/**']\n---\n";
        assert_eq!(touches(text), globs(&["cli/src/run.rs", "docs/**"]));
    }

    #[test]
    fn frontmatter_wins_over_the_body_line() {
        let text = "---\ntouches: [a.rs]\n---\n**Touches:** `b.rs`\n";
        assert_eq!(touches(text), globs(&["a.rs"]));
    }

    #[test]
    fn frontmatter_without_the_key_falls_back_to_the_body_line() {
        let text = "---\nstatus: pass\n---\n## Agent Brief\n**Touches:** `b.rs`\n";
        assert_eq!(touches(text), globs(&["b.rs"]));
    }

    #[test]
    fn comment_line() {
        let text = "🤖 *Posted by nightshift, an AI agent.*\n\n## Agent Brief\n\n**Summary:** x\n**Touches:** `cli/src/run.rs`, `skills/ns-review/**`\n";
        assert_eq!(
            touches(text),
            globs(&["cli/src/run.rs", "skills/ns-review/**"])
        );
    }

    #[test]
    fn comment_line_without_emphasis() {
        assert_eq!(touches("Touches: `a.rs`\n"), globs(&["a.rs"]));
    }

    #[test]
    fn absent() {
        assert!(touches("## Agent Brief\n**Summary:** x\n").is_empty());
        assert!(touches("---\nstatus: pass\n---\n## Agent Brief\n").is_empty());
        assert!(touches("").is_empty());
    }

    #[test]
    fn empty_list() {
        assert!(touches("---\ntouches: []\n---\n").is_empty());
        assert!(touches("---\ntouches:\n---\n").is_empty());
        assert!(touches("**Touches:**\n").is_empty());
    }

    #[test]
    fn malformed_frontmatter_value() {
        assert!(touches("---\ntouches: cli/src/run.rs\n---\n").is_empty());
        assert!(touches("---\ntouches: {a: b}\n---\n").is_empty());
        assert!(touches("---\ntouches: [a.rs, 3]\n---\n").is_empty());
        assert!(touches("---\ntouches: [a.rs, '']\n---\n").is_empty());
        assert!(touches("---\ntouches: [a.rs\n---\n**Touches:** `b.rs`\n").is_empty());
    }

    #[test]
    fn malformed_comment_line() {
        assert!(touches("**Touches:** cli/src/run.rs\n").is_empty());
        assert!(touches("**Touches:** `a.rs`, b.rs\n").is_empty());
        assert!(touches("**Touches:** `a.rs`, ``\n").is_empty());
        assert!(touches("**Touches:** `a.rs` and `b.rs`\n").is_empty());
    }
}
