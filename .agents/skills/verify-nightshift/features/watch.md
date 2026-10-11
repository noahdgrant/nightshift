# Watch

`ns watch` lists open issues with the ready label, drops blocked ones and ones an open PR already closes, sorts by priority and type, and runs each as a child `ns run --issue`, moving labels from ready to in-progress to done or stuck.

## Sub-features

- `watch-dry` prints the ordered queue with skip reasons and changes nothing.
- `watch-once` takes one unit, runs it and moves its labels.
- `watch-order` sorts by `[queue] priority`, then `[queue] order`; captured as `watch-order-default` and `watch-order-priority`.

## How to get to it (user POV)

- `ns watch --dry-run`
- `ns watch --once`
- `ns watch --until <HH:MM> [--max-units <n>] [--parallel <n>]`

## Driving it with ns

Preconditions:

- Baseline, in `$V/repo`. Write the factory file: `mkdir -p .nightshift && printf 'name = "demo"\n' > .nightshift/nightshift.toml`.
- No `7-fix-the-thing` worktree yet (a [run](./run.md) or [worktree](./worktree.md) recipe makes one; use a fresh sandbox).
- Issue #7 seeded as in the skill's Drive section, labelled `status:ready-for-agent`.
- Recipes run in the order below: **Order** comes before **Once**, because after `watch-once` #7 is no longer queued, and it undoes its own seeding.

- **Dry run.** Run `cap watch-dry "$NS" watch --dry-run --no-self-update`. `queue` holds `{"number":7,...}` and `skipped` is `[]`. `$V/ghd/labels-7` is unchanged and `$V/ghd/calls` holds only reads (`api`, `pr list`).
- **Order.** `priority:high` is already in the default `[queue] priority`, so use a label the default does not rank. Seed #8 with `priority:urgent`: `echo '{"number":8,"title":"Second","labels":[{"name":"status:ready-for-agent"}],"body":"","authorAssociation":"MEMBER"}' >> $V/ghd/issues.jsonl`, `echo '{"number":8,"title":"Second","url":"https://github.com/o/r/issues/8","state":"OPEN"}' > $V/ghd/issue-8.json` and `printf 'status:ready-for-agent\npriority:urgent\n' > $V/ghd/labels-8`. Leave #7 with no priority label. Without config ties sort by number: `cap watch-order-default "$NS" watch --dry-run --no-self-update` gives `jq '[.queue[].number]'` of `[7,8]`. Then `printf 'name = "demo"\n[queue]\npriority = ["priority:urgent"]\n' > .nightshift/nightshift.toml` and `cap watch-order-priority "$NS" watch --dry-run --no-self-update` gives `[8,7]`. Then undo the seeding: `sed -i '/"number":8/d' $V/ghd/issues.jsonl; rm $V/ghd/labels-8 $V/ghd/issue-8.json; printf 'name = "demo"\n' > .nightshift/nightshift.toml`.
- **Once.** Run `cap watch-once "$NS" watch --once --no-self-update`. Exit `0`; `units` holds `7-fix-the-thing` with outcome `done`. `$V/ghd/labels-7` is `status:in-review`, and `grep 'issue edit' $V/ghd/calls` shows ready to in-progress, then in-progress to in-review.

## Gotchas

- Always pass `--no-self-update` or `--once`/`--dry-run`. Without `--once` it loops until `--until`; start that with `setsid` and kill the group, per the skill's Cleanup.
- An issue line in `issues.jsonl` needs a matching `labels-<n>` file, or the fake `gh` lists it with stale labels.
- Without `[queue] source`, `origin`'s host picks the forge; the local bare remote means `github`.
