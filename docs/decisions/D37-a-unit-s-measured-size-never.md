---
id: D37
title: "A unit's measured size never blocks it"
issue: 212
date: 2026-10-10
amends: [D26]
---

# D37: A unit's measured size never blocks it

## Decision

Amends D26. A unit's measured size never blocks it. `docs/agents/stack.md` holds one figure, `unit size: soft 400` (400 when absent); a `hard` figure left from the old template is ignored. `ns-build` builds every criterion whatever the diff's size. Past the soft limit, `ns-review` adds a Split section to every first-pass reviewer's brief asking whether its part of the diff could ship separately, and the lead gathers the answers into one Suggestion, "<n> changed lines: consider splitting into …" naming the seams it can see (a stack, a file group, the refactor apart from the behaviour), or "<n> changed lines: no split seam found". A refactor mixed with new behaviour gets the same Suggestion at any size. Both quality-bar Size items, the unit and a file grown past roughly 1000 lines, are Suggestions. Triage still splits work estimated past the soft limit, as planning (which can still block an in-progress brief, per D26); a unit that grows past its estimate keeps going.

## Reason

A big change is sometimes the right change, such as a refactor that must move many call sites at once (#212). Under D26 size blocked in three places: build's hard-limit stop, review's Important split finding at about 1000 lines, and the untagged bar item, which since D27 and D29 blocks at the cycle limit like any open Important in changed code. A large unit does cost more review, so the answer is a prompt to look for a split, recorded where the human and a later triage see it. `ns quality` keeps reporting each unit's `changed_lines`, so that cost stays visible. The `py-unit-too-large-blocks` case became `py-unit-past-soft-limit-keeps-going`: build must pass the old hard figure and end `pass`. `past-soft-limit-split-suggestion` covers review: a Split section in all eight briefs, the Suggestion under Suggestion, no size finding at Critical or Important, and `pass`; `small-diff-reduced-panel` checks that no brief has a Split section under the limit.
