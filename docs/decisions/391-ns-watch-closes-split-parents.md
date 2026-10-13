---
id: ADR-391
title: "ns watch closes a split parent once its children are done"
issue: 391
date: 2026-10-11
amends: [D35]
---

# ADR-391: ns watch closes a split parent once its children are done

## Decision

Amends D35. A split parent waits under its own label, `[queue] split_label`, now `status:split` by default, created when missing with the description "Split into child issues; closes when they're done". At start and each time a unit ends, `ns watch` reads the children of every open issue under it: its sub-issues and the `#n` items under a `Children:` line in team-authored notes, or, when those name none, the team-authored issues a search finds with a `Part of #<parent>` line. Every child closed as completed closes the parent as completed, with a comment listing them. A child closed as not planned (or as a duplicate) moves the parent to `stuck_label` with a comment. Children still open, or none found, leave it alone. The first night in a clone sweeps `status:needs-define`: an issue there with children found is closed, sent to a human, or moved to `split_label`; the sweep then writes `ns/split-swept` in the common git dir. `ns watch --dry-run` lists each split parent and its action as `split_parents`. The `Forge` trait gains `state`, `sub_issues`, `team_comments`, `part_of` and `ensure_label`.

## Reason

Five parents had to be closed by hand on 2026-10-11, and parents under `status:needs-define` read as needing a human (#389, agreed by the maintainer there). D35 rejected closing for the cost of a second query per merge to find the parent; checking every parent at start and after each unit costs a few reads per parent instead and needs no parent lookup. A label of its own keeps a split parent apart from issues waiting on a design. Sub-issues and notes are read together, so a child linked only one way still holds the parent open; the `Part of` search runs only when both are empty, as it is the slowest read. Only team-authored notes, bodies and `Part of` issues count, since anyone can comment `Children: #n` or file `Part of #n` and close it. A dropped child goes to a human because what's left of the parent may no longer be needed. The sweep is once per clone because a later needs-define issue with a `Children:` list may be a split stopped at the cap, which a human handles. GitLab's REST API lists no child items and records no close reason, so there children come from notes and `Part of` lines and a closed child counts as completed. The summary's `split_parents` is left out of the state a self-update carries, whose layout is pinned to its version; the run log's `split_parent` events keep every change. Spec: docs/factory/watch.md ("Split parents").
