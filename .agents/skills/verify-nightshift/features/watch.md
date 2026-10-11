# Watch

`ns watch` lists open issues with the ready label, drops blocked ones and ones an open PR already closes, sorts by priority and type, and runs each as a child `ns run --issue`, moving labels from ready to in-progress to done or stuck.

## Sub-features

- `watch-dry` prints the ordered queue with skip reasons and changes nothing.
- `watch-once` takes one unit, runs it and moves its labels.
- `watch-order` sorts by `[queue] priority`, then `[queue] order`.

## How to get to it (user POV)

- `ns watch --dry-run`
- `ns watch --once`
- `ns watch --until <HH:MM> [--max-units <n>] [--parallel <n>]`

## Driving it with ns

Preconditions:

- Baseline, in `$V/repo`, with `.nightshift/nightshift.toml` holding `name = "demo"`.
- No `7-fix-the-thing` worktree yet (a [run](./run.md) or [worktree](./worktree.md) recipe makes one; use a fresh sandbox).
- Issue #7 seeded as in the skill's Drive section, labelled `status:ready-for-agent`.

- **Dry run.** Run `cap watch-dry "$NS" watch --dry-run --no-self-update`. `queue` holds `{"number":7,...}` and `skipped` is `[]`. `$V/ghd/labels-7` is unchanged and `$V/ghd/calls` holds only reads (`api`, `pr list`).
- **Once.** Run `cap watch-once "$NS" watch --once --no-self-update`. Exit `0`; `units` holds `7-fix-the-thing` with outcome `done`. `$V/ghd/labels-7` is `status:in-review`, and `grep 'issue edit' $V/ghd/calls` shows ready to in-progress, then in-progress to in-review.
- **Order.** Seed a second ready issue #8 with label `priority:high` (the issue line's `labels` and `labels-8` both carry it), set `priority = ["priority:high"]` under `[queue]`, and run the dry run again. #8 comes before #7.

## Gotchas

- Always pass `--no-self-update` or `--once`/`--dry-run`. Without `--once` it loops until `--until`; start that with `setsid` and kill the group, per the skill's Cleanup.
- An issue line in `issues.jsonl` needs a matching `labels-<n>` file, or the fake `gh` lists it with stale labels.
- Without `[queue] source`, `origin`'s host picks the forge; the local bare remote means `github`.
