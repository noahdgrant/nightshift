//! Read the `touches` prediction from an Agent Brief: the path globs the unit
//! will probably change. `brief.md` carries it as a frontmatter list; the brief
//! comment on the issue carries it as one `**Touches:**` line of backticked globs. Two units
//! overlap when their globs match a common file.

use crate::frontmatter;

const LINE_KEY: &str = "Touches:";

/// The brief's `touches` globs, from `brief.md` frontmatter or the brief comment
/// text. Empty when the brief makes no prediction or the value is malformed.
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
    let mut rest = rest.trim_start_matches(['*', '_']).trim();
    let mut globs = Vec::new();
    loop {
        let Some(open) = rest.strip_prefix('`') else {
            return Vec::new();
        };
        let Some((glob, after)) = open.split_once('`') else {
            return Vec::new();
        };
        if glob.trim().is_empty() {
            return Vec::new();
        }
        globs.push(glob.to_string());
        rest = after.trim();
        if rest.is_empty() {
            return globs;
        }
        let Some(next) = rest.strip_prefix(',') else {
            return Vec::new();
        };
        rest = next.trim();
    }
}

const MATCH: glob::MatchOptions = glob::MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

/// Whether two units' `touches` overlap: a glob from each matches one file, either a file in
/// `files` (the repo's tracked files) or a path a glob names outright, which may not exist yet.
/// A file an `ignore` glob matches never counts.
pub struct Overlap {
    files: Vec<String>,
    ignore: Vec<glob::Pattern>,
}

impl Overlap {
    pub fn new(files: Vec<String>, ignore: &[String]) -> Overlap {
        Overlap {
            files,
            ignore: patterns(ignore),
        }
    }

    pub fn overlaps(&self, a: &[String], b: &[String]) -> bool {
        let (pa, pb) = (patterns(a), patterns(b));
        let named = a
            .iter()
            .chain(b)
            .filter(|g| glob::Pattern::escape(g) == **g);
        self.files
            .iter()
            .chain(named)
            .filter(|f| !matches(&self.ignore, f))
            .any(|f| matches(&pa, f) && matches(&pb, f))
    }
}

/// A glob that doesn't parse matches nothing.
fn patterns(globs: &[String]) -> Vec<glob::Pattern> {
    globs
        .iter()
        .filter_map(|g| glob::Pattern::new(g).ok())
        .collect()
}

fn matches(pats: &[glob::Pattern], file: &str) -> bool {
    pats.iter().any(|p| p.matches_with(file, MATCH))
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
        assert!(touches("---\ntouches: [a.rs, ' ']\n---\n").is_empty());
        assert!(touches("---\ntouches: [a.rs\n---\n**Touches:** `b.rs`\n").is_empty());
    }

    #[test]
    fn malformed_comment_line() {
        assert!(touches("**Touches:** cli/src/run.rs\n").is_empty());
        assert!(touches("**Touches:** `a.rs`, b.rs\n").is_empty());
        assert!(touches("**Touches:** `a.rs`, ``\n").is_empty());
        assert!(touches("**Touches:** `a.rs` and `b.rs`\n").is_empty());
        assert!(touches("**Touches:** `a.rs`, ` `\n").is_empty());
        assert!(touches("**Touches:** ` `\n").is_empty());
        assert!(touches("**Touches:** `a.rs` junk\n").is_empty());
        assert!(touches("**Touches:** `a.rs`,\n").is_empty());
    }

    #[test]
    fn comment_line_keeps_commas_inside_a_glob() {
        assert_eq!(
            touches("**Touches:** `cli/src/{run,watch}.rs`, `docs/**`\n"),
            globs(&["cli/src/{run,watch}.rs", "docs/**"])
        );
    }

    fn overlap(files: &[&str], ignore: &[&str]) -> Overlap {
        Overlap::new(globs(files), &globs(ignore))
    }

    #[test]
    fn globs_matching_one_tracked_file_overlap() {
        let ov = overlap(
            &["cli/src/run.rs", "cli/src/watch.rs", "docs/FACTORY.md"],
            &[],
        );
        assert!(ov.overlaps(&globs(&["cli/src/*.rs"]), &globs(&["cli/src/run.rs"])));
        assert!(ov.overlaps(&globs(&["cli/**"]), &globs(&["cli/src/watch.rs"])));
        assert!(!ov.overlaps(&globs(&["cli/src/run.rs"]), &globs(&["cli/src/watch.rs"])));
        assert!(!ov.overlaps(&globs(&["docs/**"]), &globs(&["cli/**"])));
    }

    #[test]
    fn globs_meeting_on_no_file_do_not_overlap() {
        let ov = overlap(&["cli/src/run.rs"], &[]);
        assert!(!ov.overlaps(&globs(&["cli/src/new/*.rs"]), &globs(&["cli/src/new/**"])));
        // `*` stops at a separator.
        assert!(!ov.overlaps(&globs(&["cli/*"]), &globs(&["cli/src/run.rs"])));
    }

    #[test]
    fn a_path_named_outright_counts_before_it_exists() {
        let ov = overlap(&[], &[]);
        assert!(ov.overlaps(&globs(&["cli/src/new.rs"]), &globs(&["cli/src/*.rs"])));
        assert!(ov.overlaps(&globs(&["cli/src/*.rs"]), &globs(&["cli/src/new.rs"])));
        assert!(!ov.overlaps(&globs(&["cli/src/new.rs"]), &globs(&["cli/src/old.rs"])));
    }

    #[test]
    fn ignored_files_never_overlap() {
        let ov = overlap(&["docs/DESIGN.md", "cli/src/run.rs"], &["docs/DESIGN.md"]);
        assert!(!ov.overlaps(&globs(&["docs/DESIGN.md"]), &globs(&["docs/**"])));
        assert!(ov.overlaps(
            &globs(&["docs/DESIGN.md", "cli/src/run.rs"]),
            &globs(&["docs/**", "cli/**"])
        ));
    }

    #[test]
    fn no_touches_never_overlap() {
        let ov = overlap(&["cli/src/run.rs"], &[]);
        assert!(!ov.overlaps(&[], &globs(&["**"])));
        assert!(!ov.overlaps(&globs(&["**"]), &[]));
    }

    #[test]
    fn comment_line_may_be_indented() {
        assert_eq!(touches("  **Touches:** `a.rs`\n"), globs(&["a.rs"]));
        assert_eq!(touches("\t**Touches:** `a.rs`\n"), globs(&["a.rs"]));
    }
}
