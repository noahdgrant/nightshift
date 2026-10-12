# `ns quality`

## `ns quality [--since <date>] [--json]`, `ns quality import <dir> [--dry-run]`

Reads the quality records on `origin`'s `nightshift/quality` branch (fetched first) and in the outbox. For a unit with no record, it reads the review artifacts in its linked worktree (`.ns/<unit>/review.md`, `review/cycle-*.md`, `history/review*`). It adds the run log and prints JSON: first-pass yield, first-pass Critical and Important findings per 100 changed lines (overall and by axis), cycles to clean, leftovers, escapes blamed to the commit and PR that introduced the line, and a trend by local day. `--since` keeps units whose newest review artifact was updated on or after that local date. `gaps` counts artifacts it couldn't parse and findings missing a field. It changes nothing but the remote-tracking ref. `ns run` writes a unit's records when it ends, and `ns quality import` writes them for archived worktrees. The metrics and their targets are in `docs/factory/quality.md`.
