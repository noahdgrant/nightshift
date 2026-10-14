# `ns quality`

## `ns quality [--since <date>] [--until <date>] [--json|--human|--reviewers]`, `ns quality import <dir> [--dry-run]`

Reads the quality records on `origin`'s `nightshift/quality` branch (fetched first) and in the outbox. For a unit with no record, it reads the review artifacts in its linked worktree (`.ns/<unit>/review.md`, `review/cycle-*.md`, `history/review*`). It adds the run log and prints JSON: first-pass yield, first-pass Critical and Important findings per 100 changed lines (overall and by axis), cycles to clean, leftovers, escapes blamed to the commit and PR that introduced the line, and a trend by local day. `--since` keeps units whose newest review artifact was updated on or after that local date. `gaps` counts artifacts it couldn't parse and findings missing a field. It changes nothing but the remote-tracking ref. `ns run` writes a unit's records when it ends, and `ns quality import` writes them for archived worktrees. The metrics and their targets are in `docs/factory/quality.md`. `--human` prints the headline numbers and `finding_flow` as text instead, with `n/a` for a rate whose denominator is zero. It ends with a `Gaps:` line (unparsed artifacts, findings missing a field) and, when the fetch of the records branch failed, a `Records: fetch failed` line first, since the numbers then come from the records as last fetched.

`finding_flow` says how findings arrive and get fixed. It counts the Critical and Important findings of every review attempt of the units in range, not only the newest, since each attempt is its own review loop. A finding with no `Cycle:` (records from before the field) is left out of every number in it. Each rate is rounded to 3 places and is null when its denominator is zero. Its keys:

- `findings`: the findings counted, those with a known cycle. `without_cycle`: those left out for lack of one.
- `raised_per_pass`: `{critical, important}`, each `{passes, later}`. `passes[n]` counts the findings first raised in pass `n`: 0 is the first review, 1 to 5 the fix cycles. `later` counts those first raised after pass 5.
- `fix_rate`: `{critical, important}`, each over the findings still `open` and those `fixed` with a known attempt count (`Attempts:`). Dismissed and deferred findings are not in it, and a fix with no attempt count, or `Attempts: 0`, is counted in `fixed_attempts_unknown` and left out. The counts are `findings`, `fixed_first_attempt`, `fixed_second_attempt`, `fixed_later_attempt` (3 or more) and `open`. `first_attempt_rate` is `fixed_first_attempt / findings`, `second_attempt_rate` is `fixed_second_attempt` over the rest (`findings - fixed_first_attempt`), and `open_rate` is `open / findings`.
- `cycles_to_fix`: over fixed findings of both severities, the fix cycle minus the cycle that raised it. `{findings, median, max, next_cycle, left_out}`: `median` of an even count is the mean of the two middle values, `next_cycle` counts the findings fixed in the cycle after the one that raised them, and `left_out` the fixed findings with no fix cycle or one before the cycle that raised them.
- `dismiss_rate`: `{count, findings, rate}`, the findings dismissed out of `findings`.
- `late_finding_rate`: `{count, findings, rate}`, the findings first raised after pass 0 out of `findings`.

`ns quality --reviewers [--since <date>] [--until <date>] [--json]` reports each reviewer instead of the review report, as a table, or as JSON with `--json`. It reads the same units and window, and the newest record of every review attempt of each unit, not only the newest attempt, since each attempt is a separate review. A finding is raised when it is Critical or Important with `Scope: changed`. Its reviewers are its axes that name one of the eight reviewers, and only those count. The table has a row per reviewer for `critical`, `important` and `total`, a cost table and an overlap matrix, and ends with what it left out. The JSON keys:

- `ok`, `repo`, `since`, `until`, `records`: as for `ns quality`. `units` is how many units the window kept.
- `attempts`: their review attempts. `measured_attempts`: attempts with at least one pass that names its panel.
- `reviewers.<axis>`, for each of the eight axes:
  - `reviews`: passes whose panel ran this reviewer, over every attempt. A worker rerun in the same pass counts once.
  - `critical`, `important` and `total` (both severities), each with:
    - `raised`: findings this reviewer raised.
    - `unique` and `unique_fixed`: findings it alone raised, and those of them that ended `fixed`.
    - `dismissed` and `dismiss_rate`: raises the lead dismissed (the whole finding, or this reviewer's version under `Dismissed raises`), and that over `raised`.
    - `escapes`: escapes whose `missed_by` is this reviewer: findings with `Scope: pre-existing` that were not dismissed.
    - `raised_measured` and `unique_fixed_measured`: `raised` and `unique_fixed` counted in measured attempts only.
    - `per_review` and `unique_fixed_per_review`: those two over `reviews`.
    - `unique_fixed_per_usd`: `unique_fixed_measured` over `cost.cost_usd.total`, null unless every worker reported a cost.

    Rates are rounded to 3 places and null when their denominator is 0.
  - `cost`: `workers`, the reviewer's workers in passes with a panel, over every attempt. `tokens`, `wall_s` and `cost_usd` each have `reported` (how many workers reported the value), `median` and `total`. A value no worker reported is never counted as 0; `median` and `total` are null when `reported` is 0.
- `overlap`: one entry per pair of reviewers, `{reviewers: [a, b], shared, jaccard}`. `shared` counts raised findings both named. `jaccard` is `shared` over findings either raised, Critical and Important together, and null when neither raised any.
- `excluded`, what the numbers leave out:
  - `findings_without_axis`: raised findings that name no reviewer.
  - `findings_without_scope`: Critical or Important findings with no `Scope`, so neither raised nor an escape.
  - `attempts_without_passes`: attempts with no Passes table, such as records from before it. Their findings still count in `raised`.
  - `passes_without_panel`: passes with no panel. They are not counted in `reviews` or `cost`.
  - `escapes_unattributed`: Critical or Important escapes with no `missed_by`.
  - `unknown_reviewer`: workers and `missed_by` names that are none of the eight reviewers. They get no row, overlap pair or count.

`ns quality --bugs [--since <date>] [--until <date>] [--weeks N] [--json]` counts the `type:fix` issues opened in each ISO week (UTC), by source and severity, read from the `[queue] source` forge, with their rate per merged unit and per 1,000 changed lines and each rate's trend. It prints tables, or with `--json` one object:

- `ok`, `label` (`type:fix`), `since` and `until` (UTC, null when unset; `--weeks` sets `since`).
- `weeks`: one entry per ISO week, oldest first, each `{week, monday, opened, review_escape, factory_introduced, ci_main, human, critical_high, rest, merged_units, changed_lines, per_unit, per_kloc, undercounted}`. Each count is of bugs: an issue counts one, and a `review_escape` issue one per checklist item. `week` is `2026-W41` and `monday` its first day. `merged_units` counts the PRs merged that week from `ns/` branches and `changed_lines` their additions plus deletions (0 for a week with no unit; null when a merged unit has no line count, as on GitLab). `per_unit` is `opened` per merged unit and `per_kloc` per 1,000 changed lines, rounded to 3 places, each null when its divisor is zero or unknown. `undercounted` is true when the forge's cap (200 PRs, 100 on GitLab) cut the merged-PR list short and the week starts before `merged_prs.oldest`, so PRs merged in it may be missing. The forge lists by creation time, so a PR created long before it merged can be missing from a later week too; the list is read from the report's first week on, which keeps that to long-lived PRs.
- `total`: the same counts over every week.
- `trend`: `{per_unit, per_kloc}`, each `{direction, slope, weeks}`. `direction` is `rising`, `flat`, `falling` or `insufficient` (fewer than 4 weeks with a rate that are not undercounted), `slope` the least-squares change per week (null when insufficient), and `weeks` how many weeks it was fitted on.
- `merged_prs`: the merged-PR list the rates come from, `{listed, truncated, oldest}`; `oldest` is when its oldest PR merged, null when none is listed. The units and lines of a week count only PRs merged inside `--since` and `--until`.
- `issues`: each counted issue, `{number, title, opened, week, source, by, severity, bugs}`, plus `introduced_by` (`{commit, summary, pr}`) when blamed and `blame_error` when its blame failed. `source` is `review_escape`, `factory_introduced`, `ci_main` or `human`, `by` is `marker`, `review_text`, `ci_title`, `blame` or `default`, `severity` is `critical_high` or `rest`, and `bugs` is how many bugs the issue counts as.
- `excluded`: the issues left out, closed as a duplicate or not planned, each `{number, title, week}`.

How each source is decided is in `docs/factory/quality.md` (Bug counts).
