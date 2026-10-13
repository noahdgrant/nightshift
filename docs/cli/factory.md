# `ns factory`

## `ns factory validate [--factory <dir>]`

Parses `<dir>/nightshift.toml` (default `<main-worktree>/.nightshift/`) and checks gates, billing, merge policy and globs, phase names, harness names (user config or built in) and each `agents/<role>/agent.md` (frontmatter `role` matches the directory, placeholders are known). Prints `{"ok","path","name","phases","errors","warnings"}` with every phase's resolved skill, harness, model, timeout, attempts and prompt source. `warnings` lists keys that load but do nothing, such as the removed `[queue] triage_per_night`; they leave `ok` true. Exits 1 on any error.
