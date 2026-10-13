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

- Baseline, with the factory file:

  ```bash
  "$V/sandbox" sh -c 'mkdir -p .nightshift && echo "$1" > .nightshift/nightshift.toml' _ 'name = "demo"'
  ```

- No `7-fix-the-thing` worktree yet (a [run](./run.md) or [worktree](./worktree.md) recipe makes one; use a fresh sandbox).
- Issue #7 seeded as in the skill's Drive section, labelled `status:ready-for-agent`.
- Recipes run in the order below: **Order** comes before **Once**, because after `watch-once` #7 is no longer queued, and it undoes its own seeding.

- **Dry run.** `queue` holds `{"number":7,...}` and `skipped` is `[]`. `watch-dry-labels` is still `status:ready-for-agent`, and `watch-dry-calls` holds only reads (`api`, `pr list`).

  ```bash
  "$V/sandbox" cap watch-dry ns watch --dry-run --no-self-update
  "$V/sandbox" cap watch-dry-labels cat "$V/ghd/labels-7"
  "$V/sandbox" cap watch-dry-calls cat "$V/ghd/calls"
  ```

- **Order.** `priority:high` is already in the default `[queue] priority`, so seed #8 with `priority:urgent`, a label the default does not rank, and leave #7 with no priority label. Without config ties sort by number: `watch-order-default` gives `jq '[.queue[].number]'` of `[7,8]`. With `priority:urgent` ranked in the factory file, `watch-order-priority` gives `[8,7]`. The last three lines undo the seeding.

  ```bash
  "$V/sandbox" sh -c 'echo "$1" >> "$V/ghd/issues.jsonl"' _ '{"number":8,"title":"Second","labels":[{"name":"status:ready-for-agent"}],"body":"","authorAssociation":"MEMBER"}'
  "$V/sandbox" sh -c 'echo "$1" > "$V/ghd/issue-8.json"' _ '{"number":8,"title":"Second","url":"https://github.com/o/r/issues/8","state":"OPEN"}'
  "$V/sandbox" sh -c 'printf "status:ready-for-agent\npriority:urgent\n" > "$V/ghd/labels-8"'
  "$V/sandbox" cap watch-order-default ns watch --dry-run --no-self-update
  "$V/sandbox" sh -c 'printf "$1" > .nightshift/nightshift.toml' _ 'name = "demo"\n[queue]\npriority = ["priority:urgent"]\n'
  "$V/sandbox" cap watch-order-priority ns watch --dry-run --no-self-update
  "$V/sandbox" sed -i '/"number":8/d' "$V/ghd/issues.jsonl"
  "$V/sandbox" rm "$V/ghd/labels-8" "$V/ghd/issue-8.json"
  "$V/sandbox" sh -c 'echo "$1" > .nightshift/nightshift.toml' _ 'name = "demo"'
  ```

- **Once.** Exit `0`; `units` holds `7-fix-the-thing` with outcome `done`. `watch-once-labels` is `status:in-review`, and `watch-once-edits` shows ready to in-progress, then in-progress to in-review.

  ```bash
  "$V/sandbox" cap watch-once ns watch --once --no-self-update
  "$V/sandbox" cap watch-once-labels cat "$V/ghd/labels-7"
  "$V/sandbox" cap watch-once-edits grep 'issue edit' "$V/ghd/calls"
  ```

## Gotchas

- Always pass `--no-self-update` or `--once`/`--dry-run`. Without `--once` it loops until `--until`; start that with `setsid "$V/sandbox" ns watch ...` and kill the group, per the skill's Cleanup.
- An issue line in `issues.jsonl` needs a matching `labels-<n>` file, or the fake `gh` lists it with stale labels.
- Without `[queue] source`, `origin`'s host picks the forge; the local bare remote means `github`.
