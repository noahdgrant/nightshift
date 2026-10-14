# Lint

`ns lint` checks every skill under a skills directory. Under `skills/`, a directory is a skill if it has a `SKILL.md` or an `ns-` name, and others are skipped. Under a repo-local root (parent `.agents`), every non-hidden directory is a skill. For each skill: frontmatter parses, `name` matches the directory and starts with `ns-` (not required under `.agents/skills/`), `description` is set, `metadata.upstream` is well formed, relative links resolve, and each `features/*.md` has the four H2 sections in order.

## Sub-features

- `lint-clean` passes on this repo's `skills/`.
- `lint-errors` reports a bad name, a missing description and a broken link, and exits `1`.
- `lint-human` prints one line per error instead of JSON.
- `lint-local` passes on this repo's `.agents/skills/`, whose `verify-nightshift` has no `ns-` prefix.

## How to get to it (user POV)

- `ns lint [PATH] [--human]`

## Driving it with ns

Preconditions:

- Baseline. These recipes act on this repo's skills, so they run with `--src`.

- **Clean.** Exit `0`; `ok` is `true`, `errors` is `[]`, `skills` counts every `ns-*` directory.

  ```bash
  "$V/sandbox" --src cap lint-clean ns lint skills
  ```

- **Errors.** Make a broken skill under `$V/sk`, then lint it. Exit `1`; `ok` is `false` and `errors` holds three entries for the bad name, the missing description and the broken link.

  ```bash
  "$V/sandbox" sh -c 'mkdir -p "$V/sk/ns-x" && printf -- "$1" > "$V/sk/ns-x/SKILL.md"' _ '---\nname: ns-y\n---\n[a](./missing.md)\n'
  "$V/sandbox" cap lint-errors ns lint "$V/sk"
  ```

- **Local.** Exit `0`; `ok` is `true` and `skills` is at least `1`.

  ```bash
  "$V/sandbox" --src cap lint-local ns lint .agents/skills
  ```

- **Human.** Exit `1` and three lines: `name "ns-y" does not match directory "ns-x"`, `missing description`, `broken link: ./missing.md`.

  ```bash
  "$V/sandbox" cap lint-human ns lint "$V/sk" --human
  ```

## Gotchas

- The `ns-` prefix exemption comes from the lint root's parent being named `.agents`. The same skill under any other root, `skills/` included, fails the prefix rule.
- Under `skills/`, a directory with neither an `ns-` name nor a `SKILL.md` is skipped, not reported. Under a repo-local root, every non-hidden directory is a skill, and one without `SKILL.md` reports `missing SKILL.md`.
- The root is judged by its canonicalized (symlink-resolved) path, so a symlink named `skills` that points under `.agents/` is exempt, and an `.agents/skills` symlink to a directory elsewhere is not.
- Links inside fenced code blocks are not checked.
