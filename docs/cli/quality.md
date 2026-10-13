# `ns quality`

## `ns quality [--since <date>] [--json]`, `ns quality import <dir> [--dry-run]`

Reads the quality records on `origin`'s `nightshift/quality` branch (fetched first) and in the outbox. For a unit with no record, it reads the review artifacts in its linked worktree (`.ns/<unit>/review.md`, `review/cycle-*.md`, `history/review*`). It adds the run log and prints JSON: first-pass yield, first-pass Critical and Important findings per 100 changed lines (overall and by axis), cycles to clean, leftovers, escapes blamed to the commit and PR that introduced the line, and a trend by local day. `--since` keeps units whose newest review artifact was updated on or after that local date. `gaps` counts artifacts it couldn't parse and findings missing a field. It changes nothing but the remote-tracking ref. `ns run` writes a unit's records when it ends, and `ns quality import` writes them for archived worktrees. The metrics and their targets are in `docs/factory/quality.md`.

`ns quality --bugs [--since <date>] [--until <date>] [--weeks N] [--json]` counts the `type:fix` issues opened in each ISO week (UTC), by source and severity, read from the `[queue] source` forge. It prints a table, or with `--json` one object:

- `ok`, `label` (`type:fix`), `since` and `until` (UTC, null when unset; `--weeks` sets `since`).
- `weeks`: one entry per ISO week, oldest first, each `{week, monday, opened, review_escape, factory_introduced, ci_main, human, critical_high, rest}`. `week` is `2026-W41` and `monday` its first day.
- `total`: the same counts over every week.
- `issues`: each counted issue, `{number, title, opened, week, source, by, severity}`, plus `introduced_by` (`{commit, summary, pr}`) when blamed and `blame_error` when its blame failed. `source` is `review_escape`, `factory_introduced`, `ci_main` or `human`, `by` is `marker`, `review_text`, `ci_title`, `blame` or `default`, and `severity` is `critical_high` or `rest`.
- `excluded`: the issues left out, closed as a duplicate or not planned, each `{number, title, week}`.

How each source is decided is in `docs/factory/quality.md` (Bug counts).
