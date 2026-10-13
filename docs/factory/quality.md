# Quality

```
ns quality [--since YYYY-MM-DDTHH:MM:SSZ|YYYY-MM-DD] [--until YYYY-MM-DDTHH:MM:SSZ|YYYY-MM-DD] [--json]
ns quality import <dir> [--dry-run]
ns quality --bugs [--since ...] [--until ...] [--weeks N] [--json]
```

`ns quality` measures whether build writes code that passes review. It reads each unit's quality record first, and for a unit with no record, its review artifacts in the linked worktrees (`.ns/<unit>/review.md`, `review/cycle-*.md`, and archived `history/review*`). It adds the run log and prints JSON. The main checkout is skipped. First-pass numbers come from a unit's oldest review artifact, since a rebuilt unit's later review starts from a different diff. Everything else comes from the newest.

| Metric | What it counts | Target |
|---|---|---|
| First-pass yield | Units whose first review pass raised no Critical or Important finding in changed code, over units where that is known | Rising |
| Findings per 100 lines | Critical and Important findings in changed code from the first pass, per 100 changed lines, overall and per axis. A finding with two axes counts once in each | Falling |
| Cycles to clean | Fix cycles a unit needed to end `pass` with no open Critical or Important in changed code (median, max). Units that never got there are `not_clean` | Falling |
| Leftovers | Critical and Important findings in changed code still `open` when the fix loop stops, after the third cycle or the fix-only cycles that follow it | 0 |
| Escapes | Findings with `Scope: pre-existing` (D29): defects in code the unit didn't change, so an earlier unit's review let them through. Each is blamed (`git blame` at the reviewed commit, the frontmatter's `sha:`, else `head:`, else `base:`) to the commit that introduced the line, and to the PR in its subject (`(#123)`) | Falling |

An escape counts only as an escape: it never makes a unit's first pass dirty, never counts toward findings per 100 lines, and never blocks. A finding dismissed in review counts toward none of these: review judged it wrong. `trend` repeats the numbers for each local day (the newest artifact's `updated:`), `--since` keeps only units whose newest artifact was updated, and run-log events, at or after a UTC instant, and `--until` only those at or before one. A bare date means 00:00:00 UTC for `--since` and 23:59:59 UTC for `--until`, whatever `TZ` is, so `--until 2026-10-09` and `--since 2026-10-10` split units and events with no overlap and no gap. With either bound set, an undated unit or event is dropped. A malformed bound, or `--until` before `--since`, exits 2. The output echoes both as `since` and `until`, null when unset. `per_unit` has each unit's numbers, with `changed_lines`, which keeps the review cost of a large unit visible now that size never blocks one (D37), `reached_clean` (`clean`, `not_clean` or `unknown`) and its review runs, review cost and outcome from the run log. `run_log.units_without_artifacts` names units the log shows reviewed that have neither a record nor a worktree.

The numbers need the finding fields `ns-review` writes: `Axis`, `Scope`, `Cycle` and `Status`. Older artifacts lack some of them, so `ns quality` falls back:

- No `Axis`: the axes named in `Raised by`.
- No `Cycle`: a finding is first-pass when the attempt ran 0 fix cycles, or when it was `fixed (cycle 1, ...)`. Otherwise the first pass's cycle file (`cycle-0.md` or `cycle-1.md`) is its record, counted from the ids it lists and the axes on their lines; its ids are not matched against `review.md`, because old artifacts reuse ids across passes. With neither, a unit with fix cycles still counts as dirty, because fix cycles only run for an open Critical or Important, but it is left out of the per-100-lines numbers.
- No `Scope`: not an escape.
- `cycles:` of 1 or more with no Critical or Important `fixed`: cycles to clean is `unknown`, because older artifacts counted review passes there.
- No `Change size:` in the Summary: `git diff --shortstat <base>...<head> -- . ':!.ns'` from the frontmatter shas.
- A heading such as `### I1-I7 (cycle 1). ...` is seven findings sharing its fields. One-line `- S1. ... (axis). Open.` items inside a severity section are findings too.
- An id prefix other than `C`, `I` or `S`, such as `### E1.` under an `## Escapes` section or `### D1.` under `## Dismissed`, is still a finding. Its severity comes from a `raised as Important` note in its fields, else it is unknown: counted in `findings.unknown_severity`, never blocking. Its `Scope: pre-existing` still makes it an escape, whatever its section.
- A first attempt that ended without `pass` and lists no findings (a reviewer slice couldn't run, a timeout) has an unknown first pass, not a clean one.

These are heuristics. A finding's severity comes from its `C`, `I` or `S` id prefix, so an `I5` noted as downgraded to a Suggestion still counts as Important, and a first-pass cycle file's ids all count, including any it lists as dismissed. An archived attempt's cycle files are read from `history/<stem>/` when present; `ns run` archives only `review.md`, so usually only the newest attempt has them.

`gaps` reports what was missing: findings without a severity or each field, headings with a `Status:` that didn't parse as findings (`### C3-1.`), units whose first pass, first-pass count or change size is unknown, and artifacts that couldn't be read (no frontmatter, or cycle files with no review artifact). Read the numbers alongside them.

## Where the history lives

Worktrees get removed once their change merges (see ["Cleanup"](unit-lifecycle.md#cleanup)), so the history lives in the repo, on an orphan branch `nightshift/quality` on `origin` (D39). It holds one file, `records.jsonl`, with one JSON line per review attempt. When a unit ends `merged`, `done` or `stuck`, `ns run` writes the unit's records and pushes them. Each record has the unit, issue, PR, outcome, the attempt's status, `updated:`, local day, fix cycles and change size, each finding's id, severity, axes, scope, cycle, fix attempts, status and relative location, and the review's passes. A finding also keeps the raisers whose version the lead dismissed (`dismissed_raises`), and an escape the axis that missed it (`missed_by`). `passes` holds each pass's panel and, per reviewer worker, its role, provider, tokens, `wall_s` and `cost_usd`, a cost left out when the harness reported none. Records from before these fields still read: an escape there is unattributed and its passes unknown. Finding titles are left out. Paths are relative, and a record holding an absolute path is never written. A location with one is dropped.

`ns run` fetches the branch, appends on its tip and pushes. If origin rejects the push because another writer pushed first, it fetches and tries again, up to 3 more times. Any other failure (no `origin`, no access, a hook that declines the push, the network) goes straight to the outbox, as does a fourth lost race. The records wait in `<git-common-dir>/ns/quality-outbox.jsonl`, and the next `ns run` or `ns quality import` pushes them. A unit never fails because its record didn't push. The branch is data: `ns` writes it unattended, outside `human_review` and the PR flow. Commits are built with git plumbing, so writing one never touches a working tree, the index or a local branch. These git calls never prompt: credential prompts are off, ssh runs in batch mode with connect and keepalive timeouts unless you set your own ssh command, and an HTTP transfer under 1000 bytes a second for 60 seconds fails. That matters because a write holds a lock on the outbox while it talks to `origin`.

`ns quality` fetches the branch and reads `records.jsonl` plus the outbox. A unit written more than once (stuck, then rerun) counts from its newest write, the latest `recorded` time. A unit's record wins over its worktree, and a unit with no record falls back to its worktree. So the numbers stay the same after `ns worktree remove`. `per_unit.source` says which one each unit came from (`record` or `worktree`), and `records` says how many records came from the branch and the outbox, and why a fetch failed. An escape is blamed when its record is written, while the reviewed commit still exists, and the record keeps the commit and PR (`introduced_by`). An older record without one is blamed in the main checkout at the recorded commit. An escape from a record has no title.

To read the trend, run `ns quality` from any clone and look at `trend`: one summary per local day, oldest first. First-pass yield should rise and findings per 100 lines should fall. To see the raw records, run `git show origin/nightshift/quality:records.jsonl`.

`ns quality import <dir>` turns archived unit worktrees into records and pushes them in one commit. `<dir>` holds one directory per unit, laid out as in its worktree (`<dir>/<unit>/.ns/<unit>/review.md`). It skips units the branch or the outbox already has, so a rerun reports `already-done`. Issue numbers come from the unit id, PRs from `pr.md` or the run log, and outcomes from the run log. `--dry-run` fetches the branch to see what is recorded, and pushes nothing.

## Bug counts

`ns quality --bugs` counts the bugs that reach the tracker: the `type:fix` issues opened in each ISO week (UTC), in any state, read from the `[queue] source` forge. An issue counts in the week it was opened. One closed as a duplicate or not planned is left out and listed under `excluded`. GitLab records no close reason, so there every closed issue counts. Weeks run from `--since`, else the oldest issue's week, through `--until`'s week, else this one, and a week with no issues counts zero. `--weeks N` keeps the last N weeks, this one included, or starts at `--since` when that is later. `--weeks 0`, or an `--until` before the weeks start, exits 2.

Each issue gets one source, and `by` says what decided it, first match wins:

| Source | Decided by (`by`) |
|---|---|
| The one a `Source:` line names (`review_escape`, `factory_introduced`, `ci_main` or `human`) | `marker` |
| `review_escape`: `ns-review` filed it for pre-existing code; its body has "This escaped an earlier review." | `review_text` |
| `ci_main`: `ns watch` filed it for a red `main`; its title starts `fix: main is red at` | `ci_title` |
| `factory_introduced`: `git blame` of the first `path:line` in its body, on the default branch as it stood when the issue opened, names a PR (`(#123)` in the subject) that merged from an `ns/` branch | `blame` |
| `human`: anything else | `default` |

The marker is one line in the body, `Source: review_escape`, and wins over every guess. A phase that files a fix issue should write it. Merged PRs come from the forge's 200 most recent (100 on GitLab). A blame that fails leaves the issue `human` and says why in `blame_error`, and a blame on a PR outside the factory keeps its `introduced_by`. Severity is `critical_high` for an issue labelled `priority:critical` or `priority:high`, and `rest` for any other priority or none.

Each week also gets two rates: its fix issues per merged unit, and per 1,000 changed lines merged. A merged unit is a PR merged that week from an `ns/` branch, and its changed lines are the PR's additions plus deletions. Both come from the forge's PRs merged since the first week's Monday, up to 200 (100 on GitLab), and count only PRs merged inside `--since` and `--until`. When that list is full, the forge dropped the oldest PRs, so a week starting before the oldest merge it lists is marked `undercounted` and its rate may be too high; later weeks are exact. The forge lists by creation time, so a PR created long before it merged can still be missing from a later week; the bound on merge time keeps that to long-lived PRs. GitLab's list gives no line counts, so there the rate per 1,000 lines is null. A week with no merged unit, or no changed lines, has a null rate.

Each rate has a trend over the weeks shown: the least-squares slope of the rate against the week, fitted on the weeks with a rate that are not undercounted. Fewer than 4 such weeks is `insufficient`. A slope within 5% of the mean rate, per week, is `flat`; otherwise it is `rising` or `falling`.

Without `--json` it prints a table, one row per week and a total, then a table of the rates and a line per trend. With `--json` it prints `{ok, label, since, until, weeks, total, trend, merged_prs, issues, excluded}`; `docs/cli/quality.md` lists the keys.
