---
id: D41
title: "One principle, ns-principle-pave-the-road, covers agent-friendly code and config"
issue: 213
date: 2026-10-10
---

# D41: One principle, ns-principle-pave-the-road, covers agent-friendly code and config

## Decision

One principle, `ns-principle-pave-the-road`, covers agent-friendly code and config, with **paved road** as its leading word and three rules: one road per job, make the road the shortcut, pave with well-trodden stones. The quality bar's Architecture section gains two Important items: the diff adds no second way to do a job the repo already does one way, and a new dependency, tool or pattern states why the existing or conventional one doesn't fit. `ns-build` and the architecture reviewer point to the principle.

## Reason

Agents take the shortest path to done and copy the code around them (#213). nightshift held this only as fragments: `encode-lessons-in-structure` on agents copying nearby code, `laziness-protocol` on making each choice in one place, and the bar's helper-reuse item. One principle with three rules was chosen over three principles because the rules share one cause and one fix, a single paved road per job, and one name is easier to point to. It points to `migrate-callers-then-delete-legacy-apis` for removing the second way and to `encode-lessons-in-structure` for how to block the wrong one, rather than restating them. Docs and skills stay with `ns-writing-for-agents`, which already keeps one meaning in one place. The two bar items replace the old pattern and helper items, which they cover. Both are Important, the human's choice, to see how they work in practice before raising or lowering them. The principle is an original, with no `metadata.upstream`; `docs/SOURCES.md` lists its sources as inspiration. It is user-invoked, so it has no trigger evals (`docs/EVALS.md`). The case `py-reuses-duration-helper` gives py-inventory a duration helper and a brief that spells the format out, so the quickest build writes a second parser; it checks that the extend command calls the helper and uses its result, and that no other module spells out hour or day units. Checking the rules by tool (duplicate helpers, banned alternatives, a dependency allowlist) is #215.
