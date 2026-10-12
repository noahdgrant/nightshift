---
id: ADR-444
title: "scripts/ci-local.sh skips a tree whose full run already passed"
issue: 444
date: 2026-10-12
amends: [ADR-452]
---

# ADR-444: scripts/ci-local.sh skips a tree whose full run already passed

## Decision

A full `scripts/ci-local.sh` run that passes writes one file in `<git-common-dir>/ns/ci-passed/`. Its name is the SHA-256 of the tested tree's hash, the SHA-256 of `scripts/ci-tools.toml` and the SHA-256 of `scripts/ci-local.sh`; it holds the tree, when (UTC) and where (the checkout and its HEAD). A later full run with the same key prints `ci-local: tree <hash> already passed (<when>, <where>)` and exits 0. The cache lives in the script, not in its callers, so pre-push, the `ns run` gate and the local merge queue's gate share it with no change to them.

The tested tree is the index's tree (`git write-tree`), used only when the working tree matches the index and holds no untracked file that git doesn't ignore. That covers a clean checkout of a commit (pre-push's temporary worktree, a unit's worktree) and the merge queue's scratch worktree, where the merge result is staged and never committed. Anything else runs without the cache, and a run whose tree changed while it ran is not recorded. Only passes are recorded. `--no-cache` runs anyway and still records a pass; `--fast` never reads or writes the cache. A hit refreshes the entry's time, and each write keeps the 200 entries used last. `__pycache__/` and `.pytest_cache/` are ignored so that the Python steps' leftovers don't make a reused checkout look dirty.

## Reason

A factory unit ran the full suite about six times, mostly on the same tree (gate after build, gate after review, pre-push, merge-queue gate), and several runs at once on one laptop caused flaky failures and phase timeouts (#442, #443). A tree that passed passes again unless the pinned tools or the checks changed, which the two script hashes cover. Keying by the index's tree rather than a commit lets the merge result, which has no commit, share entries with a pushed commit of the same tree. A dirty checkout gets no key because its tree is not what the run tests. The human agreed the design in #444 on 2026-10-12.
