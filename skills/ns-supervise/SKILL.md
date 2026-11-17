---
name: ns-supervise
description: How a session drives `ns` for a user without the mistakes made by hand. Use when driving `ns` from a session, to start units, read their progress or decide what to do with one that is stuck.
---

# Supervising ns

Reference only. You drive `ns`; the units' own sessions do the work.

## Which ns

Run `ns` from the repository's main checkout, so the pinned version runs. A development build differs from an installed one: set `NS_DEV=1`, which skips the pin, and put the directory of the build first on `PATH`. `NS_DEV` alone is not enough, because the `ns sync` hook runs the `ns` on `PATH`.

## Never close a change request by hand

`ns` finds a unit's change request by its head branch `ns/<unit>`. A closed one sends the unit to `clean_up` and leaves it stuck. To redo a unit, resume it with `ns run <unit>`, or start over with `ns run <unit> --fresh`.

A change that needs a person (labeled `needs-human-merge`, or touching files that match `[merge] human_review`) is merged only when the person says to.

## Run in the background

Start `ns run <unit>` in the background and read the JSON lines it prints:

- `{"from","to"}`: the unit moved. A `to` of `stuck` means stuck, and `reason` says why.
- `{"waiting":"<state>"}`: it waits at a gate.
- `{"stopped":"usage_limit"}`: it stopped for a usage limit. The unit is left as it was; run it again after the reset.
- `{"blocked_by":[…]}`: open units block it. Run it anyway only with `--ignore-blockers`, and only when the user agrees.

## Gates

`--gates stop` (the default) makes you the checkpoint: the run stops before each state after the first. Before you run it on, read the artifacts under `.ns/<unit>/` (`build.md`, `evidence.md`, `verify-<n>.md`, `review-<n>.md`, `ship.md`) and `ns log <unit>`, which takes `--state` and `--last`. `--gates auto` goes on without stopping.

## A unit that is stuck

`ns status <unit>` shows the reason, the state and attempt, the checks and what a person must do. Read the artifacts and `ns log <unit> --last` for the detail. By reason:

- `failed`: the state ran out of attempts on failing checks or findings. Read the last evidence, then resume with `ns run <unit>`.
- `went_wrong`: the harness session failed, timed out or left the workspace wrong. Read `ns log`, then resume.
- `inconclusive`: no requirement failed, but a Must could not be proven. `ns status` lists them. Prove them by hand or tell the user; resuming judges the same code again.
- `base_moved`: the base branch kept moving while the unit tried to merge. Resume with `ns run <unit>` when it has settled.

## A unit waiting for a person

It carries `status:waiting-for-human`, and `ns status` shows its change request. Tell the user what it needs. Merge only when they say to.
