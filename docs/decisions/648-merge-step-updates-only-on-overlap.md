---
id: ADR-648
title: "The merge step updates a PR behind its base only when the base changed one of its files, and holds merges while main's CI is red"
issue: 648
date: 2026-10-14
amends: [ADR-466]
---

# ADR-648: The merge step updates a PR behind its base only on overlap, and holds merges while main is red

## Decision

Option B of #648, agreed with the human on 2026-10-14. Outside the local merge queue (under `[gate] local = "build-only"`, or `every-step` with `local_queue = false` or no gate), a PR behind its base is updated and its CI run again only when the commits it lacks change one of its files. The step compares `git diff --name-only --no-renames` from the merge base of the PR head and the fetched tip to each of them. Paths match exactly, and a rename counts as both its paths. With no overlap the PR head merges on the CI already there. An overlap, or either list that can't be read, updates as before. A base that moves again during CI gets the same check under the merge lock, against the new tip; the 5-move and 10-move caps are unchanged. With the local merge queue on nothing changes, since it gates the merge result itself.

In the same modes, before the merge lock, the step reads main's CI with `ns watch`'s query and waits while it is red, up to `ci_timeout_minutes`, then ends the unit stuck. Red means every check finished and one failed; running checks (even with another failed), none, or a failed query don't hold. The unit fixing main is never held.

On a repo owned by a personal account `build-only` is a supported setup: the merge step tests the PR head on GitHub and updates on overlap. `ns factory validate`'s warning (#633) says that, not that the setup can't work. The events are `merge_update_skipped`, `merge_update_needed` and `merge_held_main_red`. Spec: `docs/factory/merge.md`.

## Reason

In the run log, 82 of 158 merges paid an extra update and a full CI round, most because another unit had merged first in files the PR never touched. `main`'s CI hasn't failed since 2026-10-09: 107 runs, all green. Skipping the update for disjoint files saves that round and leaves one risk: a change in one file that breaks code in another lands without CI on the combination. Main's CI catches it after the merge, and the fix-main path (ADR-387) repairs it. Holding merges while main is red stops a second PR landing on a broken base, so a semantic conflict costs one fix unit, not a pile of merges on a red main. Module-level overlap and a merge train were left out: `merge_update_skipped` and any red main after it are the data to decide them with.

The local merge queue already tests each merge result on the laptop, so it keeps doing that, with no overlap check and no hold: the brief left that path unchanged. The `ci_timeout_minutes` bound on the hold, rather than `ci_register_timeout`'s 3 minutes, gives main's next CI run time to finish.
