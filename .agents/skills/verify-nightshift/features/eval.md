# Eval

`ns eval --dry-run` parses a skill's eval cases and prints the plan (harness, arms, trials, estimated runs) as JSON without starting a harness. A real `ns eval` runs model trials and is out of bounds for verification.

## Sub-features

- `eval-plan-all` plans every skill with an `evals/` directory.
- `eval-plan-one` plans one skill, optionally one case.
- `eval-unknown` rejects an unknown skill with exit `2`.

## How to get to it (user POV)

- `ns eval [SKILLS]... [--case <id>]... --dry-run`

## Driving it with ns

Preconditions:

- Baseline. `ns eval --dry-run` reads this repo's skills and fixtures, so it runs with `--src`.

- **All skills.** Exit `0`; `dry_run` is `true` and `skills` lists every skill with an `evals/` directory.

  ```bash
  "$V/sandbox" --src cap eval-plan-all ns eval --dry-run
  ```

- **One skill.** `skills` holds only `ns-tdd`, with its cases, and `estimated_runs` is set.

  ```bash
  "$V/sandbox" --src cap eval-plan-one ns eval ns-tdd --dry-run
  ```

- **One case.** Only that case is planned.

  ```bash
  "$V/sandbox" --src cap eval-plan-one-case ns eval ns-tdd --case py-capacity-off-by-one --dry-run
  ```

- **Unknown.** Exit `2`.

  ```bash
  "$V/sandbox" --src cap eval-unknown ns eval ns-nope --dry-run
  ```

- **No harness.** `eval-no-harness` exits non-zero, or lists no phase these recipes added: a dry run never calls the harness.

  ```bash
  "$V/sandbox" cap eval-no-harness cat "$V/ctrl/calls"
  ```

## Gotchas

- Never drop `--dry-run`. A real eval spends the plan's usage, even with the fake `claude` first on PATH, because a harness override in config can name another binary.
- `--dry-run` reads fixtures from `<skills-dir>/../evals/fixtures`, so run it with `--src`.
