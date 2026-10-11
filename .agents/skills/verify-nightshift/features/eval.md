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

- Baseline, in `$NS_SRC`.

- **All skills.** Run `cap eval-all "$NS" eval --dry-run`. Exit `0`; `dry_run` is `true` and `skills` lists every skill with an `evals/` directory.
- **One skill.** Run `cap eval-tdd "$NS" eval ns-tdd --dry-run`. `skills` holds only `ns-tdd`, with its cases, and `estimated_runs` is set.
- **One case.** Run `cap eval-case "$NS" eval ns-tdd --case py-capacity-off-by-one --dry-run`. Only that case is planned.
- **Unknown.** Run `cap eval-unknown "$NS" eval ns-nope --dry-run`. Exit `2`.
- **No harness.** Check `$FAKE_CTRL/calls` is empty or unchanged. A dry run never calls the harness.

## Gotchas

- Never drop `--dry-run`. A real eval spends the plan's usage, even with the fake `claude` first on PATH, because a harness override in config can name another binary.
- `--dry-run` reads fixtures from `<skills-dir>/../evals/fixtures`, so run it from `$NS_SRC`.
