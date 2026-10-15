//! Readers for the tables in `docs/agents/stack.md`.

/// What follows the first cell of each table row whose first cell is `task` (backticks optional).
pub fn rows<'a>(stack_md: &'a str, task: &'a str) -> impl Iterator<Item = &'a str> {
    stack_md.lines().filter_map(move |line| {
        let (first, rest) = line.trim().strip_prefix('|')?.split_once('|')?;
        (first.trim().trim_matches('`') == task).then_some(rest)
    })
}

/// The backticked spans that open the second cell of the first row whose first cell is `task`
/// and that has any: `| rules | `<cmd>`, `<cmd>` | ... |`. Spans are separated by commas or
/// spaces, and a span may contain `|`.
pub fn spans(stack_md: &str, task: &str) -> Vec<String> {
    rows(stack_md, task)
        .map(|mut rest| {
            let mut spans = Vec::new();
            while let Some(span) = rest
                .trim_start_matches(|c: char| c == ',' || c.is_whitespace())
                .strip_prefix('`')
            {
                let Some((span, after)) = span.split_once('`') else {
                    break;
                };
                if !span.trim().is_empty() {
                    spans.push(span.trim().to_string());
                }
                rest = after;
            }
            spans
        })
        .find(|spans| !spans.is_empty())
        .unwrap_or_default()
}
