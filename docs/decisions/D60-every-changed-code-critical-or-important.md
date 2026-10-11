---
id: D60
title: "Every changed-code Critical or Important gets a fix attempt before it blocks"
issue: 268
date: 2026-10-11
amends: [D24, D25, D27, D29, D30]
---

# D60: Every changed-code Critical or Important gets a fix attempt before it blocks

## Decision

Amends D27. Every Critical or Important finding in changed code gets at least one fix attempt before it can block a unit. Cycles 1 to 3 stay full fresh-eyes re-reviews. When changed-code findings are still open after cycle 3 and any has had fewer than 2 fix attempts, `ns-review` runs up to two **fix-only** cycles (4 and 5): the build agent gets the open findings, and the reviewers that raised them check only whether each is fixed, raising nothing new. `review.md` is `blocked` when a changed-code Critical or Important is still open after 2 fix attempts, or anything is open after cycle 5; its first line reads `Open after at least 2 fix attempts: C1, I2.` Each `review/cycle-<n>.md` opens with frontmatter giving `cycle`, `kind: full | fix-only`, `started` and `ended` (UTC), and each finding carries `Attempts:` beside `Cycle:`; the quality record keeps `fix_attempts` on each finding, optional so older records still read.

## Reason

Units were blocked on findings no fix cycle had tried (#268). In 102 units' quality records, 8 of the 10 cycle-limit blocks had only findings first raised in cycle 3 left open, none of the 23 findings first raised in cycle 3 were fixed by the loop, and 226 of 234 fixed findings were fixed in the cycle right after they were raised. So one attempt usually lands, and a late finding blocked only because the loop ran out. A fix-only cycle gives it that attempt without a fresh review that could raise yet more late findings. A finding that survives two attempts is unlikely to yield to a third, so it goes to a human. Pass 0 thoroughness is left alone; #338 reports the late-finding rate that would justify changing it. Eval cases: `cycle-3-finding-fixed-in-fix-only-cycle`, `open-after-two-fix-attempts-is-blocked`, `fix-only-cycle-raises-no-new-finding`, `fix-only-cycle-5-skips-findings-at-two-attempts`, and the updated `important-after-three-cycles-is-blocked` and `critical-after-three-cycles-is-blocked`. Spec: skills/ns-review/SKILL.md.
