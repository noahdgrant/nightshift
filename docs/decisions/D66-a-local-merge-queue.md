---
id: D66
title: "A local merge queue"
issue: 374
date: 2026-10-11
amends: [D64]
amended_by: [ADR-466]
---

# D66: A local merge queue

## Decision

A local merge queue (#374). With `[merge] local_queue = true`, the default, and a gate set, `ns run`'s merge step, under the merge lock, fetches `origin/<default>`, merges the PR head into it in a scratch worktree (`<git-common-dir>/ns/merge-queue/<unit>`, `--no-ff --no-commit`, no push, no hook) and runs the gate there. Green merges the PR head with no update-branch and no up-to-date requirement; a conflict goes back to build to rebase, as `DIRTY` does; red goes back to build with a note that the failure comes from the merge with `<default>`, and logs `merge_gate_failed`. The verdict is kept per (PR head, base tip) pair and reused until either moves, except a red from a timeout, the memory cap or a signal, which is not kept. A tip already in the PR head gates the PR head as before. A drop guard removes the scratch worktree on every exit, a stop included. `local_queue = false`, or no gate, keeps the merge step as it was: update-branch for `BEHIND`, then the gate on the PR head. Amends D64: the scratch worktree's checkout is one more call site that skips hooks, pinned by a test; its merge commits nothing, so it runs no hook without an override.

## Reason

With "Require branches to be up to date before merging" off, a PR could merge without being tested against the `main` it lands on; with it on, parallel units updated and re-ran CI each time `main` moved (#334 went through review three times). GitHub's merge queue tests merge results but needs an organisation-owned repo. The merge lock already serialises merges, so testing inside it costs one gate run per merge and holds other merges back while it runs, which is the intended trade. A conflict keeps today's route back to build rather than a new route to ship. Without a gate there is nothing to test the merge result with, so the step falls back to keeping the PR up to date. A repo that already tests merge results some other way turns it off. Spec: `docs/factory/merge.md` ("Local merge queue").
