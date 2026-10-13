# `ns lint`

## `ns lint [path] [--human]`

Checks the skills under `path` (default `./skills`). Under `skills/`, a directory is a skill if it has a `SKILL.md` or an `ns-` name, and others are skipped. Under a repo-local root such as `.agents/skills` (the canonicalized root's parent is named `.agents`), every non-hidden directory is a skill, and one without `SKILL.md` reports `missing SKILL.md`. For each skill:

- the frontmatter parses as YAML
- `name` equals the directory name and starts with `ns-` (the prefix is not required under a repo-local root)
- `description` is non-empty and at most 1024 characters
- each `metadata.upstream` entry (a string or a list) matches `owner/repo@<7-40 hex>:path`
- every relative link (`](x)`, `](./x)`, `](../x)`) in any `.md` file of the skill points to an existing path. URLs with a scheme, absolute paths and bare `#anchor` links are skipped, anchors are stripped, and fenced code blocks and inline code spans are skipped.
- each `features/*.md` file (except `features/README.md`) has exactly four H2 headings in order: `Sub-features`, `How to get to it (user POV)`, `Driving it with <tool>`, `Gotchas`. A mismatch reports `features/x.md: H2 headings must be, in order: ...; found: ...`.
- every `.md` file under the skill's `references/` is linked from some `.md` file in the skill. A link to a directory counts as a link to its `README.md`.

Prints `{"ok","skills","errors":[{"skill","file","message"}]}` and exits 1 if there are errors. `--human` prints one line per error.
