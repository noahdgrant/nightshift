---
id: D27
title: "An open Important finding after the third fix cycle blocks the unit"
issue: 149
date: 2026-10-09
amended_by: [D29, D60]
---

# D27: An open Important finding after the third fix cycle blocks the unit

## Decision

After the third fix cycle, an open Important finding in code the unit changed blocks the unit like an open Critical: `review.md` is `blocked` and its first line names them (`Open after 3 fix cycles: C1, I2.`). An open Important in pre-existing code still becomes a follow-up issue, and Suggestions never block. The `ns watch` stuck comment lists a blocked review's open Critical and Important findings.

## Reason

Deferring a changed-line Important merged code below the bar, and the debt compounded: the 2026-10-09 baseline was about 6 Important findings per unit (#149). Pre-existing code is not the unit's to fix, so it can't hold the unit back. Listing the findings in the comment lets a human finish them, or re-scope the issue, without opening the worktree. The CLI already stops on any `blocked` artifact; it now also names that artifact when a run stops on one left by an earlier run, so the comment can list its findings. `ns-ship`'s GO check holds the same line. The model-run eval case `important-after-three-cycles-is-blocked` checks the blocking rule; a case showing that a pre-existing Important doesn't block comes with #148, which changes how those findings are filed. Spec: skills/ns-review/SKILL.md. Amended by D29 and D60.
