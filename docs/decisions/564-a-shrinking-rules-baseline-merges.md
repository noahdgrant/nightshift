---
id: ADR-564
title: "A rules baseline that only shrinks merges without a human, and the gate after build commits the prune"
issue: 564
date: 2026-10-15
amends: [D50]
---

# ADR-564: A rules baseline that only shrinks merges without a human, and the gate after build commits the prune

## Decision

Amends D50 (#564). A fixed baseline entry is reported, never a failure. The merge step drops `.nightshift/rules-baseline.json` from the human-review hits when every entry at the PR head is matched by its own equal entry at the merge base with the default branch, all fields but the line compared and duplicates counted. Anything else about the file still needs a human. The full gate runs `ns rules check --prune`, which drops the fixed entries when nothing is new. After the gate that follows build goes green, `ns run` commits the shrunk file on its own, skipping the hooks, carries the green verdict to the new commit, and moves `build.md`'s `sha` to it. Every other gate, and a red one, restores the file.

## Reason

A unit that fixed a baselined violation had to edit a human-review file to drop the stale entry, so it stopped for a human merge (#316, #539 and more). A shrink can't hide a violation, so letting it through costs nothing. Growth or a changed entry could, so those stay with a human. Entries are compared on every field, not just the four `ns rules check` matches on, so a unit can't reword an accepted entry without a human seeing it. The prune has to land in the unit's branch, and the gate runs after build has written its artifact. A commit after that makes `build.md` stale, and build would run again for a change it didn't make. The commit only drops entries the same gate run found unmatched, so the gate's verdict and build's artifact still hold at it, and `ns run` moves both. Only the gate after build commits: after ship the PR head is already pushed, and the merge step's gate runs on the PR head or a scratch merge, where a local commit would land nowhere. Spec: docs/factory/run.md ("Rules", "Gate"), docs/factory/merge.md (step 6).
