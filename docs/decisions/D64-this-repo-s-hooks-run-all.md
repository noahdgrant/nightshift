---
id: D64
title: "This repo's hooks run all of CI and agents never skip them"
issue: 353
date: 2026-10-11
amended_by: [D66, ADR-452]
---

# D64: This repo's hooks run all of CI and agents never skip them

## Decision

This repo's `.githooks` pre-commit and pre-push hooks run `scripts/ci-local.sh`, all of CI, and its `--fast` subset is gone (#353, settled in #350). `ns-contract` ("Git hooks") forbids a phase to skip a hook: no `--no-verify` or `-n`, no `core.hooksPath` override, no edit to the hooks, no skip variable. A failing hook gets its cause fixed, or the phase stops `blocked`. `ns`'s own plumbing still skips hooks at three call sites, each pinned by a test that names why. Merge-style commands (the skills fast-forward, the default-branch merge) set `core.hooksPath=/dev/null`, because `--no-verify` leaves `post-merge` running; the pushes use `--no-verify`, since `pre-push` is their only hook: the quality-records push, the skills fast-forward, and the merge and push of the default branch into a PR head (`merge_default_with_git`).

## Reason

The fast hook skipped the tests, so a commit could pass it and fail CI, and an agent that met a red hook could pass `--no-verify` and carry on. A minute per commit costs less than a red PR found after push. Pre-push repeats the check because a rebase or merge makes commits that pre-commit never saw. The records branch holds data and the fast-forward takes commits already on the default branch. The update-branch merge does carry unit code, but it merges and pushes without local hooks because the forge's CI checks the merge result, which holds only while the default branch requires CI before merge.
