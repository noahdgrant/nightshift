---
id: D67
title: "This repo's pre-push hook checks each commit pushed"
issue: 385
date: 2026-10-11
---

# D67: This repo's pre-push hook checks each commit pushed

## Decision

This repo's `.githooks/pre-push` checks each commit pushed, not the working tree (#385). For every ref pushed it peels the sha to a commit, adds a detached temporary worktree there, runs that commit's own `scripts/ci-local.sh` in it, and removes the worktree, on failure and interrupt too. Deleted refs are skipped, a sha that is not a commit refuses the push, and a commit pushed under two refs is checked once. The runs share one cargo target directory, `pre-push-target` under the common git dir, and `ci-local.sh` runs the `ns` built under `CARGO_TARGET_DIR` when that is set.

## Reason

The old hook ran `ci-local.sh` on the working tree, so it refused any push from a dirty tree, with an untracked file, or of a ref that was not HEAD. A stray scratch file then blocked a push and tempted `--no-verify`. A worktree per commit checks exactly what is pushed, and the shared target keeps a push to about one incremental build.
