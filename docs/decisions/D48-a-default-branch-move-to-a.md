---
id: D48
title: "A default-branch move to a commit the unit holds counts as the run's push"
issue: 252
date: 2026-10-10
---

# D48: A default-branch move to a commit the unit holds counts as the run's push

## Decision

When the default branch moves during a phase to a commit the unit's HEAD holds, `ns run` counts it as this run's push unless GitHub names, for every commit the move added along the first-parent line (at most 50), a merged PR that is neither the unit's PR nor from `ns/<unit>`. Then it is another actor's move, logged as `default_moved_by_other_actor` with those PRs, and becomes the new baseline. A failed query, a commit no merged PR holds, or one the unit's own PR holds keeps the stuck "default branch moved".

## Reason

The first `--parallel 2` night (2026-10-10) marked a unit stuck because another unit merged and the unit's build rebased onto the merge, so the new main commit was reachable from its HEAD (#252). A human merge in a sequential night does the same. Reachability can't tell a push from a rebase: both leave the new main commit in HEAD, and `git push` updates `refs/remotes/origin/<default>` just as a fetch does, so the tracking ref and its reflog don't tell them apart either, and the reflog is shared by every worktree. GitHub can: a commit landed by a merge is linked to its merged PR, and a commit a phase pushes straight to the default branch is linked to no merged PR, or to the unit's own once GitHub marks it merged. Every added commit must pass, so a phase's push that another merge then lands on top of is still caught. Failing closed keeps a real push from slipping through when GitHub is down, at the cost of a stuck unit a human re-runs; a direct push to the default branch with no PR, or a rebase merge of more than 50 commits, during a phase that then rebases does the same. Not covered: a phase that lands its work through a PR from another branch and merges it itself, which the guards before this didn't catch either unless the phase rebased afterwards. Rejected: trusting the remote-tracking reflog (above), and checking only the new tip (it misses a push under a later merge). Spec: `docs/factory/run.md` (Hard rules).
