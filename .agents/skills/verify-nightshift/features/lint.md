# Lint

`ns lint` checks every `ns-*/SKILL.md` under a skills directory: frontmatter parses, `name` matches the directory and starts with `ns-`, `description` is set, `metadata.upstream` is well formed, and relative links resolve.

## Sub-features

- `lint-clean` passes on this repo's `skills/`.
- `lint-errors` reports a bad name, a missing description and a broken link, and exits `1`.
- `lint-human` prints one line per error instead of JSON.

## How to get to it (user POV)

- `ns lint [PATH] [--human]`

## Driving it with ns

Preconditions:

- Baseline, in `$NS_SRC`.

- **Clean.** Run `cap lint-clean "$NS" lint skills`. Exit `0`; `ok` is `true`, `errors` is `[]`, `skills` counts every `ns-*` directory.
- **Errors.** Make a broken skill: `mkdir -p $V/sk/ns-x && printf -- '---\nname: ns-y\n---\n[a](./missing.md)\n' > $V/sk/ns-x/SKILL.md`. Run `cap lint-errors "$NS" lint $V/sk --human`. Exit `1` and three lines: `name "ns-y" does not match directory "ns-x"`, `missing description`, `broken link: ./missing.md`.

## Gotchas

- Only `ns-*` directories are linted. `verify-nightshift` and anything else under `.agents/skills/` is skipped, so `ns lint .agents/skills` proves nothing about it.
- Links inside fenced code blocks are not checked.
