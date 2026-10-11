---
id: D45
title: "A unit's worktree and local ns/<unit> branch are removed once its change merged and its quality records are saved"
issue: 211
date: 2026-10-10
---

# D45: A unit's worktree and local ns/<unit> branch are removed once its change merged and its quality records are saved

## Decision

A unit's worktree and local `ns/<unit>` branch are removed once its change merged and its quality records are saved. `ns run` cleans its own unit right after its merge step merges the PR. `ns watch`, at start and before each unit, and `ns clean [--dry-run]`, on demand, clean every unit whose issue is closed and whose PR, named in `pr.md`, merged, closes the issue and comes from `ns/<unit>`. Records go first, with outcome `merged`, in one push per sweep; records left in the outbox count as saved, and records that can't be written at all keep the worktree. Cleanup never removes a worktree with uncommitted changes to tracked files, one whose HEAD is not on `ns/<unit>`, one whose branch holds changes the merged PR head doesn't, one with a review artifact no record holds, the main checkout, the current directory, or a unit whose run lock (D43) a live `ns run` holds. Removal is `git worktree remove --force`, then `git branch -D`, under the worktree lock.

## Reason

By 2026-10-10 merged units had left 80 worktrees and 63 GB behind, and 56 of them were cleaned by hand (#211). D39 made that safe: the record carries what `ns quality` needs, so the worktree can go once the record is saved. A squash merge leaves no ancestry between the branch and the default branch, so "merged" can't be read from git alone. The test is that the merged PR's head holds the branch: it is the tip, descends from it (`update-branch`), or has the same `git patch-id --verbatim` against the default branch (a rebase after review). Unlike artifact currency, whitespace counts: a re-indented line can change what Python, YAML or a Makefile does, and a whitespace-only local commit must not be taken for merged. A head missing locally is fetched from `refs/pull/<n>/head`; if it still can't be found, the worktree stays. Untracked files don't keep a worktree, as the issue asked: build output and `.ns/` are untracked, and a phase commits its work. The unit's run lock is taken around each recheck and removal, so no `ns run` can start on the unit meanwhile, and since it is per unit (D43), cleanup never blocks another unit's run; `ns clean` can run beside `ns watch`. The worktree lock covers the removal because `git branch -D` writes `.git/config`. `ns watch` tries each merged unit once a night, so a unit that stays isn't recorded before every unit. Clean re-records a unit even when `ns run` already did, so the newest write (D39) carries the merge as its outcome and anything a human changed since. `ns watch` cleans at its loop's top rather than right after each unit, so one path covers a merge from before the night and one during it. Rejected: cleaning on `done` (a human may never merge), deleting branches with `-d` (a squash merge always makes it refuse), and an age-based sweep (it would delete stuck work). Spec: `docs/FACTORY.md` (Unit lifecycle, Cleanup).
