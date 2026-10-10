//! Readers for the tables in `docs/agents/stack.md`.

/// What follows the first cell of each table row whose first cell is `task` (backticks optional).
pub fn rows<'a>(stack_md: &'a str, task: &'a str) -> impl Iterator<Item = &'a str> {
    stack_md.lines().filter_map(move |line| {
        let (first, rest) = line.trim().strip_prefix('|')?.split_once('|')?;
        (first.trim().trim_matches('`') == task).then_some(rest)
    })
}
