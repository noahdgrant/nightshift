---
id: ADR-466
title: "[gate] local = \"build-only\" runs all of local CI once per unit, after build; this repo sets it"
issue: 466
date: 2026-10-12
amends: [ADR-452, D67]
---

# ADR-466: `[gate] local = "build-only"` runs all of local CI once per unit, after build

## Decision

`.nightshift/nightshift.toml` takes `[gate] local`, `"every-step"` or `"build-only"` (#466, from #460). `every-step` is the default and keeps every gate as it was: the full command after build, after any phase that moved HEAD and in the merge step.

Under `build-only`, only the gate after build runs the full command. Every other gate, after review or another phase that moved HEAD and in the merge step, runs `<base> --fast` and then the `ci-affected` row of `docs/agents/stack.md`, with `NS_GATE_FILES` naming the paths changed since the full gate last went green, else since the unit's base. A red fast gate goes back to build. Each `gate` event logs `where`: `local_full` or `local_fast`; the merge step's CI events log `github`.

This repo's pre-push hook reads the setting from the commit pushed and runs `scripts/ci-local.sh --fast` under `build-only`. This repo sets `build-only`; its `ci-affected` is `scripts/ci-affected.sh`, which runs `cargo test` on any change, since the cli tests read files across the repo, plus a rule pack's or fixture's tests when a path under it changed.

## Reason

One unit ran the full suite about six times: after build, after review, at pre-push and in the merge queue, and again on each rebuild. The pass cache (ADR-444) skips a tree that already passed, but review's fixes and the merge with main make new trees. Several full runs at once on one laptop made the factory crawl and turned load into flaky failures. A full run after build catches what the build changed; the fast checks and the affected tests catch what review's small fixes break; GitHub CI runs everything before the merge. The human agreed the setting, its two values, the default and the per-gate behaviour on 2026-10-12 and left the affected-tests mechanism to the builder. A row in `stack.md` fits how the gate already finds its commands, and a script that maps paths to tests keeps that knowledge in the repo, not in `ns`.
