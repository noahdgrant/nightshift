---
id: D32
title: "ns-build marks each open risk as file: or note:"
issue: 168
date: 2026-10-10
---

# D32: ns-build marks each open risk as file: or note:

## Decision

`ns-build` marks each open risk in `build.md` as `file:` (a concrete defect or missing work outside the unit's scope) or `note:` (about this unit only). `ns-ship` files the `file:` ones after its GO call, before it writes the PR body: the same duplicate search, `needs-triage` state, category and area labels and unit link as a D29 escape, with the words "found while building <unit-id>, outside its scope" and no priority. It rewrites each line to `filed: #<n>`, or `unfiled: <reason>` when the tracker can't be reached, which never holds the unit back, rewriting each line as soon as its issue exists; a re-run files `unfiled:` lines again and skips `filed:` ones, and an unprefixed line counts as `note:`. It lists them under the PR's Follow-ups.

## Reason

Build's open risks were the last out-of-scope findings nothing filed, so they died in `build.md` (#168). Ship files them, not build, because a unit that never merges would otherwise leave issues behind for a defect nobody has confirmed past build, and a build retried after `fail` would file again; by GO the unit has passed verify and review, and the risk has been seen by both. The cost: a unit that stops before ship leaves its `file:` risks unfiled in `build.md`, where a human reading the blocked unit still sees them. Priority is left to triage, since a build risk has no review severity to map from. The model-run eval case `py-out-of-scope-defect-marked-for-filing` is a build case: a low-stock report whose obvious implementation reuses `stock_levels`, which counts expired reservations; build must leave `stock_levels` unchanged, mark the defect `file:`, and file nothing itself. Ship has no eval cases yet (it needs a forge to open a PR against), so the filing step itself has none. Spec: skills/ns-ship/SKILL.md.
