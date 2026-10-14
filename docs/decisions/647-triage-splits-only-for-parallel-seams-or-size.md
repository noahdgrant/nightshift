---
id: ADR-647
title: "Triage splits only for parallel children, a real seam, or a unit too big"
issue: 647
date: 2026-10-14
amends: [D26, D37]
---

# ADR-647: Triage splits only for parallel children, a real seam, or a unit too big

## Decision

Amends D26 and D37. `ns-triage` no longer splits work because its estimate passes the soft limit. It splits only when at least one holds: the children's `touches` don't overlap, so they can run in parallel; they are different behaviours with a real seam, each worth shipping alone; or one unit would be too big, an estimate past the split threshold or a build likely to outrun the build phase's timeout. Children whose `touches` overlap are merged back into one unless the merged unit would be too big. A repeated mechanical change stays one unit, or at most two, whatever its line count.

The threshold is a new figure on the unit size line, `unit size: soft 400, split 1200`, 1200 when absent or malformed. This repo sets 1200. The soft limit stays as `ns-review`'s size Suggestion.

## Reason

A unit's lead time is mostly fixed cost: the gates, CI, each phase's start-up. Over 120 units through 2026-10-14 (`.git/ns/runs.jsonl` and the merged PRs) the median from start to merge stayed between 50 and 70 minutes up to 1,200 changed lines and jumped to 318 past it. A split below that size adds a unit's fixed cost and saves nothing unless the children run in parallel. #596 was split into five children that all edited one file, so they ran one after another, each with a human merge.

The low-stock request in `broad-feature-split` and the cases built on it (three behaviours that all edit the CLI and the warehouse) now stays one unit, as `shared-file-split-stays-one-unit`. The split cases use a request with two behaviours in disjoint files instead. `mechanical-sweep-stays-one-unit` covers the mechanical rule.

## Upstream

`ns-triage` adapts mattpocock/skills `engineering/triage` (`metadata.upstream`). Upstream has no size-based split rule: its triage moves an issue through states and writes a brief, and does not decide to split by estimate. Step 5's sizing and split rules (the three conditions, the merge-back of overlapping children, the split threshold) are nightshift's and depart from upstream there. The old trigger, splitting past the soft limit, was a nightshift addition (D26, D37) and is removed here; no upstream text changes for it.
