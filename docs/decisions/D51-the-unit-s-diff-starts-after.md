---
id: D51
title: "The unit's diff starts after merged-in base parents and uses -U0"
issue: 257
date: 2026-10-10
---

# D51: The unit's diff starts after merged-in base parents and uses -U0

## Decision

Builds on D22. The unit's diff starts at the newest common ancestor of its sha with the base and every parent a merge on its first-parent line brought in, and is taken with `-U0`. The first run of a verify or review after it passed in the invocation uses no attempt; ship's attempts bound a ship that keeps changing the diff. Ship merges the base in and never rebases onto it.

## Reason

On the first `--parallel 2` night (#257) a unit went stuck at "verify is out of attempts (2)" with a clean PR. Every verify run that night was a rerun of a pass: after a build retry, after review's fix commits, after ship's merge of main. Attempts exist to bound failures, so a rerun of a pass shouldn't spend one. Ship is the phase that moves HEAD, so its cap still stops a loop. The merged-parent rule covers a merge of a newer main than the local or fetched ref (`git pull origin main`, `gh pr update-branch`). Context lines changed the patch id when main edited lines next to the unit's after a clean merge or rebase; non-adjacent main changes never counted, so adjacent ones shouldn't either. That night's merge was a real conflict resolution (a decision row renumbered), which still sends the unit back. Only a merged-in parent that is already in the base (an ancestor of the base ref or of `origin/<base>`, whichever is ahead) moves the start; a merge of any other branch, such as a helper branch, leaves its changes in the unit's diff. `ns clean`'s branch-deletion check keeps the strict form: full context and the plain merge base.
