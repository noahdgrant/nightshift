---
id: D43
title: "ns run's run lock is per unit"
issue: 58
date: 2026-10-10
amends: [D36]
---

# D43: ns run's run lock is per unit

## Decision

Amends D36. `ns run`'s run lock is per unit: an OS `flock` on `ns-run-<unit>.lock` in the common git dir, so runs of different units in one repo go ahead together and a second run of the same unit still exits 5. `ns watch`'s startup requeue checks every held `ns-run-*.lock`, and knows a holder that hasn't written its record yet by its file name. `ns-watch.lock` still allows one `ns watch` per repo. Two locks that wait instead of refusing join it: the merge lock (`ns-merge.lock`), held by the merge step from just before it reads the PR's merge state through the merge, and the worktree lock (`ns-worktree.lock`), held by `ns worktree new` while it creates a worktree, not while setup runs. Both waits retry every 50 ms in real time and end on a stop; the merge wait also gives up at `ns watch --until`, ending the run as `budget`. Runner locks (D23, D33) are unchanged. Run-log lines go out in one write each.

## Reason

Running units in parallel (#58) needs runs of different units to proceed, but not to collide. Two merges at once would each check CI against a main the other is about to change, so the second PR could merge without being updated; holding the lock from the merge-state read means a PR that waited sees the main the merge before it left, and is updated and checked again. GitHub reports `UNKNOWN` while it recomputes the state after its base moves, which is the usual case for a PR that waited, so an `UNKNOWN` is polled until GitHub settles or `ci_register_timeout` passes. The checks before it (PR open, head current, `review.md` passing) need no lock, so a unit with nothing to merge never waits. Twelve concurrent `git worktree add`s from a remote base failed 3 runs of 3 on `could not lock config file .git/config`; setup is left outside the lock since it can take minutes and touches only its own worktree. The waits poll in real time, not on the run's clock as D33's do: the holder is another process, so a pinned clock (`NS_NOW`) would race to its bound at once, and the merge wait has no timeout of its own, since a live holder is bounded by its own CI timeouts and the kernel drops a dead one's hold. A lock file per unit stays behind, empty, after its run, since deleting a lock file races with a process opening it. A `writeln!` to the run log could split a line across writes that a parallel run interleaves. Spec: `docs/factory/run.md`, `docs/factory/merge.md`, `docs/factory/watch.md` ("Units a night didn't finish").
